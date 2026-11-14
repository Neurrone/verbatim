//! The thread form's logic: each entry's handling read top to bottom, each
//! request a call that blocks the logic thread until the inbox thread
//! hands back its ending.

use std::sync::mpsc::{Receiver, Sender, channel};

use super::super::harness::{
    COMMAND_DUE_US, Effect, Ending, FOCUS_DUE_US, Reader, Reply, Request, WATCH_MS, Window,
    focus_speech,
};
use super::driver::{Entry, FromLogic, ToLogic};

/// The inbox thread stopped while the logic waited on it.
struct Closed;

/// The logic thread: runs each entry the inbox thread starts, until the
/// inbox closes its channel.
pub(super) fn run(messages: &Receiver<ToLogic>, to_inbox: &Sender<FromLogic>) {
    while let Ok(message) = messages.recv() {
        let ToLogic::Start(entry, now_us) = message else {
            panic!("an ending reached the logic thread outside an entry");
        };
        let cx = Cx {
            messages,
            to_inbox,
            now_us,
        };
        let handled = match entry {
            Entry::Focus { app, window, token } => focus(&cx, app, window, token),
            Entry::CaretKey {
                key, app, window, ..
            } => caret_key(&cx, key, app, window),
        };
        if handled.is_err() || to_inbox.send(FromLogic::Done).is_err() {
            return;
        }
    }
}

/// A UI Automation focus: two requests chained under one due time, then
/// spoken.
fn focus(cx: &Cx, app: u32, window: Window, token: u32) -> Result<(), Closed> {
    let due_us = cx.now_us.saturating_add(FOCUS_DUE_US);
    let element = cx.ask(
        Reader::Listener,
        window,
        Request::FocusElement { token },
        due_us,
    )?;
    let Some(Reply::Element { name, route }) = element.result() else {
        return Ok(());
    };
    let completion = cx.ask(
        Reader::Outpost(app),
        window,
        Request::CompleteFocus { route },
        due_us,
    )?;
    let Some(Reply::Completed { role }) = completion.result() else {
        return Ok(());
    };
    cx.emit(Effect::Speak(focus_speech(&name, &role)))
}

/// A caret key: its starting point, the key sent on, the watch, the line.
fn caret_key(cx: &Cx, key: u32, app: u32, window: Window) -> Result<(), Closed> {
    if cx.silent()? {
        return cx.emit(Effect::SendKey(key));
    }
    let due_us = cx.now_us.saturating_add(COMMAND_DUE_US);
    let outpost = Reader::Outpost(app);
    let start = cx.ask(outpost, window, Request::Caret, due_us)?;
    cx.emit(Effect::SendKey(key))?;
    if start.result() != Some(Reply::Caret) || cx.silent()? || cx.focus_waiting()? {
        return Ok(());
    }
    let watch = Request::Watch {
        within_ms: WATCH_MS,
    };
    if let Some(Reply::Watched { line }) = cx.ask(outpost, window, watch, due_us)?.result() {
        cx.emit(Effect::Speak(line))?;
    }
    Ok(())
}

/// The logic thread's handle to the inbox thread.
struct Cx<'a> {
    messages: &'a Receiver<ToLogic>,
    to_inbox: &'a Sender<FromLogic>,
    now_us: u64,
}

impl Cx<'_> {
    fn emit(&self, effect: Effect) -> Result<(), Closed> {
        self.to_inbox
            .send(FromLogic::Emit(effect))
            .map_err(|_| Closed)
    }

    fn silent(&self) -> Result<bool, Closed> {
        let (answer, silent) = channel();
        self.to_inbox
            .send(FromLogic::Silent(answer))
            .map_err(|_| Closed)?;
        silent.recv().map_err(|_| Closed)
    }

    fn focus_waiting(&self) -> Result<bool, Closed> {
        let (answer, waiting) = channel();
        self.to_inbox
            .send(FromLogic::FocusWaiting(answer))
            .map_err(|_| Closed)?;
        waiting.recv().map_err(|_| Closed)
    }

    /// Sends a request and blocks until its one ending.
    fn ask(
        &self,
        reader: Reader,
        window: Window,
        request: Request,
        due_us: u64,
    ) -> Result<Ending, Closed> {
        self.to_inbox
            .send(FromLogic::Ask {
                reader,
                window,
                request,
                due_us,
            })
            .map_err(|_| Closed)?;
        match self.messages.recv() {
            Ok(ToLogic::Resume(ending)) => Ok(ending),
            Ok(ToLogic::Start(..)) => panic!("an entry started while another waited"),
            Err(_) => Err(Closed),
        }
    }
}
