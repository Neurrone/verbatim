//! The one definition of a focus candidate's validity
//! (`docs/parity/focus-events.md`, "Duplicate focus suppression", "Event
//! acceptance", "While a focus is pending" and "The focus after a menu or
//! the switcher closes"). A candidate is a focus-kind tell (an MSAA focus,
//! foreground or menu start, a UI Automation focus or menu opening), the
//! menu check's read, or a recovered read. Every place that judges one
//! calls [`judge`], [`judge_run`] or [`pending_behind`], so the turn that
//! reads a candidate and the rules that ask whether one is pending agree
//! by construction.
//!
//! A candidate is judged on the facts Core holds now ([`View`]) and on
//! what an outpost read of it, if anything ([`CandidateFacts`]), the same
//! rows at its turn and while something waits behind it. An answer
//! needing a read names it ([`Validity::NeedsRead`]); nothing here asks.
//! Only a read at a candidate's turn can answer that it is the readers'
//! last focus by its identity ([`before_duplicate`]), so a pending judgement has
//! no identity row: a child id above 0 is never a duplicate there, as in
//! NVDA's duplicate test.

use std::collections::VecDeque;
use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, Depth, FocusRead, Identity, MsaaIds,
};
use verbatim_model::tell::{Event, Read, TellWindow};
use verbatim_model::{NodeSnapshot, Role, TraceId, WindowHandle};

use super::acceptance;
use super::backend::{BackendCache, Judged, JudgedWindow, Judgement, WindowKey};
use super::duplicate::{msaa_duplicate, uia_duplicate};
use super::msaa_run::{TurnDepth, UnlessSame};
use super::versions::{LastFocus, ReadBy, Versions};
use crate::entries::{Candidate, Entries, Entry, UiaFocus};
use crate::event_queue::{Cx, Item};
use verbatim_model::class::by_class;

/// File Explorer, by its executable name.
const FILE_EXPLORER: &str = "explorer";

/// The object id of a window itself.
const OBJID_WINDOW: i32 = 0;
/// The object id of a window's client area.
const OBJID_CLIENT: i32 = -4;

/// What an override makes of a UI Automation focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Override {
    /// No override applies.
    None,
    /// Refused at its tell.
    Refused,
    /// Let through whatever its keyboard-focus answer, and redirected at
    /// its read to its nearest popup menu ancestor within four parents.
    NetUiMenuItem,
}

/// How a valid candidate is executed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum How {
    /// Read in full and executed.
    Read,
    /// The committed focus executed again.
    Again,
}

/// Why a candidate is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Reason {
    /// A UI Automation candidate whose walk found no window.
    NoWindow,
    /// Its window is read through the other backend.
    Verdict,
    /// One of NVDA's focus overrides refused it.
    Override,
    /// Verbatim's hidden frame's own object (Verbatim-only).
    HiddenFrame,
    /// The acceptance test refused its window.
    Acceptance,
    /// An event of the desktop window, which no reader reads: Core starts
    /// no outpost for its process. NVDA reads the object and refuses it as
    /// not focused, its state never focused, so the speech is the same
    /// (`docs/parity/focus-events.md`).
    NoReader,
    /// Its element lost the keyboard focus by the check thread's answer.
    KeyboardFocusLost,
    /// A foreground tell whose window is not the foreground record's.
    NotInFront,
    /// A foreground tell of the last focus's window or an ancestor's.
    LastFocusWindow,
    /// A foreground tell whose ids equal the last focus's.
    SameIds,
    /// A menu start after a run that executed a focus on an MSAA menu.
    OnMenu,
    /// A menu start whose object is not a menu popup.
    NotMenuPopup,
    /// Its window is read through UI Automation, so it was not read.
    Skipped,
    /// Its object is not there.
    NotThere,
    /// Neither it nor an ancestor has the focused state.
    NotFocused,
}

impl Reason {
    /// Whether the refusal came from a read of the candidate that answered
    /// refused or not there, which ends the readers' last focus's fence
    /// (Dickson, 2026-11-18): a candidate refused before any read was never
    /// tried, and one skipped was never tried through MSAA.
    pub(crate) fn tried(self) -> bool {
        matches!(self, Self::NotThere | Self::NotFocused | Self::NotMenuPopup)
    }
}

/// The one answer about a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Validity {
    /// Valid: executed.
    Valid(How),
    /// Valid, but the focus it names is the readers' last focus: nothing is
    /// read or said, and it ends a run.
    Duplicate,
    /// Refused: a run goes on to its older candidates.
    Refused(Reason),
    /// It cannot be judged without a read.
    NeedsRead,
}

/// An answer about a candidate that needs no read: [`Validity`] without
/// its needed read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    /// Valid: executed.
    Valid(How),
    /// Valid, but a duplicate: nothing read or said.
    Duplicate,
    /// Refused.
    Refused(Reason),
}

impl From<Settled> for Validity {
    fn from(settled: Settled) -> Self {
        match settled {
            Settled::Valid(how) => Self::Valid(how),
            Settled::Duplicate => Self::Duplicate,
            Settled::Refused(reason) => Self::Refused(reason),
        }
    }
}

/// A full read's answer, parsed once as it is decoded: either refused by
/// its facts with nothing more read, or found and read in full. A found
/// candidate the depth asked to read always carries its read, so an answer
/// breaking that is undecodable, logged as a malformed answer
/// (`QueueEffect::Undecodable`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answered {
    /// Not read further, refused by its facts.
    Unread(Unread),
    /// Found and read in full.
    Read {
        /// The facts found, always [`CandidateFacts::Found`].
        facts: CandidateFacts,
        /// The full read.
        read: Box<FocusRead>,
    },
}

/// Why a full read's candidate was not read further.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unread {
    /// Its window is read through UI Automation.
    Skipped,
    /// Its object is not there.
    NotThere,
    /// Neither it nor an ancestor has the focused state, at
    /// [`Depth::FullIfFocused`].
    NotFocused,
}

impl Unread {
    /// The refusal it is.
    pub(crate) fn reason(self) -> Reason {
        match self {
            Self::Skipped => Reason::Skipped,
            Self::NotThere => Reason::NotThere,
            Self::NotFocused => Reason::NotFocused,
        }
    }
}

impl Answered {
    /// `answer`, to a read of depth `depth`, parsed: `None` for an answer
    /// that breaks the depth's rule, a reader's fault. An answer at
    /// [`Depth::FullUnlessSame`] that is not the object named follows
    /// [`Depth::FullIfFocused`]'s rule.
    pub(crate) fn parse(answer: CandidateAnswer, depth: &Depth) -> Option<Self> {
        let stops_unfocused = matches!(depth, Depth::FullIfFocused | Depth::FullUnlessSame { .. });
        if !stops_unfocused && *depth != Depth::Full {
            return None;
        }
        let unfocused = matches!(
            answer.facts,
            CandidateFacts::Found {
                focused: Some(false),
                ..
            }
        );
        let stopped = unfocused && stops_unfocused;
        match (answer.facts, answer.read) {
            (CandidateFacts::Skipped, None) => Some(Self::Unread(Unread::Skipped)),
            (CandidateFacts::NotThere, None) => Some(Self::Unread(Unread::NotThere)),
            (CandidateFacts::Found { .. }, None) if stopped => {
                Some(Self::Unread(Unread::NotFocused))
            }
            // A found candidate is read in full, but one found without the
            // focused state at a depth that stops there.
            (facts @ CandidateFacts::Found { .. }, Some(read)) if !stopped => {
                Some(Self::Read { facts, read })
            }
            _ => None,
        }
    }
}

/// A confirmation's answer, its facts alone, parsed: `None` for one that
/// carries a read it did not ask for.
pub(crate) fn parse_facts(answer: CandidateAnswer) -> Option<CandidateFacts> {
    answer.read.is_none().then_some(answer.facts)
}

/// Whether a candidate counts as pending
/// (`docs/parity/focus-events.md`, "While a focus is pending").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// It counts: NVDA would have queued it.
    Counts,
    /// It counts only if a read confirms it: the candidate of this trace.
    Unconfirmed(TraceId),
    /// It does not count.
    No,
}

/// Whether a waiting candidate's facts were read by a caret key's
/// confirmation (design, "Confirming, and its bound"): written only by
/// [`crate::event_queue::Cx::store_facts`], so each candidate is read at
/// most once while it waits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Confirmation {
    /// Not read.
    #[default]
    Unread,
    /// Read, with what the read found.
    Read(CandidateFacts),
}

impl Confirmation {
    /// The facts read, if any.
    pub(crate) fn facts(&self) -> Option<&CandidateFacts> {
        match self {
            Self::Unread => None,
            Self::Read(facts) => Some(facts),
        }
    }
}

/// Core's facts a candidate is judged against, as they are now.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View<'a> {
    /// The two versions of the focus.
    pub(crate) versions: &'a Versions,
    /// The foreground record's window.
    pub(crate) in_front: Option<WindowHandle>,
    /// The backend verdicts.
    pub(crate) backend: &'a BackendCache,
    /// The time of the input being handled.
    pub(crate) now_us: u64,
    /// Whether the run before a menu start executed a valid focus.
    pub(crate) run_before_valid: bool,
}

impl View<'_> {
    fn last(&self) -> Option<&LastFocus> {
        self.versions.last.as_ref()
    }
}

/// What is judged.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Subject<'a> {
    /// A UI Automation focus or menu opening.
    Uia(&'a UiaFocus),
    /// An MSAA focus, foreground or menu start.
    Msaa(&'a Candidate),
    /// The menu check's read, by the MSAA ids it carries, if any.
    MenuCheck(Option<&'a MsaaIds>),
    /// A recovered read: the current focus read after a hung window
    /// answered again.
    Recovered(&'a FocusRead),
}

/// The one definition: how `subject` stands against `view`, with `facts`
/// the outpost read of it, if any. The rows, in
/// order, each answer or pass on (design, "The definition").
pub(crate) fn judge(
    subject: Subject<'_>,
    view: &View<'_>,
    facts: Option<&CandidateFacts>,
) -> Validity {
    match settle(subject, view, facts.ok_or(())) {
        Ok(settled) => settled.into(),
        Err(()) => Validity::NeedsRead,
    }
}

/// The rows of [`judge`], with `facts` the outpost read or what stands for
/// its absence, `M`: an answer settled, or `M` when a read is needed.
fn settle<M>(
    subject: Subject<'_>,
    view: &View<'_>,
    facts: Result<&CandidateFacts, M>,
) -> Result<Settled, M> {
    match subject {
        Subject::Uia(focus) => Ok(uia(focus, view)),
        Subject::Recovered(read) => Ok(recovered(view, read)),
        Subject::Msaa(candidate) => msaa(candidate, view, facts),
        Subject::MenuCheck(ids) => menu_check(ids, view, facts),
    }
}

/// How far a candidate's read at its own turn goes: a focus, or the menu
/// check's read, only as far as its focused state unless it has it; a
/// foreground and a menu start in full. An MSAA focus with no key and no
/// failed try since the readers' last focus executed, whose object's own
/// child id is above 0, and which is the committed focus, `committed` its
/// reading, is read unless the object it reaches is that focus by its ids
/// and its identity ([`UnlessSame`]): the identity check in the duplicate
/// test (`docs/parity/focus-events.md`, "Duplicate focus suppression").
/// The identity is made from `committed` when asked, and kept nowhere.
pub(crate) fn depth(
    subject: Subject<'_>,
    view: &View<'_>,
    committed: Option<&NodeSnapshot>,
) -> TurnDepth {
    match subject {
        Subject::Msaa(candidate) if candidate.tell.kind == EventKind::MsaaFocus => {
            named(candidate, view, committed)
                .map_or(TurnDepth::FullIfFocused, TurnDepth::UnlessSame)
        }
        Subject::MenuCheck(_) => TurnDepth::FullIfFocused,
        Subject::Msaa(_) | Subject::Uia(_) | Subject::Recovered(_) => TurnDepth::Full,
    }
}

/// The readers' last focus as the identity check names it to `candidate`'s
/// read, when the check applies ([`depth`]).
fn named(
    candidate: &Candidate,
    view: &View<'_>,
    committed: Option<&NodeSnapshot>,
) -> Option<UnlessSame> {
    let last = view.last()?;
    if last.since != Some(candidate.fence) {
        return None;
    }
    let ReadBy::Msaa {
        event: Some(ids),
        held_child,
    } = &last.read_by
    else {
        return None;
    };
    if *held_child <= 0 {
        return None;
    }
    let kept = view.versions.caret_target()?;
    let snapshot = committed.filter(|snapshot| snapshot.id == kept.node)?;
    Some(UnlessSame {
        ids: *ids,
        identity: Identity {
            states: snapshot.states,
            role: snapshot.role,
            position_in_set: snapshot.details.position_in_set,
            set_size: snapshot.details.set_size,
            name: snapshot.name.clone(),
            value: snapshot.value.clone(),
        },
    })
}

/// Whether `answer` counts as pending: valid counts, a needed read leaves
/// it unconfirmed, and a duplicate or a refusal does not count.
pub(crate) fn pending(answer: Validity, trace: TraceId) -> Pending {
    match answer {
        Validity::Valid(_) => Pending::Counts,
        Validity::NeedsRead => Pending::Unconfirmed(trace),
        Validity::Duplicate | Validity::Refused(_) => Pending::No,
    }
}

/// One step of a run's walk ([`judge_run`]).
#[derive(Debug)]
pub(crate) struct RunWalk<'a> {
    /// Each candidate refused on the way, newest first: why.
    pub(crate) refused: Vec<Reason>,
    /// The first candidate not refused; `None` when every candidate was
    /// refused.
    pub(crate) found: Option<Found<'a>>,
}

/// The first candidate a run's walk did not refuse.
#[derive(Debug)]
pub(crate) struct Found<'a> {
    /// Its answer.
    pub(crate) answer: Validity,
    /// The candidate.
    pub(crate) candidate: &'a Candidate,
    /// The candidates older than it, newest first, where the walk goes on.
    pub(crate) older: &'a [Candidate],
}

/// The walk of a run, `candidates` newest first, each judged with the
/// facts stored on it: the refusals on the way, and the first answer that
/// is not one. A newest duplicate hides its older candidates, as NVDA's
/// pump stops at it (`IAccessibleHandler/__init__.py` 1087 to 1092). The
/// one walk of a run: the run's turn (`super::msaa_run`), the pending
/// judgement and the menu opening's drop all step through it.
pub(crate) fn judge_run<'a>(candidates: &'a [Candidate], view: &View<'_>) -> RunWalk<'a> {
    let mut refused = Vec::new();
    let mut rest = candidates;
    while let Some((candidate, older)) = rest.split_first() {
        match judge(
            Subject::Msaa(candidate),
            view,
            candidate.confirmation.facts(),
        ) {
            Validity::Refused(reason) => refused.push(reason),
            answer => {
                return RunWalk {
                    refused,
                    found: Some(Found {
                        answer,
                        candidate,
                        older,
                    }),
                };
            }
        }
        rest = older;
    }
    RunWalk {
        refused,
        found: None,
    }
}

/// A waiting entry, judged alone while it waits: its answer, its
/// trace and when it was raised; `None` for an entry that is no
/// candidate.
pub(crate) fn judge_alone(entry: &Entry, view: &View<'_>) -> Option<(Validity, TraceId, u64)> {
    let (answer, tell) = match entry {
        Entry::UiaFocus(focus) => (judge(Subject::Uia(focus), view, None), &focus.tell),
        Entry::Candidate(candidate) => (judge_candidate(candidate, view), &candidate.tell),
        Entry::Menu(candidate) if is_menu_start(candidate.tell.kind) => {
            (judge_candidate(candidate, view), &candidate.tell)
        }
        Entry::Run(_)
        | Entry::Menu(_)
        | Entry::MenuCheck(_)
        | Entry::CurrentFocus(_)
        | Entry::MsaaChange(_)
        | Entry::UiaChange(_)
        | Entry::Caret(_)
        | Entry::ForegroundMarker { .. }
        | Entry::Press(_)
        | Entry::Mark(..)
        | Entry::SayAllRead(..)
        | Entry::Announcement(_)
        | Entry::TerminalSpeech(_) => return None,
    };
    Some((answer, tell.trace_id, tell.at_us))
}

fn judge_candidate(candidate: &Candidate, view: &View<'_>) -> Validity {
    judge(
        Subject::Msaa(candidate),
        view,
        candidate.confirmation.facts(),
    )
}

/// Which waiting candidates a pending judgement looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Within {
    /// A candidate raised at or after this time is left out, as if it had
    /// not come: a menu opening judges only what was raised before it.
    pub(crate) raised_before: Option<u64>,
    /// A candidate needing a read raised at or after this time is not
    /// pending: a caret key confirms only what was raised before its
    /// bound.
    pub(crate) unconfirmed_before: Option<u64>,
}

impl Within {
    /// Everything waiting.
    pub(crate) const ALL: Self = Self {
        raised_before: None,
        unconfirmed_before: None,
    };

    /// What a caret key judges, its bound at `bound_us`.
    pub(crate) fn key(bound_us: u64) -> Self {
        Self {
            raised_before: None,
            unconfirmed_before: Some(bound_us),
        }
    }

    fn raised(at: Option<u64>, at_us: u64) -> bool {
        at.is_none_or(|before| at_us < before)
    }
}

/// Whether anything waiting, `waiting` in arrival order, counts as
/// pending, item by item, a run walked as a run ([`judge_run`]), only the
/// candidates `within` names judged.
///
/// A run walks newest first. The first valid candidate counts, even behind
/// a newer one needing a read, whose read could only make it count or be
/// refused down to the older one, so the walk goes on past a candidate
/// needing a read. A duplicate gives unconfirmed if a newer candidate
/// needed a read, else not pending; the end of the run gives unconfirmed
/// if any candidate needed a read. Over the queue: counts if any item
/// counts, otherwise the newest unconfirmed, otherwise not pending.
pub(crate) fn pending_behind(
    waiting: &VecDeque<Item<Entry>>,
    view: &View<'_>,
    within: Within,
) -> Pending {
    let mut newest_unconfirmed = None;
    for item in waiting {
        let answer = match item {
            Item::One(Entry::UiaFocus(focus)) => {
                if Within::raised(within.raised_before, focus.tell.at_us) {
                    pending(judge(Subject::Uia(focus), view, None), focus.tell.trace_id)
                } else {
                    Pending::No
                }
            }
            Item::One(entry) | Item::Menu { entry, .. } => {
                run_pending(candidates_of(std::slice::from_ref(entry)), view, within)
            }
            Item::Run(entries) => run_pending(candidates_of(entries), view, within),
        };
        match answer {
            Pending::Counts => return Pending::Counts,
            Pending::Unconfirmed(trace) => newest_unconfirmed = Some(trace),
            Pending::No => {}
        }
    }
    newest_unconfirmed.map_or(Pending::No, Pending::Unconfirmed)
}

/// The candidates among `entries`, oldest first, newest first: an MSAA
/// focus, foreground or menu start.
fn candidates_of(entries: &[Entry]) -> Vec<Candidate> {
    entries
        .iter()
        .rev()
        .filter_map(|entry| match entry {
            Entry::Candidate(candidate) => Some((**candidate).clone()),
            Entry::Menu(candidate) if is_menu_start(candidate.tell.kind) => {
                Some((**candidate).clone())
            }
            _ => None,
        })
        .collect()
}

/// A run's pending judgement, `candidates` newest first.
fn run_pending(candidates: Vec<Candidate>, view: &View<'_>, within: Within) -> Pending {
    let candidates: Vec<Candidate> = candidates
        .into_iter()
        .filter(|candidate| Within::raised(within.raised_before, candidate.tell.at_us))
        .collect();
    let mut unconfirmed = None;
    let mut rest = candidates.as_slice();
    loop {
        let found = judge_run(rest, view).found;
        let unconfirmed_or_no = unconfirmed.map_or(Pending::No, Pending::Unconfirmed);
        let Some(found) = found else {
            return unconfirmed_or_no;
        };
        match found.answer {
            Validity::Valid(_) => return Pending::Counts,
            Validity::Duplicate | Validity::Refused(_) => return unconfirmed_or_no,
            Validity::NeedsRead => {
                let tell = &found.candidate.tell;
                if Within::raised(within.unconfirmed_before, tell.at_us) {
                    unconfirmed.get_or_insert(tell.trace_id);
                }
                rest = found.older;
            }
        }
    }
}

/// `judged` with a view of Core's facts now, from inside an entry's
/// function; `run_before_valid` says whether the run before a menu start
/// executed a valid focus.
pub(crate) fn with_view<T>(
    cx: &Cx<Entries>,
    run_before_valid: bool,
    judged: impl FnOnce(&View<'_>) -> T,
) -> T {
    let state = cx.facts();
    let backend = cx.backend();
    let view = View {
        versions: &state.versions,
        in_front: cx.foreground().map(|foreground| foreground.window),
        backend: &backend,
        now_us: cx.now_us(),
        run_before_valid,
    };
    judged(&view)
}

/// [`judge`] from inside an entry's function.
pub(crate) fn at(
    cx: &Cx<Entries>,
    subject: Subject<'_>,
    facts: Option<&CandidateFacts>,
    run_before_valid: bool,
) -> Validity {
    with_view(cx, run_before_valid, |view| judge(subject, view, facts))
}

/// [`judge`] with the facts an outpost read, from inside an entry's
/// function: the answer is settled, as facts leave nothing to read, which
/// the types say.
pub(crate) fn read_at(
    cx: &Cx<Entries>,
    subject: Subject<'_>,
    facts: &CandidateFacts,
    run_before_valid: bool,
) -> Settled {
    with_view(cx, run_before_valid, |view| {
        match settle(subject, view, Ok::<_, Infallible>(facts)) {
            Ok(settled) => settled,
            Err(never) => match never {},
        }
    })
}

/// [`pending_behind`] from inside the front entry's function, over the
/// entries waiting behind it.
pub(crate) fn pending_behind_front(cx: &Cx<Entries>, within: Within) -> Pending {
    with_view(cx, false, |view| {
        pending_behind(&cx.waiting(), view, within)
    })
}

/// The waiting candidate of trace `trace`, alone or in a run: an MSAA
/// focus, foreground or menu start.
pub(crate) fn waiting_candidate(cx: &Cx<Entries>, trace: TraceId) -> Option<Candidate> {
    cx.waiting().iter().find_map(|item| {
        let entries = match item {
            Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_ref(entry),
            Item::Run(entries) => entries.as_slice(),
        };
        entries.iter().find_map(|entry| match entry {
            Entry::Candidate(candidate) | Entry::Menu(candidate)
                if candidate.tell.trace_id == trace =>
            {
                Some((**candidate).clone())
            }
            _ => None,
        })
    })
}

/// Writes the facts a caret key's confirmation read, `facts`, onto the
/// waiting candidate of trace `trace`, alone or in a run, its confirmation
/// going from unread to read: the only writer of a candidate's
/// confirmation. A candidate merged away or capped meanwhile is not
/// written.
pub(crate) fn store_facts(cx: &mut Cx<Entries>, trace: TraceId, facts: CandidateFacts) {
    let mut waiting = cx.waiting_mut();
    let found = waiting.iter_mut().find_map(|item| {
        let entries = match item {
            Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_mut(entry),
            Item::Run(entries) => entries.as_mut_slice(),
        };
        entries.iter_mut().find_map(|entry| match entry {
            Entry::Candidate(candidate) | Entry::Menu(candidate)
                if candidate.tell.trace_id == trace =>
            {
                Some(candidate)
            }
            _ => None,
        })
    });
    if let Some(candidate) = found {
        candidate.confirmation = Confirmation::Read(facts);
    }
}

fn is_menu_start(kind: EventKind) -> bool {
    matches!(kind, EventKind::MenuStart | EventKind::PopupStart)
}

/// The rows for a UI Automation focus or menu opening.
fn uia(focus: &UiaFocus, view: &View<'_>) -> Settled {
    let tell = &focus.tell;
    let facts = match &tell.window {
        TellWindow::Found(facts) => Some(facts.as_ref()),
        TellWindow::NoWindow | TellWindow::Failed => None,
    };
    // The menu opening on the committed element is a focus on it
    // (`UIAHandler/__init__.py` 826 to 835): acceptance, then its
    // keyboard-focus answer.
    if opens_on_committed(view.versions, focus) {
        if acceptance::accept(tell.kind, facts, false).is_err() {
            return Settled::Refused(Reason::Acceptance);
        }
        if keyboard_focus(tell) == Some(Read::Answered(false)) {
            return Settled::Refused(Reason::KeyboardFocusLost);
        }
        return Settled::Valid(How::Again);
    }
    let Some(window) = judged_window(tell) else {
        return Settled::Refused(Reason::NoWindow);
    };
    if view
        .backend
        .judge(Judged::Uia, &window, tell.at_us, view.now_us)
        == Judgement::Drop
    {
        return Settled::Refused(Reason::Verdict);
    }
    let overridden = focus_override(tell);
    if overridden == Override::Refused {
        return Settled::Refused(Reason::Override);
    }
    if uia_on_hidden_frame(focus) {
        return Settled::Refused(Reason::HiddenFrame);
    }
    if acceptance::accept(tell.kind, facts, false).is_err() {
        return Settled::Refused(Reason::Acceptance);
    }
    if overridden != Override::NetUiMenuItem && keyboard_focus(tell) == Some(Read::Answered(false))
    {
        return Settled::Refused(Reason::KeyboardFocusLost);
    }
    if uia_duplicate(view.last(), &focus.runtime_id, tell.checked.as_ref()) {
        return Settled::Duplicate;
    }
    Settled::Valid(How::Read)
}

/// The rows for an MSAA focus, foreground or menu start, `facts` what an
/// outpost read of it or, missing, `M`: a row that needs the missing facts
/// answers the read it needs with `M`. With facts given, `M` is
/// uninhabited, so the answer is settled.
fn msaa<M>(
    candidate: &Candidate,
    view: &View<'_>,
    facts: Result<&CandidateFacts, M>,
) -> Result<Settled, M> {
    let tell = &candidate.tell;
    let ids = &candidate.ids;
    let last = view.last();
    if let Some(reason) = before_duplicate(candidate, view) {
        return Ok(Settled::Refused(reason));
    }
    let menu_start = is_menu_start(tell.kind);
    if tell.kind == EventKind::Foreground {
        // `IAccessibleHandler/__init__.py` 834 to 863.
        if view.in_front != Some(ids.window) {
            return Ok(Settled::Refused(Reason::NotInFront));
        }
        if let Some(last) = last {
            if last.place.holds_window(ids.window) {
                return Ok(Settled::Refused(Reason::LastFocusWindow));
            }
            if last_event_ids(last) == Some(ids) {
                return Ok(Settled::Refused(Reason::SameIds));
            }
        }
    } else if !menu_start && msaa_duplicate(last, ids) {
        return Ok(Settled::Duplicate);
    }
    if menu_start {
        // `IAccessibleHandler/__init__.py` 942 to 973.
        if view.run_before_valid && last_is_msaa_menu(last) {
            return Ok(Settled::Refused(Reason::OnMenu));
        }
        let facts = facts?;
        if let Some(refused) = refused_by(facts) {
            return Ok(refused);
        }
        return Ok(match facts {
            CandidateFacts::Found {
                role: Some(Role::Menu),
                ..
            } => Settled::Valid(How::Read),
            _ => Settled::Refused(Reason::NotMenuPopup),
        });
    }
    let facts = match facts {
        Ok(facts) => facts,
        Err(missing) if tell.kind == EventKind::MsaaFocus => {
            return Err(missing);
        }
        Err(_) => return Ok(Settled::Valid(How::Read)),
    };
    Ok(refused_by(facts).unwrap_or(Settled::Valid(How::Read)))
}

/// The rows every MSAA candidate passes before its duplicate test: its
/// window's verdict, Verbatim's hidden frame, acceptance and the desktop
/// window. The refusal, if any. It is also the identity row, at the answer
/// of a read that [`depth`] named the readers' last focus to and that
/// answered it is that focus
/// ([`super::msaa_run::TurnAnswer::SameAsNamed`]): `None` is a duplicate,
/// as the readers' last focus cannot change while the read is out.
pub(crate) fn before_duplicate(candidate: &Candidate, view: &View<'_>) -> Option<Reason> {
    let tell = &candidate.tell;
    if view.backend.judge(
        Judged::Msaa,
        &candidate.window.judged(),
        tell.at_us,
        view.now_us,
    ) == Judgement::Drop
    {
        return Some(Reason::Verdict);
    }
    if msaa_on_hidden_frame(candidate) {
        return Some(Reason::HiddenFrame);
    }
    if acceptance::accept(tell.kind, candidate.facts(), false).is_err() {
        return Some(Reason::Acceptance);
    }
    if candidate.facts().is_some_and(|facts| facts.desktop) {
        return Some(Reason::NoReader);
    }
    None
}

/// The rows for the menu check's read, `facts` as for [`msaa`].
fn menu_check<M>(
    ids: Option<&MsaaIds>,
    view: &View<'_>,
    facts: Result<&CandidateFacts, M>,
) -> Result<Settled, M> {
    if ids.is_some_and(|ids| msaa_duplicate(view.last(), ids)) {
        return Ok(Settled::Duplicate);
    }
    Ok(refused_by(facts?).unwrap_or(Settled::Valid(How::Read)))
}

/// The recovered read's one row: a UI Automation answer whose runtime id
/// equals the readers' last focus's is a duplicate, its keyboard focus
/// taken as true because the read is of the keyboard focus. Any other is
/// valid, and goes on to the re-take's same-reading comparison.
fn recovered(view: &View<'_>, read: &FocusRead) -> Settled {
    match (
        view.last().map(|last| &last.read_by),
        &read.msaa,
        &read.focus.runtime_id,
    ) {
        (Some(ReadBy::Uia { runtime_id, .. }), None, Some(answered)) if runtime_id == answered => {
            Settled::Duplicate
        }
        _ => Settled::Valid(How::Read),
    }
}

/// The refusal the facts an outpost read give, if any.
fn refused_by(facts: &CandidateFacts) -> Option<Settled> {
    match facts {
        CandidateFacts::Skipped => Some(Settled::Refused(Reason::Skipped)),
        CandidateFacts::NotThere => Some(Settled::Refused(Reason::NotThere)),
        CandidateFacts::Found {
            focused: Some(false),
            ..
        } => Some(Settled::Refused(Reason::NotFocused)),
        CandidateFacts::Found { .. } => None,
    }
}

/// The event ids the readers' last focus was read by, when it is an MSAA
/// focus read from an event.
fn last_event_ids(last: &LastFocus) -> Option<&MsaaIds> {
    match &last.read_by {
        ReadBy::Msaa {
            event: Some(earlier),
            ..
        } => Some(earlier),
        ReadBy::Msaa { event: None, .. } | ReadBy::Uia { .. } | ReadBy::Plain => None,
    }
}

/// Whether the readers' last focus is an MSAA menu popup or menu item.
fn last_is_msaa_menu(last: Option<&LastFocus>) -> bool {
    last.is_some_and(|last| {
        matches!(last.read_by, ReadBy::Msaa { .. })
            && matches!(last.role, Role::Menu | Role::MenuItem)
    })
}

/// Whether `focus` is a menu opening on the committed focus's runtime id.
fn opens_on_committed(versions: &Versions, focus: &UiaFocus) -> bool {
    focus.tell.kind == EventKind::UiaMenuOpened
        && versions.committed_read().is_some_and(|kept| {
            matches!(&kept.read_by, ReadBy::Uia { runtime_id, .. } if *runtime_id == focus.runtime_id)
        })
}

/// The keyboard-focus answer the check thread carried.
fn keyboard_focus(tell: &Event) -> Option<Read<bool>> {
    tell.checked
        .as_ref()
        .and_then(|checked| checked.keyboard_focus)
}

/// Whether `focus` names Verbatim's hidden frame's own element
/// (Verbatim-only, design "Focus and foreground").
fn uia_on_hidden_frame(focus: &UiaFocus) -> bool {
    let TellWindow::Found(facts) = &focus.tell.window else {
        return false;
    };
    facts.hidden_frame
        && matches!(
            focus
                .tell
                .cached
                .answer(PropertyId::NativeWindow)
                .and_then(|answer| answer.value.as_ref()),
            Some(PropertyValue::Window(window)) if *window == facts.window.handle
        )
}

/// Whether `candidate` names Verbatim's hidden frame's own object
/// (Verbatim-only, design "Focus and foreground").
fn msaa_on_hidden_frame(candidate: &Candidate) -> bool {
    candidate.facts().is_some_and(|facts| {
        facts.hidden_frame
            && candidate.ids.window == facts.window.handle
            && matches!(candidate.ids.object, OBJID_WINDOW | OBJID_CLIENT)
            && candidate.ids.child == 0
    })
}

fn text(values: &PropertyValues, id: PropertyId) -> Option<&str> {
    match values.answer(id).and_then(|answer| answer.value.as_ref()) {
        Some(PropertyValue::Text(text)) => Some(text),
        _ => None,
    }
}

fn role(values: &PropertyValues) -> Option<Role> {
    match values
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
}

/// NVDA's focus overrides for a UI Automation focus `tell`, from its
/// cached automation id, class name and role and its nearest window's
/// class: the console window's own element, Excel's `EXCEL6` pane and an
/// unnamed first `NetUITWMenuItem`; and File Explorer's task switcher
/// frame, by the tell's application name.
pub(crate) fn focus_override(tell: &Event) -> Override {
    let values = &tell.cached;
    let window_class = match &tell.window {
        TellWindow::Found(facts) => Some(facts.window.class.as_str()),
        TellWindow::NoWindow | TellWindow::Failed => None,
    };
    let automation_id = text(values, PropertyId::AutomationId);
    let class_name = text(values, PropertyId::ClassName);
    let role = role(values);
    if automation_id == Some("Console Window") && window_class == Some("ConsoleWindowClass") {
        return Override::Refused;
    }
    if window_class == Some("EXCEL6") && role == Some(Role::Pane) {
        return Override::Refused;
    }
    if tell.application == FILE_EXPLORER
        && matches!(
            (class_name, role),
            (Some("MultitaskingViewFrame"), Some(Role::Window))
                | (
                    Some("Windows.UI.Input.InputSite.WindowClass"),
                    Some(Role::Pane)
                )
        )
    {
        return Override::Refused;
    }
    let unnamed = text(values, PropertyId::Name).is_none_or(str::is_empty);
    let first = tell
        .checked
        .as_ref()
        .and_then(|checked| checked.previous_sibling)
        == Some(Read::Answered(false));
    if class_name == Some("NetUITWMenuItem") && role == Some(Role::MenuItem) && unnamed && first {
        return Override::NetUiMenuItem;
    }
    Override::None
}

/// The window `tell` is judged by, as the backend cache judges it.
fn judged_window(tell: &Event) -> Option<JudgedWindow> {
    let TellWindow::Found(facts) = &tell.window else {
        return None;
    };
    Some(JudgedWindow {
        key: WindowKey {
            handle: facts.window.handle.0,
            thread: facts.window.thread,
        },
        by_class: by_class(&facts.window.class, &facts.root.class),
    })
}

#[cfg(test)]
#[path = "validity_tests.rs"]
mod tests;
