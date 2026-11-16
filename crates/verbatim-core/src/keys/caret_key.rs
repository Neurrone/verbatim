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
    StartingRead, TextDetails, TextOp, TextProvider, TextReply, TraceId, UtteranceSegment, Watch,
    WatchFinalRead, Watched, WatchedRead,
};

use crate::editing;
use crate::entries::{Entries, Entry};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
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
    Some(CaretBinding {
        node: kept.node,
        window: kept.window.node_window(),
        role: kept.role,
        provider: match kept.read_by {
            crate::focus::versions::ReadBy::Uia { .. } => TextProvider::Uia,
            crate::focus::versions::ReadBy::Msaa { .. } => TextProvider::EditMessages,
        },
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
    type Reply = Started;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<Started> {
        match reply {
            Reply::Text(TextReply::Started(started)) => Some(*started),
            _ => None,
        }
    }
}

/// The watch, answered with how it ended.
#[derive(Debug)]
struct WatchRequest(OnNode);

impl Request for WatchRequest {
    type Reply = Watched;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<Watched> {
        match reply {
            Reply::Text(TextReply::Watched(watched)) => Some(*watched),
            _ => None,
        }
    }
}

/// The move by sentence from the starting point.
#[derive(Debug)]
struct SentenceRequest(OnNode);

impl Request for SentenceRequest {
    type Reply = SentenceMoved;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.0.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.0.window
    }

    fn wire(&self) -> Wire {
        self.0.wire()
    }

    fn decode(reply: Reply) -> Option<SentenceMoved> {
        match reply {
            Reply::Text(TextReply::Sentence(moved)) => Some(*moved),
            _ => None,
        }
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

/// Whether `arriving`, joining behind the front caret key `front`, withdraws
/// `request`. A later key, command or key Core sends withdraws its watch
/// (Table 7, S4, column B), and a terminal's final read; a focus counting
/// as pending withdraws its starting read, but Backspace's and
/// Control+Backspace's, its watch and its sentence move (column C).
pub(crate) fn withdrawn_by(
    front: &CaretPress,
    request: &Wire,
    (arriving_key, arriving_focus): (bool, bool),
) -> bool {
    let Wire::Text { op, .. } = request else {
        return false;
    };
    match op {
        TextOp::StartingRead(_) => arriving_focus && !front.key.motion.deletes_back(),
        TextOp::ReadTerminal(read) if read.starting.is_some() => arriving_focus,
        TextOp::Watch(_) | TextOp::ReadTerminal(_) => arriving_key || arriving_focus,
        TextOp::MoveBySentence(_) => arriving_focus,
        _ => false,
    }
}

/// The caret key's function at its turn (Table 7).
pub(crate) async fn run(mut cx: Cx<Entries>, press: CaretPress) {
    let status = if press.held {
        cx.facts().strokes.status(press.key_seq)
    } else {
        StrokeStatus::Sent
    };
    match status {
        StrokeStatus::Dropped => {
            cx.facts_mut().strokes.forget_dropped(press.key_seq);
            return;
        }
        StrokeStatus::Sent => {
            // S2: released by its 150 ms timer, or never held. Its selection
            // words, if any, come from its caret tell. A held key bound to
            // the focused terminal passed at its release, which its model
            // takes now.
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
            stop_say_all(&mut cx);
            if !editing::selects(press.key) {
                cx.facts_mut().typed_word.clear();
            }
            return;
        }
        StrokeStatus::Unsent => {}
    }
    let Some(binding) = press
        .bound
        .clone()
        .filter(|binding| held_node(&cx, binding))
    else {
        pass(&mut cx, &press);
        return;
    };
    if editing::is_grid(binding.role) {
        terminal_key(&mut cx, &press, &binding).await;
        return;
    }
    if marked(&cx, &press) {
        pass(&mut cx, &press);
        return;
    }
    if focus_pending(&cx) && !press.key.motion.deletes_back() {
        pass(&mut cx, &press);
        return;
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let starting = StartingRead {
        provider: binding.provider,
        details: TextDetails::ALL,
        unit: press.key.motion.unit(),
        word: matches!(
            press.key.motion,
            CaretMotion::Delete | CaretMotion::DeleteWord
        ),
    };
    let request = StartingReadRequest(OnNode::of(&binding, TextOp::StartingRead(starting)));
    let Outcome::Result(started) = cx.ask(request, due_us).await else {
        pass(&mut cx, &press);
        return;
    };
    let passed_at_us = cx.now_us();
    pass(&mut cx, &press);
    if marked(&cx, &press) {
        if moves_by_sentence(&press, &binding) {
            sentence(&mut cx, &press, (&binding, &started), due_us).await;
        }
        return;
    }
    if focus_pending(&cx) {
        if press.key.motion.deletes_back() {
            let words = deleted(&cx.facts(), &binding, press.key, &started);
            say(&mut cx, &press, words);
        }
        return;
    }
    watching(
        &mut cx,
        &press,
        (&binding, &started),
        (passed_at_us, due_us),
    )
    .await;
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
    if marked(cx, press) || focus_pending(cx) {
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
    if marked(cx, press) || focus_pending(cx) {
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
    let request = WatchRequest(OnNode::of(binding, TextOp::Watch(watch)));
    match cx.ask(request, due_us).await {
        Outcome::Result(_) => {}
        Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => return,
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
        Outcome::Result(reply) => Some(reply),
        _ => None,
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

/// S4: the watch, then the key's words by its script.
async fn watching(
    cx: &mut Cx<Entries>,
    press: &CaretPress,
    (binding, started): (&CaretBinding, &Started),
    (passed_at_us, due_us): (u64, u64),
) {
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
        final_read: if moves_by_sentence(press, binding) {
            WatchFinalRead::UnitAtEffect {
                unit: press.key.motion.unit(),
                details: TextDetails::ALL,
            }
        } else {
            WatchFinalRead::Unit {
                unit: press.key.motion.unit(),
                details: TextDetails::ALL,
            }
        },
    };
    let request = WatchRequest(OnNode::of(binding, TextOp::Watch(watch)));
    match cx.ask(request, due_us).await {
        Outcome::Result(Watched::Effect(read)) => {
            let words = effect_words(&mut cx.facts_mut(), binding, press.key, (started, &read));
            finish(cx, press, binding, read, words);
        }
        Outcome::Result(Watched::Bound(read)) => {
            let words = bound_words(&mut cx.facts_mut(), binding, press.key, &read);
            finish(cx, press, binding, read, words);
        }
        Outcome::Result(Watched::BoundUnread | Watched::EffectUnread) => {
            if moves_by_sentence(press, binding) {
                sentence(cx, press, (binding, started), due_us).await;
            }
        }
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => {
            if focus_pending(cx) {
                if press.key.motion.deletes_back() {
                    let words = deleted(&cx.facts(), binding, press.key, started);
                    say(cx, press, words);
                }
            } else if moves_by_sentence(press, binding) {
                // Marked: the sentence key still moves, saying nothing.
                sentence(cx, press, (binding, started), due_us).await;
            }
        }
        Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => {}
    }
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
            details: TextDetails::ALL,
        }),
    ));
    let Outcome::Result(moved) = cx.ask(request, due_us).await else {
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
/// tell.
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
        facts.typed_word.clear();
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

/// Whether a key, command, braille gesture or key Core sends joined behind
/// the key.
fn marked(cx: &Cx<Entries>, press: &CaretPress) -> bool {
    cx.facts().arrivals.marks(press.key_seq)
}

/// Whether a focus waiting behind the key counts as pending, judged now.
pub(crate) fn focus_pending(cx: &Cx<Entries>) -> bool {
    let facts = cx.facts();
    let backend = cx.backend();
    let now_us = cx.now_us();
    cx.any_waiting(|entry| crate::entries::pending_focus(&facts, entry, &backend, now_us))
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

/// Whether the key moves by sentence when no effect is seen: a sentence
/// key in text whose provider has a sentence unit.
fn moves_by_sentence(press: &CaretPress, binding: &CaretBinding) -> bool {
    matches!(
        press.key.motion,
        CaretMotion::PreviousSentence | CaretMotion::NextSentence
    ) && binding.provider.has_sentences()
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
        (&started.caret, Some(&started.unit)),
    );
    editing::focus_first_words(
        state,
        key,
        (&before.unit, &before.line, before.removed.as_deref()),
    )
}

/// The words of a key whose effect was seen: Backspace and
/// Control+Backspace what they deleted, from the starting read; any other
/// key its unit at the caret (a sentence key's is the line).
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
    editing::unit_of(
        state,
        binding.node,
        key.motion,
        &read.caret.line,
        &read.unit,
    )
}

/// The words of a key at its bound: a movement key's or Delete's unit at
/// the caret; nothing for Backspace and Control+Backspace.
fn bound_words(
    state: &mut SrState,
    binding: &CaretBinding,
    key: CaretKey,
    read: &WatchedRead,
) -> Vec<UtteranceSegment> {
    if editing::selects(key) || key.motion.deletes_back() || key.motion == CaretMotion::Other {
        return Vec::new();
    }
    editing::unit_of(
        state,
        binding.node,
        key.motion,
        &read.caret.line,
        &read.unit,
    )
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
