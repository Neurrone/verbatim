//! A caret key as an event-queue entry (`docs/design/readers-tell-core-asks-build1.md`,
//! "A caret key at its turn", "The watch", "A later key and a pending
//! focus", "The sentence move", Table 7).
//!
//! At its turn a held key asks its starting read of the node it was bound
//! to at its press, which the outpost holds as the starting point; on the
//! answer Core sends the key on (the pass) and asks the watch, which looks
//! for the key's effect from the starting point until its bound, then reads
//! the caret once more. A later key, command or key Core sends, or a focus
//! pending behind the key, cancels it and the key says nothing, except
//! Backspace and Control+Backspace, which say what they deleted from the
//! starting read (Dickson, 2026-11-16). A sentence key that found no
//! effect in text with a sentence unit moves by sentence from its starting
//! point. There is no settle: the next key's starting read is taken at its
//! turn. The 150 ms release counts from the press (`crate::keys::strokes`).
//!
//! Its states are its function's places: waiting (Table 7's S1, its stroke
//! unsent, or S2, sent), starting (S3, the starting read out), watching
//! (S4) and moving by sentence (S5). Whether it is marked is read from the
//! arrivals and whether its stroke is unsent from the stroke line, each
//! when needed, never stored on the entry.

use verbatim_model::request::{CancelReason, CaretBinding, Reply, Request as Wire};
use verbatim_model::{
    CaretKey, CaretMotion, KeySeq, NodeId, OutpostId, SentenceMove, SentenceMoved, Started,
    StartingRead, StartingUnit, TextDetails, TextFailure, TextOp, TextProvider, TextReply, TraceId,
    UtteranceSegment, Watch, WatchFinalRead, Watched, WatchedRead,
};

use verbatim_model::TextAnchor;
use verbatim_model::request::{CandidateFacts, Depth};

use crate::editing;
use crate::entries::{Entries, Entry};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::msaa_run::{FactsOnly, RunRequest};
use crate::focus::validity::{Pending, Within, pending_behind_front};
use crate::focus::versions::Committed;
use crate::keys::strokes::StrokeStatus;
use crate::state::SrState;

/// How long after the pass a watch looks for the key's effect: NVDA's
/// `caretMoveTimeoutMs` (`docs/parity/text.md`).
const BOUND_US: u32 = 100_000;

/// How often a watch checks a text field's caret: NVDA's sleep between
/// its tries (`docs/parity/text.md`).
const POLL_US: u32 = 10_000;

/// How often a watch checks the console host's caret: Verbatim-only
/// (ruled 2026-10-26).
const CONSOLE_POLL_US: u32 = 5_000;

/// How long a watch looks in Windows Terminal: NVDA's bound multiplied by
/// its terminal's 3 (`NVDAObjects/UIA/winConsoleUIA.py` 470, 486); the
/// console host's is NVDA's own, its multiplier 1 on consoles of the
/// improved interface (374 to 376).
const TERMINAL_BOUND_US: u32 = 300_000;

/// A caret key's entry, as its function takes it.
#[derive(Clone, Debug)]
pub(crate) struct CaretPress {
    /// The trace minted when the hook saw the key.
    pub(crate) trace_id: TraceId,
    /// Its number.
    pub(crate) key_seq: KeySeq,
    /// When the hook saw it, in microseconds on the shared clock.
    pub(crate) pressed_at_us: u64,
    /// Which key.
    pub(crate) key: CaretKey,
    /// Whether the hook held it back.
    pub(crate) held: bool,
    /// The committed focus's record at its press.
    pub(crate) bound: Option<CaretBinding>,
}

/// The committed focus's record a caret key pressed now is bound to, as
/// the hook's view publishes it: `None` with nothing committed, a focus
/// whose reader ended, Core's own focus, or a focus that cannot have text.
pub(crate) fn binding(state: &SrState) -> Option<CaretBinding> {
    let Committed::Read(kept) = &state.versions.committed else {
        return None;
    };
    if !editing::may_have_text(kept.role) && state.focus_caret(kept.node).is_none() {
        return None;
    }
    let (record, formatting) = match editing::focus_record(state) {
        Some((node, record, formatting)) if node == kept.node => (record, formatting),
        _ => (None, verbatim_model::TextAttributes::default()),
    };
    Some(CaretBinding {
        node: kept.node,
        window: kept.window.node_window(),
        role: kept.role,
        provider: match kept.read_by {
            crate::focus::versions::ReadBy::Uia { .. } => TextProvider::Uia,
            crate::focus::versions::ReadBy::Msaa { .. } => TextProvider::EditMessages,
        },
        record,
        formatting,
    })
}

/// One of a caret key's requests on its bound node.
#[derive(Clone, Debug)]
struct OnNode {
    window: RequestWindow,
    node: NodeId,
    op: TextOp,
}

impl OnNode {
    fn of(binding: &CaretBinding, op: TextOp) -> Self {
        Self {
            window: RequestWindow::of_node(&binding.window),
            node: binding.node,
            op,
        }
    }

    fn wire(&self) -> Wire {
        Wire::Text {
            node: self.node,
            op: self.op.clone(),
        }
    }
}

/// The starting read, answered with the starting point.
#[derive(Debug)]
struct StartingReadRequest(OnNode);

impl Request for StartingReadRequest {
    type Reply = Result<Started, TextFailure>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        crate::text_request::decode_text(reply, |reply| match reply {
            TextReply::Started(started) => Some(*started),
            _ => None,
        })
    }
}

/// The watch, answered with how it ended.
#[derive(Debug)]
struct WatchRequest(OnNode);

impl Request for WatchRequest {
    type Reply = Result<Watched, TextFailure>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        crate::text_request::decode_text(reply, |reply| match reply {
            TextReply::Watched(watched) => Some(*watched),
            _ => None,
        })
    }
}

/// The move by sentence from the starting point.
#[derive(Debug)]
struct SentenceRequest(OnNode);

impl Request for SentenceRequest {
    type Reply = Result<SentenceMoved, TextFailure>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        crate::text_request::decode_text(reply, |reply| match reply {
            TextReply::Sentence(moved) => Some(*moved),
            _ => None,
        })
    }
}

/// Whether the front entry `front`, a caret key, no longer needs
/// `request`: its starting read, once its stroke went by its 150 ms
/// release or was dropped. Table 7, S3, columns D and H.
pub(crate) fn unneeded(state: &SrState, front: &CaretPress, request: &Wire) -> bool {
    let starting = match request {
        Wire::Text { op, .. } => match op {
            TextOp::StartingRead(_) => true,
            TextOp::ReadTerminal(read) => read.starting.is_some(),
            _ => false,
        },
        _ => false,
    };
    starting && front.held && state.strokes.status(front.key_seq) != StrokeStatus::Unsent
}

/// What joins the queue behind a caret key, as its stage tables read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arrival {
    /// A key, command or key Core sends.
    Key,
    /// A focus-kind entry, judged alone by the one definition before the
    /// limiter places it, raised at `at_us`.
    Focus {
        /// Whether it counts as pending.
        pending: Pending,
        /// When it was raised.
        at_us: u64,
    },
    /// Anything else.
    Other,
}

impl Arrival {
    /// A focus that counts.
    fn counts(self) -> bool {
        matches!(
            self,
            Self::Focus {
                pending: Pending::Counts,
                ..
            }
        )
    }

    /// An unconfirmed focus raised before `bound_at_us`.
    fn unconfirmed_before(self, bound_at_us: u64) -> bool {
        matches!(
            self,
            Self::Focus {
                pending: Pending::Unconfirmed(_),
                at_us,
            } if at_us < bound_at_us
        )
    }
}

/// The stage of a caret key in an edit field, derived from the request it
/// has out, never stored (design, "Stage and arrival"; ADR 0003 rejects
/// "Core's sequences written as step functions in a reducer, each wait a
/// named stage", so the key stays an async function and this only reads
/// its request).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaretStage {
    /// Its starting read is out.
    Starting {
        /// Whether the key is Backspace or Control+Backspace, whose read
        /// is kept to say what it deleted.
        deletes_back: bool,
    },
    /// Its watch is out, looking for its effect until `bound_at_us`.
    Watching {
        /// The watch's bound.
        bound_at_us: u64,
    },
    /// It asks an unconfirmed focus's facts.
    Confirming,
    /// It moves by sentence.
    Sentence,
}

impl CaretStage {
    /// The stage `request`, out for `press`, puts the key in.
    fn of(press: &CaretPress, request: &Wire) -> Option<Self> {
        match request {
            Wire::Text { op, .. } => match op {
                TextOp::StartingRead(_) => Some(Self::Starting {
                    deletes_back: press.key.motion.deletes_back(),
                }),
                TextOp::Watch(watch) => Some(Self::Watching {
                    bound_at_us: watch.passed_at_us.saturating_add(u64::from(watch.bound_us)),
                }),
                TextOp::MoveBySentence(_) => Some(Self::Sentence),
                _ => None,
            },
            Wire::MsaaRun(run) if run.depth == Depth::Facts => Some(Self::Confirming),
            _ => None,
        }
    }

    /// Whether `arrival` withdraws the request of this stage: one arm per
    /// cell of the edit path's stage table (design, "The stage table").
    fn withdrawn_by(self, arrival: Arrival) -> bool {
        match self {
            Self::Starting { deletes_back } => arrival.counts() && !deletes_back,
            Self::Watching { bound_at_us } => {
                arrival == Arrival::Key
                    || arrival.counts()
                    || arrival.unconfirmed_before(bound_at_us)
            }
            Self::Confirming => arrival == Arrival::Key || arrival.counts(),
            Self::Sentence => arrival.counts(),
        }
    }
}

/// The stage of a caret key in a terminal, derived from its request as
/// [`CaretStage`] is, with a stage table of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalStage {
    /// Its starting read, the terminal's full read, is out.
    Starting,
    /// Its watch is out, looking for its effect until `bound_at_us`.
    Watching {
        /// The watch's bound.
        bound_at_us: u64,
    },
    /// It asks an unconfirmed focus's facts.
    Confirming,
    /// Its final read is out, after the bound.
    Final,
}

impl TerminalStage {
    /// The stage `request` puts a terminal's key in.
    fn of(request: &Wire) -> Option<Self> {
        match request {
            Wire::Text { op, .. } => match op {
                TextOp::ReadTerminal(read) if read.starting.is_some() => Some(Self::Starting),
                TextOp::ReadTerminal(_) => Some(Self::Final),
                TextOp::Watch(watch) => Some(Self::Watching {
                    bound_at_us: watch.passed_at_us.saturating_add(u64::from(watch.bound_us)),
                }),
                _ => None,
            },
            Wire::MsaaRun(run) if run.depth == Depth::Facts => Some(Self::Confirming),
            _ => None,
        }
    }

    /// Whether `arrival` withdraws the request of this stage: one arm per
    /// cell of the terminal path's stage table. The final read comes after
    /// the bound, so an unconfirmed focus never withdraws it.
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per cell of the stage table, as the design asks"
    )]
    fn withdrawn_by(self, arrival: Arrival) -> bool {
        match self {
            Self::Starting => arrival.counts(),
            Self::Watching { bound_at_us } => {
                arrival == Arrival::Key
                    || arrival.counts()
                    || arrival.unconfirmed_before(bound_at_us)
            }
            Self::Confirming => arrival == Arrival::Key || arrival.counts(),
            Self::Final => arrival == Arrival::Key || arrival.counts(),
        }
    }
}

/// Whether `press` takes the terminal's path: it is bound to a grid, a
/// terminal. The key picks its path once, at its turn, by the same test.
fn terminal_path(press: &CaretPress) -> bool {
    press
        .bound
        .as_ref()
        .is_some_and(|bound| editing::is_grid(bound.role))
}

/// Whether `arrival`, joining behind the front caret key `front`,
/// withdraws `request`, by the stage table of the key's path.
pub(crate) fn withdrawn_by(front: &CaretPress, request: &Wire, arrival: Arrival) -> bool {
    if terminal_path(front) {
        TerminalStage::of(request).is_some_and(|stage| stage.withdrawn_by(arrival))
    } else {
        CaretStage::of(front, request).is_some_and(|stage| stage.withdrawn_by(arrival))
    }
}

/// How a confirmation ended.
enum Confirmed {
    /// Its facts were stored, or there was nothing to read: judge again.
    Judged,
    /// Withdrawn by an arrival: judge the arrival.
    Withdrawn,
    /// Not responding, past its due time, stopped by the foreground, or a
    /// malformed answer: the key ends saying nothing, Backspace included,
    /// as a key whose watch ends so does (the stage table's "Core stops"
    /// and "as today" cells).
    Stopped,
}

/// Confirms the waiting candidate `trace` with a read of its facts alone,
/// naming the candidate's window, so that the hang check, the
/// not-responding sound and the foreground stop all judge the window
/// being read; `spare`, the key's node and starting point, stays held
/// while it is out (design, "Confirming, and its bound").
async fn confirm(
    cx: &mut Cx<Entries>,
    trace: TraceId,
    spare: Option<(NodeId, TextAnchor)>,
    due_us: u64,
) -> Confirmed {
    let Some(candidate) = crate::focus::validity::waiting_candidate(cx, trace) else {
        return Confirmed::Judged;
    };
    let known = crate::focus::msaa_run::known_for(cx, candidate.tell.process);
    let request = RunRequest::of(cx, (&candidate, &FactsOnly), spare, &known);
    match cx.ask(request, due_us).await {
        Outcome::Result(facts) => {
            crate::focus::validity::store_facts(cx, trace, facts);
            Confirmed::Judged
        }
        // The candidate's outpost ended, or its object is gone: stored as
        // not there, and judged again.
        Outcome::ReaderEnded | Outcome::NotThere => {
            crate::focus::validity::store_facts(cx, trace, CandidateFacts::NotThere);
            Confirmed::Judged
        }
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => Confirmed::Withdrawn,
        Outcome::Refused(_) | Outcome::NotResponding | Outcome::NoLongerAwaited(_) => {
            Confirmed::Stopped
        }
    }
}

/// Asks a caret key's starting read: its answer, or `None` for any other
/// outcome, after which the key only passes. A node with no text has no
/// caret to report, as NVDA binds its caret scripts to editable text
/// alone (`editableText.py`); any other failure passes the key unread.
async fn starting_read(
    cx: &mut Cx<Entries>,
    request: StartingReadRequest,
    due_us: u64,
) -> Option<Started> {
    match cx.ask(request, due_us).await {
        Outcome::Result(Ok(started)) => Some(started),
        Outcome::Result(Err(
            TextFailure::NoText
            | TextFailure::Unsupported
            | TextFailure::UnsupportedUnit(_)
            | TextFailure::AnchorLost
            | TextFailure::Gone
            | TextFailure::Unanswered,
        ))
        | Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => None,
    }
}

/// The caret key's function at its turn (Table 7), then what the
/// character it types does to the word being typed ([`typed_at_end`]),
/// unless it was dropped and never reached the application.
pub(crate) async fn run(mut cx: Cx<Entries>, press: CaretPress) {
    if run_key(&mut cx, &press).await == Reached::Dropped {
        return;
    }
    typed_at_end(&mut cx, &press);
}

/// Whether a key's stroke reached the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reached {
    /// Sent, at its pass or by its 150 ms release.
    Sent,
    /// Dropped at a switch away from the user's desktop.
    Dropped,
}

/// The caret key at its turn, to its end (Table 7).
async fn run_key(cx: &mut Cx<Entries>, press: &CaretPress) -> Reached {
    let status = if press.held {
        cx.facts().strokes.status(press.key_seq)
    } else {
        StrokeStatus::Sent
    };
    match status {
        StrokeStatus::Dropped => {
            cx.facts_mut().strokes.forget_dropped(press.key_seq);
            return Reached::Dropped;
        }
        StrokeStatus::Sent => {
            // S2: released by its 150 ms timer, or never held. Its selection
            // words, if any, come from its caret tell. A held key bound to
            // the focused terminal passed at its release, which its model
            // takes now. Its script never ran, so the word typed is kept,
            // as NVDA forgets it only when a caret script speaks
            // (`docs/parity/input.md`, "Typed-character echo").
            if press.held && press.terminal_bound(&cx.facts()) {
                let released_at_us = press
                    .pressed_at_us
                    .saturating_add(crate::keys::strokes::RELEASE_AFTER_US)
                    .min(cx.now_us());
                crate::keys::terminal::passed(
                    &mut cx.facts_mut(),
                    crate::terminal::caret_key_kind(press.key),
                    released_at_us,
                    press.trace_id,
                );
            }
            stop_say_all(cx);
            return Reached::Sent;
        }
        StrokeStatus::Unsent => {}
    }
    let Some(binding) = press.bound.clone().filter(|binding| held_node(cx, binding)) else {
        pass(cx, press);
        return Reached::Sent;
    };
    if editing::is_grid(binding.role) {
        if press.key.motion == CaretMotion::NewLine {
            // Enter held for a terminal, by a hook view behind the focus:
            // the carriage return it types there, with no read, as where
            // the view is current.
            terminal_pass(cx, press);
        } else {
            terminal_key(cx, press, &binding).await;
        }
        return Reached::Sent;
    }
    if marked(cx, press) {
        pass(cx, press);
        return Reached::Sent;
    }
    // Before the starting read, only a focus that counts stops the key;
    // an unconfirmed one waits behind it, judged at the pass.
    if !press.key.motion.deletes_back() && counts_behind(cx) {
        pass(cx, press);
        return Reached::Sent;
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let starting = StartingRead {
        provider: binding.provider,
        details: key_details(cx, press),
        unit: StartingUnit::for_motion(press.key.motion),
    };
    let request = StartingReadRequest(OnNode::of(&binding, TextOp::StartingRead(starting)));
    let Some(started) = starting_read(cx, request, due_us).await else {
        pass(cx, press);
        return Reached::Sent;
    };
    let passed_at_us = cx.now_us();
    pass(cx, press);
    if marked(cx, press) {
        marked_end(cx, press, (&binding, &started), due_us).await;
        return Reached::Sent;
    }
    after_pass(cx, press, (&binding, &started), (passed_at_us, due_us)).await;
    Reached::Sent
}

/// What the character the key typed does to the word being typed, at the
/// key's end, after its own speech, as NVDA's typed-character event runs
/// after the key's script (`docs/parity/input.md`, "Typed-character
/// echo"): Backspace's U+0008 drops the word's last character, one code
/// point; Enter's carriage return ends the word and speaks it, unless a
/// focus that counts waits behind the key, whose taking forgets the word
/// unspoken, as NVDA's focus change forgets it (`docs/parity/text.md`,
/// "Enter in a text field"). In a terminal Enter's carriage return is
/// typed text, as where the hook did not hold it.
fn typed_at_end(cx: &mut Cx<Entries>, press: &CaretPress) {
    match press.key.motion {
        CaretMotion::Backspace => {
            cx.facts_mut().typed_word.pop();
        }
        CaretMotion::NewLine => {
            if counts_behind(cx) {
                cx.facts_mut().typed_word.clear();
                return;
            }
            let effects = editing::character_typed(&mut cx.facts_mut(), press.trace_id, "\r");
            for effect in effects {
                cx.emit(effect);
            }
        }
        _ => {}
    }
}

/// After the pass: the key judges what waits behind it by the one
/// definition until its bound (design, "Confirming, and its bound"). A
/// focus that counts silences it; an unconfirmed one is confirmed by a
/// read of its facts, then judged again; with neither, the watch runs
/// for the rest of the bound. A withdrawn watch or confirmation judges
/// the arrival: a key marks it, a focus is judged again.
async fn after_pass(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Started),
    (passed_at_us, due_us): (u64, u64),
) {
    let bound_at_us = passed_at_us.saturating_add(u64::from(BOUND_US));
    loop {
        let withdrawn = match pending_behind_front(cx, Within::key(bound_at_us)) {
            Pending::Counts => {
                silent(cx, press, (binding, started));
                return;
            }
            Pending::Unconfirmed(trace) => {
                match confirm(cx, trace, Some((binding.node, started.point)), due_us).await {
                    Confirmed::Judged => false,
                    Confirmed::Withdrawn => true,
                    Confirmed::Stopped => return,
                }
            }
            Pending::No => {
                match watching(cx, press, (binding, started), (passed_at_us, due_us)).await {
                    WatchEnd::Ended => return,
                    WatchEnd::Withdrawn => true,
                }
            }
        };
        // A key, command or key Core sends waiting behind the key is tested
        // before a pending focus, as NVDA's wait returns on a waiting script
        // before it looks for a focus event (`editableText.py` 94 to 98).
        if withdrawn && marked(cx, press) {
            marked_end(cx, press, (binding, started), due_us).await;
            return;
        }
    }
}

/// The key is silent because a focus counts or its read failed: Backspace
/// and Control+Backspace still say what they deleted.
fn silent(cx: &mut Cx<Entries>, press: &CaretPress, (binding, started): (&CaretBinding, &Started)) {
    if press.key.motion.deletes_back() {
        let words = deleted(&cx.facts(), binding, press.key, started);
        say(cx, press, words);
    }
}

/// The key is marked: Backspace says nothing and the sentence key still
/// moves, saying nothing.
async fn marked_end(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Started),
    due_us: u64,
) {
    if moves_by_sentence(press, binding) {
        sentence(cx, press, (binding, started), due_us).await;
    }
}

/// A terminal's caret key at its turn (`docs/design/readers-tell-core-asks-build1.md`,
/// "A terminal's caret key"): its starting read is the terminal's full
/// read, which mints the caret starting point; the key passes, its model
/// takes it, and the watch looks for its effect with no event shortcut;
/// then the final read, another full read, whose answer the model takes.
/// The key says nothing itself: the reading rules speak each read, the
/// terminal's speech waiting in the queue ahead of the first focus first
/// (build 3, "Answers and speech").
async fn terminal_key(cx: &mut Cx<Entries>, press: &CaretPress, binding: &CaretBinding) {
    if marked(cx, press) || counts_behind(cx) {
        terminal_pass(cx, press);
        return;
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let Some(started) = terminal_read(
        cx,
        press,
        binding.node,
        Some(press.key.motion.unit()),
        due_us,
    )
    .await
    else {
        terminal_pass(cx, press);
        return;
    };
    let passed_at_us = cx.now_us();
    terminal_pass(cx, press);
    if marked(cx, press) {
        return;
    }
    // The starting point is the read's; a read that minted none, a
    // reader's fault, leaves the key's effect to the change it tells.
    let Some(point) = started else {
        return;
    };
    let console = crate::terminal::reading::focused(&cx.facts())
        .is_some_and(|terminal| terminal.node == binding.node && terminal.console);
    let watch = Watch {
        provider: binding.provider,
        point,
        motion: press.key.motion,
        passed_at_us,
        poll_us: if console { CONSOLE_POLL_US } else { POLL_US },
        events: false,
        bound_us: if console { BOUND_US } else { TERMINAL_BOUND_US },
        final_read: WatchFinalRead::Terminal,
    };
    let bound_at_us = passed_at_us.saturating_add(u64::from(watch.bound_us));
    // As the edit path judges after its pass, with the terminal's own
    // stages: a counting focus silences the key, an unconfirmed one is
    // confirmed, and with neither the watch runs for the rest of the bound.
    loop {
        let withdrawn = match pending_behind_front(cx, Within::key(bound_at_us)) {
            Pending::Counts => return,
            Pending::Unconfirmed(trace) => {
                match confirm(cx, trace, Some((binding.node, point)), due_us).await {
                    Confirmed::Judged => false,
                    Confirmed::Withdrawn => true,
                    Confirmed::Stopped => return,
                }
            }
            Pending::No => {
                let request = WatchRequest(OnNode::of(binding, TextOp::Watch(watch)));
                match cx.ask(request, due_us).await {
                    Outcome::Result(Ok(_)) => break,
                    Outcome::NoLongerAwaited(CancelReason::Withdrawn) => true,
                    // A watch answered with a failure ended as one refused.
                    Outcome::Result(Err(_))
                    | Outcome::Refused(_)
                    | Outcome::NotThere
                    | Outcome::NotResponding
                    | Outcome::ReaderEnded
                    | Outcome::NoLongerAwaited(_) => return,
                }
            }
        };
        if withdrawn && marked(cx, press) {
            return;
        }
    }
    let _ = terminal_read(cx, press, binding.node, None, due_us).await;
}

/// A terminal key's full read of `node`, its starting read when `starting`
/// names the key's unit: the model takes its answer, and the terminal's
/// waiting speech, then the read's own, is said. `None` when it brought no
/// read; else the caret starting point it minted, if any.
async fn terminal_read(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    node: NodeId,
    starting: Option<verbatim_model::TextUnit>,
    due_us: u64,
) -> Option<Option<verbatim_model::TextAnchor>> {
    let now_us = cx.now_us();
    let asked = crate::terminal::reading::entry_read(&mut cx.facts_mut(), node, starting, now_us)?;
    let reply = match cx.ask(asked, due_us).await {
        // A read that fails, the terminal's text unread included, brings
        // nothing.
        Outcome::Result(Ok(reply)) => Some(reply),
        Outcome::Result(Err(_))
        | Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => None,
    };
    if let Some(reply) = reply.as_deref() {
        cx.emit(crate::terminal::reading::observed(
            &asked.terminal,
            reply,
            press.trace_id,
        ));
    }
    let now_us = cx.now_us();
    let entry = crate::terminal::reading::entry_read_ended(
        &mut cx.facts_mut(),
        &asked,
        reply.as_deref(),
        (now_us, press.trace_id),
    );
    crate::terminal::reading::speak_read(cx, entry);
    reply.map(|reply| reply.point)
}

/// A terminal's held key passes: its stroke is sent, and the focused
/// terminal's model takes it at its pass.
fn terminal_pass(cx: &mut Cx<Entries>, press: &CaretPress) {
    let unsent = press.held && cx.facts().strokes.status(press.key_seq) == StrokeStatus::Unsent;
    pass(cx, press);
    if unsent {
        let now_us = cx.now_us();
        crate::keys::terminal::passed(
            &mut cx.facts_mut(),
            crate::terminal::caret_key_kind(press.key),
            now_us,
            press.trace_id,
        );
    }
}

/// How a watch ended.
enum WatchEnd {
    /// It answered, and the key said what its script says, or failed.
    Ended,
    /// An arrival withdrew it.
    Withdrawn,
}

/// S4: the watch, then the key's words by its script.
async fn watching(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Started),
    (passed_at_us, due_us): (u64, u64),
) -> WatchEnd {
    let watch = Watch {
        provider: binding.provider,
        point: started.point,
        motion: press.key.motion,
        passed_at_us,
        poll_us: POLL_US,
        // NVDA's `caretMovementDetectionUsesEvents` holds for every UI
        // Automation field but a terminal, whose keys this entry does not
        // run: a caret or text-change event of the node within 60 ms of
        // the pass is the key's effect (Dickson, 2026-11-16). A node no
        // longer the committed focus has no `Follow` to hear them.
        events: binding.provider == TextProvider::Uia && committed(&cx.facts(), binding.node),
        bound_us: BOUND_US,
        // A sentence key reads only at its effect: at its bound it moves by
        // sentence where the text has sentences and says nothing where it
        // has none, as NVDA's move by a unit UI Automation lacks fails
        // silently (`editableText.py` 253 to 265; `UIAHandler/__init__.py`
        // 170 to 176).
        final_read: if is_sentence_key(press) {
            WatchFinalRead::UnitAtEffect {
                unit: press.key.motion.unit(),
                details: key_details(cx, press),
            }
        } else {
            WatchFinalRead::Unit {
                unit: press.key.motion.unit(),
                details: key_details(cx, press),
            }
        },
    };
    let request = WatchRequest(OnNode::of(binding, TextOp::Watch(watch)));
    match cx.ask(request, due_us).await {
        Outcome::Result(Ok(Watched::Effect(read))) => {
            let words = effect_words(&mut cx.facts_mut(), binding, press.key, (started, &read));
            finish(cx, press, binding, read, words);
        }
        Outcome::Result(Ok(Watched::Bound(read))) => {
            let words = bound_words(&mut cx.facts_mut(), binding, press.key, &read);
            finish(cx, press, binding, read, words);
        }
        Outcome::Result(Ok(Watched::BoundUnread | Watched::EffectUnread)) => {
            if moves_by_sentence(press, binding) {
                sentence(cx, press, (binding, started), due_us).await;
            }
        }
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => return WatchEnd::Withdrawn,

        // A watch answered with a failure says nothing, as one refused.
        Outcome::Result(Err(_))
        | Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => {}
    }
    WatchEnd::Ended
}

/// S5: the move by sentence from the starting point, its words said
/// unless the key is marked by the time it answers.
async fn sentence(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Started),
    due_us: u64,
) {
    let forward = press.key.motion == CaretMotion::NextSentence;
    let request = SentenceRequest(OnNode::of(
        binding,
        TextOp::MoveBySentence(SentenceMove {
            provider: binding.provider,
            point: started.point,
            forward,
            details: key_details(cx, press),
        }),
    ));
    // A move that fails says nothing, as NVDA's sentence script swallows
    // its exception (`editableText.py` 240 to 264), a source with no
    // sentence unit or a starting point no longer held included.
    let Outcome::Result(Ok(moved)) = cx.ask(request, due_us).await else {
        return;
    };
    // The answer mints the next selection baseline, as Verbatim's own move,
    // whether or not the key speaks (Table 17, column F).
    let (line, words, baseline) = match moved {
        SentenceMoved::Moved {
            sentence,
            line,
            baseline,
        }
        | SentenceMoved::StayedAtStart {
            sentence,
            line,
            baseline,
        } => {
            let words = editing::sentence_words(&line, &sentence);
            (line, words, baseline)
        }
        SentenceMoved::End { line, baseline } => {
            (line, crate::text::character_segments(None, None), baseline)
        }
    };
    crate::text_facts::minted(&mut cx.facts_mut(), binding.node, baseline);
    if marked(cx, press) {
        return;
    }
    let read_at_us = cx.now_us();
    {
        let mut facts = cx.facts_mut();
        facts.typed_word.clear();
        editing::caret_placed(&mut facts, binding.node, line, read_at_us);
    }
    say(cx, press, words);
}

/// The key's words and end, from its final read: the key speaks, so the
/// typed word is forgotten and the caret read becomes Core's, the review
/// cursor following it; a key with nothing to say leaves both to its caret
/// tell. Enter's script forgets no word: the carriage return it types ends
/// the word after it ([`typed_at_end`]).
fn finish(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    binding: &CaretBinding,
    read: WatchedRead,
    words: Vec<UtteranceSegment>,
) {
    if words.is_empty() {
        return;
    }
    {
        let mut facts = cx.facts_mut();
        if press.key.motion != CaretMotion::NewLine {
            facts.typed_word.clear();
        }
        editing::take_caret(&mut facts, binding.node, read.caret);
    }
    say(cx, press, words);
}

/// Says `words` for the key.
fn say(cx: &mut Cx<Entries>, press: &CaretPress, words: Vec<UtteranceSegment>) {
    if !words.is_empty() {
        cx.emit(editing::speak(press.trace_id, words));
    }
}

/// Sends the key's stroke if it is still unsent (the pass), and stops
/// say-all.
fn pass(cx: &mut Cx<Entries>, press: &CaretPress) {
    stop_say_all(cx);
    if !press.held {
        return;
    }
    let sent = cx.facts_mut().strokes.pass(press.key_seq);
    if let Some(sent) = sent {
        cx.emit(verbatim_model::Effect::Queue(sent));
    }
}

/// Stops say-all, as any key does.
fn stop_say_all(cx: &mut Cx<Entries>) {
    let stopped = crate::say_all::stop(&mut cx.facts_mut());
    for effect in stopped {
        cx.emit(effect);
    }
}

/// What `press`'s text reads fetch beyond the text: what the settings
/// report for its unit, read for a caret key ([`crate::details`]).
fn key_details(cx: &Cx<Entries>, press: &CaretPress) -> TextDetails {
    crate::details::text_details(
        &cx.facts(),
        press.key.motion.unit(),
        crate::details::TextReason::CaretKey,
    )
}

/// Whether a key, command, braille gesture or key Core sends joined behind
/// the key.
fn marked(cx: &Cx<Entries>, press: &CaretPress) -> bool {
    cx.facts().arrivals.marks(press.key_seq)
}

/// Whether a focus waiting behind the key counts as pending, judged now
/// by the one definition: an unconfirmed MSAA focus does not.
fn counts_behind(cx: &Cx<Entries>) -> bool {
    pending_behind_front(cx, Within::ALL) == Pending::Counts
}

/// Whether the key's bound node is still held: its outpost generation is
/// the one running.
fn held_node(cx: &Cx<Entries>, binding: &CaretBinding) -> bool {
    cx.instance(Reader::Outpost(binding.window.process))
        .is_some_and(|instance| OutpostId(instance.generation) == binding.node.outpost())
}

/// Whether `node` is the committed focus, whose `Follow` registers its
/// events.
fn committed(state: &SrState, node: NodeId) -> bool {
    matches!(&state.versions.committed, Committed::Read(kept) if kept.node == node)
}

/// Whether the key is Alt+Up or Alt+Down, a sentence key.
fn is_sentence_key(press: &CaretPress) -> bool {
    matches!(
        press.key.motion,
        CaretMotion::PreviousSentence | CaretMotion::NextSentence
    )
}

/// Whether the key moves by sentence when no effect is seen: a sentence
/// key in text whose provider has a sentence unit.
fn moves_by_sentence(press: &CaretPress, binding: &CaretBinding) -> bool {
    is_sentence_key(press) && binding.provider.has_sentences()
}

/// What Backspace or Control+Backspace deleted, from the starting read:
/// the text before the caret, or before the selection's start, "blank"
/// at the text's start.
fn deleted(
    state: &SrState,
    binding: &CaretBinding,
    key: CaretKey,
    started: &Started,
) -> Vec<UtteranceSegment> {
    let before = editing::before_of_caret(
        state,
        binding.node,
        key.motion,
        (&started.caret, &started.unit),
    );
    editing::focus_first_words(
        state,
        key,
        (&before.unit, &before.line, before.removed.as_deref()),
    )
}

/// The words of a key whose effect was seen: Backspace and
/// Control+Backspace what they deleted, from the starting read; Enter the
/// new line's text up to the caret; any other key its unit at the caret (a
/// sentence key's is the line).
fn effect_words(
    state: &mut SrState,
    binding: &CaretBinding,
    key: CaretKey,
    (started, read): (&Started, &WatchedRead),
) -> Vec<UtteranceSegment> {
    if editing::selects(key) || key.motion == CaretMotion::Other {
        return Vec::new();
    }
    if key.motion.deletes_back() {
        return deleted(state, binding, key, started);
    }
    if key.motion == CaretMotion::NewLine {
        let mut reported = binding.formatting.clone();
        let words = editing::new_line_segments(&read.caret.line, &mut reported);
        editing::formatting_back(state, (binding.node, binding.record), reported);
        return words;
    }
    unit_remembered(state, binding, key, read)
}

/// The key's unit at `read`, its formatting compared with and written to
/// the memory of the focus record its binding carries, which is written
/// back to the focus object when that is still the same record (ADR 0001,
/// "The formatting last reported goes with the focus object").
fn unit_remembered(
    state: &mut SrState,
    binding: &CaretBinding,
    key: CaretKey,
    read: &WatchedRead,
) -> Vec<UtteranceSegment> {
    let mut reported = binding.formatting.clone();
    let words = editing::unit_with(
        state,
        binding.node,
        (key.motion, &read.caret.line, &read.unit),
        &mut reported,
    );
    editing::formatting_back(state, (binding.node, binding.record), reported);
    words
}

/// The words of a key at its bound: a movement key's or Delete's unit at
/// the caret; nothing for Backspace, Control+Backspace and Enter, whose
/// scripts speak only a move.
fn bound_words(
    state: &mut SrState,
    binding: &CaretBinding,
    key: CaretKey,
    read: &WatchedRead,
) -> Vec<UtteranceSegment> {
    if editing::selects(key)
        || key.motion.deletes_back()
        || matches!(key.motion, CaretMotion::Other | CaretMotion::NewLine)
    {
        return Vec::new();
    }
    unit_remembered(state, binding, key, read)
}

/// The caret key `entry` is, when it is one this module runs.
pub(crate) fn of_entry(entry: &Entry) -> Option<CaretPress> {
    let Entry::Press(press) = entry else {
        return None;
    };
    CaretPress::of(press)
}

impl CaretPress {
    /// The caret key `press` is, `None` for another press.
    pub(crate) fn of(press: &crate::keys::entry::PressEntry) -> Option<Self> {
        let verbatim_model::request::PressAction::CaretKey { key, held, bound } =
            &press.press.action
        else {
            return None;
        };
        Some(Self {
            trace_id: press.trace_id,
            key_seq: press.press.key_seq,
            pressed_at_us: press.press.pressed_at_us,
            key: *key,
            held: held.is_some(),
            bound: bound.as_deref().cloned(),
        })
    }

    /// Whether it is bound to the focused terminal.
    fn terminal_bound(&self, state: &SrState) -> bool {
        self.bound.as_ref().is_some_and(|bound| {
            editing::is_grid(bound.role)
                && crate::terminal::reading::modelled(state) == Some(bound.node)
        })
    }
}

#[cfg(test)]
#[path = "caret_key_tests.rs"]
mod tests;
