//! What the three forms share: Core's inputs and effects under ADR 0003
//! ("Readers tell, Core asks"), the few pure rules every form calls, a fake
//! reader that answers Core's requests from a script, and the runner that
//! feeds a script to a form.
//!
//! Nothing here holds the event queue or decides when a request is out;
//! each form does that its own way.

use std::collections::BTreeMap;

/// A window: its own id and the top-level window whose tree it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// The top-level window, standing for its tree and owner chain.
    pub top: u32,
    /// The window itself.
    pub id: u32,
}

/// A reader Core asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// The focus listener, which keeps the UI Automation element a focus
    /// tell names.
    Listener,
    /// The outpost of one application.
    Outpost(u32),
}

/// Core's number for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// What Core asks a reader to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The listener reads the element it kept for a focus tell and
    /// answers a snapshot and the element's route.
    FocusElement { token: u32 },
    /// The outpost completes a UI Automation focus read from the route.
    CompleteFocus { route: u32 },
    /// The outpost reads the caret and holds it as the key's starting
    /// point.
    Caret,
    /// The outpost watches for the key's effect within the bound.
    Watch { within_ms: u32 },
}

/// A request sent to a reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: RequestId,
    pub reader: Reader,
    /// The window the request names, which a foreground change judges.
    pub window: Window,
    pub request: Request,
}

/// A result, the first of the five outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Element { name: String, route: u32 },
    Completed { role: String },
    Caret,
    Watched { line: String },
}

/// The five outcomes of a request (ADR 0003, "Readers tell, Core asks").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Result(Reply),
    Refused(String),
    NotThere,
    NotResponding,
    ReaderEnded,
}

/// Why Core stopped waiting for a request and told its reader to cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The request's due time passed.
    DueTime,
    /// The foreground moved to a window outside the request's tree.
    Foreground,
    /// A later key arrived while a caret key watched.
    LaterKey,
    /// A focus arrived while a caret key watched.
    PendingFocus,
}

/// How a request ended, as the logic waiting for it learns: an answer
/// (Core answers "the reader ended" itself when the reader ends), or
/// Core's own give-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    Answered(Outcome),
    Stopped(Stop),
}

impl Ending {
    /// The result, when the request ended with one.
    pub fn result(self) -> Option<Reply> {
        match self {
            Ending::Answered(Outcome::Result(reply)) => Some(reply),
            _ => None,
        }
    }
}

/// An input to Core, in the inbox's arrival order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A UI Automation focus tell from the listener: joins the event queue.
    Focus {
        app: u32,
        window: Window,
        token: u32,
    },
    /// A caret key the hook holds: joins the event queue.
    CaretKey { key: u32, app: u32, window: Window },
    /// A reader's answer: handled at once.
    Answer { id: RequestId, outcome: Outcome },
    /// The listener's foreground change: handled at once.
    Foreground { window: Window },
    /// A reader ended: handled at once.
    ReaderEnded { reader: Reader },
    /// A timer came due: handled at once.
    Tick,
}

/// An input with the time it carries, in microseconds on the run's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    pub at_us: u64,
    pub input: Input,
}

/// What Core does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Ask(Ask),
    Cancel {
        id: RequestId,
        reason: Stop,
    },
    /// The hook releases the held key to the application.
    SendKey(u32),
    Speak(String),
    NotRespondingSound,
}

/// A focus read's two requests share one due time, 13 seconds from the
/// first ask (ADR 0003, "A frozen window").
pub const FOCUS_DUE_US: u64 = 13_000_000;
/// A command's chain of requests shares one due time, 11 seconds.
pub const COMMAND_DUE_US: u64 = 11_000_000;
/// The caret watch's bound (ADR 0003, "Keys and commands").
pub const WATCH_MS: u32 = 100;

/// Whether `window` is in the tree of the foreground window `foreground`.
pub fn within(window: Window, foreground: Window) -> bool {
    window.top == foreground.top
}

/// What a focus says.
pub fn focus_speech(name: &str, role: &str) -> String {
    format!("{name} {role}")
}

/// One of the three forms of Core's event queue.
pub trait Form: Sized {
    fn new() -> Self;
    /// Handles one input and returns what it did, in order.
    fn feed(&mut self, input: &Timed) -> Vec<Effect>;
    /// The state as a dump writes it.
    fn dump(&self) -> String;
    /// A copy of the whole state, from which the run can go on, where the
    /// form can take one now.
    fn checkpoint(&self) -> Option<Self>;
    /// The one time Core waits for, which a timer is set to.
    fn next_due(&self) -> Option<u64>;
}

/// The kind of request a scripted answer goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    FocusElement,
    CompleteFocus,
    Caret,
    Watch,
}

fn kind(request: &Request) -> Kind {
    match request {
        Request::FocusElement { .. } => Kind::FocusElement,
        Request::CompleteFocus { .. } => Kind::CompleteFocus,
        Request::Caret => Kind::Caret,
        Request::Watch { .. } => Kind::Watch,
    }
}

/// One step of a script.
#[derive(Clone, Debug)]
pub enum Step {
    /// An input from anywhere but the fake reader, at a time.
    In(u64, Input),
    /// The fake reader answers the newest request of a kind, at a time,
    /// whether or not Core has since cancelled it: the script decides the
    /// order and the delay of every answer.
    Reply(u64, Kind, Outcome),
}

/// The fake reader: it keeps every request Core sent and every cancel,
/// and answers as the script says.
#[derive(Default)]
pub struct FakeReader {
    asked: Vec<Ask>,
    cancelled: BTreeMap<RequestId, Stop>,
}

impl FakeReader {
    /// Takes in what Core did.
    pub fn observe(&mut self, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::Ask(ask) => self.asked.push(ask.clone()),
                Effect::Cancel { id, reason } => {
                    self.cancelled.insert(*id, *reason);
                }
                _ => {}
            }
        }
    }

    /// The answer to the newest request of `to`.
    pub fn answer(&self, to: Kind, outcome: Outcome) -> Input {
        let ask = self
            .asked
            .iter()
            .rev()
            .find(|ask| kind(&ask.request) == to)
            .unwrap_or_else(|| panic!("the script answers a {to:?} Core never asked"));
        Input::Answer {
            id: ask.id,
            outcome,
        }
    }
}

/// What a run produced: every input as it was fed, and every effect in
/// order.
pub struct Run {
    pub inputs: Vec<Timed>,
    pub effects: Vec<Effect>,
}

/// Feeds `script` to `form`, the fake reader turning each scripted answer
/// into an input.
pub fn run<F: Form>(form: &mut F, script: &[Step]) -> Run {
    let mut reader = FakeReader::default();
    let mut inputs = Vec::new();
    let mut effects = Vec::new();
    for step in script {
        let timed = match step {
            Step::In(at_us, input) => Timed {
                at_us: *at_us,
                input: input.clone(),
            },
            Step::Reply(at_us, to, outcome) => Timed {
                at_us: *at_us,
                input: reader.answer(*to, outcome.clone()),
            },
        };
        let produced = form.feed(&timed);
        reader.observe(&produced);
        effects.extend(produced);
        inputs.push(timed);
    }
    Run { inputs, effects }
}

/// Feeds recorded inputs to `form`, returning every effect in order.
pub fn replay<F: Form>(form: &mut F, inputs: &[Timed]) -> Vec<Effect> {
    inputs.iter().flat_map(|input| form.feed(input)).collect()
}
