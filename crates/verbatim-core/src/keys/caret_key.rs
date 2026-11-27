//! A caret key as an event-queue entry, at its turn on the node it was
//! bound to at its press (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The caret key on the event queue"). Its states are its function's
//! places: waiting, its stroke unsent or sent; starting, the starting read
//! out; confirming a focus behind it; watching; and moving by sentence.
//! Whether it is marked is read from the arrivals and whether its stroke is
//! unsent from the stroke line, each when needed, never stored on the
//! entry.

use verbatim_model::batch::{CaretWait, EffectTest, Op, OpKind, StepRefusal, Waited};
use verbatim_model::request::{CancelReason, CaretBinding, Question};
use verbatim_model::{
    CaretKey, CaretMotion, CaretReport, KeySeq, NodeId, SentenceMoved, TextAnchor, TextChunk,
    TextDetails, TextMovement, TextProvider, TextUnit, TraceId, UtteranceSegment,
};

use crate::batch::Got;
use crate::editing;
use crate::entries::{Entries, Entry};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, RequestWindow};
use crate::focus::validity::{Pending, Within, pending_behind_front};
use crate::keys::strokes::StrokeStatus;
use crate::state::SrState;
use crate::terminal::reader::Asked;

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
    let focus = state.versions.live()?;
    let (node, role) = (focus.context.snapshot.id, focus.context.snapshot.role);
    if !editing::may_have_text(role) && state.focus_caret(node).is_none() {
        return None;
    }
    let (record, formatting) = match editing::focus_record(state) {
        Some((recorded, record, formatting)) if recorded == node => (record, formatting),
        _ => (None, verbatim_model::TextAttributes::default()),
    };
    Some(CaretBinding {
        node,
        window: focus.kept.asked.served_window(),
        role,
        provider: crate::text_facts::provider(&focus.kept),
        record,
        formatting,
    })
}

/// The window a caret key's batches name, its bound node's.
fn window_of(binding: &CaretBinding) -> RequestWindow {
    RequestWindow::of_served(&binding.window)
}

/// What a caret key's starting batch read before the key: the caret, the
/// key's unit where it is read, and the word at the caret where the wait
/// compares it. The caret's held read is the key's starting point, which
/// its wait and its move by sentence name.
#[derive(Clone, Debug)]
struct Start {
    /// The starting point.
    point: TextAnchor,
    /// The caret: its line, with the caret at its offset, and the
    /// selection.
    caret: CaretReport,
    /// For a key that deletes back, the provider's unit around the
    /// character before the caret, empty at the text's start, or the line
    /// where the text has no such unit; for any other key the line, whose
    /// starting batch reads no unit (Claude, 2026-11-22).
    unit: TextChunk,
    /// The word at the caret, which the wait of a key that compares the
    /// unit at the caret compares.
    word: Option<String>,
}

/// What a caret key's reads found at its end: the caret, and the key's
/// unit at it.
#[derive(Clone, Debug)]
struct EndRead {
    /// The caret: its line, with the caret at its offset, and the
    /// selection.
    caret: CaretReport,
    /// The key's unit at the caret, the line where the text has no such
    /// unit.
    unit: TextChunk,
}

/// Whether `batch`'s first step is `kind`.
fn first_is(question: &Question, kind: OpKind) -> bool {
    matches!(question, Question::Batch(batch) if batch.steps.first().map(|step| step.op.kind()) == Some(kind))
}

/// Whether the front entry `front`, a caret key, no longer needs
/// `request`: its starting read, once its stroke went by its 150 ms
/// release or was dropped at a switch away from the user's desktop. A
/// starting read opens with the caret, an edit field's and a terminal's
/// alike.
pub(crate) fn unneeded(state: &SrState, front: &CaretPress, request: &Question) -> bool {
    first_is(request, OpKind::Caret)
        && front.held
        && state.strokes.status(front.key_seq) != StrokeStatus::Unsent
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
/// has out, never stored (ADR 0003 rejects
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
    fn of(press: &CaretPress, request: &Question) -> Option<Self> {
        if let Some(bound_at_us) = wait_bound(request) {
            return Some(Self::Watching { bound_at_us });
        }
        match request {
            question if crate::focus::msaa_run::is_confirmation(question) => Some(Self::Confirming),
            question if first_is(question, OpKind::Caret) => Some(Self::Starting {
                deletes_back: press.key.motion.deletes_back(),
            }),
            question if first_is(question, OpKind::MoveBySentence) => Some(Self::Sentence),
            _ => None,
        }
    }

    /// Whether `arrival` withdraws the request of this stage: one arm per
    /// cell of the edit path's stage table.
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
    fn of(request: &Question) -> Option<Self> {
        if let Some(bound_at_us) = wait_bound(request) {
            return Some(Self::Watching { bound_at_us });
        }
        match request {
            question if crate::focus::msaa_run::is_confirmation(question) => Some(Self::Confirming),
            // The terminal's full read, as its reader wrote it: the
            // starting read reads the caret first, and any other is the
            // final read.
            Question::Batch(batch) => match crate::terminal::reader::asked(batch) {
                Some(Asked::Window(ask)) if ask.starting => Some(Self::Starting),
                Some(Asked::Window(_)) => Some(Self::Final),
                Some(Asked::History(_) | Asked::Search(_)) | None => None,
            },
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

/// When the bound of `request`'s wait passes, when its first step is a
/// caret wait.
fn wait_bound(request: &Question) -> Option<u64> {
    let Question::Batch(batch) = request;
    match &batch.steps.first()?.op {
        Op::WaitForCaret { wait, .. } => {
            Some(wait.passed_at_us.saturating_add(u64::from(wait.bound_us)))
        }
        _ => None,
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
pub(crate) fn withdrawn_by(front: &CaretPress, request: &Question, arrival: Arrival) -> bool {
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
/// while it is out (the key's confirming place).
async fn confirm(
    cx: &mut Cx<Entries>,
    trace: TraceId,
    spare: Option<(NodeId, TextAnchor)>,
    due_us: u64,
) -> Confirmed {
    let Some(candidate) = crate::focus::validity::waiting_candidate(cx, trace) else {
        return Confirmed::Judged;
    };
    match crate::focus::msaa_run::facts(cx, &candidate, spare, due_us).await {
        Outcome::Result(facts) => {
            crate::focus::validity::store_facts(cx, trace, facts);
            Confirmed::Judged
        }
        // The candidate's outpost ended, or its object is gone: stored as
        // not there, and judged again.
        Outcome::ReaderEnded | Outcome::NotThere => {
            crate::focus::validity::store_facts(
                cx,
                trace,
                crate::focus::validity::CandidateFacts::NotThere,
            );
            Confirmed::Judged
        }
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => Confirmed::Withdrawn,
        Outcome::Refused(_) | Outcome::NotResponding | Outcome::NoLongerAwaited(_) => {
            Confirmed::Stopped
        }
    }
}

/// Asks a caret key's starting batch (the request list's design, section
/// 6, `StartingRead`): the caret; its line; the key's unit at the caret,
/// or for a key that deletes back the unit around the character before it,
/// as NVDA's Backspace scripts read what they delete (`docs/parity/text.md`,
/// "What Backspace and Delete say"); and for Delete the word at the caret,
/// which its wait compares. Its answer, or `None` for any other outcome,
/// after which the key only passes. A node with no text has no caret to
/// report, as NVDA binds its caret scripts to editable text alone
/// (`editableText.py`); any other failure passes the key unread.
async fn starting_read(
    cx: &mut Cx<Entries>,
    (binding, motion): (&CaretBinding, CaretMotion),
    due_us: u64,
) -> Option<Start> {
    let node = binding.node;
    let mut start = cx.batch(|| ());
    let caret = start.caret(node, None);
    let line = start.read_text(
        node,
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let back = TextMovement {
        unit: TextUnit::Character,
        count: -1,
    };
    let deleted = motion.deletes_back().then(|| {
        start.read_text(
            node,
            caret.caret(),
            Some(back),
            motion.unit(),
            1,
            TextDetails::NONE,
        )
    });
    let word = motion.compares_unit().then(|| {
        start.read_text(
            node,
            caret.caret(),
            None,
            TextUnit::Word,
            1,
            TextDetails::NONE,
        )
    });
    let request = start
        .finish()
        .of(Reader::Outpost(binding.window.process), window_of(binding));
    let Outcome::Result(answers) = cx.ask(request, due_us).await else {
        return None;
    };
    let (Got::Done(at), Got::Done(line)) = (answers.get(caret), answers.get(line)) else {
        return None;
    };
    let line = line.chunks.first()?.clone();
    let unit = match deleted.map(|deleted| answers.get(deleted)) {
        None | Some(Got::Refused(StepRefusal::NoSuchUnit)) => line.clone(),
        // At the text's start the caret cannot move back: nothing before it.
        Some(Got::Done(read)) if read.moved == 0 => TextChunk {
            text: String::new(),
            offset: 0,
            languages: Vec::new(),
            truncated: false,
            formats: Vec::new(),
            ..line.clone()
        },
        Some(Got::Done(read)) => read.chunks.first()?.clone(),
        Some(_) => return None,
    };
    // A text with no word unit compares its line, as the word read there.
    let word = match word.map(|word| answers.get(word)) {
        None => None,
        Some(Got::Done(read)) => Some(read.chunks.first()?.text.clone()),
        Some(Got::Refused(StepRefusal::NoSuchUnit)) => Some(line.text.clone()),
        Some(_) => return None,
    };
    Some(Start {
        point: at.held,
        caret: crate::text_facts::caret_report(at, line),
        unit,
        word,
    })
}

/// The unit a key of `motion` speaks at its end in text read through
/// `provider`: the key's unit; a sentence as the paragraph where the
/// provider splits sentences by paragraph, else as the line; a document as
/// the line.
fn key_unit(provider: TextProvider, motion: CaretMotion) -> TextUnit {
    match motion.unit() {
        TextUnit::Sentence if provider.has_sentences() => TextUnit::Paragraph,
        TextUnit::Sentence | TextUnit::Document => TextUnit::Line,
        unit => unit,
    }
}

/// The caret key's function at its turn, then what the
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

/// The caret key at its turn, to its end.
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
            // Waiting, its stroke sent: released by its 150 ms timer, or never held. Its selection
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
                    (press.trace_id, press.key_seq),
                );
            }
            stop_say_all(cx);
            return Reached::Sent;
        }
        StrokeStatus::Unsent => {}
    }
    let Some(binding) = press.bound.clone().filter(|binding| held_node(cx, binding)) else {
        return pass(cx, press);
    };
    if editing::is_grid(binding.role) {
        if press.key.motion != CaretMotion::NewLine {
            return terminal_key(cx, press, &binding).await;
        }
        // Enter held for a terminal, by a hook view behind the focus: the
        // carriage return it types there, with no read, as where the view
        // is current.
        let reached = terminal_pass(cx, press);
        new_line_typed(cx, press);
        return reached;
    }
    if marked(cx, press) {
        return pass(cx, press);
    }
    // Before the starting read, only a focus that counts stops the key;
    // an unconfirmed one waits behind it, judged at the pass.
    if !press.key.motion.deletes_back() && counts_behind(cx) {
        return pass(cx, press);
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    // A stroke dropped while the read is out withdraws it (`unneeded`).
    let Some(started) = starting_read(cx, (&binding, press.key.motion), due_us).await else {
        return pass(cx, press);
    };

    let passed_at_us = cx.now_us();
    if pass(cx, press) == Reached::Dropped {
        return Reached::Dropped;
    }
    if marked(cx, press) {
        marked_end(cx, press, (&binding, &started), due_us).await;
        return Reached::Sent;
    }
    after_pass(cx, press, (&binding, &started), (passed_at_us, due_us)).await;
    Reached::Sent
}

/// What Backspace's U+0008 does to the word being typed, at the key's
/// end, after its own speech, as NVDA's typed-character event runs after
/// the key's script (`docs/parity/input.md`, "Typed-character echo"): it
/// drops the word's last character, one code point. Enter's carriage
/// return is applied only where its watch saw the caret move
/// ([`new_line_typed`]).
fn typed_at_end(cx: &mut Cx<Entries>, press: &CaretPress) {
    if press.key.motion == CaretMotion::Backspace {
        cx.facts_mut().typed_word.pop();
    }
}

/// Enter's carriage return reached the application, typed text: the word
/// being typed ends and is spoken by the settings, or in a terminal is
/// forgotten. Only an Enter whose watch saw the caret move, or one held
/// for a terminal, is taken for one: a dialog's manager takes Enter
/// before the field receives a character, so a word an Enter that moved
/// nothing keeps is forgotten by the focus change that follows, as NVDA's
/// is (Dickson, 2026-11-19; `docs/parity/text.md`, "Enter in a text
/// field").
fn new_line_typed(cx: &mut Cx<Entries>, press: &CaretPress) {
    let effects = editing::character_typed(&mut cx.facts_mut(), press.trace_id, "\r");
    for effect in effects {
        cx.emit(effect);
    }
}

/// After the pass: the key judges what waits behind it by the one
/// definition until its bound. A
/// focus that counts silences it; an unconfirmed one is confirmed by a
/// read of its facts, then judged again; with neither, the watch runs
/// for the rest of the bound. A withdrawn watch or confirmation judges
/// the arrival: a key marks it, a focus is judged again.
async fn after_pass(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Start),
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
fn silent(cx: &mut Cx<Entries>, press: &CaretPress, (binding, started): (&CaretBinding, &Start)) {
    if press.key.motion.deletes_back() {
        let words = deleted(binding, press.key, started);
        say(cx, press, words);
    }
}

/// The key is marked: Backspace says nothing and the sentence key still
/// moves, saying nothing.
async fn marked_end(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Start),
    due_us: u64,
) {
    if moves_by_sentence(press, binding) {
        sentence(cx, press, (binding, started), due_us).await;
    }
}

/// A terminal's caret key at its turn
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Terminals"): its starting read is the terminal's full
/// read, which mints the caret starting point; the key passes, its model
/// takes it, and the watch looks for its effect with no event shortcut;
/// then the final read, another full read, whose answer the model takes.
/// The key says nothing itself: the reading rules speak each read, the
/// terminal's speech waiting in the queue ahead of the first focus first
/// (build 3, "Answers and speech").
async fn terminal_key(cx: &mut Cx<Entries>, press: &CaretPress, binding: &CaretBinding) -> Reached {
    if marked(cx, press) || counts_behind(cx) {
        return terminal_pass(cx, press);
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let Some(started) = terminal_read(cx, press, binding.node, true, due_us).await else {
        return terminal_pass(cx, press);
    };
    let passed_at_us = cx.now_us();
    if terminal_pass(cx, press) == Reached::Dropped {
        return Reached::Dropped;
    }
    if marked(cx, press) {
        return Reached::Sent;
    }
    // The starting point is the read's; a read that minted none, a
    // reader's fault, leaves the key's effect to the change it tells.
    let Some((point, caret)) = started else {
        return Reached::Sent;
    };
    let console = crate::terminal::reading::focused(&cx.facts())
        .is_some_and(|terminal| terminal.node == binding.node && terminal.console);
    let motion = press.key.motion;
    // A terminal's starting read reads no word: the wait compares its
    // line, as the word read there.
    let wait = CaretWait {
        from: point,
        word: motion.compares_unit().then(|| caret.line.text.clone()),
        line: caret.line.text,
        offset: caret.line.offset,
        effect: effect_test(motion),
        passed_at_us,
        poll_us: if console { CONSOLE_POLL_US } else { POLL_US },
        bound_us: if console { BOUND_US } else { TERMINAL_BOUND_US },
        events: false,
    };
    let bound_at_us = passed_at_us.saturating_add(u64::from(wait.bound_us));
    // As the edit path judges after its pass, with the terminal's own
    // stages: a counting focus silences the key, an unconfirmed one is
    // confirmed, and with neither the watch runs for the rest of the bound.
    loop {
        let withdrawn = match pending_behind_front(cx, Within::key(bound_at_us)) {
            Pending::Counts => return Reached::Sent,
            Pending::Unconfirmed(trace) => {
                match confirm(cx, trace, Some((binding.node, point)), due_us).await {
                    Confirmed::Judged => false,
                    Confirmed::Withdrawn => true,
                    Confirmed::Stopped => return Reached::Sent,
                }
            }
            Pending::No => {
                // The wait alone: the terminal's full read follows it.
                let mut watch = cx.batch(|| ());
                let waited = watch.wait_for_caret(binding.node, wait.clone());
                let request = watch
                    .finish()
                    .of(Reader::Outpost(binding.window.process), window_of(binding));
                match cx.ask(request, due_us).await {
                    Outcome::Result(answers) if matches!(answers.get(waited), Got::Done(_)) => {
                        break;
                    }
                    Outcome::NoLongerAwaited(CancelReason::Withdrawn) => true,
                    // A wait that failed ended as one refused.
                    Outcome::Result(_)
                    | Outcome::Refused(_)
                    | Outcome::NotThere
                    | Outcome::NotResponding
                    | Outcome::ReaderEnded
                    | Outcome::NoLongerAwaited(_) => return Reached::Sent,
                }
            }
        };
        if withdrawn && marked(cx, press) {
            return Reached::Sent;
        }
    }
    let _ = terminal_read(cx, press, binding.node, false, due_us).await;
    Reached::Sent
}

/// A terminal key's full read of `node`, its starting read when
/// `starting`: the model takes its answer, and the terminal's waiting
/// speech, then the read's own, is said. `None` when it brought no read;
/// else the caret starting point it held, if any, with the caret it was
/// held from.
async fn terminal_read(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    node: NodeId,
    starting: bool,
    due_us: u64,
) -> Option<Option<(TextAnchor, CaretReport)>> {
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
    reply.map(|reply| reply.point.zip(reply.snapshot.caret.clone()))
}

/// A terminal's held key passes: its stroke is sent, and the focused
/// terminal's model takes it at its pass; a dropped stroke is neither.
fn terminal_pass(cx: &mut Cx<Entries>, press: &CaretPress) -> Reached {
    let unsent = press.held && cx.facts().strokes.status(press.key_seq) == StrokeStatus::Unsent;
    let reached = pass(cx, press);
    if unsent {
        let now_us = cx.now_us();
        crate::keys::terminal::passed(
            &mut cx.facts_mut(),
            crate::terminal::caret_key_kind(press.key),
            now_us,
            (press.trace_id, press.key_seq),
        );
    }
    reached
}

/// How a watch ended.
enum WatchEnd {
    /// It answered, and the key said what its script says, or failed.
    Ended,
    /// An arrival withdrew it.
    Withdrawn,
}

/// The effect test of a key of `motion`'s wait: a change of the unit at
/// the caret counts after Delete and Control+Delete, and a change of the
/// line after a key that deletes (`docs/parity/text.md`).
fn effect_test(motion: CaretMotion) -> EffectTest {
    EffectTest {
        unit: motion.compares_unit(),
        line: motion.deletes(),
    }
}

/// How a key's wait ended, with its end read when it made one.
enum WaitEnd {
    /// The effect was seen.
    Effect(Option<EndRead>),
    /// The bound passed.
    Bound(Option<EndRead>),
}

/// Asks a key's wait (the request list's design, section 6, `Watch`):
/// `WaitForCaret` from the starting point with the starting texts and the
/// key's effect test, then the caret, its line and the key's unit at it,
/// read only at the effect for a sentence key. The line carries the
/// formatting a line or a character speaks, the unit that of a word or a
/// paragraph. `None` for a wait that failed or was not answered; `Err`
/// when an arrival withdrew it.
async fn wait(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Start),
    (passed_at_us, due_us): (u64, u64),
) -> Result<Option<WaitEnd>, ()> {
    let node = binding.node;
    let motion = press.key.motion;
    let details = key_details(cx, press);
    let unit = key_unit(binding.provider, motion);
    let mut watch = cx.batch(|| ());
    let waited = watch.wait_for_caret(
        node,
        CaretWait {
            from: started.point,
            line: started.caret.line.text.clone(),
            offset: started.caret.line.offset,
            word: started.word.clone(),
            effect: effect_test(motion),
            passed_at_us,
            poll_us: POLL_US,
            bound_us: BOUND_US,
            // NVDA's `caretMovementDetectionUsesEvents` holds for every UI
            // Automation field but a terminal, whose keys this entry does
            // not run: a caret or text-change event of the node within 60
            // ms of the pass is the key's effect (Dickson, 2026-11-16). A
            // node no longer the committed focus has no `Follow` to hear
            // them.
            events: binding.provider == TextProvider::Uia && committed(&cx.facts(), node),
        },
    );
    // A sentence key reads only at its effect: at its bound it moves by
    // sentence where the text has sentences and says nothing where it has
    // none, as NVDA's move by a unit UI Automation lacks fails silently
    // (`editableText.py` 253 to 265; `UIAHandler/__init__.py` 170 to 176).
    if is_sentence_key(press) {
        watch.when(waited.found());
    }
    let caret = watch.caret(node, None);
    let spoken_by_line = matches!(unit, TextUnit::Line | TextUnit::Character);
    let line = watch.read_text(
        node,
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        if spoken_by_line {
            details
        } else {
            TextDetails::NONE
        },
    );
    let unit_read =
        (!spoken_by_line).then(|| watch.read_text(node, caret.caret(), None, unit, 1, details));
    let request = watch
        .finish()
        .of(Reader::Outpost(binding.window.process), window_of(binding));
    let answers = match cx.ask(request, due_us).await {
        Outcome::Result(answers) => answers,
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => return Err(()),
        Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => return Ok(None),
    };
    let read = match (answers.get(caret), answers.get(line)) {
        (Got::Done(at), Got::Done(line)) => line.chunks.first().map(|line| {
            let unit = match unit_read.map(|read| answers.get(read)) {
                Some(Got::Done(read)) => read.chunks.first().cloned(),
                None if unit == TextUnit::Character => Some(character_of(line)),
                None | Some(_) => None,
            };
            EndRead {
                unit: unit.unwrap_or_else(|| line.clone()),
                caret: crate::text_facts::caret_report(at, line.clone()),
            }
        }),
        _ => None,
    };
    Ok(match answers.get(waited) {
        Got::Done(Waited::Effect) => Some(WaitEnd::Effect(read)),
        Got::Done(Waited::Bound) => Some(WaitEnd::Bound(read)),
        _ => None,
    })
}

/// The character at the caret of `line`, cut from it with its formatting
/// and languages, as a key that moves by character speaks it.
fn character_of(line: &TextChunk) -> TextChunk {
    let offset = crate::text::byte_offset(line.offset);
    // The grapheme at the caret, as the outpost cut a character key's
    // character from its line.
    let range = verbatim_text::grapheme_at(&line.text, offset).unwrap_or(offset..offset);
    let (start, end) = (
        u32::try_from(range.start).unwrap_or(u32::MAX),
        u32::try_from(range.end).unwrap_or(u32::MAX),
    );
    let clip = |(from, to): (u32, u32)| {
        let (from, to) = (from.max(start), to.min(end));
        (from < to).then(|| (from.saturating_sub(start), to.saturating_sub(start)))
    };
    TextChunk {
        unit: TextUnit::Character,
        text: line.text.get(range).unwrap_or_default().to_owned(),
        offset: 0,
        languages: line
            .languages
            .iter()
            .filter_map(|run| {
                clip((run.start, run.end)).map(|(start, end)| verbatim_model::LanguageRun {
                    start,
                    end,
                    language: run.language.clone(),
                })
            })
            .collect(),
        formats: line
            .formats
            .iter()
            .filter_map(|run| {
                clip((run.start, run.end)).map(|(start, end)| verbatim_model::FormatRun {
                    start,
                    end,
                    attributes: run.attributes.clone(),
                })
            })
            .collect(),
        first: false,
        last: false,
        ..line.clone()
    }
}

/// Watching: the wait, then the key's words by its script.
async fn watching(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Start),
    times: (u64, u64),
) -> WatchEnd {
    let (_, due_us) = times;
    match wait(cx, press, (binding, started), times).await {
        Err(()) => return WatchEnd::Withdrawn,
        Ok(Some(WaitEnd::Effect(Some(read)))) => {
            let words = effect_words(&mut cx.facts_mut(), binding, press.key, (started, &read));
            finish(cx, press, binding, read, words);
            if press.key.motion == CaretMotion::NewLine {
                new_line_typed(cx, press);
            }
        }
        Ok(Some(WaitEnd::Bound(Some(read)))) => {
            let words = bound_words(&mut cx.facts_mut(), binding, press.key, &read);
            finish(cx, press, binding, read, words);
        }
        Ok(Some(WaitEnd::Bound(None))) if is_sentence_key(press) => {
            if moves_by_sentence(press, binding) {
                sentence(cx, press, (binding, started), due_us).await;
            }
        }
        // A wait whose reads failed says nothing, as one refused.
        Ok(Some(WaitEnd::Effect(None) | WaitEnd::Bound(None)) | None) => {}
    }
    WatchEnd::Ended
}

/// Moving by sentence: the move by sentence from the starting point, its words said
/// unless the key is marked by the time it answers.
async fn sentence(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Start),
    due_us: u64,
) {
    let forward = press.key.motion == CaretMotion::NextSentence;
    let mut batch = cx.batch(|| ());
    let move_by =
        batch.move_by_sentence(binding.node, started.point, forward, key_details(cx, press));
    let request = batch
        .finish()
        .of(Reader::Outpost(binding.window.process), window_of(binding));
    // A move that fails says nothing, as NVDA's sentence script swallows
    // its exception (`editableText.py` 240 to 264), a source with no
    // sentence unit or a starting point no longer held included.
    let Outcome::Result(answers) = cx.ask(request, due_us).await else {
        return;
    };
    let Got::Done(moved) = answers.get(move_by) else {
        return;
    };
    let moved = moved.clone();
    // The answer mints the next selection baseline, as Verbatim's own move,
    // whether or not the key speaks.
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
    {
        let mut facts = cx.facts_mut();
        facts.typed_word.clear();
        editing::caret_placed(&mut facts, binding.node, line);
    }
    say(cx, press, words);
}

/// The key's words and end, from its final read: the key speaks, so the
/// typed word is forgotten and the caret read becomes Core's, the review
/// cursor following it; a key with nothing to say leaves both to its caret
/// tell. Enter's script forgets no word: the carriage return it types ends
/// the word after it ([`new_line_typed`]).
fn finish(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    binding: &CaretBinding,
    read: EndRead,
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
/// say-all; says whether the stroke reached the application. One dropped
/// at a switch away from the user's desktop, before the key's turn or
/// while its starting read was out, never does: its dropped mark is read
/// here, once, and forgotten.
fn pass(cx: &mut Cx<Entries>, press: &CaretPress) -> Reached {
    stop_say_all(cx);
    if !press.held {
        return Reached::Sent;
    }
    let now_us = cx.now_us();
    let sent = {
        let mut facts = cx.facts_mut();
        if facts.strokes.status(press.key_seq) == StrokeStatus::Dropped {
            facts.strokes.forget_dropped(press.key_seq);
            return Reached::Dropped;
        }
        facts.strokes.pass(press.key_seq, now_us)
    };
    if let Some(sent) = sent {
        cx.emit(verbatim_model::Effect::Queue(sent));
    }
    Reached::Sent
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
    crate::details::text_details(&cx.facts(), press.key.motion.unit())
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
        .is_some_and(|instance| Some(instance.outpost_id()) == binding.node.outpost())
}

/// Whether `node` is the committed focus, whose `Follow` registers its
/// events.
fn committed(state: &SrState, node: NodeId) -> bool {
    state
        .versions
        .live()
        .is_some_and(|focus| focus.context.snapshot.id == node)
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
fn deleted(binding: &CaretBinding, key: CaretKey, started: &Start) -> Vec<UtteranceSegment> {
    let grid = editing::is_grid(binding.role);
    let before = editing::before_of_caret(grid, key.motion, (&started.caret, &started.unit));
    editing::focus_first_words(
        grid,
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
    (started, read): (&Start, &EndRead),
) -> Vec<UtteranceSegment> {
    if editing::selects(key) || key.motion == CaretMotion::Other {
        return Vec::new();
    }
    if key.motion.deletes_back() {
        return deleted(binding, key, started);
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
    read: &EndRead,
) -> Vec<UtteranceSegment> {
    let mut reported = binding.formatting.clone();
    let words = editing::unit_with(
        editing::is_grid(binding.role),
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
    read: &EndRead,
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
