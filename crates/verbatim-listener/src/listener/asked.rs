//! Core's requests to the listener, and the reading threads' jobs (design
//! `docs/design/readers-tell-core-asks-build1.md`, "The listener in this
//! build", Tables 6 and 9): the generic read of the object an event names,
//! for a UI Automation element the listener kept for a tell (a focus
//! event's, read up its ancestors until it reaches one Core already knows,
//! answering what it read and the element's route; a menu opening's; any
//! other event's object, by property set) or an MSAA object at an event's
//! address (the desktop window's name, a classic progress bar's role,
//! value, states and place). Each request is answered once, with one of the
//! five outcomes, by the job that runs it or, for one cancelled or
//! abandoned, by the reading threads' shell.
//!
//! The releases of kept elements run on the same threads, behind Core's
//! requests.

use std::cell::RefCell;
use std::sync::Arc;

use verbatim_listener_logic::kept::Start;
use verbatim_listener_logic::schedule::{Ended, Ending};
use verbatim_model::property::{
    Number, PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, KnownAncestor, NodeRead, ObjectRead, ObjectRef, Outcome, Purpose, ReadObject,
    Refusal, Reply, Request, RequestNumber, Route, RouteEnd, RouteStep,
};
use verbatim_model::tell::ElementToken;
use verbatim_model::{Pid, State, WindowHandle};
use verbatim_uia::map::{CachedUiaParts, add_legacy_checked_state, wants_legacy_checked_state};
use verbatim_uia::{
    Agile, Ancestor, AncestorGates, Cached, ElementExt as _, Fill, FocusInput, ListenerEvent, Node,
    NodeGates, Shape, Uia,
};
use verbatim_uia_rops::{FocusAncestry, FocusQuery, Progress, focus_ancestry_classic};
use verbatim_win32::window::{Readability, WindowReadabilityReader};
use verbatim_win32::window_facts::node_window;
use windows::Win32::UI::Accessibility::UIA_LegacyIAccessibleStatePropertyId;

use super::Outgoing;
use super::checking::Kept;
use super::tells::SystemReads;
use crate::call_log::{CallLog, MAX_ANCESTORS};
use crate::reading::Reading;

/// One job of a reading thread.
pub(crate) enum UiaJob {
    /// Core's request, on an object of the thread's application.
    Request(Box<Ask>),
    /// The elements kept for tells and let go, released here, on their
    /// application's reading thread, since a release can call into the
    /// application.
    Release(Vec<Agile<ListenerEvent>>),
}

/// What a reading thread keeps between its jobs.
pub(crate) struct ReadingThread {
    /// Its UI Automation client, `None` when none could be made.
    uia: Option<Uia>,
}

/// A request's read of a kept element, handed back so its end judges the
/// element under the lock (Table 6, column B).
pub(crate) struct KeptRead {
    pid: Pid,
    token: ElementToken,
    element: Agile<ListenerEvent>,
}

/// The listener's reading: Core's requests and releases.
pub(crate) struct UiaReading {
    pub(super) outgoing: Arc<Outgoing>,
    pub(super) kept: Arc<Kept>,
    /// The one judge of whether a window is hung or ghosted, which the
    /// walks and the UIA callbacks are given.
    pub(super) readability: WindowReadabilityReader,
    /// What reads an MSAA object's properties at an event's address:
    /// `AccessibleObjectFromEvent` then one call for each of the set
    /// ([`verbatim_ia2::acquire::read_at_event`]), unless a test gives its
    /// own.
    pub(super) read_at_event: MsaaReads,
}

impl UiaReading {
    /// The listener's reading, judging windows by the system's state.
    pub(super) fn new(outgoing: &Arc<Outgoing>, kept: &Arc<Kept>) -> Self {
        Self {
            outgoing: Arc::clone(outgoing),
            kept: Arc::clone(kept),
            readability: Arc::new(verbatim_win32::window::readability),
            read_at_event: verbatim_ia2::acquire::read_at_event,
        }
    }
}

impl UiaReading {
    fn answer(&self, number: RequestNumber, outcome: Outcome, detail: &str) {
        self.outgoing.answer(Answer {
            number,
            outcome,
            probes: Vec::new(),
            detail: detail.to_owned(),
        });
    }

    /// Runs request `ask` of `pid`'s on `thread`.
    fn request(&self, thread: &mut ReadingThread, ask: &Ask) -> (Outcome, Option<KeptRead>) {
        let Request::ReadObject(read) = &ask.request else {
            return (Outcome::Refused(Refusal::Unsupported), None);
        };
        match &read.object {
            ObjectRef::Kept { token, .. } => {
                self.read_kept(thread, (*token, ask.number), ask.window, read)
            }
            ObjectRef::Msaa {
                window,
                object,
                child,
            } => (
                read_msaa(self.read_at_event, (*window, *object, *child), read),
                None,
            ),
            ObjectRef::Node(_) => (Outcome::Refused(Refusal::Unsupported), None),
        }
    }

    /// The read of the kept element `token`, whose nearest window Core
    /// names, `window`.
    fn read_kept(
        &self,
        thread: &ReadingThread,
        (token, number): (ElementToken, RequestNumber),
        window: WindowHandle,
        read: &ReadObject,
    ) -> (Outcome, Option<KeptRead>) {
        let Some(pid) = self.kept.owner(token) else {
            return (Outcome::NotThere, None);
        };
        let focus = matches!(read.purpose, Purpose::Focus);
        let element = match self.kept.start_read(pid, token, (number.0, focus)) {
            Start::Read(element) => element,
            Start::NotThere => return (Outcome::NotThere, None),
            Start::NotAsked => {
                tracing::error!(target: verbatim_model::DEFECT_TARGET, ?token, "a request asked of a kept element already read");
                return (Outcome::Refused(Refusal::Unsupported), None);
            }
        };
        let outcome = match (thread.uia.as_ref(), element.resolve()) {
            (Some(uia), Ok(sender)) => read_element(
                (uia, &self.readability),
                &sender,
                (pid, hwnd_of(window)),
                read,
                &CallLog::default(),
            ),
            (None, _) => Outcome::Ended,
            (_, Err(_)) => Outcome::NotThere,
        };
        (
            outcome,
            Some(KeptRead {
                pid,
                token,
                element,
            }),
        )
    }
}

impl Reading for UiaReading {
    type Job = UiaJob;
    type Thread = ReadingThread;
    type Done = Option<(Answer, Option<KeptRead>)>;
    type Leftover = Option<Agile<ListenerEvent>>;

    fn thread(&self, pid: Pid) -> ReadingThread {
        let uia = Uia::new()
            .inspect_err(
                |error| tracing::warn!(%pid, %error, "a reading thread could not create a UIA client"),
            )
            .ok();
        ReadingThread { uia }
    }

    fn run(
        &self,
        thread: &mut ReadingThread,
        job: UiaJob,
        _number: Option<RequestNumber>,
    ) -> Self::Done {
        match job {
            UiaJob::Request(ask) => {
                let (outcome, kept) = self.request(thread, &ask);
                Some((
                    Answer {
                        number: ask.number,
                        outcome,
                        probes: Vec::new(),
                        detail: String::new(),
                    },
                    kept,
                ))
            }
            UiaJob::Release(elements) => {
                drop(elements);
                None
            }
        }
    }

    fn done(&self, done: Self::Done, send: bool) -> Self::Leftover {
        let (answer, kept) = done?;
        // The element's end comes before its answer, so a view Core sends
        // after it handled the answer finds it read.
        let released = kept.and_then(|kept| {
            self.kept
                .read_ended(&self.outgoing, (kept.pid, kept.token), kept.element, send)
        });
        if send {
            self.outgoing.answer(answer);
        }
        released
    }

    fn ended(&self, ended: Ended<UiaJob>) {
        let Some(number) = ended.number else {
            return;
        };
        match ended.ending {
            Ending::Cancelled => self.answer(number, Outcome::Ended, "cancelled"),
            Ending::Exited => self.answer(number, Outcome::NotThere, "its process exited"),
            Ending::NoThread => self.answer(number, Outcome::Ended, "no reading thread"),
            // Dropped unanswered at the shutdown.
            Ending::Stopped => {}
        }
    }

    fn abandoned(&self, number: RequestNumber) {
        self.answer(number, Outcome::Ended, "abandoned");
    }

    fn finish(&self, thread: ReadingThread) {
        let joined = thread.uia.is_some();
        drop(thread);
        verbatim_uia::release_thread_state();
        if joined {
            verbatim_uia::leave_mta();
        }
    }
}

/// `window` as the listener names a window.
pub(super) fn hwnd_of(window: WindowHandle) -> isize {
    isize::try_from(window.0.cast_signed()).unwrap_or(0)
}

/// `window` as the model names a window.
pub(super) fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

/// What reads an MSAA object's properties at an event's address, the
/// window, object and child ids, for the set asked; `None` when the object
/// cannot be acquired.
pub(super) type MsaaReads = fn(isize, i32, i32, &[PropertyId]) -> Option<PropertyValues>;

/// The properties an MSAA object at an event's address is read for: its
/// name, as the desktop window's name is read for the virtual desktop's
/// (`eventHandler.py` 338 to 346), and its role, value, states and place,
/// as a classic progress bar's are read at its value change
/// (`NVDAObjects/behaviors.py` 41 to 86); anything more is the outpost's.
const MSAA_SET: [PropertyId; 5] = [
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Rect,
];

/// The read of an MSAA object at an event's address `read` names, as a
/// reading thread makes it for Core's request ([`read_msaa`], through
/// [`verbatim_ia2::acquire::read_at_event`]); a read of any other object is
/// refused. Public so that a test standing in for the listener reads
/// mockapp's objects as the listener reads them.
#[must_use]
pub fn read_msaa_object(read: &ReadObject) -> Outcome {
    match read.object {
        ObjectRef::Msaa {
            window,
            object,
            child,
        } => read_msaa(
            verbatim_ia2::acquire::read_at_event,
            (window, object, child),
            read,
        ),
        ObjectRef::Kept { .. } | ObjectRef::Node(_) => Outcome::Refused(Refusal::Unsupported),
    }
}

/// The read of an MSAA object at an event's address, through `reads`: of
/// the properties [`MSAA_SET`] holds, with no relation, walk or
/// descendants; any other read is refused.
fn read_msaa(
    reads: MsaaReads,
    (window, object, child): (WindowHandle, i32, i32),
    read: &ReadObject,
) -> Outcome {
    if read.set.iter().any(|id| !MSAA_SET.contains(id))
        || read.walk.is_some()
        || read.relation.is_some()
        || read.descendants
    {
        return Outcome::Refused(Refusal::Unsupported);
    }
    let hwnd = hwnd_of(window);
    let Some(values) = reads(hwnd, object, child, &read.set) else {
        return Outcome::NotThere;
    };
    Outcome::Result(Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: None,
            window: node_window(&SystemReads, hwnd),
            values,
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: None,
    })))
}

/// One element a read read: its parts and what the properties beyond them
/// are read from.
struct Read {
    parts: CachedUiaParts,
    automation_id: Option<String>,
    /// Its range value, when it has the pattern.
    range: Option<f64>,
    /// Whether it has a text pattern, when that was read.
    has_text: Option<bool>,
    /// Its own window, 0 for none.
    hwnd: isize,
    /// Whether UIA counts it both a control and content.
    content: bool,
    /// Whether its value is its first selected item's name, when the
    /// element itself was read.
    value_from_selection: Option<bool>,
}

impl Read {
    /// `element`, read as an [`Ancestor`].
    fn of(element: &Cached<Ancestor>) -> Self {
        let read = element.read();
        Self {
            parts: read.parts(element.runtime_id()),
            automation_id: read.automation_id.clone(),
            range: read
                .node
                .value
                .range
                .filter(|_| read.node.value.range_available),
            has_text: None,
            hwnd: read.node.role.hwnd,
            content: read.is_control_and_content(),
            value_from_selection: None,
        }
    }

    /// Its properties of `set`, of process `pid`.
    fn values(&self, set: &[PropertyId], pid: Pid) -> PropertyValues {
        let details = &self.parts.details;
        let text = |text: &Option<String>| text.clone().map(PropertyValue::Text);
        PropertyValues(
            set.iter()
                .map(|&id| PropertyAnswer {
                    id,
                    value: match id {
                        PropertyId::Name => text(&self.parts.name),
                        PropertyId::Role => Some(PropertyValue::Role(self.parts.role)),
                        PropertyId::Value => text(&self.parts.value),
                        PropertyId::States => Some(PropertyValue::States(self.parts.states)),
                        PropertyId::Description => text(&details.description),
                        PropertyId::KeyboardShortcut => text(&details.keyboard_shortcut),
                        PropertyId::PositionInSet => {
                            details.position_in_set.map(PropertyValue::Count)
                        }
                        PropertyId::SetSize => details.set_size.map(PropertyValue::Count),
                        PropertyId::Level => details.level.map(PropertyValue::Count),
                        PropertyId::Rect => details.rect.map(PropertyValue::Rect),
                        PropertyId::ClassName => text(&details.class_name),
                        PropertyId::AutomationId => self
                            .automation_id
                            .clone()
                            .filter(|id| !id.is_empty())
                            .map(PropertyValue::Text),
                        PropertyId::NativeWindow => details.window.map(PropertyValue::Window),
                        PropertyId::Process => Some(PropertyValue::Process(pid)),
                        PropertyId::RuntimeId => Some(PropertyValue::RuntimeId(RuntimeId(
                            self.parts.runtime_id.clone(),
                        ))),
                        PropertyId::HasTextPattern => self.has_text.map(PropertyValue::Flag),
                        PropertyId::RangeValue => {
                            self.range.map(|range| PropertyValue::Number(Number(range)))
                        }
                        PropertyId::Cell => details.cell.map(PropertyValue::Cell),
                        PropertyId::TableCounts => {
                            details.table_counts.map(PropertyValue::TableCounts)
                        }
                        // Not read by the listener: the focus's keyboard
                        // focus is its check's, and an object's text is
                        // the outpost's.
                        PropertyId::ValueFromSelection => {
                            self.value_from_selection.map(PropertyValue::Flag)
                        }
                        // Read by the outpost that holds the node, when Core
                        // asks it of a node; the parent's automation id by
                        // the check thread, for a focus tell alone.
                        PropertyId::HasKeyboardFocus
                        | PropertyId::TextSummary
                        | PropertyId::Suggestions
                        | PropertyId::HasValuePattern
                        | PropertyId::WindowClass
                        | PropertyId::ComboBoxList
                        | PropertyId::ParentAutomationId
                        | PropertyId::ChildCount => None,
                    },
                })
                .collect(),
        )
    }
}

/// What a walk has read so far, kept as it reads, so a walk that fails is
/// answered with what it read.
#[derive(Default)]
struct Walked {
    /// The element read afresh, and whether its value is its first
    /// selected item's name.
    fresh: Option<(Read, bool)>,
    /// The first selected child's name, when it was read and has one.
    selected_name: Option<String>,
    /// The ancestors read, nearest first.
    ancestors: Vec<Read>,
}

impl Walked {
    fn take_in(&mut self, read: Progress<'_>) {
        match read {
            // No earlier element is named: the check read the kept
            // elements' keyboard focus.
            Progress::PreviousFocused(_) => {}
            Progress::Element(fresh, table) => {
                let mut read = Read::of(fresh);
                read.parts.details.set_table(table);
                self.fresh = Some((read, fresh.read().node.value_from_selection()));
            }
            Progress::SelectedChild(child) => {
                self.selected_name = child.read().node.parts(child.runtime_id()).name;
            }
            Progress::Ancestor(ancestor) => self.ancestors.push(Read::of(ancestor)),
        }
    }
}

/// The ancestors Core already knows that a UI Automation walk can meet, as
/// runtime ids, a window's by its own element's (`[42, handle]`), each with
/// its place in Core's list; a node of an outpost is none of the
/// listener's.
fn known_runtime_ids(walk: &[KnownAncestor]) -> (Vec<Vec<i32>>, Vec<u32>) {
    walk.iter()
        .enumerate()
        .filter_map(|(at, known)| {
            let runtime_id = match known {
                KnownAncestor::Uia(runtime_id) => runtime_id.0.clone(),
                KnownAncestor::Window(window) => {
                    let low = u32::try_from(window.0 & 0xFFFF_FFFF).ok()?;
                    vec![42, low.cast_signed()]
                }
                KnownAncestor::Node(_) => return None,
            };
            Some((runtime_id, u32::try_from(at).ok()?))
        })
        .unzip()
}

/// Reads `sender`, a kept element of `pid`'s whose nearest window is
/// `nearest`, as `read` asks, with `uia`, a window `readability` finds
/// hung or ghosted read as a plain window: the element afresh, its name
/// and class as its event's cache had them, as NVDA's object for the event
/// reads them, a selection's item with the name read afresh; for a walk,
/// its ancestors up to one Core already knows, the desktop root or the
/// process boundary, and its route. A walk whose own
/// read fails is answered with what it read, the event's cache giving the
/// rest, and the chain going on from the parent window of the outermost
/// element's nearest window, as NVDA's failed parent read goes on by
/// `Window._get_parent`. `calls` logs each call the read makes.
pub(crate) fn read_element(
    (uia, readability): (&Uia, &WindowReadabilityReader),
    sender: &Cached<ListenerEvent>,
    (pid, nearest): (Pid, isize),
    read: &ReadObject,
    calls: &CallLog,
) -> Outcome {
    if read.relation.is_some() || read.descendants {
        return Outcome::Refused(Refusal::Unsupported);
    }
    let fields = sender.read();
    let hwnd = fields.role.hwnd;
    let nearest = if hwnd == 0 { nearest } else { hwnd };
    let (known, places) = known_runtime_ids(read.walk.as_deref().unwrap_or_default());
    let (ended, mut walked) = walk(
        (uia, readability),
        sender,
        nearest,
        (read.walk.is_some(), &known),
        calls,
    );
    let mut element = element_read(&fields, sender, &mut walked, read.purpose);
    if read.set.contains(&PropertyId::States)
        && wants_legacy_checked_state(element.parts.role, element.parts.states)
    {
        // A menu item without the Toggle pattern: its checked state may be
        // only in its legacy MSAA state, one live call.
        calls.call("the legacy MSAA state");
        let legacy = sender
            .live()
            .element()
            .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
            .ok()
            .flatten();
        add_legacy_checked_state(&mut element.parts.states, legacy);
    }
    let known_met = |met: usize| {
        let place = places.get(met).copied()?;
        read.walk
            .as_deref()?
            .get(usize::try_from(place).ok()?)
            .cloned()
    };
    let end = match route_end(&ended, &mut walked, (hwnd, nearest), known_met) {
        Ok(end) => end,
        Err(outcome) => return outcome,
    };
    let ancestor_windows = nearest_windows((hwnd, nearest), &walked.ancestors);
    let route = read.walk.as_ref().map(|_| Route {
        window: handle(nearest),
        steps: std::iter::once(&element)
            .chain(walked.ancestors.iter())
            .map(|read| RouteStep {
                runtime_id: RuntimeId(read.parts.runtime_id.clone()),
                window: (read.hwnd != 0).then(|| handle(read.hwnd)),
                content: read.content,
            })
            .collect(),
        end,
    });
    Outcome::Result(Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: None,
            window: node_window(&SystemReads, nearest),
            values: element.values(&read.set, pid),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: walked
            .ancestors
            .iter()
            .zip(ancestor_windows)
            .map(|(ancestor, window)| NodeRead {
                node: None,
                window: node_window(&SystemReads, window),
                values: ancestor.values(&read.set, pid),
            })
            .collect(),
        // A read with a route names what its walk met in the route's end.
        met: None,
        route,
    })))
}

/// Where a walk that `ended` so ended, as its route says: `walked` loses
/// the known ancestor it met, which `known_met` names by its place among
/// the walk's known runtime ids; `hwnd` is the element's own window and
/// `nearest` its nearest. An outcome answers the read instead.
fn route_end(
    ended: &Result<Option<FocusAncestry>, verbatim_uia_rops::Error>,
    walked: &mut Walked,
    (hwnd, nearest): (isize, isize),
    known_met: impl Fn(usize) -> Option<KnownAncestor>,
) -> Result<RouteEnd, Outcome> {
    if let Ok(Some(FocusAncestry::Focused(ancestry))) = ended
        && let Some(met) = ancestry.met_known
    {
        // The known ancestor the walk met is the last read, and not among
        // the ancestors answered.
        walked.ancestors.pop();
        let Some(known) = known_met(met) else {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, "a walk met a known ancestor the request did not name");
            return Err(Outcome::Refused(Refusal::Unsupported));
        };
        return Ok(RouteEnd::Known(known));
    }
    let windows =
        || std::iter::once(hwnd).chain(walked.ancestors.iter().map(|ancestor| ancestor.hwnd));
    let continue_from =
        || verbatim_uia::parent_window(verbatim_uia::outermost_nearest_window(nearest, windows()));
    Ok(match ended {
        Ok(Some(FocusAncestry::Focused(ancestry))) => {
            if let Some(plain) = ancestry.plain_window {
                RouteEnd::ContinueFrom(handle(plain))
            } else if ancestry.failed_hop.is_some() {
                RouteEnd::ContinueFrom(handle(continue_from()))
            } else if ancestry.depth_limited {
                RouteEnd::PlainFrom(handle(verbatim_uia::plain_windows_start(
                    nearest,
                    windows(),
                )))
            } else {
                // The walk ends at the process's top-level window or at the
                // window below the process boundary
                // (`verbatim_uia_rops::Ancestry`).
                let outermost = walked
                    .ancestors
                    .last()
                    .map_or(hwnd, |ancestor| ancestor.hwnd);
                if outermost != 0 && verbatim_win32::window::top_level_of(outermost) != outermost {
                    RouteEnd::ProcessBoundary
                } else {
                    RouteEnd::Root
                }
            }
        }
        // Not asked for: the check read the keyboard focus.
        Ok(Some(FocusAncestry::NotFocused)) => {
            return Err(Outcome::Refused(Refusal::NotFocused));
        }
        // The element alone: no walk, so no route carries this end.
        Ok(None) => RouteEnd::Root,
        Err(error) => {
            tracing::debug!(%error, "a kept element's read failed; answered with what was read");
            RouteEnd::ContinueFrom(handle(continue_from()))
        }
    })
}

/// Reads `sender`, whose nearest window is `nearest`: with `walking`, its
/// ancestors too, up to the first with one of the `known` runtime ids, by
/// the classic walk, each call told to `calls`; else the element alone,
/// one call. Answers how the walk ended, `None` for the element alone, and
/// what it read, kept as it read, so a walk that fails is answered with
/// what it read.
fn walk(
    (uia, readability): (&Uia, &WindowReadabilityReader),
    sender: &Cached<ListenerEvent>,
    nearest: isize,
    (walking, known): (bool, &[Vec<i32>]),
    calls: &CallLog,
) -> (
    Result<Option<FocusAncestry>, verbatim_uia_rops::Error>,
    Walked,
) {
    let walked = RefCell::new(Walked::default());
    let ancestors = AncestorGates {
        node: NodeGates {
            place: None,
            cell: None,
            counts: None,
            ..Node::all_on()
        },
        ..Ancestor::all_on()
    };
    let element_gates = AncestorGates {
        node: NodeGates {
            counts: None,
            ..Node::all_on()
        },
        ..Ancestor::all_on()
    };
    let ended = if walking {
        uia.request::<Ancestor>(ancestors)
            .and_then(|request| Ok((request, uia.request::<Ancestor>(element_gates)?)))
            .map_err(verbatim_uia_rops::Error::Uia)
            .and_then(|(request, element_request)| {
                let fill = Fill::new(&request);
                let element_fill = Fill::new(&element_request);
                let input = sender.narrow::<FocusInput>();
                let query = FocusQuery {
                    element: &input,
                    known,
                    previous: None,
                    depth_limit: MAX_ANCESTORS,
                    fill: &fill,
                    element_fill: &element_fill,
                    // The check read its keyboard focus at delivery.
                    require_focus: false,
                    refresh: true,
                    nearest_window: Some(nearest),
                    before_call: Some(&|call| calls.call(call)),
                    value_available: None,
                    is_cell: None,
                    plain: Some(&|window| (readability)(window) != Readability::Readable),
                    progress: Some(&|progress| walked.borrow_mut().take_in(progress)),
                };
                focus_ancestry_classic(uia, &query)
            })
            .map(Some)
    } else {
        calls.call("the element's fresh cache");
        uia.request::<Ancestor>(element_gates)
            .and_then(|request| sender.live().refresh(&request))
            .map(|fresh| {
                let mut read = Read::of(&fresh);
                read.parts.details.set_table(
                    fresh
                        .read()
                        .node
                        .is_cell()
                        .then(|| verbatim_uia::containing_grid(fresh.live().element()))
                        .flatten(),
                );
                walked.borrow_mut().fresh = Some((read, false));
                None
            })
            .map_err(verbatim_uia_rops::Error::Uia)
    };
    (ended, walked.into_inner())
}

/// The element as a read for `purpose` answers it: its class from its
/// event's cache, and its name too, as NVDA's object for the event reads
/// them, except for a selection's item, whose name is the one read afresh
/// (Dickson, 2026-10-20, [`Purpose::Selection`]); its role, value, states
/// and details as read afresh, or from the event's cache when they were
/// not; a value taken from its first selected item's name when that is
/// its value; and, for a focus read, the focused state.
fn element_read(
    fields: &ListenerEvent,
    sender: &Cached<ListenerEvent>,
    walked: &mut Walked,
    purpose: Purpose,
) -> Read {
    let event = fields.parts(sender.runtime_id());
    let (mut read, from_selection) = walked.fresh.take().unwrap_or_else(|| {
        (
            Read {
                parts: event.clone(),
                automation_id: fields.automation_id.clone(),
                range: None,
                has_text: None,
                hwnd: fields.role.hwnd,
                content: fields.content && fields.control,
                value_from_selection: None,
            },
            false,
        )
    });
    // The fresh read's name came in the same cache request as its role and
    // position, so a selection's costs no call; with no fresh read, the
    // parts are the event's, its name included.
    if purpose != Purpose::Selection {
        read.parts.name.clone_from(&event.name);
    }
    read.parts
        .details
        .class_name
        .clone_from(&fields.role.class_name);
    read.parts.runtime_id = event.runtime_id;
    read.has_text = Some(fields.text_available);
    read.content = fields.content && fields.control;
    if from_selection {
        read.parts.value = walked.selected_name.take();
    }
    read.value_from_selection = Some(from_selection);
    if purpose == Purpose::Focus {
        read.parts.states = read.parts.states.with(State::Focused);
    }
    read
}

/// The nearest window of each ancestor, nearest first: its own window, or
/// the next window above it in the chain; above the outermost element read
/// that is a window, that window's parent window, and with none a window,
/// the element's nearest window, `nearest`. `hwnd` is the element's own
/// window, 0 for none.
fn nearest_windows((hwnd, nearest): (isize, isize), ancestors: &[Read]) -> Vec<isize> {
    let outermost = ancestors
        .iter()
        .rev()
        .map(|ancestor| ancestor.hwnd)
        .chain(std::iter::once(hwnd))
        .find(|&window| window != 0);
    let mut above = match outermost.map(verbatim_uia::parent_window) {
        None => nearest,
        Some(0) => outermost.unwrap_or(nearest),
        Some(parent) => parent,
    };
    let mut windows: Vec<isize> = ancestors
        .iter()
        .rev()
        .map(|ancestor| {
            if ancestor.hwnd != 0 {
                above = ancestor.hwnd;
            }
            above
        })
        .collect();
    windows.reverse();
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A walk stops at the UI Automation ancestors and windows Core knows,
    /// a window by its own element's runtime id, each answered by its place
    /// in Core's list; a node of an outpost is none of the listener's.
    #[test]
    fn the_known_ancestors_a_walk_meets_keep_their_places_in_cores_list() {
        let (ids, places) = known_runtime_ids(&[
            KnownAncestor::Uia(RuntimeId(vec![7, 1, 2])),
            KnownAncestor::Node(verbatim_model::NodeId::new(4)),
            KnownAncestor::Window(WindowHandle(0x10)),
        ]);
        assert_eq!(ids, [vec![7, 1, 2], vec![42, 0x10]]);
        assert_eq!(places, [0, 2]);
    }
}
