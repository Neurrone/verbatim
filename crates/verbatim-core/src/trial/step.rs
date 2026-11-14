//! The step form: the event queue as a pure reducer. The front entry's
//! progress through its requests is one value, [`Stage`], whose variants
//! carry what each stage knows; every input is one call of
//! [`StepCore::feed`], which matches on that value.

use std::collections::VecDeque;

use super::harness::{
    Ask, COMMAND_DUE_US, Effect, FOCUS_DUE_US, Form, Input, Outcome, Reader, Reply, Request,
    RequestId, Stop, Timed, WATCH_MS, Window, focus_speech, within,
};

/// Core's state: the event queue, the front entry's request, and the
/// request numbers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StepCore {
    now_us: u64,
    next_request: u64,
    /// The front entry, while its request is out.
    asked: Option<Asked>,
    /// Entries not yet at their turn, in arrival order.
    waiting: VecDeque<Entry>,
}

/// An entry waiting its turn.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
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

/// The front entry with its one request out.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    id: RequestId,
    reader: Reader,
    window: Window,
    /// The due time of the entry's whole chain of requests.
    due_us: u64,
    stage: Stage,
}

/// Where the front entry is, and what it knows there.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    /// A focus's first request: the listener reads the element.
    FocusElement { app: u32 },
    /// A focus's second request: its outpost completes the read.
    FocusCompletion { name: String },
    /// A caret key's caret read; the key is still held.
    CaretStart { key: u32, app: u32, silent: bool },
    /// A caret key's watch; the key has been sent on.
    CaretWatch,
}

impl Form for StepCore {
    fn new() -> Self {
        Self::default()
    }

    fn feed(&mut self, timed: &Timed) -> Vec<Effect> {
        self.now_us = timed.at_us;
        let mut effects = Vec::new();
        match &timed.input {
            Input::Focus { app, window, token } => {
                if self.watching() {
                    self.give_up(Stop::PendingFocus, &mut effects);
                }
                self.waiting.push_back(Entry::Focus {
                    app: *app,
                    window: *window,
                    token: *token,
                });
            }
            Input::CaretKey { key, app, window } => {
                self.later_key(&mut effects);
                self.waiting.push_back(Entry::CaretKey {
                    key: *key,
                    app: *app,
                    window: *window,
                    silent: false,
                });
            }
            Input::Answer { id, outcome } => {
                if self.asked.as_ref().is_some_and(|asked| asked.id == *id) {
                    self.answered(outcome.clone(), &mut effects);
                }
            }
            Input::Foreground { window } => self.foreground(*window, &mut effects),
            Input::ReaderEnded { reader } => {
                if self
                    .asked
                    .as_ref()
                    .is_some_and(|asked| asked.reader == *reader)
                {
                    self.answered(Outcome::ReaderEnded, &mut effects);
                }
                if *reader == Reader::Listener {
                    self.waiting
                        .retain(|entry| !matches!(entry, Entry::Focus { .. }));
                }
            }
            Input::Tick => {
                if self.next_due().is_some_and(|due| due <= self.now_us) {
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
        Some(self.clone())
    }

    fn next_due(&self) -> Option<u64> {
        self.asked.as_ref().map(|asked| asked.due_us)
    }
}

impl StepCore {
    /// Whether the front entry is a caret key watching for its effect.
    fn watching(&self) -> bool {
        self.asked
            .as_ref()
            .is_some_and(|asked| asked.stage == Stage::CaretWatch)
    }

    /// A key arrived: every caret key ahead of it is marked silent, and a
    /// watching one is cancelled.
    fn later_key(&mut self, effects: &mut Vec<Effect>) {
        for entry in &mut self.waiting {
            if let Entry::CaretKey { silent, .. } = entry {
                *silent = true;
            }
        }
        if let Some(Asked {
            stage: Stage::CaretStart { silent, .. },
            ..
        }) = &mut self.asked
        {
            *silent = true;
        }
        if self.watching() {
            self.give_up(Stop::LaterKey, effects);
        }
    }

    /// The foreground moved to `foreground`: the front's request and every
    /// waiting entry outside its tree end.
    fn foreground(&mut self, foreground: Window, effects: &mut Vec<Effect>) {
        if self
            .asked
            .as_ref()
            .is_some_and(|asked| !within(asked.window, foreground))
        {
            self.give_up(Stop::Foreground, effects);
        }
        let mut kept = VecDeque::new();
        for entry in self.waiting.drain(..) {
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
        self.waiting = kept;
    }

    /// Core stops waiting for the front's request and tells its reader.
    fn give_up(&mut self, reason: Stop, effects: &mut Vec<Effect>) {
        if let Some(asked) = self.asked.take() {
            effects.push(Effect::Cancel {
                id: asked.id,
                reason,
            });
            ended(&asked.stage, effects);
        }
    }

    /// The front's request ended with `outcome`.
    fn answered(&mut self, outcome: Outcome, effects: &mut Vec<Effect>) {
        let Some(asked) = self.asked.take() else {
            return;
        };
        if outcome == Outcome::NotResponding {
            effects.push(Effect::NotRespondingSound);
        }
        let Outcome::Result(reply) = outcome else {
            ended(&asked.stage, effects);
            return;
        };
        match (asked.stage, reply) {
            (Stage::FocusElement { app }, Reply::Element { name, route }) => {
                let request = Request::CompleteFocus { route };
                let stage = Stage::FocusCompletion { name };
                self.ask(
                    Reader::Outpost(app),
                    asked.window,
                    request,
                    asked.due_us,
                    stage,
                    effects,
                );
            }
            (Stage::FocusCompletion { name }, Reply::Completed { role }) => {
                effects.push(Effect::Speak(focus_speech(&name, &role)));
            }
            (Stage::CaretStart { key, app, silent }, Reply::Caret) => {
                effects.push(Effect::SendKey(key));
                if !silent && !self.focus_waiting() {
                    let request = Request::Watch {
                        within_ms: WATCH_MS,
                    };
                    let reader = Reader::Outpost(app);
                    self.ask(
                        reader,
                        asked.window,
                        request,
                        asked.due_us,
                        Stage::CaretWatch,
                        effects,
                    );
                }
            }
            (Stage::CaretWatch, Reply::Watched { line }) => effects.push(Effect::Speak(line)),
            (stage, _) => ended(&stage, effects),
        }
    }

    fn focus_waiting(&self) -> bool {
        self.waiting
            .iter()
            .any(|entry| matches!(entry, Entry::Focus { .. }))
    }

    /// Takes waiting entries in order until one has a request out.
    fn take_next(&mut self, effects: &mut Vec<Effect>) {
        while self.asked.is_none() {
            let Some(entry) = self.waiting.pop_front() else {
                return;
            };
            match entry {
                Entry::Focus { app, window, token } => {
                    let request = Request::FocusElement { token };
                    let due_us = self.now_us.saturating_add(FOCUS_DUE_US);
                    let stage = Stage::FocusElement { app };
                    self.ask(Reader::Listener, window, request, due_us, stage, effects);
                }
                Entry::CaretKey {
                    key, silent: true, ..
                } => effects.push(Effect::SendKey(key)),
                Entry::CaretKey {
                    key,
                    app,
                    window,
                    silent: false,
                } => {
                    let due_us = self.now_us.saturating_add(COMMAND_DUE_US);
                    let stage = Stage::CaretStart {
                        key,
                        app,
                        silent: false,
                    };
                    self.ask(
                        Reader::Outpost(app),
                        window,
                        Request::Caret,
                        due_us,
                        stage,
                        effects,
                    );
                }
            }
        }
    }

    fn ask(
        &mut self,
        reader: Reader,
        window: Window,
        request: Request,
        due_us: u64,
        stage: Stage,
        effects: &mut Vec<Effect>,
    ) {
        self.next_request = self.next_request.saturating_add(1);
        let id = RequestId(self.next_request);
        effects.push(Effect::Ask(Ask {
            id,
            reader,
            window,
            request,
        }));
        self.asked = Some(Asked {
            id,
            reader,
            window,
            due_us,
            stage,
        });
    }
}

/// The front entry ended without its result: a caret key still held is
/// sent on, as a key is never swallowed.
fn ended(stage: &Stage, effects: &mut Vec<Effect>) {
    if let Stage::CaretStart { key, .. } = *stage {
        effects.push(Effect::SendKey(key));
    }
}
