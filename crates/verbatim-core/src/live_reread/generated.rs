//! The generated test of Core's focus speech and its live re-reads against
//! a reference model of NVDA's focus queue.
//!
//! The world, which knows nothing of Core's rules: three or four top-level
//! windows of two applications, read through MSAA by the application's
//! outpost or through UI Automation by the listener; windows owned by
//! another application's window or by one of their own, as Notepad's Find
//! dialog is; and windows shared by a frame process and a content process,
//! as `ApplicationFrameHost` hosts `SystemSettings`. Each focus reader keeps
//! an ancestry cache and answers a live re-read live for its last read, by
//! trace, and gone otherwise or when the element died; the outpost follows
//! every UI Automation focus the listener reads in its application, which
//! replaces its last MSAA read. Each application's messages reach Core in
//! order, the listener's in order, the two interleaving freely. The
//! listener and an application's outpost can be replaced; a new outpost
//! generation's nodes are new objects but for windows (ADR 0001, "Object
//! identity").
//!
//! The model ([`World::nvda`]) is NVDA's one ordered queue, taking the
//! reports in the order they reach Core (Dickson, 2026-11-08), each judged
//! against the last queued focus by the duplicate and foreground rules and
//! executed against the focus spoken before it: entered containers are
//! never cancelled, a focus's own speech is removed only when it expires,
//! and speech is cut when the lines meet only at the desktop
//! (`docs/nvda/events.md`; `docs/parity/focus-context.md`). To it are added:
//!
//! - the left mark (ADR 0001, "The foreground and the window title"), on the
//!   last queued focus, written in the order observed, with Dickson's limit
//!   that a change reaching Core late applies only to the reports Core has
//!   not yet judged (Dickson, 2026-11-08);
//! - the stale rule: a focus from another outpost generation observed
//!   before a report Core accepted is never queued, unless that report was a
//!   whole-window report of its own top-level window (Dickson, 2026-11-08,
//!   as clarified by Claude); this is the limit on focuses A, B, A;
//! - the background refusal: a focus observed before the newest foreground
//!   change Core has had, into an unrelated window, is never queued (ADR
//!   0001, "One input to Core, in arrival order");
//! - a report must wait for a live re-read when, at its place in the queue,
//!   it would enter an ancestor its reader took from its cache (ADR 0001,
//!   "Reusing an earlier ancestry"); its re-read is asked with the input
//!   that lets Core execute it, its own arrival or the end of the last wait
//!   ahead of it;
//! - a re-read answered gone, refused because its window was left, or ended
//!   with its reader ends the report unanswered: it stays the last queued
//!   focus, unspoken (Dickson, 2026-11-08), and the next focus is planned
//!   against the last focus spoken (Claude's); a report whose outpost
//!   generation has ended before it can execute ends so with no request
//!   (Claude's);
//! - a repeat of the last queued focus while it waits is no duplicate (ADR
//!   0001, "Reusing an earlier ancestry");
//! - after an outpost is replaced, the replacement's report of the kept
//!   focus is taken silently unless that focus was left (Dickson,
//!   2026-11-08);
//! - a report answered live is spoken with its answer's names (Claude's).
//!
//! Speech is judged at one timing, a judgement of the test's (Claude's):
//! nothing plays until the inputs stop, a cut empties the queue, and each
//! focus change removes the focus utterances that expired
//! (`docs/nvda/speech.md`, "Expired focus speech"), which Verbatim's queue
//! observes through [`FocusValidity::holds`].
//!
//! The invariants, whenever every message has reached Core: Verbatim's
//! speech queue holds what the model's holds, by element; Core's focus is
//! the model's; Core asks a live re-read for exactly the reports that must
//! wait, at the input that lets it execute them, each naming a report Core
//! took in of its own reader; and no name spoken for a report is one its
//! element had lost before the report was observed.

use std::collections::{BTreeMap, VecDeque};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_model::{
    Backend, Effect, FirstCachedAncestor, FocusReaderKind, FocusValidity, ForegroundWindow, Input,
    LiveReread, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role,
    SegmentContent, SpeechPriority, StateSet, TraceId, Utterance, WindowFacts, WindowHandle,
};

use crate::SrState;

/// A part of a window, by window index and part (0 window, 1 outer
/// container, 2 inner container, 3 and 4 controls).
type Part = (usize, u8);

/// An element of the world: a part and its identity, which changes when a
/// control dies and another takes its place, and for every part but the
/// window when its outpost generation is replaced.
type Element = (usize, u8, u32);

/// The desktop, the root of every NVDA focus line, never presentable.
const DESKTOP: Element = (usize::MAX, 0, 0);

/// Node numbers: the outpost's reads above the listener's, stand-ins
/// above both.
const OUTPOST_NODES: u64 = 1 << 40;
const STAND_IN_NODES: u64 = 1 << 41;

fn part_of((window, part, _): Element) -> Part {
    (window, part)
}

/// The labels an utterance says, in order.
fn labels(utterance: &Utterance) -> impl Iterator<Item = &String> {
    utterance
        .segments
        .iter()
        .filter_map(|segment| match &segment.content {
            SegmentContent::Label(label) => Some(label),
            _ => None,
        })
}

/// A top-level window of the world.
#[derive(Clone, Copy, Debug)]
struct Win {
    /// Its application, 1 or 2.
    app: u64,
    /// Whether it is native UI Automation, read by the listener, rather
    /// than MSAA, read by the outpost.
    uia: bool,
    /// The unowned window that owns it, of either application.
    owner: Option<usize>,
    /// Whether it has the pop-up style, so that its root owner is its
    /// owner.
    popup: bool,
    /// The other application, whose process holds the window's content,
    /// for a window shared by a frame and its content.
    content: Option<u64>,
}

fn world() -> impl Strategy<Value = Vec<Win>> {
    use proptest::prelude::*;
    proptest::collection::vec(
        (
            1u64..3,
            any::<bool>(),
            0u8..4,
            any::<bool>(),
            proptest::bool::weighted(0.3),
        ),
        3..5,
    )
    .prop_map(|raw| {
        let mut wins: Vec<Win> = Vec::new();
        for (index, (app, uia, owner, popup, shared)) in raw.into_iter().enumerate() {
            // An owner of the same application is read through the same
            // API as its dialog, as Notepad and its Find dialog are.
            let owner = (owner > 0 && index > 0)
                .then(|| usize::from(owner) % index)
                .filter(|&owner| {
                    let Win {
                        app: owner_app,
                        uia: owner_uia,
                        owner: owned,
                        content,
                        ..
                    } = wins[owner];
                    owned.is_none() && content.is_none() && (owner_app != app || owner_uia == uia)
                });
            wins.push(Win {
                app,
                uia,
                owner,
                popup,
                content: (shared && owner.is_none()).then_some(3 - app),
            });
        }
        wins
    })
}

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// The name of a window (`part` 0) or of one of its containers
    /// changes.
    Rename { window: u8, part: u8 },
    /// A focus on a control of the window in front, read by its reader.
    Focus { control: u8 },
    /// The user switches to `window`: `focus` on that control, twice with
    /// `repeat`, observed before the foreground change with `focus_first`;
    /// the change, and the outpost's whole-window focus of an MSAA window.
    Switch {
        window: u8,
        focus: Option<u8>,
        repeat: bool,
        focus_first: bool,
    },
    /// The oldest message on a channel reaches Core: application 1's (0),
    /// application 2's (1), or the listener's (2).
    Deliver { channel: u8 },
    /// A focus reader, by application and kind, takes the oldest live
    /// re-read asked of it and answers it; with `died`, the element died.
    Answer { reader: u8, died: bool },
    /// The listener is replaced.
    ReplaceListener,
    /// The outpost of application 1 or 2 ends, and a new generation takes
    /// its place.
    ReplaceOutpost { app: u8 },
    /// Every message reaches Core and every live re-read is answered.
    Drain,
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        2 => (0u8..4, 0u8..3).prop_map(|(window, part)| Action::Rename { window, part }),
        3 => (3u8..5).prop_map(|control| Action::Focus { control }),
        3 => (
            0u8..4,
            proptest::option::weighted(0.8, 3u8..5),
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(window, focus, repeat, focus_first)| Action::Switch {
                window,
                focus,
                repeat,
                focus_first,
            }),
        6 => (0u8..3).prop_map(|channel| Action::Deliver { channel }),
        3 => (0u8..4, proptest::bool::weighted(0.2))
            .prop_map(|(reader, died)| Action::Answer { reader, died }),
        1 => Just(Action::ReplaceListener),
        1 => (1u8..3).prop_map(|app| Action::ReplaceOutpost { app }),
        1 => Just(Action::Drain),
    ]
}

/// A focus reader's ancestry cache: its last read's ancestors, outermost
/// first, as it reported them, and the window titles it saved.
#[derive(Clone, Debug, Default)]
struct Cache {
    ancestors: Vec<(Element, String)>,
    titles: Vec<(u64, String)>,
}

/// A focus reader: the listener's reading thread for an application, or
/// the application's outpost.
#[derive(Clone, Debug, Default)]
struct Reader {
    cache: Cache,
    /// The trace of its last read.
    last: Option<u64>,
}

/// A channel to Core, whose messages arrive in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Channel {
    App(u64),
    Listener,
}

/// A message on its way to Core.
#[derive(Clone, Debug)]
enum Msg {
    /// The listener's foreground change.
    Change {
        seq: u64,
        window: usize,
        input: Input,
    },
    /// A focus report, a whole-window focus included.
    Report { seq: u64, input: Input },
    /// A live re-read's answer, with how it reads the report's chain.
    Answer {
        seq: u64,
        request: u64,
        input: Input,
        names: Vec<Reading>,
    },
    /// The outpost generation ended: on its application's channel, after
    /// every message it sent.
    Ended { outpost: u64 },
    /// A live re-read answered gone.
    Gone {
        seq: u64,
        request: u64,
        outpost: u64,
    },
}

/// What became of an observed focus at Core's intake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// Not yet at Core.
    InFlight,
    /// Refused at Core's intake: never queued.
    Removed,
    /// Taken into Core's order.
    Accepted,
}

/// How the live re-read Core asked for a report ended, with the input that
/// ended it, by its place in the order of inputs to Core.
#[derive(Clone, Debug)]
enum Outcome {
    /// Answered live, with how the answer reads the report's chain.
    Live(Vec<Reading>, u64),
    /// Answered gone, refused as its window was left, or ended with its
    /// reader: it ends unanswered.
    Unanswered(u64),
}

/// An observed focus.
#[derive(Clone, Debug)]
struct Rep {
    /// Its element: a window's part 0 for a whole-window focus.
    element: Element,
    /// The outpost generation of its reader.
    outpost: u64,
    status: Status,
    /// NVDA's live chain of it, the desktop first, as it was observed.
    chain: Vec<Element>,
    /// The ancestors its reader took from its cache.
    cached: Vec<Element>,
    /// Its chain below the desktop and itself, as read.
    names: Vec<Reading>,
    /// The live re-read Core asked for it, with the input it came with,
    /// and how it ended.
    asked: Option<(u64, u64)>,
    outcome: Option<Outcome>,
    /// Its place in the order of inputs to Core, once it has arrived.
    arrived_at: Option<u64>,
}

/// An entry as a report reads it: its name, and the role it is read
/// with, a window read through MSAA or as a stand-in being a pane.
type Reading = (String, &'static str);

/// An observation, in the order observed.
#[derive(Clone, Debug)]
enum Obs {
    /// A foreground change into the window, and its place in the order of
    /// inputs to Core, once it has arrived.
    Change(usize, Option<u64>),
    Focus(Rep),
}

/// A version of a part's name: the part, and until when it held the name.
#[derive(Clone, Copy, Debug)]
struct Named {
    part: Part,
    until: Option<u64>,
}

/// NVDA's focus state, as the model keeps it.
#[derive(Clone, Debug, Default)]
struct Nvda {
    /// The last queued focus, its observation, its place in the order of
    /// inputs to Core, and, when it waited for a live re-read, the input
    /// that ended the wait.
    last_queued: Option<(Element, u64, u64, Option<u64>)>,
    /// The focus spoken last, with the outpost generation that read it,
    /// and its ancestors, the desktop first.
    focus: Option<Element>,
    focus_outpost: u64,
    /// The reading Core keeps of the line below the desktop and the focus:
    /// a window matched by its handle keeps the name it was entered with
    /// (`docs/parity/focus-context.md`, "Focus-ancestry context").
    named: Vec<Reading>,
    line: Vec<Element>,
    foreground: Option<Element>,
    /// The speech queue: each element spoken and whether it had the focus.
    queue: Vec<(Element, bool)>,
}

/// A live re-read on its way to a reader: its request number, the
/// report's observation, and the outpost generation asked.
type Request = (u64, u64, u64);

/// One case's world, its focus readers, Core, and what reached Core.
struct World {
    wins: Vec<Win>,
    /// Each part's current name.
    names: BTreeMap<Part, String>,
    /// Every name ever given, each unique.
    labels: BTreeMap<String, Named>,
    renames: u32,
    /// Each control's incarnation.
    incarnations: BTreeMap<Part, u32>,
    /// Each application's outpost generation.
    generations: BTreeMap<u64, u32>,
    readers: BTreeMap<(u64, FocusReaderKind), Reader>,
    /// The live re-reads asked of each reader, by application and kind, not
    /// yet taken.
    requests: BTreeMap<(u64, FocusReaderKind), VecDeque<Request>>,
    channels: BTreeMap<Channel, VecDeque<Msg>>,
    state: SrState,
    obs: BTreeMap<u64, Obs>,
    /// The newest foreground change Core has had: its observation and
    /// window.
    core_front: (u64, usize),
    /// The ordering stamps: the observation, outpost generation and window
    /// of each report Core accepted into its order, and whether it was a
    /// whole-window report.
    stamps: Vec<(u64, u64, usize, bool)>,
    /// Each outpost generation's end: its input, when it happened, and the
    /// generation.
    reader_ends: Vec<(u64, u64, u64)>,
    /// Verbatim's speech queue: each utterance's parts and validity.
    heard: Vec<(Vec<Part>, Option<FocusValidity>)>,
    /// How many inputs Core has taken.
    arrivals: u64,
    /// The system's foreground window.
    front: usize,
    now: u64,
    log: Vec<String>,
}

impl World {
    fn new(wins: Vec<Win>) -> Self {
        let mut names = BTreeMap::new();
        let mut labels = BTreeMap::new();
        for window in 0..wins.len() {
            for (part, name) in [
                (0, format!("Window {window}")),
                (1, format!("Outer {window}")),
                (2, format!("Inner {window}")),
                (3, format!("A{window}")),
                (4, format!("B{window}")),
            ] {
                labels.insert(
                    name.clone(),
                    Named {
                        part: (window, part),
                        until: None,
                    },
                );
                names.insert((window, part), name);
            }
        }
        let mut world = Self {
            wins,
            names,
            labels,
            renames: 0,
            incarnations: BTreeMap::new(),
            generations: BTreeMap::new(),
            readers: BTreeMap::new(),
            requests: BTreeMap::new(),
            channels: BTreeMap::new(),
            state: SrState::new(),
            obs: BTreeMap::new(),
            core_front: (100, 0),
            stamps: Vec::new(),
            reader_ends: Vec::new(),
            heard: Vec::new(),
            arrivals: 0,
            front: 0,
            now: 100,
            log: Vec::new(),
        };
        // The user starts in the first window, with no focus taken.
        world.obs.insert(100, Obs::Change(0, Some(0)));
        let change = world.change(0, 100);
        let effects = crate::reduce(&mut world.state, &change);
        world.take(&effects);
        world
    }

    fn name(&self, element: Element) -> String {
        self.names[&part_of(element)].clone()
    }

    fn handle(window: usize) -> u64 {
        0x110 + 0x10 * window as u64
    }

    /// `GA_ROOTOWNER`: the owner for a pop-up owned window, the window
    /// itself otherwise.
    fn root_owner(&self, window: usize) -> u64 {
        match self.wins[window] {
            Win {
                owner: Some(owner),
                popup: true,
                ..
            } => Self::handle(owner),
            _ => Self::handle(window),
        }
    }

    /// Whether two windows are unrelated: neither the same top-level
    /// window nor of the same root owner (`docs/nvda/events.md`,
    /// "Acceptance filtering"; ADR 0001, "The foreground and the window
    /// title").
    fn unrelated(&self, one: usize, other: usize) -> bool {
        one != other && self.root_owner(one) != self.root_owner(other)
    }

    fn kind(&self, window: usize) -> FocusReaderKind {
        if self.wins[window].uia {
            FocusReaderKind::Listener
        } else {
            FocusReaderKind::Outpost
        }
    }

    /// The application whose process holds `part` of `window`: the
    /// content's for the containers and the second control of a shared
    /// window, the window's own otherwise.
    fn app_of(&self, (window, part): Part) -> u64 {
        match self.wins[window].content {
            Some(content) if matches!(part, 1 | 2 | 4) => content,
            _ => self.wins[window].app,
        }
    }

    /// The outpost generation of application `app`, as its node ids carry
    /// it.
    fn outpost(&self, app: u64) -> u64 {
        app + 10 * u64::from(self.generations.get(&app).copied().unwrap_or(0))
    }

    /// `part` of `window` as it is now: a window is the same object across
    /// outpost generations, any other part is not, and a control that died
    /// is another (ADR 0001, "Object identity").
    fn el(&self, window: usize, part: u8) -> Element {
        if part == 0 {
            return (window, 0, 0);
        }
        let generation = self
            .generations
            .get(&self.app_of((window, part)))
            .copied()
            .unwrap_or(0);
        let incarnation = self.incarnations.get(&(window, part)).copied().unwrap_or(0);
        (window, part, 100 * generation + incarnation)
    }

    /// The node a reader of `kind` gives `element` now, under the current
    /// outpost generation of its application, as every node is minted when
    /// it is read.
    fn node_id(&self, kind: FocusReaderKind, (window, part, identity): Element) -> NodeId {
        let offset = match kind {
            FocusReaderKind::Listener => 0,
            FocusReaderKind::Outpost => OUTPOST_NODES,
        };
        NodeId::new(
            offset + 10_000 * u64::from(identity) + 10 * window as u64 + u64::from(part) + 1,
        )
        .with_outpost(OutpostId(self.outpost(self.app_of((window, part)))))
    }

    /// The element whose node `id` is, by either reader or as a stand-in.
    fn element_of(&self, id: NodeId) -> Option<Element> {
        let number = id.number().checked_sub(1)?;
        let (number, identity) = if number >= STAND_IN_NODES {
            (number - STAND_IN_NODES, 0)
        } else {
            let number = number % OUTPOST_NODES;
            (number % 10_000, u32::try_from(number / 10_000).ok()?)
        };
        let window = usize::try_from(number / 10).ok()?;
        let part = u8::try_from(number % 10).ok()?;
        (window < self.wins.len() && part < 5).then_some((window, part, identity))
    }

    fn snapshot(&self, kind: FocusReaderKind, element: Element, name: String) -> NodeSnapshot {
        let (window, part, _) = element;
        let backend = match kind {
            FocusReaderKind::Listener => Backend::Uia,
            FocusReaderKind::Outpost => Backend::Msaa,
        };
        NodeSnapshot {
            id: self.node_id(kind, element),
            backend,
            role: match (part, backend) {
                // An MSAA window is reached as its client, a pane marked as
                // a top-level window's (`docs/parity/focus-context.md`,
                // "Top-level windows in the ancestry").
                (0, Backend::Msaa) => Role::Pane,
                (0, _) => Role::Window,
                (1 | 2, _) => Role::Group,
                _ => Role::Button,
            },
            name: Some(name),
            value: None,
            states: StateSet::new(),
            details: NodeDetails {
                window: (part == 0).then_some(WindowHandle(Self::handle(window))),
                top_level_client: part == 0 && backend == Backend::Msaa,
                ..NodeDetails::default()
            },
        }
    }

    /// The stand-in outpost generation `outpost` gives the window `window`
    /// above its process boundary, described from Windows' state
    /// (`docs/parity/windows-and-processes.md`, "Content hosted in another
    /// process's window").
    fn stand_in(outpost: u64, window: usize, name: String) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId::new(STAND_IN_NODES + 10 * window as u64 + 1)
                .with_outpost(OutpostId(outpost)),
            backend: Backend::Msaa,
            role: Role::Pane,
            name: Some(name),
            value: None,
            states: StateSet::new(),
            details: NodeDetails {
                window: Some(WindowHandle(Self::handle(window))),
                top_level_client: true,
                above_boundary: true,
                ..NodeDetails::default()
            },
        }
    }

    /// The parts of `control`'s window its own process walks, innermost
    /// first: a shared window's frame control is directly in the window
    /// and its content control in the containers; a UI Automation window
    /// owned by a window of its own application has the owner above it.
    fn own_walk(&self, control: Element) -> Vec<Element> {
        let window = control.0;
        let win = self.wins[window];
        match (win.content, control.1) {
            (Some(_), 3) => vec![self.el(window, 0)],
            (Some(_), _) => vec![self.el(window, 2), self.el(window, 1)],
            (None, _) => {
                let mut walk = vec![self.el(window, 2), self.el(window, 1), self.el(window, 0)];
                if let (true, Some(owner)) = (win.uia, win.owner)
                    && self.wins[owner].app == win.app
                {
                    walk.push(self.el(owner, 0));
                }
                walk
            }
        }
    }

    /// The reader's read of `control`: its ancestors, outermost first,
    /// each with the name it reports and whether it came from the cache;
    /// with `live`, no cache is used. The cache is refreshed by the read.
    /// A window above the reader's process boundary, the owner of a UI
    /// Automation window owned across processes or a shared window's frame
    /// for its content, is a stand-in, read live and never cached.
    fn read(
        &mut self,
        kind: FocusReaderKind,
        control: Element,
        live: bool,
    ) -> Vec<(Element, String, bool)> {
        let window = control.0;
        let application = self.app_of(part_of(control));
        let cache = self
            .readers
            .entry((application, kind))
            .or_default()
            .cache
            .clone();
        let titles_hold = cache.titles.iter().all(|(handle, title)| {
            let index = (0..self.wins.len())
                .find(|&w| Self::handle(w) == *handle)
                .unwrap();
            self.names[&(index, 0)] == *title
        });
        let usable = !live && titles_hold;
        let mut chain = Vec::new();
        let mut met = None;
        for element in self.own_walk(control) {
            chain.push((element, self.name(element), false));
            if usable
                && let Some(at) = cache
                    .ancestors
                    .iter()
                    .position(|(cached, _)| *cached == element)
            {
                met = Some(at);
                break;
            }
        }
        chain.reverse();
        if let Some(at) = met {
            let above: Vec<_> = cache.ancestors[..at]
                .iter()
                .map(|(element, name)| (*element, name.clone(), true))
                .collect();
            chain = above.into_iter().chain(chain).collect();
        }
        let titles = chain
            .iter()
            .filter(|((_, part, _), _, _)| *part == 0)
            .map(|((window, _, _), name, cached)| {
                let title = if *cached {
                    cache
                        .titles
                        .iter()
                        .find(|(handle, _)| *handle == Self::handle(*window))
                        .map_or_else(|| name.clone(), |(_, title)| title.clone())
                } else {
                    self.names[&(*window, 0)].clone()
                };
                (Self::handle(*window), title)
            })
            .collect();
        self.readers.get_mut(&(application, kind)).unwrap().cache = Cache {
            ancestors: chain
                .iter()
                .map(|(element, name, _)| (*element, name.clone()))
                .collect(),
            titles,
        };
        if let (true, Some(owner)) = (self.wins[window].uia, self.wins[window].owner)
            && self.wins[owner].app != self.wins[window].app
        {
            chain.insert(
                0,
                (self.el(owner, 0), self.names[&(owner, 0)].clone(), false),
            );
        }
        if self.wins[window].content.is_some() && control.1 == 4 {
            chain.insert(
                0,
                (self.el(window, 0), self.names[&(window, 0)].clone(), false),
            );
        }
        chain
    }

    /// How `control`'s read `chain` by a reader of `kind` reads NVDA's
    /// chain of it below the desktop and itself.
    fn reading(
        &self,
        kind: FocusReaderKind,
        control: Element,
        chain: &[(Element, String, bool)],
    ) -> Vec<Reading> {
        let app = self.app_of(part_of(control));
        self.nvda_chain(control)
            .into_iter()
            .skip(1)
            .map(|element| {
                let name = chain
                    .iter()
                    .find(|(read, _, _)| *read == element)
                    .map_or_else(|| self.name(element), |(_, name, _)| name.clone());
                let role = match element.1 {
                    0 if kind == FocusReaderKind::Outpost
                        || self.app_of(part_of(element)) != app =>
                    {
                        "pane"
                    }
                    0 => "window",
                    _ => "group",
                };
                (name, role)
            })
            .chain([(
                self.name(control),
                if control.1 == 0 { "pane" } else { "button" },
            )])
            .collect()
    }

    fn facts(&self, window: usize) -> WindowFacts {
        let handle = WindowHandle(Self::handle(window));
        let front = Self::handle(self.front);
        let root_owner = self.root_owner(window);
        WindowFacts {
            top_level: handle,
            root_owner: WindowHandle(root_owner),
            topmost: false,
            under_active_window: None,
            in_foreground: handle.0 == front
                || root_owner == front
                || root_owner == self.root_owner(self.front),
            foreground_window: WindowHandle(front),
            class: String::new(),
            application: String::new(),
            alt_held: false,
            foreground_is_shell: false,
        }
    }

    fn change(&self, window: usize, at_us: u64) -> Input {
        Input::Foreground(ForegroundWindow {
            window: WindowHandle(Self::handle(window)),
            root_owner: WindowHandle(self.root_owner(window)),
            process: Pid(u32::try_from(self.wins[window].app).unwrap()),
            observed_at_us: at_us,
            own: false,
            thread: 0,
            console: false,
            integrity: None,
            own_integrity: None,
        })
    }

    /// The report of `control` by a reader of `kind` with its read `chain`:
    /// a whole-window focus for a window, the outpost's, with no chain.
    fn report_input(
        &self,
        kind: FocusReaderKind,
        control: Element,
        chain: &[(Element, String, bool)],
        (at_us, answers): (u64, Option<u64>),
    ) -> Input {
        let window = control.0;
        let first_cached_ancestor = chain
            .iter()
            .rposition(|(_, _, cached)| *cached)
            .map(|index| FirstCachedAncestor {
                index: u32::try_from(index).unwrap(),
                reader: kind,
            });
        let node = self.snapshot(kind, control, self.name(control));
        let app = self.app_of(part_of(control));
        let outpost = self.outpost(app);
        Input::Event {
            trace_id: TraceId::from_raw(at_us),
            observed_at_us: at_us,
            source: Pid(u32::try_from(app).unwrap()),
            backend: node.backend,
            window: Some(self.facts(window)),
            event: NormalizedEvent::FocusChanged {
                node,
                whole_window: control.1 == 0,
                ancestors: chain
                    .iter()
                    .map(|(element, name, _)| {
                        if self.app_of(part_of(*element)) == app {
                            self.snapshot(kind, *element, name.clone())
                        } else {
                            Self::stand_in(outpost, element.0, name.clone())
                        }
                    })
                    .collect(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                first_cached_ancestor,
                answers,
            },
            purpose: None,
        }
    }

    // ---- NVDA's model ----

    /// NVDA's live chain of `element`, its ancestors, the desktop first.
    /// An MSAA window's client, NVDA's focus on a foreground change, is
    /// directly under the desktop; the window object between them is left
    /// out, as it is never presentable and every line through its window
    /// holds it (`docs/parity/focus-context.md`, "Focus-ancestry
    /// context"). A UI Automation window owned by another window has the
    /// owner above it, NVDA's raw-view walk going on from the dialog to its
    /// owner, read through the owner's own API, the same object as the
    /// owner's own line holds; an MSAA window's parent is the desktop
    /// (`docs/parity/windows-and-processes.md`, "Content hosted in another
    /// process's window"). A shared window's frame control is directly in
    /// its window, and its content under the window, which NVDA reads
    /// through the frame's own API (the same document, "A window and its
    /// content in two processes").
    fn nvda_chain(&self, element: Element) -> Vec<Element> {
        let (window, part, _) = element;
        let mut chain = vec![DESKTOP];
        if part == 0 {
            return chain;
        }
        if self.wins[window].content.is_some() && part == 3 {
            chain.push(self.el(window, 0));
            return chain;
        }
        if let (true, Some(owner)) = (self.wins[window].uia, self.wins[window].owner) {
            chain.push(self.el(owner, 0));
        }
        chain.extend([self.el(window, 0), self.el(window, 1), self.el(window, 2)]);
        chain
    }

    /// The stale rule at Core's intake (Dickson, 2026-11-08, as clarified
    /// by Claude): a focus observed at `seq` from another outpost
    /// generation than a report Core accepted, observed after it, is never
    /// queued, unless that report was a whole-window report of its own
    /// top-level window, which Windows can time after the content's focus;
    /// two ordinary reports in a shared window follow the rule.
    fn stale(&self, seq: u64, rep: &Rep) -> bool {
        self.stamps.iter().any(|&(at, outpost, stamped, whole)| {
            seq < at && outpost != rep.outpost && !(whole && stamped == rep.element.0)
        })
    }

    /// The background refusal: a focus observed at `seq` in `window` before
    /// the newest foreground change Core has had, into a window unrelated
    /// to its own (ADR 0001, "One input to Core, in arrival order"). It is
    /// never queued at intake, and a live re-read's answer refused so ends
    /// its report unanswered (ADR 0001, "Limits accepted"; Dickson,
    /// 2026-11-08).
    fn background(&self, seq: u64, window: usize) -> bool {
        let (changed_at, front) = self.core_front;
        changed_at > seq && self.unrelated(window, front)
    }

    /// A reader ends, the listener or an outpost generation: a report
    /// whose live re-read was asked of it and not answered ends unanswered.
    fn reader_ended(&mut self, ended: impl Fn(&Self, &Rep) -> bool) {
        let at = self.arrivals;
        let lost: Vec<u64> = self
            .obs
            .iter()
            .filter_map(|(&seq, obs)| match obs {
                Obs::Focus(rep) if rep.element.1 != 0 && ended(self, rep) => Some(seq),
                _ => None,
            })
            .collect();
        for seq in lost {
            let rep = self.rep(seq);
            if rep.asked.is_some() && rep.outcome.is_none() {
                rep.outcome = Some(Outcome::Unanswered(at));
            }
        }
    }

    /// Whether the outpost generation of `rep`, observed at `seq`, ended by
    /// input `executes_at`, so that it can no longer be re-read (Claude's).
    /// The listener's replacement carries no generation Core could judge
    /// by.
    fn reader_gone(&self, rep: &Rep, seq: u64, executes_at: u64) -> bool {
        self.reader_ends
            .iter()
            .any(|&(at, when, ended)| at <= executes_at && when > seq && ended == rep.outpost)
    }

    /// NVDA's queue-time judgement of `rep` in `nvda`: whether it is
    /// dropped, and whether the left mark makes it meet nothing of the
    /// line left.
    ///
    /// The mark lives on the last queued focus (Dickson, 2026-11-08). It is
    /// written by the foreground changes that reached Core before `rep`,
    /// were observed before it, and apply to the last queued focus:
    /// observed after it, or reaching Core after it, Dickson's limit. The
    /// newest such change, in the order observed, into a window unrelated
    /// to the last queued focus is the window it was left for. While it
    /// stands, a focus not in that window is no duplicate and is not
    /// refused, and meets nothing of the line left but the desktop
    /// (`docs/parity/focus-context.md`, "Window announcement on switching
    /// applications").
    ///
    /// A focus is a duplicate when it is the last queued focus
    /// (`docs/parity/focus-events.md`, "Duplicate focus suppression"); a
    /// whole-window focus, NVDA's foreground event, is dropped when the
    /// last queued focus is in its window (`docs/nvda/events.md`,
    /// "Foreground windows"). A repeat of the last queued focus while that
    /// one waits for its live re-read, `repeats`, is no duplicate of it.
    fn judge(&self, nvda: &Nvda, rep: &Rep, seq: u64, repeats: bool) -> (bool, bool) {
        let x = rep.element;
        let arrived = rep.arrived_at.unwrap_or(u64::MAX);
        let marked =
            nvda.last_queued
                .as_ref()
                .is_some_and(|(queued, queued_seq, queued_arrived, _)| {
                    self.obs
                        .iter()
                        .filter_map(|(&at, obs)| match obs {
                            Obs::Change(window, Some(change_arrived)) => {
                                Some((at, *change_arrived, *window))
                            }
                            _ => None,
                        })
                        .rfind(|(at, change_arrived, window)| {
                            *change_arrived < arrived
                                && *at < seq
                                && (*at > *queued_seq || *change_arrived > *queued_arrived)
                                && self.unrelated(queued.0, *window)
                        })
                        .is_some_and(|(_, _, left_for)| left_for != x.0)
                });
        let dropped = !marked
            && !repeats
            && nvda.last_queued.as_ref().is_some_and(|(queued, _, _, _)| {
                if x.1 == 0 {
                    queued.0 == x.0
                } else {
                    *queued == x
                }
            });
        (dropped, marked)
    }

    /// NVDA's execution of `rep`, judged with `marked`, in `nvda`: returns
    /// the containers it enters (`docs/nvda/events.md`, "What a focus event
    /// actually does"; `docs/parity/focus-context.md`, "Focus-ancestry
    /// context"). The new chain meets the old line at its innermost shared
    /// entry, and the entries below it are entered, each spoken as never
    /// having had the focus, then the focus. Once the new line is set, each
    /// waiting focus utterance whose object is no longer the focus, an
    /// ancestor or the foreground object is removed (`docs/nvda/speech.md`,
    /// "Expired focus speech"); when the lines meet only at the desktop,
    /// speech is cut first and the window becomes the foreground object.
    fn execute(nvda: &mut Nvda, rep: &Rep, marked: bool) -> Vec<Element> {
        let x = rep.element;
        // Core keeps the focus of an outpost generation that ended, and a
        // report of its replacement that matches that copy, the same part
        // under the same names, is taken silently, unless the focus was
        // left, when it is announced (Dickson, 2026-11-08;
        // `docs/parity/focus-events.md`, "Recovery after an outpost is
        // replaced").
        if let Some(focus) = nvda.focus
            && !marked
            && rep.outpost != nvda.focus_outpost
            && rep.outpost % 10 == nvda.focus_outpost % 10
            && part_of(focus) == part_of(x)
            && nvda.named == rep.names
        {
            nvda.focus = Some(x);
            nvda.focus_outpost = rep.outpost;
            nvda.line = rep.chain.clone();
            nvda.named.clone_from(&rep.names);
            return Vec::new();
        }
        let old: Vec<Element> = if marked {
            vec![DESKTOP]
        } else {
            nvda.line.iter().copied().chain(nvda.focus).collect()
        };
        let chain: Vec<Element> = rep.chain.iter().copied().chain([x]).collect();
        let meet = chain.iter().rposition(|entry| old.contains(entry));
        let (full, entered, named) = match meet {
            Some(at) => {
                let kept = old.iter().position(|entry| *entry == chain[at]).unwrap();
                let mut full = old[..=kept].to_vec();
                full.extend(&chain[at + 1..]);
                let mut named: Vec<Reading> = if marked {
                    Vec::new()
                } else {
                    nvda.named[..kept].to_vec()
                };
                named.extend(rep.names[at..].iter().cloned());
                (full, chain[at + 1..].to_vec(), named)
            }
            None => (chain.clone(), chain.clone(), rep.names.clone()),
        };
        nvda.focus = Some(x);
        nvda.focus_outpost = rep.outpost;
        nvda.named = named;
        nvda.line = full[..full.len() - 1].to_vec();
        let (focus, line, foreground) = (nvda.focus, &nvda.line, nvda.foreground);
        nvda.queue.retain(|(element, had_focus)| {
            !had_focus
                || Some(*element) == focus
                || line.contains(element)
                || foreground == Some(*element)
        });
        if meet.is_none_or(|at| at == 0) {
            nvda.queue.clear();
            nvda.foreground = Some((x.0, 0, 0));
        }
        let entered: Vec<Element> = entered
            .into_iter()
            .filter(|entry| *entry != DESKTOP && *entry != x)
            .collect();
        nvda.queue
            .extend(entered.iter().map(|entry| (*entry, false)));
        nvda.queue.push((x, true));
        entered
    }

    /// A repeat of the focus keeps its newer reading of the focus and of
    /// the line's live nodes, never of its cached ones
    /// (`docs/parity/focus-context.md`, "Focus-ancestry context").
    fn keep_newer_reading(nvda: &mut Nvda, rep: &Rep) {
        if nvda.focus != Some(rep.element) {
            return;
        }
        let held: Vec<Element> = nvda.line[1..].iter().copied().chain(nvda.focus).collect();
        let read: Vec<Element> = rep.chain[1..]
            .iter()
            .copied()
            .chain([rep.element])
            .collect();
        for (index, element) in held.iter().enumerate() {
            if let Some(at) = read.iter().position(|entry| entry == element)
                && !rep.cached.contains(element)
                && index < nvda.named.len()
            {
                nvda.named[index].clone_from(&rep.names[at]);
            }
        }
    }

    /// NVDA's queue over the reports Core accepted, in the order they
    /// reached it (Dickson, 2026-11-08), with whether each had to wait for
    /// a live re-read decided at its place in it, every wait ahead of it
    /// known, and Core's live re-reads checked against that.
    fn nvda(&self) -> Nvda {
        let fail = |what: String| -> ! { panic!("{what}: {}", self.log.join("\n")) };
        let mut nvda = Nvda::default();
        let mut order: Vec<(u64, u64, &Rep)> = self
            .obs
            .iter()
            .filter_map(|(&seq, obs)| match obs {
                Obs::Focus(rep) if rep.status == Status::Accepted => {
                    rep.arrived_at.map(|arrived| (arrived, seq, rep))
                }
                _ => None,
            })
            .collect();
        order.sort_by_key(|(arrived, _, _)| *arrived);
        // The input that ends each wait so far.
        let mut waits: Vec<u64> = Vec::new();
        for (arrived, seq, rep) in order {
            let repeats = nvda.last_queued.is_some_and(|(queued, _, _, ended)| {
                queued == rep.element && ended.is_some_and(|ended| ended > arrived)
            });
            let (dropped, marked) = self.judge(&nvda, rep, seq, repeats);
            if dropped {
                if rep.asked.is_some() {
                    fail(format!(
                        "a live re-read for {seq}, which NVDA's queue drops"
                    ));
                }
                Self::keep_newer_reading(&mut nvda, rep);
                continue;
            }
            let mut taken = nvda.clone();
            let entered = Self::execute(&mut taken, rep, marked);
            let due = entered.iter().any(|entry| rep.cached.contains(entry));
            let executes_at = waits
                .iter()
                .copied()
                .filter(|ended| *ended > arrived)
                .max()
                .unwrap_or(arrived);
            if due && self.reader_gone(rep, seq, executes_at) {
                if let Some((_, at)) = rep.asked {
                    fail(format!(
                        "a live re-read for {seq} at input {at}, whose outpost generation had ended"
                    ));
                }
                waits.push(executes_at);
                nvda.last_queued = Some((rep.element, seq, arrived, Some(executes_at)));
                continue;
            }
            match (due, rep.asked) {
                (false, None) => {
                    nvda = taken;
                    nvda.last_queued = Some((rep.element, seq, arrived, None));
                }
                (true, Some((_, at))) => {
                    if at != executes_at {
                        fail(format!(
                            "the live re-read for {seq} asked at input {at}, \
                             not at input {executes_at}, which lets Core execute it"
                        ));
                    }
                    let Some(outcome) = &rep.outcome else {
                        fail(format!("the live re-read for {seq} undecided"));
                    };
                    let ended = match outcome {
                        Outcome::Live(names, ended) => {
                            let answered = Rep {
                                names: names.clone(),
                                ..rep.clone()
                            };
                            Self::execute(&mut nvda, &answered, marked);
                            *ended
                        }
                        Outcome::Unanswered(ended) => *ended,
                    };
                    waits.push(ended);
                    nvda.last_queued = Some((rep.element, seq, arrived, Some(ended)));
                }
                (true, None) => fail(format!(
                    "no live re-read for {seq}, which must wait for one, at input {executes_at}"
                )),
                (false, Some((_, at))) => fail(format!(
                    "a live re-read for {seq} at input {at}, which needs none"
                )),
            }
        }
        nvda
    }

    // ---- Verbatim's speech ----

    /// Takes Core's `effects`: speech into Verbatim's speech queue, where
    /// an interrupting utterance or a stop empties the queue and each focus
    /// Core names removes the waiting utterances whose validity fails
    /// against it ([`FocusValidity::holds`]), each name spoken for a
    /// report being one its element still held when the report was
    /// observed or later; and the live re-reads it asks, routed to their
    /// readers. Any other effect fails the case.
    fn take(&mut self, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::ForKey(_, inner) => self.take(std::slice::from_ref(inner)),
                Effect::Speak(utterance, _) => {
                    if utterance.priority == SpeechPriority::Interrupt {
                        self.heard.clear();
                    }
                    let observed: u64 = utterance.trace_id.to_string().parse().unwrap();
                    let parts = labels(utterance)
                        .map(|label| {
                            let named = self.labels.get(label).unwrap_or_else(|| {
                                panic!("{label:?} is no name: {}", self.log.join("\n"))
                            });
                            assert!(
                                named.until.is_none_or(|until| until > observed),
                                "{label:?} spoken for the report observed at {observed}, \
                                 renamed at {:?}: {}",
                                named.until,
                                self.log.join("\n")
                            );
                            named.part
                        })
                        .collect();
                    self.heard.push((parts, utterance.validity));
                }
                Effect::StopSpeech => self.heard.clear(),
                Effect::DropExpiredSpeech(now) => self
                    .heard
                    .retain(|(_, validity)| validity.is_none_or(|validity| validity.holds(now))),
                Effect::LiveReread(reread) => self.route(reread),
                other => panic!(
                    "an effect the world does not take: {other:?}: {}",
                    self.log.join("\n")
                ),
            }
        }
    }

    fn render(&self, parts: &[Part]) -> Vec<String> {
        parts.iter().map(|part| self.names[part].clone()).collect()
    }

    fn said(effects: &[Effect]) -> String {
        let mut said = Vec::new();
        for effect in effects {
            let effect = match effect {
                Effect::ForKey(_, inner) => inner.as_ref(),
                other => other,
            };
            match effect {
                Effect::Speak(utterance, _) => {
                    if utterance.priority == SpeechPriority::Interrupt {
                        said.push("(cut)".to_owned());
                    }
                    said.extend(labels(utterance).cloned());
                }
                Effect::StopSpeech => said.push("(stop)".to_owned()),
                Effect::LiveReread(reread) => {
                    said.push(format!("(live re-read {})", reread.request));
                }
                _ => {}
            }
        }
        format!("{said:?}")
    }

    // ---- The steps ----

    fn send(&mut self, channel: Channel, msg: Msg) {
        self.channels.entry(channel).or_default().push_back(msg);
    }

    fn rep(&mut self, seq: u64) -> &mut Rep {
        match self.obs.get_mut(&seq) {
            Some(Obs::Focus(rep)) => rep,
            _ => panic!("no focus observed at {seq}"),
        }
    }

    /// A focus on `control` observed now and read by its window's reader;
    /// a window's whole-window focus, the outpost's, has no chain and
    /// leaves the outpost's last read as it was.
    fn focus(&mut self, control: Element) {
        self.now += 10;
        let seq = self.now;
        let whole = control.1 == 0;
        let kind = if whole {
            FocusReaderKind::Outpost
        } else {
            self.kind(control.0)
        };
        let app = self.app_of(part_of(control));
        let chain = if whole {
            Vec::new()
        } else {
            self.read(kind, control, false)
        };
        self.obs.insert(
            seq,
            Obs::Focus(Rep {
                element: control,
                outpost: self.outpost(app),
                status: Status::InFlight,
                chain: self.nvda_chain(control),
                cached: chain
                    .iter()
                    .filter(|(_, _, cached)| *cached)
                    .map(|(element, _, _)| *element)
                    .collect(),
                names: self.reading(kind, control, &chain),
                asked: None,
                outcome: None,
                arrived_at: None,
            }),
        );
        if !whole {
            self.readers.entry((app, kind)).or_default().last = Some(seq);
            self.follows(app, kind);
        }
        let input = self.report_input(kind, control, &chain, (seq, None));
        self.log.push(format!(
            "  {seq} {kind:?} reads {control:?}, cached {:?}",
            chain
                .iter()
                .filter(|(_, _, cached)| *cached)
                .map(|(_, name, _)| name.as_str())
                .collect::<Vec<_>>()
        ));
        self.send(Channel::App(app), Msg::Report { seq, input });
    }

    /// The outpost of application `app` completes a focus the listener read
    /// (`kind`), finding its own element, so it follows that focus and its
    /// last MSAA read is replaced: a live re-read of that read is answered
    /// gone (`verbatim-outpost`'s tracking, which clears its MSAA report on
    /// any followed focus; a whole-window focus follows nothing).
    fn follows(&mut self, app: u64, kind: FocusReaderKind) {
        if kind == FocusReaderKind::Listener
            && let Some(outpost) = self.readers.get_mut(&(app, FocusReaderKind::Outpost))
        {
            outpost.last = None;
        }
    }

    /// The user switches to `window`.
    fn switch(&mut self, window: usize, focus: Option<u8>, repeat: bool, focus_first: bool) {
        let changed = window != self.front;
        self.front = window;
        let repeats = if repeat { 2 } else { 1 };
        if focus_first && let Some(control) = focus {
            for _ in 0..repeats {
                self.focus(self.el(window, control));
            }
        }
        if changed {
            self.now += 10;
            let seq = self.now;
            self.obs.insert(seq, Obs::Change(window, None));
            let input = self.change(window, seq);
            self.send(Channel::Listener, Msg::Change { seq, window, input });
            if !self.wins[window].uia {
                self.focus((window, 0, 0));
            }
        }
        if !focus_first && let Some(control) = focus {
            for _ in 0..repeats {
                self.focus(self.el(window, control));
            }
        }
    }

    /// Reader `key` takes its oldest live re-read and answers it.
    fn answer(&mut self, key: (u64, FocusReaderKind), died: bool) {
        let Some((request, seq, outpost)) =
            self.requests.get_mut(&key).and_then(VecDeque::pop_front)
        else {
            return;
        };
        let Obs::Focus(rep) = &self.obs[&seq] else {
            unreachable!()
        };
        let element = rep.element;
        let channel = match key.1 {
            FocusReaderKind::Listener => Channel::Listener,
            FocusReaderKind::Outpost => Channel::App(key.0),
        };
        // A reader of a later generation holds no read of an earlier one.
        let holds = outpost == self.outpost(key.0)
            && self
                .readers
                .get(&key)
                .is_some_and(|reader| reader.last == Some(seq));
        let msg = if holds && !died {
            self.follows(key.0, key.1);
            let chain = self.read(key.1, element, true);
            let input = self.report_input(key.1, element, &chain, (seq, Some(request)));
            let names = self.reading(key.1, element, &chain);
            Msg::Answer {
                seq,
                request,
                input,
                names,
            }
        } else {
            if holds && died {
                // The element died: another takes its place, a new object.
                *self.incarnations.entry(part_of(element)).or_default() += 1;
            }
            Msg::Gone {
                seq,
                request,
                outpost,
            }
        };
        let answer = match &msg {
            Msg::Answer { .. } => "live",
            _ => "gone",
        };
        self.log
            .push(format!("  {key:?} answers {request} for {seq} {answer}"));
        self.send(channel, msg);
    }

    /// Core asked `reread` with input number `self.arrivals`: it names a
    /// report Core took in, of its own reader, and goes to that reader.
    fn route(&mut self, reread: &LiveReread) {
        let seq: u64 = reread.trace_id.to_string().parse().unwrap();
        let named = match self.obs.get(&seq) {
            Some(Obs::Focus(rep)) => {
                rep.status == Status::Accepted && OutpostId(rep.outpost) == reread.outpost
            }
            _ => false,
        };
        assert!(
            named,
            "a live re-read {reread:?} names no report Core took in of its reader: {}",
            self.log.join("\n")
        );
        let at = self.arrivals;
        let rep = self.rep(seq);
        rep.asked = Some((reread.request, at));
        rep.outcome = None;
        let outpost = rep.outpost;
        self.requests
            .entry((outpost % 10, reread.reader))
            .or_default()
            .push_back((reread.request, seq, outpost));
    }

    /// The live re-read `request` of the report observed at `seq` ended,
    /// with this input, as `outcome`, if it is the one Core asked last.
    fn ended(&mut self, seq: u64, request: u64, outcome: impl FnOnce(u64) -> Outcome) {
        let at = self.arrivals;
        let rep = self.rep(seq);
        if rep.asked.is_some_and(|(asked, _)| asked == request) && rep.outcome.is_none() {
            rep.outcome = Some(outcome(at));
        }
    }

    /// `msg` reaches Core.
    fn deliver(&mut self, msg: Msg) {
        self.arrivals += 1;
        let effects = match msg {
            Msg::Change { seq, window, input } => {
                let effects = crate::reduce(&mut self.state, &input);
                self.core_front = (seq, window);
                self.obs
                    .insert(seq, Obs::Change(window, Some(self.arrivals)));
                let said = Self::said(&effects);
                self.log.push(format!("  {seq} change to {window}: {said}"));
                effects
            }
            Msg::Report { seq, input } => {
                let rep = self.rep(seq).clone();
                let refused = self.stale(seq, &rep) || self.background(seq, rep.element.0);
                if !refused {
                    self.stamps
                        .push((seq, rep.outpost, rep.element.0, rep.element.1 == 0));
                }
                let element = rep.element;
                let arrived = self.arrivals;
                let rep = self.rep(seq);
                rep.arrived_at = Some(arrived);
                rep.status = if refused {
                    Status::Removed
                } else {
                    Status::Accepted
                };
                let status = rep.status;
                let effects = crate::reduce(&mut self.state, &input);
                let said = Self::said(&effects);
                self.log.push(format!(
                    "  {seq} {element:?} reaches Core: {said}, {status:?}"
                ));
                effects
            }
            Msg::Ended { outpost } => {
                self.now += 10;
                let effects = crate::reduce(
                    &mut self.state,
                    &Input::OutpostEnded {
                        trace_id: TraceId::from_raw(self.now),
                        outpost: OutpostId(outpost),
                    },
                );
                self.reader_ended(|_, rep| rep.outpost == outpost);
                self.reader_ends.push((self.arrivals, self.now, outpost));
                self.log.push(format!(
                    "  outpost {outpost} ended: {}",
                    Self::said(&effects)
                ));
                effects
            }
            Msg::Answer {
                seq,
                request,
                input,
                names,
            } => {
                let effects = crate::reduce(&mut self.state, &input);
                let window = self.rep(seq).element.0;
                let refused = self.background(seq, window);
                self.ended(seq, request, |at| {
                    if refused {
                        Outcome::Unanswered(at)
                    } else {
                        Outcome::Live(names, at)
                    }
                });
                let said = Self::said(&effects);
                self.log.push(format!(
                    "  {seq} answer {request} reaches Core: {said}{}",
                    if refused { ", refused" } else { "" }
                ));
                effects
            }
            Msg::Gone {
                seq,
                request,
                outpost,
            } => {
                let effects = crate::reduce(
                    &mut self.state,
                    &Input::LiveRereadGone {
                        outpost: OutpostId(outpost),
                        request,
                    },
                );
                self.ended(seq, request, Outcome::Unanswered);
                let said = Self::said(&effects);
                self.log
                    .push(format!("  {seq} gone {request} reaches Core: {said}"));
                effects
            }
        };
        self.take(&effects);
    }

    fn replace_listener(&mut self) {
        self.arrivals += 1;
        let effects = crate::reduce(&mut self.state, &Input::ListenerReplaced);
        self.reader_ended(|world, rep| world.wins[rep.element.0].uia);
        self.readers
            .retain(|(_, kind), _| *kind != FocusReaderKind::Listener);
        self.requests
            .retain(|(_, kind), _| *kind != FocusReaderKind::Listener);
        self.log
            .push(format!("  listener replaced: {}", Self::said(&effects)));
        self.take(&effects);
    }

    /// Application `app`'s outpost generation ends and a new one takes its
    /// place: the end reaches Core after every message the ended one sent.
    /// The listener's reads of the application carry the outpost's
    /// generation too, so both its readers start afresh.
    fn replace_outpost(&mut self, app: u64) {
        let outpost = self.outpost(app);
        *self.generations.entry(app).or_default() += 1;
        self.readers.retain(|(reader_app, _), _| *reader_app != app);
        self.requests
            .retain(|(reader_app, _), _| *reader_app != app);
        self.send(Channel::App(app), Msg::Ended { outpost });
    }

    fn drained(&self) -> bool {
        self.channels.values().all(VecDeque::is_empty)
            && self.requests.values().all(VecDeque::is_empty)
    }

    fn drain(&mut self) {
        while !self.drained() {
            let channels: Vec<Channel> = self.channels.keys().copied().collect();
            for channel in channels {
                while let Some(msg) = self.channels.get_mut(&channel).unwrap().pop_front() {
                    self.deliver(msg);
                }
            }
            let readers: Vec<_> = self.requests.keys().copied().collect();
            for key in readers {
                while self
                    .requests
                    .get(&key)
                    .is_some_and(|queue| !queue.is_empty())
                {
                    self.answer(key, false);
                }
            }
        }
    }

    /// With every message at Core and every live re-read answered, what
    /// Verbatim's speech queue holds is what NVDA's model's holds, and
    /// Core's focus object is the model's.
    fn check(&self) {
        let nvda = self.nvda();
        let expected: Vec<Part> = nvda
            .queue
            .iter()
            .map(|(element, _)| part_of(*element))
            .collect();
        // Core keeps a focus whose outpost generation ended as dead data,
        // no focus object, until the replacement is asked for its focus
        // (`docs/parity/focus-events.md`, "Recovery after an outpost is
        // replaced"), a question this world does not generate.
        let focus = nvda
            .focus
            .filter(|_| nvda.focus_outpost == self.outpost(nvda.focus_outpost % 10));
        let heard: Vec<Part> = self
            .heard
            .iter()
            .flat_map(|(parts, _)| parts.iter().copied())
            .collect();
        let core = self
            .state
            .focused()
            .and_then(|(_, node)| self.element_of(node.id));
        assert_eq!(
            self.render(&heard),
            self.render(&expected),
            "Verbatim's speech queue against NVDA's model's, Core's focus {core:?}, \
             NVDA's {focus:?}: {}",
            self.log.join("\n")
        );
        assert_eq!(
            core,
            focus,
            "Core's focus against NVDA's model's: {}",
            self.log.join("\n")
        );
    }

    fn step(&mut self, action: Action) {
        self.log.push(format!("{action:?}"));
        let count = self.wins.len();
        match action {
            Action::Rename { window, part } => {
                let part = (usize::from(window) % count, part);
                self.now += 10;
                self.renames += 1;
                let renamed = format!("{} {}", self.names[&part], self.renames);
                let old = self.names.insert(part, renamed.clone()).unwrap();
                self.labels.get_mut(&old).unwrap().until = Some(self.now);
                self.labels.insert(renamed, Named { part, until: None });
            }
            Action::Focus { control } => self.focus(self.el(self.front, control)),
            Action::Switch {
                window,
                focus,
                repeat,
                focus_first,
            } => self.switch(usize::from(window) % count, focus, repeat, focus_first),
            Action::Deliver { channel } => {
                let channel = match channel {
                    0 => Channel::App(1),
                    1 => Channel::App(2),
                    _ => Channel::Listener,
                };
                if let Some(msg) = self
                    .channels
                    .get_mut(&channel)
                    .and_then(VecDeque::pop_front)
                {
                    self.deliver(msg);
                }
            }
            Action::Answer { reader, died } => {
                let kind = if reader % 2 == 0 {
                    FocusReaderKind::Listener
                } else {
                    FocusReaderKind::Outpost
                };
                self.answer((1 + u64::from(reader / 2), kind), died);
            }
            Action::ReplaceListener => self.replace_listener(),
            Action::ReplaceOutpost { app } => self.replace_outpost(u64::from(app)),
            Action::Drain => self.drain(),
        }
        if self.drained() {
            self.check();
        }
    }
}

/// Runs `actions` in a new world of `wins`, then lets every message reach
/// Core, returning its log.
fn case(wins: &[Win], actions: &[Action]) -> Vec<String> {
    let mut world = World::new(wins.to_vec());
    for &action in actions {
        world.step(action);
    }
    world.drain();
    world.check();
    world.log
}

fn run(config: Config) {
    let mut runner = TestRunner::new(config);
    let result = runner.run(
        &(world(), proptest::collection::vec(action(), 1..40)),
        |(wins, actions)| {
            case(&wins, &actions);
            Ok(())
        },
    );
    if let Err(TestError::Fail(reason, (wins, actions))) = result {
        panic!("{reason}: case(&{wins:?}, &{actions:?})");
    }
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_108;
const CASES: u32 = 400;

#[test]
fn generated_cases_match_nvdas_focus_queue() {
    run(crate::queries::explore_config(SEED, CASES));
}
