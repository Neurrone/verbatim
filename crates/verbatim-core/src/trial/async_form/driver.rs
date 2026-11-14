//! The async form's driver: the event queue and the one request out, with
//! the front entry's handling paused beside them as an async function.
//!
//! The driver owns the queue and the waiting. It starts the front entry's
//! function in place inside the step that takes the entry, and resumes it
//! in place inside the step that ends its request, so whatever the
//! function emits takes that step's place in the effects. It is polled
//! with a waker that does nothing, on Core's thread: no runtime, no lock.
//! The function reaches the queue only through its [`Cx`], between waits.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use super::super::harness::{
    Ask, Effect, Ending, Form, Input, Outcome, Reader, Request, RequestId, Stop, Timed, Window,
    within,
};
use super::logic;

/// Core: the queue, shared with the paused function, and the function.
pub struct AsyncCore {
    queue: Rc<RefCell<Queue>>,
    /// The front entry's function, paused at its request; present exactly
    /// when the queue has a front.
    task: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

/// The state the driver and the front's function share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Queue {
    now_us: u64,
    next_request: u64,
    front: Option<Front>,
    waiting: VecDeque<Entry>,
    /// What the function emitted while it ran, taken by the driver when
    /// the poll returns; empty between steps.
    emitted: Vec<Effect>,
    /// How the function's request ended, put here by the driver for the
    /// function's next poll; empty between steps.
    ending: Option<Ending>,
}

/// The front entry and its request, absent only while its function runs
/// between two requests.
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

impl Form for AsyncCore {
    fn new() -> Self {
        Self::from_queue(Queue::default())
    }

    fn feed(&mut self, timed: &Timed) -> Vec<Effect> {
        self.queue.borrow_mut().now_us = timed.at_us;
        let mut effects = Vec::new();
        match &timed.input {
            Input::Focus { app, window, token } => {
                if self.watching() {
                    self.give_up(Stop::PendingFocus, &mut effects);
                }
                self.queue.borrow_mut().waiting.push_back(Entry::Focus {
                    app: *app,
                    window: *window,
                    token: *token,
                });
            }
            Input::CaretKey { key, app, window } => {
                self.later_key(&mut effects);
                self.queue.borrow_mut().waiting.push_back(Entry::CaretKey {
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
                        .borrow_mut()
                        .waiting
                        .retain(|entry| !matches!(entry, Entry::Focus { .. }));
                }
            }
            Input::Tick => {
                let now_us = self.queue.borrow().now_us;
                if self.next_due().is_some_and(|due| due <= now_us) {
                    self.give_up(Stop::DueTime, &mut effects);
                }
            }
        }
        self.take_next(&mut effects);
        effects
    }

    fn dump(&self) -> String {
        format!("{self:?}")
    }

    fn checkpoint(&self) -> Option<Self> {
        self.task
            .is_none()
            .then(|| Self::from_queue(self.queue.borrow().clone()))
    }

    fn next_due(&self) -> Option<u64> {
        self.out().map(|out| out.due_us)
    }
}

impl std::fmt::Debug for AsyncCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let task = self.task.as_ref().map(|_| "paused");
        formatter
            .debug_struct("AsyncCore")
            .field("queue", &*self.queue.borrow())
            .field("task", &task)
            .finish()
    }
}

impl AsyncCore {
    fn from_queue(queue: Queue) -> Self {
        Self {
            queue: Rc::new(RefCell::new(queue)),
            task: None,
        }
    }

    fn out(&self) -> Option<Out> {
        self.queue
            .borrow()
            .front
            .as_ref()
            .and_then(|front| front.out.clone())
    }

    fn watching(&self) -> bool {
        self.out()
            .is_some_and(|out| matches!(out.request, Request::Watch { .. }))
    }

    /// A key arrived: every caret key ahead of it is marked silent, and a
    /// watching one is cancelled.
    fn later_key(&mut self, effects: &mut Vec<Effect>) {
        {
            let mut queue = self.queue.borrow_mut();
            if let Some(Front {
                entry: Entry::CaretKey { silent, .. },
                ..
            }) = &mut queue.front
            {
                *silent = true;
            }
            for entry in &mut queue.waiting {
                if let Entry::CaretKey { silent, .. } = entry {
                    *silent = true;
                }
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
        let mut queue = self.queue.borrow_mut();
        let mut kept = VecDeque::new();
        for entry in queue.waiting.drain(..) {
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
        queue.waiting = kept;
    }

    fn give_up(&mut self, reason: Stop, effects: &mut Vec<Effect>) {
        if let Some(out) = self.out() {
            effects.push(Effect::Cancel { id: out.id, reason });
            self.resume(Ending::Stopped(reason), effects);
        }
    }

    fn answered(&mut self, outcome: Outcome, effects: &mut Vec<Effect>) {
        if outcome == Outcome::NotResponding {
            effects.push(Effect::NotRespondingSound);
        }
        self.resume(Ending::Answered(outcome), effects);
    }

    /// Ends the front's request with `ending` and runs its function to its
    /// next request or its end.
    fn resume(&mut self, ending: Ending, effects: &mut Vec<Effect>) {
        {
            let mut queue = self.queue.borrow_mut();
            if let Some(front) = queue.front.as_mut() {
                front.out = None;
            }
            queue.ending = Some(ending);
        }
        self.poll(effects);
    }

    fn take_next(&mut self, effects: &mut Vec<Effect>) {
        while self.task.is_none() {
            let Some(entry) = self.queue.borrow_mut().waiting.pop_front() else {
                return;
            };
            self.queue.borrow_mut().front = Some(Front {
                entry: entry.clone(),
                out: None,
            });
            let cx = Cx {
                queue: Rc::clone(&self.queue),
            };
            self.task = Some(logic::start(cx, &entry));
            self.poll(effects);
        }
    }

    /// Polls the front's function once.
    fn poll(&mut self, effects: &mut Vec<Effect>) {
        let Some(task) = self.task.as_mut() else {
            return;
        };
        let polled = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        let mut queue = self.queue.borrow_mut();
        effects.append(&mut queue.emitted);
        if polled.is_ready() {
            queue.front = None;
            self.task = None;
        }
    }
}

/// The front function's handle to the queue.
pub(super) struct Cx {
    queue: Rc<RefCell<Queue>>,
}

impl Cx {
    pub(super) fn now_us(&self) -> u64 {
        self.queue.borrow().now_us
    }

    pub(super) fn emit(&self, effect: Effect) {
        self.queue.borrow_mut().emitted.push(effect);
    }

    /// Whether a later key has marked this caret key silent.
    pub(super) fn silent(&self) -> bool {
        matches!(
            self.queue.borrow().front,
            Some(Front {
                entry: Entry::CaretKey { silent: true, .. },
                ..
            })
        )
    }

    /// Whether a focus waits behind this entry.
    pub(super) fn focus_waiting(&self) -> bool {
        self.queue
            .borrow()
            .waiting
            .iter()
            .any(|entry| matches!(entry, Entry::Focus { .. }))
    }

    /// Sends a request and waits for its one ending.
    pub(super) fn ask(
        &self,
        reader: Reader,
        window: Window,
        request: Request,
        due_us: u64,
    ) -> Asking<'_> {
        Asking {
            cx: self,
            sending: Some((reader, window, request, due_us)),
        }
    }
}

/// One request: sent at its first poll, ended at the poll that finds its
/// ending.
pub(super) struct Asking<'a> {
    cx: &'a Cx,
    sending: Option<(Reader, Window, Request, u64)>,
}

impl Future for Asking<'_> {
    type Output = Ending;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Ending> {
        let mut queue = self.cx.queue.borrow_mut();
        if let Some((reader, window, request, due_us)) = self.sending.take() {
            queue.next_request = queue.next_request.saturating_add(1);
            let id = RequestId(queue.next_request);
            queue.emitted.push(Effect::Ask(Ask {
                id,
                reader,
                window,
                request: request.clone(),
            }));
            if let Some(front) = queue.front.as_mut() {
                front.out = Some(Out {
                    id,
                    reader,
                    window,
                    request,
                    due_us,
                });
            }
            return Poll::Pending;
        }
        match queue.ending.take() {
            Some(ending) => Poll::Ready(ending),
            None => Poll::Pending,
        }
    }
}
