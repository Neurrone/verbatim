//! The async form's logic: each entry's handling read top to bottom, its
//! requests awaited in line.

use std::future::Future;
use std::pin::Pin;

use super::super::harness::{
    COMMAND_DUE_US, Effect, FOCUS_DUE_US, Reader, Reply, Request, WATCH_MS, Window, focus_speech,
};
use super::driver::{Cx, Entry};

/// The function that handles `entry` at its turn.
pub(super) fn start(cx: Cx, entry: &Entry) -> Pin<Box<dyn Future<Output = ()>>> {
    match *entry {
        Entry::Focus { app, window, token } => Box::pin(focus(cx, app, window, token)),
        Entry::CaretKey {
            key, app, window, ..
        } => Box::pin(caret_key(cx, key, app, window)),
    }
}

/// A UI Automation focus: two requests chained under one due time, then
/// spoken.
async fn focus(cx: Cx, app: u32, window: Window, token: u32) {
    let due_us = cx.now_us().saturating_add(FOCUS_DUE_US);
    let element = cx.ask(
        Reader::Listener,
        window,
        Request::FocusElement { token },
        due_us,
    );
    let Some(Reply::Element { name, route }) = element.await.result() else {
        return;
    };
    let completion = cx.ask(
        Reader::Outpost(app),
        window,
        Request::CompleteFocus { route },
        due_us,
    );
    let Some(Reply::Completed { role }) = completion.await.result() else {
        return;
    };
    cx.emit(Effect::Speak(focus_speech(&name, &role)));
}

/// A caret key: its starting point, the key sent on, the watch, the line.
async fn caret_key(cx: Cx, key: u32, app: u32, window: Window) {
    if cx.silent() {
        cx.emit(Effect::SendKey(key));
        return;
    }
    let due_us = cx.now_us().saturating_add(COMMAND_DUE_US);
    let outpost = Reader::Outpost(app);
    let start = cx.ask(outpost, window, Request::Caret, due_us).await;
    cx.emit(Effect::SendKey(key));
    if start.result() != Some(Reply::Caret) || cx.silent() || cx.focus_waiting() {
        return;
    }
    let watch = Request::Watch {
        within_ms: WATCH_MS,
    };
    if let Some(Reply::Watched { line }) = cx.ask(outpost, window, watch, due_us).await.result() {
        cx.emit(Effect::Speak(line));
    }
}
