//! The thread form's driver: an inbox thread that owns the event queue and
//! the waiting, and a logic thread that runs the front entry's handling as
//! a plain function, blocking on each request until the inbox thread
//! hands it the request's ending.
//!
//! The two threads run in lockstep: whenever the inbox thread hands the
//! logic thread an entry or an ending, it takes the logic thread's
//! messages until the logic thread asks again or finishes, so effects keep
//! one order and the inbox handles its next input only once the logic is
//! paused. Only channels cross between the threads.

#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "this form is made of threads; the trial is compiled only into tests"
)]

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use super::super::harness::{
    Ask, Effect, Ending, Form, Input, Outcome, Reader, Request, RequestId, Stop, Timed, Window,
    within,
};
use super::logic;

/// Core, as the harness reaches it: the inbox thread's channel.
pub struct ThreadCore {
    to_inbox: Sender<ToInbox>,
    inbox: Option<JoinHandle<()>>,
}

enum ToInbox {
    Input(Timed, Sender<Vec<Effect>>),
    Dump(Sender<String>),
    Due(Sender<Option<u64>>),
    /// The queue, when the logic thread is inside no entry.
    Copy(Sender<Option<Queue>>),
    Stop,
}

/// What the inbox thread sends the logic thread.
pub(super) enum ToLogic {
    /// Run this entry's handling; the time is the step's.
    Start(Entry, u64),
    /// The request the logic waits on ended.
    Resume(Ending),
}

/// What the logic thread sends the inbox thread.
pub(super) enum FromLogic {
    Emit(Effect),
    Ask {
        reader: Reader,
        window: Window,
        request: Request,
        due_us: u64,
    },
    Silent(Sender<bool>),
    FocusWaiting(Sender<bool>),
    Done,
}

/// The inbox thread's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Queue {
    now_us: u64,
    next_request: u64,
    front: Option<Front>,
    waiting: VecDeque<Entry>,
}

/// The front entry and its request, absent only while the logic thread
/// runs between two requests.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Front {
    entry: Entry,
    out: Option<Out>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Out {
    id: RequestId,
    reader: Reader,
    window: Window,
    request: Request,
    due_us: u64,
}

/// An entry of the event queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Focus {
        app: u32,
        window: Window,
        token: u32,
    },
    CaretKey {
        key: u32,
        app: u32,
        window: Window,
        silent: bool,
    },
}

impl Form for ThreadCore {
    fn new() -> Self {
        Self::from_queue(Queue::default())
    }

    fn feed(&mut self, timed: &Timed) -> Vec<Effect> {
        let (reply, effects) = channel();
        self.send(ToInbox::Input(timed.clone(), reply));
        effects
            .recv()
            .expect("the inbox thread answers every input")
    }

    fn dump(&self) -> String {
        let (reply, dump) = channel();
        self.send(ToInbox::Dump(reply));
        dump.recv().expect("the inbox thread answers a dump")
    }

    fn checkpoint(&self) -> Option<Self> {
        let (reply, copy) = channel();
        self.send(ToInbox::Copy(reply));
        let queue = copy.recv().expect("the inbox thread answers a copy");
        queue.map(Self::from_queue)
    }

    fn next_due(&self) -> Option<u64> {
        let (reply, due) = channel();
        self.send(ToInbox::Due(reply));
        due.recv().expect("the inbox thread answers its due time")
    }
}

impl Drop for ThreadCore {
    fn drop(&mut self) {
        self.send(ToInbox::Stop);
        if let Some(inbox) = self.inbox.take() {
            inbox.join().expect("the inbox thread ends cleanly");
        }
    }
}

impl ThreadCore {
    fn from_queue(queue: Queue) -> Self {
        let (to_inbox, inbox_messages) = channel();
        let inbox = std::thread::spawn(move || Inbox::start(queue).run(&inbox_messages));
        Self {
            to_inbox,
            inbox: Some(inbox),
        }
    }

    fn send(&self, message: ToInbox) {
        self.to_inbox
            .send(message)
            .expect("the inbox thread runs while Core does");
    }
}

struct Inbox {
    queue: Queue,
    to_logic: Option<Sender<ToLogic>>,
    from_logic: Receiver<FromLogic>,
    logic: Option<JoinHandle<()>>,
}

impl Inbox {
    fn start(queue: Queue) -> Self {
        let (to_logic, logic_messages) = channel();
        let (to_inbox, from_logic) = channel();
        let logic = std::thread::spawn(move || logic::run(&logic_messages, &to_inbox));
        Self {
            queue,
            to_logic: Some(to_logic),
            from_logic,
            logic: Some(logic),
        }
    }

    fn run(mut self, messages: &Receiver<ToInbox>) {
        while let Ok(message) = messages.recv() {
            match message {
                ToInbox::Input(timed, reply) => {
                    let effects = self.feed(&timed);
                    reply
                        .send(effects)
                        .expect("the harness waits for its effects");
                }
                ToInbox::Dump(reply) => {
                    let dump = format!("{:?}", self.queue);
                    reply.send(dump).expect("the harness waits for its dump");
                }
                ToInbox::Due(reply) => {
                    let due = self.next_due();
                    reply.send(due).expect("the harness waits for its due time");
                }
                ToInbox::Copy(reply) => {
                    let copy = self.queue.front.is_none().then(|| self.queue.clone());
                    reply.send(copy).expect("the harness waits for its copy");
                }
                ToInbox::Stop => break,
            }
        }
        // The logic thread's channel closes, which ends a wait it is in.
        self.to_logic = None;
        if let Some(logic) = self.logic.take() {
            logic.join().expect("the logic thread ends cleanly");
        }
    }

    fn next_due(&self) -> Option<u64> {
        self.out().map(|out| out.due_us)
    }

    fn feed(&mut self, timed: &Timed) -> Vec<Effect> {
        self.queue.now_us = timed.at_us;
        let mut effects = Vec::new();
        match &timed.input {
            Input::Focus { app, window, token } => {
                if self.watching() {
                    self.give_up(Stop::PendingFocus, &mut effects);
                }
                self.queue.waiting.push_back(Entry::Focus {
                    app: *app,
                    window: *window,
                    token: *token,
                });
            }
            Input::CaretKey { key, app, window } => {
                self.later_key(&mut effects);
                self.queue.waiting.push_back(Entry::CaretKey {
                    key: *key,
                    app: *app,
                    window: *window,
                    silent: false,
                });
            }
            Input::Answer { id, outcome } => {
                if self.out().is_some_and(|out| out.id == *id) {
                    self.answered(outcome.clone(), &mut effects);
                }
            }
            Input::Foreground { window } => self.foreground(*window, &mut effects),
            Input::ReaderEnded { reader } => {
                if self.out().is_some_and(|out| out.reader == *reader) {
                    self.answered(Outcome::ReaderEnded, &mut effects);
                }
                if *reader == Reader::Listener {
                    self.queue
                        .waiting
                        .retain(|entry| !matches!(entry, Entry::Focus { .. }));
                }
            }
            Input::Tick => {
                if self.next_due().is_some_and(|due| due <= self.queue.now_us) {
                    self.give_up(Stop::DueTime, &mut effects);
                }
            }
        }
        self.take_next(&mut effects);
        effects
    }

    fn out(&self) -> Option<&Out> {
        self.queue
            .front
            .as_ref()
            .and_then(|front| front.out.as_ref())
    }

    fn watching(&self) -> bool {
        self.out()
            .is_some_and(|out| matches!(out.request, Request::Watch { .. }))
    }

    /// A key arrived: every caret key ahead of it is marked silent, and a
    /// watching one is cancelled.
    fn later_key(&mut self, effects: &mut Vec<Effect>) {
        if let Some(Front {
            entry: Entry::CaretKey { silent, .. },
            ..
        }) = &mut self.queue.front
        {
            *silent = true;
        }
        for entry in &mut self.queue.waiting {
            if let Entry::CaretKey { silent, .. } = entry {
                *silent = true;
            }
        }
        if self.watching() {
            self.give_up(Stop::LaterKey, effects);
        }
    }

    /// The foreground moved to `foreground`: the front's request and every
    /// waiting entry outside its tree end.
    fn foreground(&mut self, foreground: Window, effects: &mut Vec<Effect>) {
        if self
            .out()
            .is_some_and(|out| !within(out.window, foreground))
        {
            self.give_up(Stop::Foreground, effects);
        }
        let mut kept = VecDeque::new();
        for entry in self.queue.waiting.drain(..) {
            match entry {
                Entry::Focus { window, .. } | Entry::CaretKey { window, .. }
                    if within(window, foreground) =>
                {
                    kept.push_back(entry);
                }
                Entry::CaretKey { key, .. } => effects.push(Effect::SendKey(key)),
                Entry::Focus { .. } => {}
            }
        }
        self.queue.waiting = kept;
    }

    fn give_up(&mut self, reason: Stop, effects: &mut Vec<Effect>) {
        if let Some(id) = self.out().map(|out| out.id) {
            effects.push(Effect::Cancel { id, reason });
            self.resume(Ending::Stopped(reason), effects);
        }
    }

    fn answered(&mut self, outcome: Outcome, effects: &mut Vec<Effect>) {
        if outcome == Outcome::NotResponding {
            effects.push(Effect::NotRespondingSound);
        }
        self.resume(Ending::Answered(outcome), effects);
    }

    /// Ends the front's request with `ending` and runs the logic thread to
    /// its next request or the entry's end.
    fn resume(&mut self, ending: Ending, effects: &mut Vec<Effect>) {
        if let Some(front) = self.queue.front.as_mut() {
            front.out = None;
        }
        self.lockstep(ToLogic::Resume(ending), effects);
    }

    fn take_next(&mut self, effects: &mut Vec<Effect>) {
        while self.queue.front.is_none() {
            let Some(entry) = self.queue.waiting.pop_front() else {
                return;
            };
            self.queue.front = Some(Front {
                entry: entry.clone(),
                out: None,
            });
            self.lockstep(ToLogic::Start(entry, self.queue.now_us), effects);
        }
    }

    /// Hands the logic thread `message`, then serves it until it asks
    /// again or finishes the entry.
    fn lockstep(&mut self, message: ToLogic, effects: &mut Vec<Effect>) {
        self.to_logic
            .as_ref()
            .expect("the logic thread's channel is open until the inbox stops")
            .send(message)
            .expect("the logic thread runs while the inbox does");
        loop {
            let reply = self
                .from_logic
                .recv()
                .expect("the logic thread answers while it runs an entry");
            match reply {
                FromLogic::Emit(effect) => effects.push(effect),
                FromLogic::Silent(answer) => {
                    let silent = matches!(
                        self.queue.front,
                        Some(Front {
                            entry: Entry::CaretKey { silent: true, .. },
                            ..
                        })
                    );
                    answer.send(silent).expect("the logic thread waits for it");
                }
                FromLogic::FocusWaiting(answer) => {
                    let waiting = self
                        .queue
                        .waiting
                        .iter()
                        .any(|entry| matches!(entry, Entry::Focus { .. }));
                    answer.send(waiting).expect("the logic thread waits for it");
                }
                FromLogic::Ask {
                    reader,
                    window,
                    request,
                    due_us,
                } => {
                    self.queue.next_request = self.queue.next_request.saturating_add(1);
                    let id = RequestId(self.queue.next_request);
                    effects.push(Effect::Ask(Ask {
                        id,
                        reader,
                        window,
                        request: request.clone(),
                    }));
                    if let Some(front) = self.queue.front.as_mut() {
                        front.out = Some(Out {
                            id,
                            reader,
                            window,
                            request,
                            due_us,
                        });
                    }
                    return;
                }
                FromLogic::Done => {
                    self.queue.front = None;
                    return;
                }
            }
        }
    }
}
