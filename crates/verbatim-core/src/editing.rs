//! Editing (milestone M4): caret keys and the selection, following NVDA's
//! wait for evidence (`docs/nvda/editable-text-and-terminals.md`), and
//! typed character and word echo with the password rules
//! (`docs/parity/text.md` and `docs/parity/terminals.md`, "Typing").
//!
//! A caret key reaches the application unchanged; the hook reports it, and
//! the reducer asks the focus's outpost to watch for evidence of what the
//! key did and report the caret ([`TextOp::AwaitCaret`]); the outpost never
//! waits for it, and a key that moves nothing gets no answer worth
//! speaking ([`TextReply::WatchEnded`]), so it is silent. The reply is
//! spoken: the character after Left or Right Arrow, Home, and End; the
//! provider's word after Control with Left or Right Arrow; the line after
//! Up or Down Arrow, the page keys, and Control with Home or End; the
//! paragraph after Control with Up or Down Arrow; what Backspace deleted;
//! the character or word now at the caret after Delete. A key with Shift
//! speaks what became selected or unselected instead, as does a selection
//! Verbatim makes itself (the review copy's first press, in `review_text`);
//! one the mouse or the application makes is not spoken. A newer key, a typed
//! character, or a focus change supersedes a key still watched, so speech
//! never lags behind fast typing, a later edit is not spoken as an earlier
//! key's answer, and a focus announcement wins over a caret line.
//!
//! Formatting (milestone M4 item 7) is spoken as it changes, as NVDA
//! speaks it (`docs/nvda/document-formatting.md`): the outpost sends the
//! formatting of the text a caret key or a focus speaks, and only what
//! differs from the formatting last reported in that node is said, a
//! spelling error where it starts, and for a character or a word also where
//! it ends.

use std::sync::Arc;

use verbatim_model::{
    CaretKey, CaretMotion, CaretReply, CaretReport, CaretWatch, Effect, FocusText, FocusValidity,
    Message, NodeId, NodeSnapshot, Phrase, Role, SegmentContent, Selection, SelectionChange,
    SelectionText, SpeechPriority, State, TextAttributes, TextChunk, TextOp, TextPoint,
    TextPosition, TextRead, TextReply, TextRequest, TextUnit, TraceId, TypingEcho, Utterance,
    UtteranceSegment,
};

use crate::state::{
    CARET_HISTORY, CaretContext, HeldKey, HeldRead, NavigatorRead, PendingCaret, PendingText,
    ReviewText, SrState, TextFollowUp, TimedCaret, WaitingKey,
};
use crate::text;

/// The most bytes of a word typed so far that word echo keeps; a longer
/// run of letters is not a word anyone needs echoed whole.
pub(crate) const MAX_TYPED_WORD: usize = 256;

/// The longest a held key waits for the watch of the key ahead of it to be
/// answered, counted from when that key was passed on: 100 ms, NVDA's
/// default wait for the caret to move, which NVDA lengthens in Windows
/// Terminal (`docs/design/key-holding.md`, "Core"). Past it the waiting
/// key's caret is read as if the watch had been answered, since a key that
/// moved nothing gets no answer before the watch's own bound
/// (`docs/design/key-holding.md`, "Keys behind a held key").
pub(crate) const KEY_AHEAD_BOUND_MS: u64 = 100;

/// NVDA speaks a selection change of this many characters or more as a
/// count rather than the text.
const SELECTION_SPOKEN_AS_COUNT: u32 = 512;

/// What a protected field echoes for each typed character: NVDA's
/// protected character, spoken by its name ("star").
const PROTECTED_CHARACTER: &str = "*";

/// Whether a role may have text the caret moves in: an edit field, a
/// document, or a terminal.
pub(crate) fn may_have_text(role: Role) -> bool {
    matches!(role, Role::EditableText | Role::Document | Role::Terminal)
}

/// Whether text in a node of `role` is a grid of terminal cells.
pub(crate) fn is_grid(role: Role) -> bool {
    role == Role::Terminal
}

/// Whether a node is a place text can be typed, for the typing echo
/// settings' "only in edit controls": an edit field, a terminal, or a
/// document that is not read-only.
fn is_editable(node: &NodeSnapshot) -> bool {
    match node.role {
        Role::EditableText | Role::Terminal => true,
        Role::Document => !node.states.contains(State::ReadOnly),
        _ => false,
    }
}

/// Queued speech for how a selection changed, each phrase its own
/// utterance, as NVDA speaks each text selected or unselected, and
/// "selection removed", on its own (`docs/parity/text.md`,
/// "TextInfo-equivalent layer").
pub(crate) fn speak_each(trace_id: TraceId, segments: Vec<UtteranceSegment>) -> Vec<Effect> {
    segments
        .into_iter()
        .map(|segment| speak(trace_id, vec![segment]))
        .collect()
}

/// Queued speech with no source node.
pub(crate) fn speak(trace_id: TraceId, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )
}

/// Handles a caret key passed to the application: when the focus has text,
/// or may have, ask its outpost to wait for evidence and report the caret.
/// `pressed_at_ms` is when the hook saw the key, on the outposts' clock.
pub(crate) fn caret_key(state: &mut SrState, key: CaretKey, pressed_at_ms: u64) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    // The word being typed is forgotten when a caret key without Shift
    // moves the caret elsewhere or deletes; a key that selects, or one
    // outside the caret table such as Control+C, leaves it, so typing over
    // a selection goes on with the word (`docs/parity/input.md`,
    // "Typed-character echo").
    if !selects(key) && key.motion != CaretMotion::Other {
        state.typed_word.clear();
    }
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    let node = focus.snapshot.id;
    let role = focus.snapshot.role;
    let current = state.caret.as_ref().filter(|caret| caret.node == node);
    if current.is_none() && !may_have_text(role) {
        return effects;
    }
    let grid = is_grid(role);
    let unit = key.motion.unit();
    // What the key deletes, the text whose change is evidence, where the
    // caret starts, and the selection the key may change all come from the
    // caret as it was when the key was pressed, which may be older than the
    // caret Core now has.
    let line_break = current.and_then(|caret| caret.line_break.as_deref());
    let context = caret_before(state, node, pressed_at_ms);
    // Over a selection, a Backspace removes what comes before the
    // selection's start, where NVDA's caret is, which the outpost reports
    // as a position in the caret's line or in the earlier line it starts
    // in; a start in neither has nothing known before it.
    let start = context
        .and_then(|caret| caret.selection.map(|selection| (caret, selection)))
        .filter(|_| key.motion.deletes_back())
        .map(|(caret, selection)| {
            [Some(caret.line), caret.selection_line]
                .into_iter()
                .flatten()
                .find(|line| line.start == selection.start.anchor)
                .map(|line| (line, selection.start.offset))
        });
    let before_unknown = matches!(start, Some(None));
    let deleted = context.and_then(|caret| {
        let (line, offset) = start.flatten().unwrap_or((caret.line, caret.line.offset));
        deleted_text(line, offset, line_break, key.motion, grid)
    });
    let before = context
        .filter(|_| key.motion.deletes_back() || key.motion == CaretMotion::Other)
        .map(|caret| (caret.line.text.clone(), caret.line.offset));
    let compare = context.and_then(|caret| compared_text(caret.line, key.motion, grid));
    let selected_before = context.is_some_and(|caret| caret.selection.is_some());
    // The key's answer compares with the selection the outpost kept for
    // the node, whatever the key, as a caret report does, so a selection it
    // makes, leaves or replaces is spoken from one comparison, as NVDA
    // reports any selection change of a control that reports its
    // selection at the key's caret event
    // (`docs/nvda/editable-text-and-terminals.md`, "Selection changes").
    let watch = CaretWatch {
        since: context.map(CaretBefore::caret),
        pressed_at_ms,
        unit,
        compare,
        landing: key.motion == CaretMotion::Other,
        deletes: key.motion.deletes(),
    };
    // The key is moving the caret, which the review cursor follows: a
    // review command from now on reads where the caret is, not where Core
    // last saw it, however late the application reports the move.
    follow_caret(state, node);
    // A key still waiting on the same node is superseded: the change of
    // the selection its answer may yet carry, and the words of reports held
    // for it, are said with this key's (`superseded_reply`).
    let held = match state.pending_caret.take() {
        Some(old) if old.node == node => {
            state.superseded_caret = Some((old.query_id, old.node));
            old.heard
        }
        _ => Vec::new(),
    };
    let query_id = state.allocate_query_id();
    // A held key waiting behind a held key's watch waits for the newest
    // watch: a key handled between them moves the caret after it.
    if state.held_keys.awaiting_watch.is_some() {
        state.held_keys.awaiting_watch = Some(query_id);
    }
    state.pending_caret = Some(PendingCaret {
        query_id,
        node,
        key,
        deleted,
        before,
        later_key_at_ms: None,
        selected_before,
        before_unknown,
        heard: held,
    });
    effects.push(Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op: TextOp::AwaitCaret(watch),
    }));
    effects
}

/// The focus whose caret keys are watched for evidence: alive, with a
/// caret Core knows or a role that may have text ([`caret_key`]'s test).
pub(crate) fn text_focus(state: &SrState) -> Option<NodeId> {
    let focus = state.focus.as_ref().filter(|focus| focus.alive)?;
    let node = focus.snapshot.id;
    let known = state.caret.as_ref().is_some_and(|caret| caret.node == node);
    (known || may_have_text(focus.snapshot.role)).then_some(node)
}

/// Handles a caret key the keyboard hook held back
/// (`docs/design/key-holding.md`): the focus's outpost reads the caret
/// before it, as NVDA's caret scripts read the caret before they send the
/// key. A key pressed while another's caret is read waits behind it, and
/// is handled in order once the keys before it are.
pub(crate) fn held_key(
    state: &mut SrState,
    trace_id: TraceId,
    key: CaretKey,
    pressed_at_ms: u64,
) -> Vec<Effect> {
    state.held_keys.waiting.push_back(WaitingKey::Held(HeldKey {
        trace_id,
        key,
        pressed_at_ms,
    }));
    next_held(state, false)
}

/// Handles the keys waiting behind held keys, in order, until a held key's
/// read is in flight or no key waits: a held key's read is started, or
/// the key passed on at once when the focus has no caret to read; a key
/// the hook deferred behind a held key is handled as the input it came as
/// ([`crate::reduce::key_input`]), taken as pressed no earlier than the
/// held key ahead of it was passed on, since that is when it reached the
/// application. A held key waits until the watch of the held key passed
/// on before it has been answered (`HeldKeys::awaiting_watch`), so that
/// its read comes after that key's effect, and that key's answer is read
/// before the next key reaches the application, but no longer than
/// [`KEY_AHEAD_BOUND_MS`] after that key was passed on: the shell is asked
/// to end the wait then ([`Effect::BoundHeldKeyWait`]), and a key pressed
/// later than that is read at once. A review command or script waiting
/// behind a held key waits for the same watch and the same bound; it was
/// pressed before that key was passed on, so it always waits for the
/// bound, and it is run as the input it came as
/// ([`crate::reduce::command_input`]). `behind` says the keys wait behind a key just passed on:
/// that key's watch is then marked as having a later key
/// ([`later_key`]), as the later key's press would have marked it had the
/// watch existed then, so its answer only keeps the caret current, as
/// NVDA's caret script speaks nothing while another script waits. Each
/// key's effects carry its own trace ([`Effect::ForKey`]).
fn next_held(state: &mut SrState, mut behind: bool) -> Vec<Effect> {
    let mut effects = Vec::new();
    while state.held_keys.reading.is_none() {
        let Some(front) = state.held_keys.waiting.front() else {
            break;
        };
        // What waits for the watch of the key ahead: a held key, with its
        // press time, or a command, pressed before that key was passed on.
        let front_waits = match front {
            WaitingKey::Held(held) => Some((held.trace_id, held.pressed_at_ms)),
            WaitingKey::Command(input) => input.trace_id().map(|trace_id| (trace_id, 0)),
            WaitingKey::Later(_) => None,
        };
        if behind {
            later_key(state, state.held_keys.passed_at_ms);
        }
        // A held key's caret is read once the key ahead of it has been
        // answered, so that the read sees the application after that key,
        // or once the bound since that key was passed on has run out. This
        // wait is Verbatim's own: NVDA's next caret script runs as soon as
        // it is queued, the first giving up its wait at once.
        if let Some((trace_id, pressed_at_ms)) = front_waits
            && state.held_keys.awaiting_watch.is_some()
        {
            let due = state.held_keys.passed_at_ms + KEY_AHEAD_BOUND_MS;
            if pressed_at_ms < due {
                if state.held_keys.bounded != Some(trace_id) {
                    state.held_keys.bounded = Some(trace_id);
                    let now = pressed_at_ms.max(state.held_keys.passed_at_ms);
                    effects.push(Effect::BoundHeldKeyWait {
                        key: trace_id,
                        within_ms: due - now,
                    });
                }
                break;
            }
            state.held_keys.awaiting_watch = None;
        }
        let Some(waiting) = state.held_keys.waiting.pop_front() else {
            break;
        };
        behind = true;
        let (trace_id, own) = match waiting {
            WaitingKey::Held(held) => (Some(held.trace_id), start_held(state, held)),
            WaitingKey::Later(input) => {
                let at_ms = state.held_keys.passed_at_ms;
                let input = crate::reduce::pressed_no_earlier(*input, at_ms);
                (
                    crate::reduce::key_trace(&input),
                    crate::reduce::key_input(state, &input),
                )
            }
            WaitingKey::Command(input) => (
                input.trace_id(),
                crate::reduce::command_input(state, &input),
            ),
        };
        effects.extend(own.into_iter().map(|effect| match trace_id {
            Some(trace_id) => Effect::ForKey(trace_id, Box::new(effect)),
            None => effect,
        }));
    }
    effects
}

/// Starts the read of the caret before `held`, or passes it on at once
/// when the focus has no caret to read.
fn start_held(state: &mut SrState, held: HeldKey) -> Vec<Effect> {
    let Some(node) = text_focus(state) else {
        return pass_held(state, held, held.pressed_at_ms);
    };
    let query_id = state.allocate_query_id();
    state.held_keys.reading = Some(HeldRead {
        query_id,
        node,
        key: held,
    });
    vec![Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op: TextOp::ReadCaret,
    })]
}

/// Watches for `held`'s evidence as for a caret key the hook passed
/// itself, the key taken as pressed at `at_ms`, and passes it on to the
/// application. The watch is asked for first, so that it reaches the
/// outpost before the key reaches the application, as a key the hook
/// passes is reported before it is passed: the outpost then handles the
/// watch before the application's events for the key, as it would
/// without holding.
fn pass_held(state: &mut SrState, held: HeldKey, at_ms: u64) -> Vec<Effect> {
    state.held_keys.passed_at_ms = at_ms;
    // A terminal Core reads keeps it for its reading rules, and asks for
    // the colours from its time, before the key reaches the program.
    let kind = crate::terminal::caret_key_kind(held.key);
    let mut effects = crate::terminal_model::key(state, kind, at_ms, held.trace_id);
    effects.extend(caret_key(state, held.key, at_ms));
    state.held_keys.awaiting_watch = effects.iter().find_map(|effect| match effect {
        Effect::Text(TextRequest {
            query_id,
            op: TextOp::AwaitCaret(_),
            ..
        }) => Some(*query_id),
        _ => None,
    });
    effects.push(Effect::PassHeldKey(held.trace_id));
    effects
}

/// The watch of the held key last passed on was answered: the keys
/// waiting behind it are handled ([`next_held`]).
pub(crate) fn held_watch_answered(state: &mut SrState) -> Vec<Effect> {
    next_held(state, true)
}

/// The bound on the wait of the held key or command `trace_id` for the
/// watch of the key ahead has run out: if it still waits, its caret is
/// read, or the command run, now, as if that watch had been answered, and
/// the watch is left to end as it would ([`next_held`]).
pub(crate) fn held_wait_ended(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let front = match state.held_keys.waiting.front() {
        Some(WaitingKey::Held(held)) => Some(held.trace_id),
        Some(WaitingKey::Command(input)) => input.trace_id(),
        _ => None,
    };
    let waits = state.held_keys.reading.is_none()
        && state.held_keys.awaiting_watch.is_some()
        && front == Some(trace_id);
    if !waits {
        return Vec::new();
    }
    state.held_keys.awaiting_watch = None;
    next_held(state, true)
}

/// Handles the read of the caret before a held key: a caret read becomes
/// the focus's caret, as a report does, and the key, passed on, is taken
/// as pressed just after the read ended, so that its watch, here and in
/// the outpost, takes the read as the caret before the key (the newest
/// report from before the key). Any other answer, the read having failed
/// or passed its bound in the shell, passes the key on with the hook's
/// time, judged as a key the hook passed itself. The keys waiting behind
/// it are handled after it was passed on: a held key's caret is read
/// then, as NVDA's next caret script reads after the first's key was
/// sent ([`next_held`]).
pub(crate) fn held_read(state: &mut SrState, reading: HeldRead, reply: TextReply) -> Vec<Effect> {
    let held = reading.key;
    let at_ms = match reply {
        TextReply::Caret(reply) => {
            let at_ms = if reply.read_at_ms == 0 {
                held.pressed_at_ms
            } else {
                reply.read_at_ms + 1
            };
            update_caret(state, reading.node, reply.caret, reply.read_at_ms);
            at_ms
        }
        _ => held.pressed_at_ms,
    };
    let mut effects = pass_held(state, held, at_ms);
    effects.extend(next_held(state, true));
    effects
}

/// Whether `key` selects: with Shift, or Control+A.
fn selects(key: CaretKey) -> bool {
    key.select || key.motion == CaretMotion::SelectAll
}

/// The caret of a node as a caret key found it: its line, with the caret
/// at the chunk's offset, the selection, and the line the selection starts
/// in when that is an earlier line.
#[derive(Clone, Copy)]
struct CaretBefore<'a> {
    line: &'a TextChunk,
    selection: Option<Selection>,
    selection_line: Option<&'a TextChunk>,
}

impl CaretBefore<'_> {
    /// Where the caret was.
    fn caret(self) -> TextPosition {
        TextPosition {
            anchor: self.line.start,
            offset: self.line.offset,
        }
    }
}

/// The caret of `node` as a key pressed at `pressed_at_ms` found it: the
/// newest of Core's timed caret reports observed before the key was
/// pressed, as the outpost picks its own baseline (`docs/parity/text.md`,
/// "TextInfo-equivalent layer", caret-key reporting). A report observed in the
/// same millisecond as the key, or later, may already show what the key
/// did, so it is never the caret before the key. With no time for the key, or no timed report,
/// the caret Core has now stands in; with timed reports but none from
/// before the key, the caret before the key is not known.
fn caret_before(state: &SrState, node: NodeId, pressed_at_ms: u64) -> Option<CaretBefore<'_>> {
    let mut timed = state
        .caret_history
        .iter()
        .flatten()
        .filter(|report| report.node == node)
        .peekable();
    if pressed_at_ms == 0 || timed.peek().is_none() {
        return state
            .caret
            .as_ref()
            .filter(|caret| caret.node == node)
            .map(|caret| CaretBefore {
                line: &caret.line,
                selection: caret.selection,
                selection_line: caret.selection_line.as_ref(),
            });
    }
    // Reports arrive in the order their paths deliver them, not the order
    // they were observed, so the newest is the latest observed; of two
    // observed in the same millisecond, the later to arrive.
    timed
        .filter(|report| report.observed_at_ms < pressed_at_ms)
        .max_by_key(|report| report.observed_at_ms)
        .map(|report| CaretBefore {
            line: &report.line,
            selection: report.selection,
            selection_line: report.selection_line.as_ref(),
        })
}

/// What a Backspace is about to delete, from the caret's line before the
/// key, `offset` being the caret's byte offset in it, or a selection's
/// start: the character before it, or for Control+Backspace the text from
/// the start of the word before it up to it. At the
/// start of a line other than a terminal's, a Backspace, with or without
/// Control, deletes the line break before it, the kind of break the text uses, `line_break`
/// ([`CaretContext::line_break`]), which NVDA names
/// (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
/// character"). `None` when that is not known, and for other keys.
fn deleted_text(
    line: &TextChunk,
    offset: u32,
    line_break: Option<&str>,
    motion: CaretMotion,
    grid: bool,
) -> Option<String> {
    let content = text::line_content(&line.text, grid);
    let offset = text::boundary(content, offset as usize);
    match motion {
        CaretMotion::Backspace => match text::previous_grapheme(content, offset) {
            Some(range) => Some(content[range].to_owned()),
            None if !grid && !line.first => line_break.map(str::to_owned),
            None => None,
        },
        CaretMotion::BackspaceWord => {
            let words = text::words(content, line.language_at(0));
            match words.iter().rev().find(|range| range.start < offset) {
                Some(word) => Some(content[word.start..offset].to_owned()),
                None if offset == 0 && !grid && !line.first => line_break.map(str::to_owned),
                None => None,
            }
        }
        _ => None,
    }
}

/// What a key that deletes backward really did to the caret's line,
/// compared before and after it.
#[derive(Debug, PartialEq, Eq)]
enum Removal {
    /// This text, which ended at the caret before the key, is gone, and the
    /// caret is where it began: the guard by text positions, which keeps a
    /// selection list's marker moving between lines from being spoken as a
    /// deletion (`docs/parity/text.md`, "TextInfo-equivalent layer").
    Removed(String),
    /// The line after the key ends with the whole of the line before, and
    /// the caret is where it began: the line was joined to the one above,
    /// its line break removed.
    Joined,
    /// Nothing was removed at the caret (text was added, or the line is
    /// another).
    Nothing,
}

/// What a key that deletes backward did, from the caret's line `before`
/// and `after` it, each its text and the caret's byte offset in it. In a
/// terminal (`grid`) a row's padding is not text, but the caret may stand
/// in it, past the text: there the row reads as spaces up to the caret, so
/// a typed space a Backspace removes is found, which padding hides.
fn removal(before: (&str, usize), after: (&str, usize), grid: bool) -> Removal {
    let read = |(text, caret): (&str, usize)| {
        let mut content = text::line_content(text, grid).to_owned();
        if grid && caret > content.len() {
            content.push_str(&" ".repeat(caret - content.len()));
        }
        (content, caret)
    };
    let ((was, b), (now, a)) = (read(before), read(after));
    if a > b
        || !was.is_char_boundary(b)
        || !now.is_char_boundary(a)
        || b > was.len()
        || a > now.len()
    {
        return if a == now.len().saturating_sub(was.len()) && now.ends_with(&was) {
            Removal::Joined
        } else {
            Removal::Nothing
        };
    }
    let kept_before = was.get(..a) == now.get(..a);
    if a == b && kept_before && was.len() > now.len() {
        // The caret stayed and text after it went (Control+K).
        let gone = was.len() - now.len();
        if was.is_char_boundary(a + gone) && was.get(a + gone..) == now.get(a..) {
            return Removal::Removed(was[a..a + gone].to_owned());
        }
    }
    let kept_after = was.get(b..) == now.get(a..)
        || (grid && was.get(b..).map(str::trim_end) == now.get(a..).map(str::trim_end));
    if kept_before && kept_after && a < b {
        Removal::Removed(was[a..b].to_owned())
    } else {
        Removal::Nothing
    }
}

/// The text at the caret before a Delete, whose change is evidence the key
/// did something even though the caret stays where it is, from the caret's
/// line before the key.
fn compared_text(line: &TextChunk, motion: CaretMotion, grid: bool) -> Option<String> {
    let content = text::line_content(&line.text, grid);
    let offset = line.offset as usize;
    match motion {
        // The character as the outpost cuts it from the caret's line, a
        // line break at the line's end included: compared with the line's
        // content alone, a caret before the break would find a change
        // before the application had handled the key.
        CaretMotion::Delete => {
            let text = if grid { content } else { line.text.as_str() };
            Some(
                text::grapheme_at(text, offset)
                    .map_or(String::new(), |range| text[range].to_owned()),
            )
        }
        CaretMotion::DeleteWord => {
            let words = text::words(content, line.language_at(0));
            Some(
                text::word_at(&words, offset)
                    .map_or(String::new(), |range| content[range].to_owned()),
            )
        }
        _ => None,
    }
}

/// Handles the outpost's answer to a caret key's watch: keeps the caret
/// current, moves the review cursor with it, and speaks what the key did.
pub(crate) fn caret_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingCaret,
    reply: TextReply,
) -> Vec<Effect> {
    // A watch that ended without evidence (`TextReply::WatchEnded`), or a
    // reply without it, says nothing: a key that did not move the caret is
    // silent (`docs/parity/text.md`, "TextInfo-equivalent layer").
    let TextReply::Caret(reply) = reply else {
        return heard(state, trace_id, pending);
    };
    // Read once a later key had been pressed, the answer may show that
    // key's effect rather than this one's, so it is not spoken for this
    // key: a key's effect is never answered by a later key's (the live
    // caret checks of 2026-10-08). The caret it read is still the newest.
    if pending
        .later_key_at_ms
        .is_some_and(|later| reply.read_at_ms == 0 || reply.read_at_ms >= later)
    {
        // Its selection's change is no later key's to say: the outpost
        // compared it with the selection it kept, which the later key's
        // answer or report then compares with no more.
        let changed = reply_changes(&reply);
        if !state.focus_matches(pending.node) {
            return Vec::new();
        }
        update_caret(state, pending.node, reply.caret, reply.read_at_ms);
        let mut said = pending.heard.clone();
        said.extend(changed);
        return speak_each(trace_id, said);
    }
    if !reply.moved {
        return heard(state, trace_id, pending);
    }
    let Some(focus) = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.id == pending.node)
    else {
        return Vec::new();
    };
    let grid = is_grid(focus.snapshot.role);
    // A terminal's reading rules say what the key did
    // (`crate::terminal`); its answer keeps the caret current.
    if focus.snapshot.role == Role::Terminal {
        update_caret(state, pending.node, reply.caret, reply.read_at_ms);
        return heard(state, trace_id, pending);
    }
    let CaretReply {
        moved: _,
        caret,
        read_at_ms,
        unit,
        selection_changes,
        content_changed,
        new_selection,
        same_line,
        removed,
    } = *reply;
    let general = content_changed.then_some(General {
        selection: new_selection.as_ref(),
        collapsed: caret.selection.is_none(),
    });
    // A key that selects has no words of its own: what it selected or
    // unselected follows, as for any key.
    let segments = if selects(pending.key) {
        Vec::new()
    } else {
        match pending.key.motion {
            CaretMotion::Backspace | CaretMotion::BackspaceWord => {
                backspace_segments(pending, &caret.line, grid)
            }
            // The selection removed with its text, as by a cut: said by its
            // change alone, after, never the removed text, as NVDA's caret
            // event says it (`docs/parity/text.md`, "TextInfo-equivalent layer").
            // NVDA has no script for these keys, and says only its caret
            // event's words.
            CaretMotion::Other if removes_selection(pending, &caret) => Vec::new(),
            // A typed line that had wrapped onto more rows, removed.
            CaretMotion::Other if removed.is_some() => removed_segments(
                removed.as_deref().unwrap_or_default(),
                caret.line.language_at(0),
            ),
            CaretMotion::Other => {
                let mut reported = reported_format(state, pending.node);
                let segments =
                    other_key_segments(pending.before.as_ref(), &caret.line, same_line, grid)
                        .unwrap_or_else(|| line_segments(&caret.line, grid, &mut reported));
                state.reported_format = Some((pending.node, reported));
                segments
            }
            motion => {
                let mut reported = reported_format(state, pending.node);
                let segments =
                    unit_segments(&caret, unit.as_ref(), (motion.unit(), grid), &mut reported);
                state.reported_format = Some((pending.node, reported));
                segments
            }
        }
    };
    update_caret(state, pending.node, caret, read_at_ms);
    let mut effects: Vec<Effect> = Vec::new();
    if !segments.is_empty() {
        effects.push(speak(trace_id, segments));
    }
    effects.extend(changed_after(
        trace_id,
        pending,
        &selection_changes,
        general,
    ));
    effects
}

/// What a Backspace said, from `pending` and the caret's `line` after it.
/// Over a selection, what was before the selection's start when the key
/// was pressed, "blank" when nothing was, as NVDA's Backspace script reads
/// it from its caret, the selection's start, before passing the key and
/// says it once the caret moved (`docs/nvda/editable-text-and-terminals.md`);
/// nothing when the selection started on an earlier line, whose text
/// before it is not known; the selection's change follows. Otherwise what
/// the key removed, by comparing the line before and after it.
fn backspace_segments(
    pending: &PendingCaret,
    line: &TextChunk,
    grid: bool,
) -> Vec<UtteranceSegment> {
    if pending.before_unknown {
        return Vec::new();
    }
    if pending.selected_before {
        return match &pending.deleted {
            Some(deleted) => deleted_segments(deleted, pending.key.motion),
            None => text::character_segments(None, None),
        };
    }
    let removed = pending.before.as_ref().map(|(text, offset)| {
        removal(
            (text, *offset as usize),
            (&line.text, line.offset as usize),
            grid,
        )
    });
    match (removed, &pending.deleted) {
        (Some(Removal::Removed(text)), _) => deleted_segments(&text, pending.key.motion),
        // A line joined to the one before it: the line break went, as
        // predicted.
        (Some(Removal::Joined) | None, Some(deleted)) => {
            deleted_segments(deleted, pending.key.motion)
        }
        // Nothing removed (a control that inserted a character instead, as
        // the classic edit control does for Control+Backspace), or no
        // deletion predicted before the key.
        _ => Vec::new(),
    }
}

/// What a key says after its own words, how the selection changed: the
/// changes caret reports carried while it waited
/// ([`PendingCaret::heard`]), then its answer's, each compared once with
/// the selection the outpost kept.
fn changed_after(
    trace_id: TraceId,
    pending: &PendingCaret,
    changes: &[SelectionChange],
    general: Option<General<'_>>,
) -> Vec<Effect> {
    let mut effects = speak_each(trace_id, pending.heard.clone());
    effects.extend(speak_each(trace_id, selection_segments(changes, general)));
    effects
}

/// How a caret key's answer says the selection changed, apart from the
/// key's own words.
fn reply_changes(reply: &CaretReply) -> Vec<UtteranceSegment> {
    let general = reply.content_changed.then_some(General {
        selection: reply.new_selection.as_ref(),
        collapsed: reply.caret.selection.is_none(),
    });
    selection_segments(&reply.selection_changes, general)
}

/// The answer to a caret key a later key on the same node superseded
/// (`SrState::superseded_caret`), on `node`: its caret and words are not
/// spoken, the later key's being the newer, but the selection's change it
/// carries is, after the later key's own words, held as a caret report's
/// is ([`PendingCaret::heard`]), or at once when no key waits: the
/// outpost compared it with the selection it kept, which the later key's
/// answer then compares with no more, so it is said once, as NVDA says
/// every caret event's change.
pub(crate) fn superseded_reply(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    reply: &TextReply,
) -> Vec<Effect> {
    let TextReply::Caret(reply) = reply else {
        return Vec::new();
    };
    let changed = reply_changes(reply);
    if changed.is_empty() || !state.focus_matches(node) {
        return Vec::new();
    }
    if let Some(pending) = state
        .pending_caret
        .as_mut()
        .filter(|pending| pending.node == node)
    {
        pending.heard.extend(changed);
        return Vec::new();
    }
    speak_each(trace_id, changed)
}

/// The selection changes caret reports carried while `pending`'s key
/// waited ([`PendingCaret::heard`]), spoken when its answer says nothing
/// of its own, while its node is still the focus.
fn heard(state: &SrState, trace_id: TraceId, pending: &PendingCaret) -> Vec<Effect> {
    if !state.focus_matches(pending.node) {
        return Vec::new();
    }
    speak_each(trace_id, pending.heard.clone())
}

/// Whether the answer to `pending`'s key shows the selection removed:
/// something was selected when the key was pressed, and nothing is now.
fn removes_selection(pending: &PendingCaret, caret: &CaretReport) -> bool {
    pending.selected_before && caret.selection.is_none()
}

/// What a key outside the caret table did (`docs/parity/text.md`,
/// "TextInfo-equivalent layer"), from the caret's line `before` it (its text and
/// the caret's byte offset) and `after` it, and whether the caret is on the
/// line it was on (`same_line`, from the outpost): text removed at the
/// caret is spoken; a caret on another line speaks that line (`None`
/// here, which the caller reads with its formatting); a caret that moved
/// over an unchanged line speaks by where it landed ([`landing_segments`]);
/// anything else (text added, which typing echo or terminal output speak)
/// says nothing.
fn other_key_segments(
    before: Option<&(String, u32)>,
    after: &TextChunk,
    same_line: Option<bool>,
    grid: bool,
) -> Option<Vec<UtteranceSegment>> {
    let Some((text, offset)) = before else {
        return (same_line != Some(false)).then(Vec::new);
    };
    let removed = removal(
        (text, *offset as usize),
        (&after.text, after.offset as usize),
        grid,
    );
    if let Removal::Removed(removed) = removed {
        return Some(removed_segments(&removed, after.language_at(0)));
    }
    if same_line == Some(false) {
        return None;
    }
    if text::line_content(text, grid) == text::line_content(&after.text, grid) {
        return Some(landing_segments(*offset as usize, after, grid));
    }
    Some(Vec::new())
}

/// The speech for text a key outside the caret table removed: a single
/// character by its name, otherwise the text.
fn removed_segments(removed: &str, language: Option<&str>) -> Vec<UtteranceSegment> {
    if text::graphemes_count(removed) == 1 {
        text::character_segments(Some(removed), language)
    } else {
        text::text_segments(removed.trim(), language)
    }
}

/// The landing rule (`docs/parity/text.md`, "TextInfo-equivalent layer"): a
/// caret that moved from `from` (a byte offset) on a line whose text is
/// unchanged, now at `line`'s offset, speaks by where it landed. By exactly
/// one character, or at the start or end of the line: the character at the
/// caret. At the start of a word, or just after its end: the whole word.
/// Inside a word: the word and then the character at the caret.
fn landing_segments(from: usize, line: &TextChunk, grid: bool) -> Vec<UtteranceSegment> {
    let content = text::line_content(&line.text, grid);
    let at = text::boundary(content, line.offset as usize);
    let from = text::boundary(content, from);
    let language = line.language_at(at);
    let character = || {
        text::character_segments(
            text::grapheme_at(content, at).map(|range| &content[range]),
            language,
        )
    };
    let one_character = text::grapheme_at(content, from.min(at))
        .is_some_and(|range| range.start == from.min(at) && range.end == from.max(at));
    if at == 0 || at >= content.len() || one_character {
        return character();
    }
    let words = text::words(content, language);
    if let Some(word) = words
        .iter()
        .find(|word| word.start == at)
        .or_else(|| words.iter().find(|word| word.end == at))
    {
        return text::word_segments(&content[word.clone()], language);
    }
    if let Some(word) = words.iter().find(|word| word.start < at && at < word.end) {
        let mut segments = text::word_segments(&content[word.clone()], language);
        segments.extend(character());
        return segments;
    }
    character()
}

/// The speech for text a Backspace deleted: a character, or a line break
/// whichever key deleted it, by its name, and a word as text. A carriage
/// return and line feed deleted together are spoken as the line feed, as
/// NVDA speaks them (`docs/parity/text.md`, "TextInfo-equivalent layer").
fn deleted_segments(deleted: &str, motion: CaretMotion) -> Vec<UtteranceSegment> {
    if motion == CaretMotion::Backspace || text::line_break(deleted) == Some(deleted) {
        let deleted = if deleted == "\r\n" { "\n" } else { deleted };
        text::character_segments(Some(deleted), None)
    } else {
        text::text_segments(deleted.trim(), None)
    }
}

/// The formatting last reported in `node`'s text, NVDA's per-object cache:
/// nothing yet for a node other than the one last spoken in.
fn reported_format(state: &SrState, node: NodeId) -> TextAttributes {
    state
        .reported_format
        .as_ref()
        .filter(|(reported, _)| *reported == node)
        .map(|(_, attributes)| attributes.clone())
        .unwrap_or_default()
}

/// The speech for a chunk's content with the formatting the outpost sent
/// for it, as [`text::formatted_segments`] makes it: the whole content
/// without its line break (and in a terminal its padding) carries the
/// formatting, and only the content without surrounding white space is
/// spoken for a word. `None` when the chunk has no formatting.
fn formatted(
    chunk: &TextChunk,
    grid: bool,
    how: text::Spoken,
    reported: &mut TextAttributes,
) -> Option<Vec<UtteranceSegment>> {
    let content = text::chunk_content(chunk, grid);
    let spoken = match how {
        text::Spoken::Word => {
            let start = content.len() - content.trim_start().len();
            start..content.trim_end().len().max(start)
        }
        // The character the chunk starts with, a line break included.
        text::Spoken::Character => text::characters(&chunk.text, grid)
            .into_iter()
            .next()
            .unwrap_or(0..0),
        text::Spoken::Text => 0..content.len(),
    };
    text::formatted_segments(
        chunk,
        (0..content.len(), spoken),
        how,
        reported,
        chunk.language_at(0),
    )
}

/// The speech for the unit at the caret after a caret key: the character,
/// the provider's word or paragraph when it sent one, and the line
/// otherwise. A word that is a single character is spoken by its name
/// ([`text::word_segments`]). Formatting the outpost sent with the text is
/// spoken as it changes from `reported`, which it then updates.
fn unit_segments(
    caret: &CaretReport,
    unit_chunk: Option<&TextChunk>,
    (unit, grid): (TextUnit, bool),
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    let line = &caret.line;
    let content = text::line_content(&line.text, grid);
    let offset = line.offset as usize;
    match (unit, unit_chunk) {
        (TextUnit::Character, chunk) => {
            if let Some(segments) =
                chunk.and_then(|chunk| formatted(chunk, grid, text::Spoken::Character, reported))
            {
                return segments;
            }
            let character = text::character_at(&line.text, offset, grid);
            text::character_segments(character, line.language_at(offset))
        }
        (TextUnit::Line, _) | (_, None) => formatted(line, grid, text::Spoken::Text, reported)
            .unwrap_or_else(|| text::text_segments(content, line.language_at(0))),
        (unit, Some(chunk)) => {
            if unit == TextUnit::Word
                && let Some(segments) = formatted(chunk, grid, text::Spoken::Word, reported)
            {
                return segments;
            }
            let read = text::chunk_content(chunk, grid).trim();
            if unit == TextUnit::Word {
                text::word_segments(read, chunk.language_at(0))
            } else {
                text::text_segments(read, chunk.language_at(0))
            }
        }
    }
}

/// The new selection of a report whose selection changes are said in
/// general terms, the text having changed since the selection compared
/// with was reported ([`selection_segments`]).
#[derive(Clone, Copy)]
pub(crate) struct General<'a> {
    /// The whole new selection's text, when it holds something.
    pub(crate) selection: Option<&'a SelectionChange>,
    /// Nothing is selected now.
    pub(crate) collapsed: bool,
}

/// The speech for a selection change: each change as NVDA's "selected" or
/// "unselected", a single character by its name, 512 characters or more as
/// a count, and a change with no text as nothing. With `general`, the text
/// having changed since the selection compared with was reported, the
/// changes say only what happened and the words come from the new
/// selection (`docs/parity/text.md`, "TextInfo-equivalent layer";
/// `docs/nvda/editable-text-and-terminals.md`, "Selection changes"): the
/// new selection's text as "selected" when something was selected, then,
/// when something was unselected, the new selection's text as "selected
/// instead", or "selection removed" when nothing is selected now.
pub(crate) fn selection_segments(
    changes: &[SelectionChange],
    general: Option<General<'_>>,
) -> Vec<UtteranceSegment> {
    let Some(general) = general else {
        return changes
            .iter()
            .filter(|change| change.characters > 0)
            .map(|change| {
                let text = selection_text(change);
                let phrase = if change.selected {
                    Phrase::Selected(text)
                } else {
                    Phrase::Unselected(text)
                };
                UtteranceSegment::new(SegmentContent::Phrase(phrase))
            })
            .collect();
    };
    let new_text = general
        .selection
        .filter(|selection| selection.characters > 0)
        .map(selection_text);
    let mut segments = Vec::new();
    if changes.iter().any(|change| change.selected)
        && let Some(text) = new_text.clone()
    {
        segments.push(UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(text),
        )));
    }
    if changes.iter().any(|change| !change.selected) {
        if general.collapsed {
            segments.push(UtteranceSegment::new(SegmentContent::Message(
                Message::SelectionRemoved,
            )));
        } else if let Some(text) = new_text {
            segments.push(UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::SelectedInstead(text),
            )));
        }
    }
    segments
}

/// A selection change's text as it is spoken: a single character by its
/// name, 512 characters or more as a count.
fn selection_text(change: &SelectionChange) -> SelectionText {
    if change.characters >= SELECTION_SPOKEN_AS_COUNT {
        SelectionText::Characters(change.characters)
    } else if change.characters == 1 {
        SelectionText::Character(change.text.clone())
    } else {
        SelectionText::Text(change.text.clone())
    }
}

/// Speaks how the selection of the focus `node` changed with a caret
/// report no key made, observed at `observed_at_ms`: the mouse's or the
/// application's selection, with the same words as a selecting key's
/// ([`selection_segments`]), as NVDA reports any selection change of a
/// control that reports its selection through caret events
/// (`docs/nvda/editable-text-and-terminals.md`, "Selection changes";
/// `docs/parity/text.md`, "TextInfo-equivalent layer"). The caller passes
/// only reports that came after the focus's announcement was complete.
/// While a caret key waits for its answer, the report says nothing now:
/// the key's answer speaks the report's change after the key's own words
/// ([`PendingCaret::heard`]), since the outpost compared it with the
/// selection it kept, which the answer then compares with no more.
pub(crate) fn selection_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    (changes, general): (&[SelectionChange], Option<General<'_>>),
) -> Vec<Effect> {
    if changes.is_empty() || !state.focus_matches(node) {
        return Vec::new();
    }
    if let Some(pending) = state
        .pending_caret
        .as_mut()
        .filter(|pending| pending.node == node)
    {
        pending.heard.extend(selection_segments(changes, general));
        return Vec::new();
    }
    speak_each(trace_id, selection_segments(changes, general))
}

/// Takes a caret report for `node` as the focus's caret, and moves the
/// review cursor to it when the review cursor follows the caret and the
/// navigator is on that node (to read the caret afresh, as the review
/// cursor does at [`ReviewText::Caret`]). `observed_at_ms` is when the report was
/// observed or read, 0 when unknown; a timed report is also kept for a
/// later caret key to find the caret as it was when that key was pressed
/// ([`caret_before`]). A timed report observed before the caret Core holds,
/// such as a caret key's reply read before a caret event that reached Core
/// first, is only kept: the caret stays the newest known, and the review
/// cursor stays with it. An untimed report is taken as it arrives.
pub(crate) fn update_caret(
    state: &mut SrState,
    node: NodeId,
    caret: CaretReport,
    observed_at_ms: u64,
) {
    if !state.focus_matches(node) {
        return;
    }
    let line = Arc::new(caret.line);
    if observed_at_ms == 0 || state.caret.as_ref().is_some_and(|known| known.node != node) {
        state.caret_history = Default::default();
    }
    if observed_at_ms != 0 {
        state.caret_history.rotate_left(1);
        state.caret_history[CARET_HISTORY - 1] = Some(TimedCaret {
            observed_at_ms,
            node,
            line: Arc::clone(&line),
            selection: caret.selection,
            selection_line: caret.selection_line.as_deref().cloned(),
        });
    }
    if let Some(known) = state.caret.as_mut().filter(|known| {
        known.node == node && observed_at_ms != 0 && known.observed_at_ms > observed_at_ms
    }) {
        // The older report still shows the kind of line break the text uses.
        if known.line_break.is_none() {
            known.line_break = text::line_break(&line.text).map(str::to_owned);
        }
        return;
    }
    follow_caret(state, node);
    let line_break = text::line_break(&line.text).map(str::to_owned).or_else(|| {
        state
            .caret
            .take()
            .filter(|caret| caret.node == node)
            .and_then(|caret| caret.line_break)
    });
    state.caret = Some(CaretContext {
        node,
        line,
        selection: caret.selection,
        selection_line: caret.selection_line.map(|line| *line),
        line_break,
        observed_at_ms,
    });
}

/// Takes a caret Core itself moved `node`'s caret to, `line` with the caret
/// at its offset and nothing selected, as the focus's caret. It is also
/// kept as a timed report observed at 0, before any key, so it stays the
/// caret a later key found ([`caret_before`]) when the application's own
/// report of the move was observed no earlier than that key.
pub(crate) fn caret_moved_by_core(state: &mut SrState, node: NodeId, line: TextChunk) {
    if !state.focus_matches(node) {
        return;
    }
    update_caret(
        state,
        node,
        CaretReport {
            line,
            selection: None,
            selection_line: None,
        },
        0,
    );
    if let Some(caret) = &state.caret {
        state.caret_history[CARET_HISTORY - 1] = Some(TimedCaret {
            observed_at_ms: 0,
            node,
            line: Arc::clone(&caret.line),
            selection: None,
            selection_line: None,
        });
    }
}

/// Puts the review cursor at the caret of `node` when it follows the caret
/// and the navigator is on that node: the next review command reads the
/// line at the caret as the outpost finds it ([`ReviewText::Caret`]).
///
/// A terminal's review cursor follows the caret only when its reading rules
/// find that a key moved the caret, which its outpost tells
/// (`crate::terminal`), never on a caret report alone, since a program
/// moves the cursor as it draws (`docs/design/terminal-model.md`,
/// "Reading rules", rule 24).
fn follow_caret(state: &mut SrState, node: NodeId) {
    if state.settings.follow_caret
        && let Some(navigator) = state.navigator.as_mut()
        && navigator.object.id == node
        && !is_grid(navigator.object.role)
    {
        navigator.text = ReviewText::Caret;
    }
}

/// Ends the announcement of the focus `node` with what the outpost's caret
/// read found, carried with the focus (`docs/nvda/speech.md`, "What an
/// object with text says"): the caret's line, or "selected" and the
/// selected text when text is selected, or, for an object with no text to
/// read, its value, which the focus's announcement left out. Its name and
/// role were not held back for it, and nothing comes between them, since
/// both come from the one message. A protected field's text is read as any
/// field's, masked as the control gives it (`docs/parity/focus-announcement.md`,
/// "Text on focus"), and one whose text could not be read speaks neither
/// its line nor its value. The caret, when read, becomes Core's.
pub(crate) fn focus_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    (text, observed_at_ms): (Option<&FocusText>, u64),
) -> Vec<Effect> {
    keep_focus_caret(state, node.id, text, observed_at_ms);
    if !may_have_text(node.role) {
        return Vec::new();
    }
    match text {
        Some(FocusText::Caret {
            caret,
            selected_text,
        }) => {
            let selected = caret
                .selection
                .and(selected_text.clone())
                .and_then(selected_segments);
            match selected {
                Some(segments) => focus_speech(state, trace_id, node.id, segments),
                None => focus_line(state, trace_id, node.id),
            }
        }
        Some(FocusText::NoText) => focus_value(state, trace_id, node.id),
        Some(FocusText::NotRead) | None => Vec::new(),
    }
}

/// Takes the caret a focus `node` came with, when its text was read, as
/// Core's caret, without speaking: for a focus announced
/// ([`focus_text`]) and for one taken silently, a duplicate or the same
/// focus reported again by a replacement outpost.
pub(crate) fn keep_focus_caret(
    state: &mut SrState,
    node: NodeId,
    text: Option<&FocusText>,
    observed_at_ms: u64,
) {
    if let Some(FocusText::Caret { caret, .. }) = text {
        update_caret(state, node, caret.clone(), observed_at_ms);
    }
}

/// The speech for an object's selected text: "selected" and the text (NVDA's
/// word order for text already selected, unlike a change's), or
/// its number of characters when there are 512 or more. `None` when nothing
/// is selected.
fn selected_segments(text: String) -> Option<Vec<UtteranceSegment>> {
    let characters = u32::try_from(verbatim_text::graphemes(&text).len()).unwrap_or(u32::MAX);
    if characters == 0 {
        return None;
    }
    let selected = if characters >= SELECTION_SPOKEN_AS_COUNT {
        SelectionText::Characters(characters)
    } else {
        SelectionText::Text(text)
    };
    Some(vec![UtteranceSegment::new(SegmentContent::Phrase(
        Phrase::Preselected(selected),
    ))])
}

/// Speaks the value of a focus that turned out to have no text to read, as
/// any object without text speaks its value.
fn focus_value(state: &SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(value) = state
        .focus
        .as_ref()
        .and_then(|focus| focus.snapshot.value.clone())
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    focus_speech(state, trace_id, node, vec![UtteranceSegment::value(value)])
}

/// Speaks the caret's line of the focus `node`, "blank" when it has nothing
/// to read, with the formatting at its start and wherever it changes, as a
/// change from none reported yet.
fn focus_line(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(segments) = caret_line(state, node) else {
        return Vec::new();
    };
    focus_speech(state, trace_id, node, segments)
}

/// The speech for the caret's line in the focus `node`, "blank" when it has
/// nothing to read, with the formatting at its start and wherever it
/// changes, as a change from the formatting last reported there. `None`
/// when Core does not know the caret of `node`.
fn caret_line(state: &mut SrState, node: NodeId) -> Option<Vec<UtteranceSegment>> {
    let caret = state.caret.as_ref().filter(|caret| caret.node == node)?;
    let grid = state
        .focus
        .as_ref()
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    let mut reported = reported_format(state, node);
    let segments = line_segments(&caret.line, grid, &mut reported);
    state.reported_format = Some((node, reported));
    Some(segments)
}

/// The speech for a line read as a caret movement reads it: its content,
/// "blank" when it has none, with the formatting the outpost sent spoken as
/// it changes from `reported`.
fn line_segments(
    line: &TextChunk,
    grid: bool,
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    formatted(line, grid, text::Spoken::Text, reported).unwrap_or_else(|| {
        text::text_segments(text::line_content(&line.text, grid), line.language_at(0))
    })
}

/// Reads the text of the navigator object `node` for `read`: for its
/// announcement, made by object navigation or by reporting the current
/// object, an object that may have text says its text in place of its
/// value, as a focus does (`docs/nvda/speech.md`, "What an object with text
/// says"), whether or not it has the focus; reporting it a second or third
/// time spells or copies its name and that text. The text is the selection,
/// announced as "selected" and the selected text, or the line at the caret;
/// a control that reports no caret reads its first line. The focus's caret,
/// once Core knows it, is read without asking the outpost; otherwise the
/// outpost is asked for the selected text, then, when nothing is selected,
/// the caret's line ([`navigator_text_reply`]). A protected field's text is
/// read as any field's, as on focus.
pub(crate) fn navigator_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
) -> Vec<Effect> {
    if !may_have_text(node.role) {
        return Vec::new();
    }
    let (start, end) = match state.caret.as_ref().filter(|caret| caret.node == node.id) {
        Some(caret) => match caret.selection {
            Some(selection) => (TextPoint::At(selection.start), TextPoint::At(selection.end)),
            None if read == NavigatorRead::Announce => {
                return caret_line(state, node.id)
                    .map(|segments| vec![speak_about(trace_id, node, segments)])
                    .unwrap_or_default();
            }
            None => {
                let line = text::line_content(&caret.line.text, is_grid(node.role)).to_owned();
                return named_text(trace_id, node, read, &line);
            }
        },
        None => (TextPoint::SelectionStart, TextPoint::SelectionEnd),
    };
    navigator_request(
        state,
        node.id,
        TextOp::ReadRange {
            start,
            end,
            through: None,
        },
        TextFollowUp::NavigatorSelection(read),
    )
}

/// Spells or copies, by `read`, the name of `node` followed by `body`, the
/// text it would announce, as reporting the current object a second or
/// third time does for an object with text.
fn named_text(
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
    body: &str,
) -> Vec<Effect> {
    let text = [node.name.as_deref().unwrap_or_default(), body]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Vec::new();
    }
    match read {
        NavigatorRead::Copy => vec![Effect::CopyToClipboard(text)],
        _ => vec![speak_about(trace_id, node, crate::reduce::spelled(&text))],
    }
}

/// Asks the outpost of `node` for its text, for the navigator's
/// announcement to go on with `then`. A newer text request supersedes it.
fn navigator_request(
    state: &mut SrState,
    node: NodeId,
    op: TextOp,
    then: TextFollowUp,
) -> Vec<Effect> {
    let query_id = state.allocate_query_id();
    state.pending_text = Some(PendingText {
        query_id,
        node,
        then,
    });
    vec![Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op,
    })]
}

/// Ends what [`navigator_text`] started with the answer it asked for: the
/// selected text when there is some, else, after asking for it, the caret's
/// line; the value when the object turns out to have no text to read, and
/// for spelling or copying the name and value, as for any object. Nothing
/// is done once the object asked about is neither the navigator object nor
/// the focus (reporting the focus reads the focus's text the same way).
pub(crate) fn navigator_text_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingText,
    reply: TextReply,
) -> Vec<Effect> {
    let navigator = state.navigator.as_ref().map(|navigator| &navigator.object);
    let focus = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive)
        .map(|focus| &focus.snapshot);
    let Some(object) = navigator
        .into_iter()
        .chain(focus)
        .find(|object| object.id == pending.node)
        .cloned()
    else {
        return Vec::new();
    };
    let (TextFollowUp::NavigatorSelection(read) | TextFollowUp::NavigatorLine(read)) = pending.then
    else {
        return Vec::new();
    };
    let segments = match (&pending.then, reply) {
        (TextFollowUp::NavigatorSelection(_), TextReply::Range { text, .. }) => {
            if verbatim_text::graphemes(&text).is_empty() {
                return navigator_line(state, pending.node, read);
            }
            if read != NavigatorRead::Announce {
                return named_text(trace_id, &object, read, &text);
            }
            selected_segments(text).unwrap_or_default()
        }
        (TextFollowUp::NavigatorSelection(_), TextReply::Unsupported) => {
            return navigator_line(state, pending.node, read);
        }
        (TextFollowUp::NavigatorLine(_), TextReply::Read { chunk, .. }) => {
            let grid = is_grid(object.role);
            if read != NavigatorRead::Announce {
                return named_text(
                    trace_id,
                    &object,
                    read,
                    text::line_content(&chunk.text, grid),
                );
            }
            line_segments(&chunk, grid, &mut TextAttributes::default())
        }
        (_, TextReply::NoText | TextReply::Unsupported) => {
            if read != NavigatorRead::Announce {
                return named_text(
                    trace_id,
                    &object,
                    read,
                    object.value.as_deref().unwrap_or_default(),
                );
            }
            match object.value.as_ref().filter(|value| !value.is_empty()) {
                Some(value) => vec![UtteranceSegment::value(value.clone())],
                None => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };
    vec![speak_about(trace_id, &object, segments)]
}

/// Asks for the line at the caret of the navigator object `node`, or its
/// first line when it reports no caret, for `read`.
fn navigator_line(state: &mut SrState, node: NodeId, read: NavigatorRead) -> Vec<Effect> {
    navigator_request(
        state,
        node,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        }),
        TextFollowUp::NavigatorLine(read),
    )
}

/// Queued speech about `node`, valid however the focus moves, as the rest
/// of a navigator announcement is.
fn speak_about(trace_id: TraceId, node: &NodeSnapshot, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(node)),
            say_all: false,
            validity: None,
        },
        None,
    )
}

/// The rest of a focus announcement, queued and valid while `node` is the
/// focus, as the announcement itself is.
fn focus_speech(
    state: &SrState,
    trace_id: TraceId,
    node: NodeId,
    segments: Vec<UtteranceSegment>,
) -> Vec<Effect> {
    let Some(focus) = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.id == node)
    else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(&focus.snapshot)),
            validity: Some(FocusValidity {
                node,
                had_focus: true,
                window: focus.snapshot.details.window,
            }),
            say_all: false,
        },
        None,
    )]
}

/// Notes a key pressed at `at_ms` (any key that cut speech off, as nearly
/// every key does): a caret key still watched was pressed before it, so
/// an answer read from then on is not that key's alone.
pub(crate) fn later_key(state: &mut SrState, at_ms: u64) {
    if let Some(pending) = state.pending_caret.as_mut()
        && pending.later_key_at_ms.is_none()
        && at_ms != 0
    {
        pending.later_key_at_ms = Some(at_ms);
    }
}

/// Handles a key that ends or clears the command line being typed (Escape,
/// Control+C, Control+D, Control+Break): in a terminal, the word being
/// typed is forgotten; the terminal's reading rules, told of the key by the
/// hook, forget the typing they hold, never shown, as NVDA forgets its
/// queued characters on Enter, Tab, Control+C, Control+D, and
/// Control+Break (`docs/parity/terminals.md`, "New terminal output").
pub(crate) fn clearing_key(state: &mut SrState) {
    let terminal = state
        .focus
        .as_ref()
        .is_some_and(|focus| focus.alive && focus.snapshot.role == Role::Terminal);
    if terminal {
        state.typed_word.clear();
    }
}

/// Handles text typed into the focused application: echoes characters and
/// words by the settings, a protected field's characters as the protected
/// character only. What is typed into a terminal is echoed once the
/// terminal shows it, which its reading rules tell (`crate::terminal`),
/// unless the user asked for terminal passwords to be spoken, when it is
/// echoed as it is typed.
pub(crate) fn character_typed(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    // A caret key still watched is not answered by the caret the typing
    // moves: the typing's key press has marked it (`later_key`), so its
    // answer only keeps the caret current.
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    if focus.snapshot.role == Role::Terminal {
        if typed.chars().any(|c| matches!(c, '\r' | '\n' | '\t')) {
            // Enter or Tab ends the word being typed, which in a terminal
            // is never echoed whole (Escape and the Control keys that clear
            // a line come as [`clearing_key`]).
            state.typed_word.clear();
        } else if state.settings.speak_terminal_passwords {
            effects.extend(echo(state, trace_id, typed));
        }
        return effects;
    }
    effects.extend(echo(state, trace_id, typed));
    effects
}

/// Echoes typed text by the settings: a finished word first, when word echo
/// is on and the text ends one, then the characters.
pub(crate) fn echo(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let editable = is_editable(&focus.snapshot);
    let protected = focus.snapshot.states.contains(State::Protected);
    let applies = |mode: TypingEcho| match mode {
        TypingEcho::Off => false,
        TypingEcho::EditControls => editable,
        TypingEcho::Always => true,
    };
    let echo_characters = applies(state.settings.speak_typed_characters);
    let echo_words = applies(state.settings.speak_typed_words) && !protected;
    let mut effects = Vec::new();
    for range in verbatim_text::graphemes(typed) {
        let character = &typed[range];
        // A letter, mark, or number continues the word, so a virama, a
        // Thai tone mark, or a zero-width non-joiner typed on its own does
        // not end it.
        let word_character = verbatim_text::is_word_grapheme(character);
        if word_character {
            if echo_words && state.typed_word.len() + character.len() <= MAX_TYPED_WORD {
                state.typed_word.push_str(character);
            }
        } else {
            let word = std::mem::take(&mut state.typed_word);
            if echo_words && !word.is_empty() {
                effects.push(speak(trace_id, vec![UtteranceSegment::text(word)]));
            }
        }
        // A control character, Tab, Enter, or Backspace among them, ends a
        // word but is never spelled, as in NVDA (docs/nvda/input.md, "Typed
        // characters, IME, and composition"): Tab in a dialog moves focus,
        // and saying "tab" there would cut off the control it reached.
        let printable = !character.chars().any(char::is_control);
        if echo_characters && printable {
            let spoken = if protected {
                PROTECTED_CHARACTER
            } else {
                character
            };
            effects.push(speak(
                trace_id,
                vec![UtteranceSegment::new(SegmentContent::Character(
                    spoken.to_owned(),
                ))],
            ));
        }
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::{Removal, landing_segments, other_key_segments, removal};
    use verbatim_model::{SegmentContent, TextAnchor, TextChunk, TextUnit, UtteranceSegment};

    /// The line `text` with the caret at byte `offset`.
    fn line(text: &str, offset: u32) -> TextChunk {
        TextChunk {
            unit: TextUnit::Line,
            text: text.to_owned(),
            start: TextAnchor(1),
            offset,
            languages: Vec::new(),
            first: false,
            last: false,
            truncated: false,
            formats: Vec::new(),
        }
    }

    /// What the segments say: characters and text as they are, "blank" for
    /// the blank message.
    fn said(segments: &[UtteranceSegment]) -> Vec<String> {
        segments
            .iter()
            .map(|segment| match &segment.content {
                SegmentContent::Character(character) | SegmentContent::Text(character) => {
                    character.clone()
                }
                SegmentContent::Message(verbatim_model::Message::Blank) => "blank".to_owned(),
                other => panic!("unexpected segment {other:?}"),
            })
            .collect()
    }

    fn removed(text: &str) -> Removal {
        Removal::Removed(text.to_owned())
    }

    #[test]
    fn what_a_backspace_removed_comes_from_the_line_before_and_after() {
        // A character, and with Control a word.
        assert_eq!(removal(("abc\n", 3), ("ab\n", 2), false), removed("c"));
        assert_eq!(
            removal(("hello world\r\n", 11), ("hello \r\n", 6), false),
            removed("world")
        );
        // The classic edit control's Control+Backspace inserts a DEL
        // character: nothing was removed.
        assert_eq!(
            removal(("hello\r\n", 5), ("hello\u{7f}\r\n", 6), false),
            Removal::Nothing
        );
        // A Hindi vowel sign removed on its own, as edit controls do.
        assert_eq!(removal(("कि", 6), ("क", 3), false), removed("ि"));
        // Backspace at a line's start joins it to the line above.
        assert_eq!(
            removal(("def\r\n", 0), ("abcdef\r\n", 3), false),
            Removal::Joined
        );
        // Text changed elsewhere is not a removal at the caret.
        assert_eq!(removal(("abc", 3), ("xb", 2), false), Removal::Nothing);
    }

    #[test]
    fn a_terminal_space_hidden_by_padding_is_found() {
        // A typed space after "echo": the row reads "ready> echo" and padding,
        // with the caret one past the text; Backspace leaves the caret at
        // its end.
        assert_eq!(
            removal(("ready> echo     ", 12), ("ready> echo     ", 11), true),
            removed(" ")
        );
        assert_eq!(
            removal(("ready> echo     ", 11), ("ready> ech      ", 10), true),
            removed("o")
        );
    }

    #[test]
    fn a_key_outside_the_table_speaks_by_where_the_caret_landed() {
        let text = "ready> echo hello world";
        let landed = |from: usize, at: u32| said(&landing_segments(from, &line(text, at), true));
        // To the start or the end of the line: the character there.
        assert_eq!(landed(12, 0), ["r"]);
        assert_eq!(landed(12, 23), ["blank"]);
        // By exactly one character: the character at the caret.
        assert_eq!(landed(12, 13), ["e"]);
        // To a word's start, or just after its end: the word.
        assert_eq!(landed(23, 12), ["hello"]);
        assert_eq!(landed(7, 17), ["hello"]);
        // Inside a word: the word, then the character at the caret.
        assert_eq!(landed(7, 14), ["hello", "l"]);
    }

    #[test]
    fn a_key_outside_the_table_speaks_text_it_removed_and_a_new_line() {
        let before = ("ready> echo hello".to_owned(), 17);
        // Control+W: the word before the caret.
        assert_eq!(
            other_key_segments(Some(&before), &line("ready> echo ", 12), Some(true), true)
                .map(|segments| said(&segments)),
            Some(vec!["hello".to_owned()])
        );
        // Control+K from "echo": what followed the caret.
        let at_echo = ("ready> echo hello".to_owned(), 7);
        assert_eq!(
            other_key_segments(Some(&at_echo), &line("ready> ", 7), Some(true), true)
                .map(|segments| said(&segments)),
            Some(vec!["echo hello".to_owned()])
        );
        // Another line: read by the caller, with its formatting.
        assert_eq!(
            other_key_segments(Some(&before), &line("other", 0), Some(false), true),
            None
        );
        // Text added (a paste): typing echo or output speak it.
        assert_eq!(
            other_key_segments(
                Some(&before),
                &line("ready> echo hello!", 18),
                Some(true),
                true
            ),
            Some(Vec::new())
        );
    }
}
