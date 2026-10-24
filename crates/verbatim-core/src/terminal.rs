//! What a focused terminal says (`docs/design/terminal-model.md`, section
//! 5): the terminal's outpost reads it, applies the reading rules, and sends
//! what they say as `NormalizedEvent::TerminalSpeech`. Here it is spoken:
//!
//! - The answers to keys (typing the terminal showed, a caret key's unit or
//!   the landing rule's, text the caret's line gained or lost, a row away
//!   from the caret that is a key's answer, a suggestion) are spoken at
//!   once, typing by the typing echo settings. With "speak passwords" on,
//!   typing is echoed as it is typed instead, so the terminal showing it is
//!   not echoed again.
//! - Output, in order, queued, as it arrives, one line per utterance; blank
//!   lines are not spoken, but count as lines like any other. Newer output
//!   never cancels older output still waiting.
//! - Lines are handed to speech a few at a time, each starting and ending
//!   with an index mark, and the rest wait here, so the backlog of output
//!   not yet spoken is known. Output is spoken in groups (the flood policy):
//!   the first "Lines spoken in full" lines of a burst of output are spoken
//!   whole, however fast the rest arrives. Only once a group's last line
//!   has been heard is the backlog looked at: when more lines wait than "Lines
//!   spoken in full", the older are replaced by "skipped N lines"
//!   ("skipped lines" when the outpost could not count them) and the newest
//!   "Last lines to speak" are kept (every line of output counts, a
//!   shell's prompt shown after the output included), and those make the
//!   next group; when
//!   no more wait than that, they are the next group as they are. This
//!   repeats until the output stops and everything waiting is heard, and
//!   the next output starts a burst of its own. Output shorter than the
//!   limit is never touched, however many batches it arrives in. A burst
//!   ends only once its last line has been heard: one whose first line
//!   starts playing before its second arrives is still one burst. Nothing
//!   is lost for good: every line stays reachable with the review cursor.
//! - Anything that cuts speech off (a key, a focus change, an interrupting
//!   utterance) drops the output still waiting, as a key press does in
//!   NVDA, and output read before the cut is never spoken after it. "Report
//!   new output" off (Verbatim+5) speaks no output at all.
//! - On-demand reading (`docs/design/terminal-model.md`, section 5.1): once
//!   as many lines wait as a group speaks, the outpost is told to hold
//!   ([`TextOp::TerminalHold`]), and only notes that the terminal changed.
//!   When the group's last line is handed to speech, Core asks for what is
//!   new ([`TextOp::TerminalRead`]), so the answer is ready before that line
//!   ends; the backlog is looked at once the line plays and the answer is
//!   in, never before. Once nothing waits, reading goes live again. A cut
//!   while holding tells the outpost to read to the end without speaking
//!   ([`TextOp::TerminalCancel`]); of what was read before a cut, still on
//!   its way, only the answers to keys are spoken.
//! - Every key that reaches a focused terminal is told to its outpost, with
//!   when it reached the program, since the reading rules judge by that
//!   time which reads a key's effects can be in (section 5.5): the keyboard
//!   hook's thread tells it of each key it passes itself, before passing
//!   it on, and Core of a key it held, as it passes it on
//!   ([`TextOp::TerminalKey`]).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    CaretKey, CaretMotion, Effect, FocusValidity, Message, NodeId, Phrase, QueryId, SegmentContent,
    Skipped, SpeechMark, SpeechPriority, TerminalItem, TerminalKey, TerminalKeyKind,
    TerminalSpeech, TerminalUnit, TextOp, TextReply, TextRequest, TraceId, Utterance,
    UtteranceSegment,
};

use crate::editing;
use crate::state::{ReviewText, SrState};
use crate::text;

/// How many utterances of output are handed to speech before their marks
/// are reached: two queued behind the one playing, so there is no gap
/// between lines and the backlog stays here, where it can be trimmed.
const AHEAD: usize = 2;

/// The focused terminal's output still to be spoken.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalSpeechState {
    /// The terminal this is about.
    pub(crate) node: Option<NodeId>,
    /// Output not yet handed to speech, oldest first: what is left of the
    /// group being spoken, and after it at most as many lines as either
    /// limit keeps, older ones counted as skipped ([`bound`]).
    pub(crate) waiting: VecDeque<Waiting>,
    /// How many more lines the group being spoken hands to speech before
    /// the backlog is looked at; `None` between bursts of output, when
    /// everything has been heard.
    #[serde(default)]
    pub(crate) group: Option<usize>,
    /// The opening marks of output handed to speech whose playback has not
    /// reached them yet, oldest first, with whether each is a line.
    pub(crate) ahead: VecDeque<(SpeechMark, bool)>,
    /// The closing mark of the last utterance of output handed to speech,
    /// until playback reaches it: while it is set, output is still being
    /// heard.
    #[serde(default)]
    pub(crate) sounding: Option<SpeechMark>,
    /// The trace of the newest output, which output handed to speech when
    /// a mark is reached is spoken under.
    pub(crate) trace: Option<TraceId>,
    /// Whether the outpost was told to hold ([`TextOp::TerminalHold`]).
    #[serde(default)]
    pub(crate) held: bool,
    /// Core's request for what is new ([`TextOp::TerminalRead`]), not yet
    /// answered.
    #[serde(default)]
    pub(crate) request: Option<QueryId>,
    /// The group's last line is playing, and the backlog is looked at once
    /// the request is answered.
    #[serde(default)]
    pub(crate) deciding: bool,
    /// The cancel sent to the outpost ([`TextOp::TerminalCancel`]), not yet
    /// answered.
    #[serde(default)]
    pub(crate) cancel: Option<QueryId>,
    /// Core's request for what is new that a cancel overtook, not yet
    /// answered. The outpost answers it before the cancel, from a read made
    /// before it took the cancel, so its output was read before a cut, and
    /// only its answers to keys are spoken.
    #[serde(default)]
    pub(crate) overtaken: Option<QueryId>,
    /// When speech was last cut off (milliseconds on the shared clock):
    /// output from a read that began before it was read before the cut,
    /// and is not spoken.
    #[serde(default)]
    pub(crate) cut_at_ms: u64,
}

/// One piece of output waiting to be spoken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Waiting {
    /// A line, or what changed of one.
    Line(String),
    /// Lines skipped as too much to read.
    Skipped(Skipped),
    /// The line before was longer than all the output kept waiting
    /// ([`MAX_WAITING_BYTES`]) and was cut: "line cut". Not a line.
    Cut,
}

/// The most bytes of terminal output waiting to be spoken (Dickson,
/// 2026-10-07): extremely long output is not cut, but when the bound is
/// reached the oldest waiting lines join the skipped count, and a single
/// line larger than the whole bound is cut on a grapheme boundary, saying
/// so.
pub(crate) const MAX_WAITING_BYTES: usize = 10 * 1024 * 1024;

/// `line` cut to [`MAX_WAITING_BYTES`] on a grapheme boundary, and whether
/// it was.
fn within_bound(line: &str) -> (String, bool) {
    if line.len() <= MAX_WAITING_BYTES {
        return (line.to_owned(), false);
    }
    let end = verbatim_text::graphemes(line)
        .into_iter()
        .take_while(|grapheme| grapheme.end <= MAX_WAITING_BYTES)
        .last()
        .map_or(0, |grapheme| grapheme.end);
    (line[..end].to_owned(), true)
}

/// Adds a line to `waiting`, cut and followed by [`Waiting::Cut`] when it
/// is larger than the bound.
fn push_line(waiting: &mut VecDeque<Waiting>, line: &str) {
    let (line, cut) = within_bound(line);
    waiting.push_back(Waiting::Line(line));
    if cut {
        waiting.push_back(Waiting::Cut);
    }
}

/// Whether the focus is a live terminal.
fn terminal_focus(state: &SrState) -> Option<NodeId> {
    state
        .focus
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.role == verbatim_model::Role::Terminal)
        .map(|focus| focus.snapshot.id)
}

/// Tells the focused terminal's outpost of a key of `kind` that reached it
/// at `at_ms` (milliseconds on the shared clock): nothing when the focus is
/// not a terminal. The answer is not waited for.
pub(crate) fn key(state: &mut SrState, kind: TerminalKeyKind, at_ms: u64) -> Vec<Effect> {
    let Some(node) = terminal_focus(state) else {
        return Vec::new();
    };
    let query_id = state.allocate_query_id();
    vec![Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op: TextOp::TerminalKey(TerminalKey { kind, at_ms }),
    })]
}

/// What a caret key is to the reading rules: one Verbatim knows, by the
/// unit it moves by, or any other (section 5.5).
#[must_use]
pub fn caret_key_kind(key: CaretKey) -> TerminalKeyKind {
    match key.motion {
        CaretMotion::PreviousCharacter
        | CaretMotion::NextCharacter
        | CaretMotion::StartOfLine
        | CaretMotion::EndOfLine => TerminalKeyKind::Caret(TerminalUnit::Character),
        CaretMotion::PreviousWord | CaretMotion::NextWord => {
            TerminalKeyKind::Caret(TerminalUnit::Word)
        }
        CaretMotion::PreviousLine
        | CaretMotion::NextLine
        | CaretMotion::PreviousPage
        | CaretMotion::NextPage
        | CaretMotion::PreviousParagraph
        | CaretMotion::NextParagraph
        | CaretMotion::Top
        | CaretMotion::Bottom => TerminalKeyKind::Caret(TerminalUnit::Line),
        _ => TerminalKeyKind::Other,
    }
}

/// What text typed is to the reading rules: Enter and Tab forget the
/// typing not yet shown, and so does a control character, which types
/// nothing the terminal shows as itself; anything else is typed.
#[must_use]
pub fn typed_kind(typed: &str) -> TerminalKeyKind {
    if typed.chars().any(|c| matches!(c, '\r' | '\n' | '\t')) {
        TerminalKeyKind::Forget
    } else if typed.chars().any(char::is_control) {
        TerminalKeyKind::Other
    } else {
        TerminalKeyKind::Typed(typed.to_owned())
    }
}

/// Handles what the focused terminal `node` said, read at `observed_at_ms`:
/// the answers to keys spoken at once, then the output queued by the flood
/// policy and the oldest handed to speech.
pub(crate) fn speech(
    state: &mut SrState,
    trace_id: TraceId,
    observed_at_ms: u64,
    node: NodeId,
    speech: &TerminalSpeech,
) -> Vec<Effect> {
    if !state.focus_matches(node) {
        return Vec::new();
    }
    focus_moved(state, node);
    state.terminal.node = Some(node);
    // Read before speech was last cut off, or before the outpost took the
    // cancel: the output pending then is dropped, as NVDA drops it.
    let before_cut = state.terminal.cancel.is_some()
        || (observed_at_ms != 0 && observed_at_ms < state.terminal.cut_at_ms);
    take(state, trace_id, speech, before_cut)
}

/// The segments of a key's answer.
fn answer_segments(item: &TerminalItem) -> Option<Vec<UtteranceSegment>> {
    Some(match item {
        TerminalItem::Caret { unit, text } => match unit {
            TerminalUnit::Character => {
                text::character_segments(Some(text.as_str()).filter(|text| !text.is_empty()), None)
            }
            TerminalUnit::Word => text::word_segments(text, None),
            TerminalUnit::Line => text::text_segments(text, None),
        },
        TerminalItem::Lost(text) => text::word_segments(text, None),
        TerminalItem::Gained(text)
        | TerminalItem::Answer(text)
        | TerminalItem::Suggestion(text) => text::text_segments(text, None),
        TerminalItem::Output(_) | TerminalItem::Skipped(_) | TerminalItem::Echo(_) => return None,
    })
}

/// Takes in what the focused terminal said: speaks the answers to keys,
/// then, unless the output is `before_cut` (read before a cut), queues it
/// by the flood policy and hands the oldest to speech.
fn take(
    state: &mut SrState,
    trace_id: TraceId,
    speech: &TerminalSpeech,
    before_cut: bool,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    for item in &speech.items {
        match item {
            // With passwords spoken, typing was echoed as it was typed.
            TerminalItem::Echo(typed) if !state.settings.speak_terminal_passwords => {
                effects.extend(editing::echo(state, trace_id, typed));
            }
            item => {
                if let Some(segments) = answer_segments(item) {
                    effects.push(editing::speak(trace_id, segments));
                }
            }
        }
    }
    if speech.followed
        && let Some(navigator) = state.navigator.as_mut()
        && Some(navigator.object.id) == state.terminal.node
    {
        // The review cursor followed the caret: the next review command
        // reads at the caret as the outpost finds it.
        navigator.text = ReviewText::Caret;
    }
    let output = speech.items.iter().any(TerminalItem::is_output);
    if !state.settings.report_terminal_output || before_cut || !output {
        return effects;
    }
    state.terminal.trace = Some(trace_id);
    if state.terminal.group.is_none() {
        state.terminal.group = Some(state.settings.full_lines());
    }
    queue(state, speech);
    bound(state);
    effects.extend(pump(state, trace_id));
    effects.extend(hold_if_full(state));
    effects
}

/// The number of lines waiting.
fn waiting_lines(waiting: &VecDeque<Waiting>) -> usize {
    waiting
        .iter()
        .filter(|item| matches!(item, Waiting::Line(_)))
        .count()
}

/// A text request about the focused terminal.
fn request(state: &mut SrState, op: TextOp) -> Option<(QueryId, Effect)> {
    let node = state.terminal.node?;
    let query_id = state.allocate_query_id();
    Some((
        query_id,
        Effect::Text(TextRequest {
            query_id,
            node_id: node,
            op,
        }),
    ))
}

/// Tells the outpost to hold once as many lines wait, or wait in speech to
/// start playing, as a group speaks.
fn hold_if_full(state: &mut SrState) -> Vec<Effect> {
    if state.terminal.held
        || state.terminal.cancel.is_some()
        || waiting_lines(&state.terminal.waiting)
            + state
                .terminal
                .ahead
                .iter()
                .filter(|(_, line)| *line)
                .count()
            < state.settings.full_lines()
    {
        return Vec::new();
    }
    let Some((_, effect)) = request(state, TextOp::TerminalHold) else {
        return Vec::new();
    };
    state.terminal.held = true;
    vec![effect]
}

/// Asks the outpost for what is new, then to hold (`hold`) or read each
/// change at once again.
fn ask(state: &mut SrState, hold: bool) -> Vec<Effect> {
    if state.terminal.request.is_some() || state.terminal.cancel.is_some() {
        return Vec::new();
    }
    let Some((query_id, effect)) = request(state, TextOp::TerminalRead { hold }) else {
        return Vec::new();
    };
    state.terminal.request = Some(query_id);
    state.terminal.held = hold;
    vec![effect]
}

/// Whether `query_id` is a terminal request Core is waiting on.
pub(crate) fn is_pending(state: &SrState, query_id: QueryId) -> bool {
    state.terminal.request == Some(query_id)
        || state.terminal.cancel == Some(query_id)
        || state.terminal.overtaken == Some(query_id)
}

/// Handles the outpost's answer to a terminal request: what was new when
/// Core asked, taken in, and the backlog looked at when the group's last
/// line is already playing; or the cancel's read, or the answer to a
/// request the cancel overtook, of which only the answers to keys are
/// spoken.
pub(crate) fn reply(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    reply: TextReply,
) -> Vec<Effect> {
    let output = match reply {
        TextReply::Terminal(speech) => *speech,
        _ => TerminalSpeech::default(),
    };
    if state.terminal.cancel == Some(query_id) {
        state.terminal.cancel = None;
        return take(state, trace_id, &output, true);
    }
    if state.terminal.overtaken == Some(query_id) {
        state.terminal.overtaken = None;
        return take(state, trace_id, &output, true);
    }
    state.terminal.request = None;
    let mut effects = take(state, trace_id, &output, false);
    if std::mem::take(&mut state.terminal.deciding) {
        effects.extend(decide_and_pump(state));
    }
    effects
}

/// Adds the output to the waiting queue, in the order it is spoken.
fn queue(state: &mut SrState, speech: &TerminalSpeech) {
    let terminal = &mut state.terminal;
    // Every line counts, blank ones included (Dickson, 2026-10-07, and the
    // coherence review of 2026-10-09): a blank line waits like any other
    // and is counted when skipped, but is never spoken ([`pump`]).
    for item in &speech.items {
        match item {
            TerminalItem::Output(line) => push_line(&mut terminal.waiting, line),
            TerminalItem::Skipped(skipped) => push_skipped(&mut terminal.waiting, *skipped),
            _ => {}
        }
    }
}

/// Adds a skipped count after the waiting output, merged with one that is
/// last already.
fn push_skipped(waiting: &mut VecDeque<Waiting>, skipped: Skipped) {
    if let Some(Waiting::Skipped(last)) = waiting.back_mut() {
        *last = last.plus(skipped);
    } else {
        waiting.push_back(Waiting::Skipped(skipped));
    }
}

/// Where the backlog starts in `waiting`: past the lines the group being
/// spoken still hands to speech, `group_left` of them, and anything among
/// them.
fn backlog_start(waiting: &VecDeque<Waiting>, group_left: usize) -> usize {
    if group_left == 0 {
        return 0;
    }
    let mut lines = 0;
    for (index, item) in waiting.iter().enumerate() {
        if matches!(item, Waiting::Line(_)) {
            lines += 1;
            if lines == group_left {
                return index + 1;
            }
        }
    }
    waiting.len()
}

/// Replaces everything in `waiting` from `start` on but its newest `keep`
/// lines with one skipped count, which then comes first among them.
fn skip_all_but(waiting: &mut VecDeque<Waiting>, start: usize, keep: usize) {
    let lines = waiting
        .iter()
        .skip(start)
        .filter(|item| matches!(item, Waiting::Line(_)))
        .count();
    let mut dropping = lines.saturating_sub(keep);
    let mut skipped: Option<Skipped> = None;
    let index = start;
    while index < waiting.len()
        && (dropping > 0 || matches!(waiting.get(index), Some(Waiting::Skipped(_))))
    {
        match waiting.remove(index) {
            Some(Waiting::Line(_)) => {
                skipped = Some(skipped.map_or(Skipped::Count(1), |s| s.plus(Skipped::Count(1))));
                dropping -= 1;
            }
            Some(Waiting::Skipped(count)) => {
                skipped = Some(skipped.map_or(count, |s| s.plus(count)));
            }
            // A dropped line's note that it was cut goes with it.
            Some(Waiting::Cut) => {}
            None => break,
        }
    }
    if let Some(skipped) = skipped {
        waiting.insert(start, Waiting::Skipped(skipped));
    }
}

/// Keeps the backlog behind the group being spoken bounded: at most as
/// many lines as either limit could keep, older ones counted as skipped,
/// and everything waiting at most [`MAX_WAITING_BYTES`], the oldest lines
/// counted as skipped beyond it. Which of them is spoken is decided once the
/// group is heard ([`decide`]).
fn bound(state: &mut SrState) {
    let keep = state.settings.full_lines().max(state.settings.last_lines());
    let terminal = &mut state.terminal;
    let start = backlog_start(&terminal.waiting, terminal.group.unwrap_or(0));
    let backlog = terminal
        .waiting
        .iter()
        .skip(start)
        .filter(|item| matches!(item, Waiting::Line(_)))
        .count();
    if backlog > keep {
        skip_all_but(&mut terminal.waiting, start, keep);
    }
    // Then by bytes: the oldest waiting lines join the skipped count until
    // what waits fits the bound, the backlog's first, then the group's.
    let mut bytes: usize = terminal
        .waiting
        .iter()
        .map(|item| match item {
            Waiting::Line(line) => line.len(),
            Waiting::Skipped(_) | Waiting::Cut => 0,
        })
        .sum();
    while bytes > MAX_WAITING_BYTES {
        let start = backlog_start(&terminal.waiting, terminal.group.unwrap_or(0));
        let index = terminal
            .waiting
            .iter()
            .enumerate()
            .skip(start)
            .chain(terminal.waiting.iter().enumerate().take(start))
            .find(|(_, item)| matches!(item, Waiting::Line(_)))
            .map(|(index, _)| index);
        let Some(index) = index else {
            break;
        };
        if index < start
            && let Some(left) = terminal.group.as_mut()
        {
            *left = left.saturating_sub(1);
        }
        if let Some(Waiting::Line(line)) = terminal.waiting.get(index) {
            bytes -= line.len();
        }
        let merged =
            index > 0 && matches!(terminal.waiting.get(index - 1), Some(Waiting::Skipped(_)));
        terminal.waiting.remove(index);
        if matches!(terminal.waiting.get(index), Some(Waiting::Cut)) {
            terminal.waiting.remove(index);
        }
        if merged {
            if let Some(Waiting::Skipped(count)) = terminal.waiting.get_mut(index - 1) {
                *count = count.plus(Skipped::Count(1));
            }
        } else {
            terminal
                .waiting
                .insert(index, Waiting::Skipped(Skipped::Count(1)));
        }
    }
}

/// The flood policy, once the group being spoken has handed its last line
/// to speech and that line is playing: when more lines wait than "Lines
/// spoken in full" (those skipped already included), everything but the
/// newest "Last lines to speak" becomes one skipped count. What waits then
/// is the next group; with nothing waiting, the burst is over.
fn decide(state: &mut SrState) {
    let full = state.settings.full_lines();
    let last = state.settings.last_lines();
    let terminal = &mut state.terminal;
    if terminal.waiting.is_empty() {
        terminal.group = None;
        return;
    }
    let mut lines = 0usize;
    let mut skipped = Skipped::Count(0);
    for item in &terminal.waiting {
        match item {
            Waiting::Line(_) => lines += 1,
            Waiting::Skipped(count) => skipped = skipped.plus(*count),
            Waiting::Cut => {}
        }
    }
    let waiting = match skipped {
        Skipped::Count(count) => lines.saturating_add(usize::try_from(count).unwrap_or(usize::MAX)),
        Skipped::Uncounted | Skipped::MoreThan(_) => usize::MAX,
    };
    if waiting > full {
        skip_all_but(&mut terminal.waiting, 0, last);
        lines = lines.min(last);
    }
    terminal.group = Some(full.max(lines));
}

/// Hands waiting output to speech until enough is ahead of playback, as
/// far as the group being spoken goes. Handing the group's last line to
/// speech while the outpost holds asks it for what is new.
fn pump(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let mut effects = Vec::new();
    while state.terminal.ahead.len() < AHEAD {
        if state.terminal.group == Some(0) {
            break;
        }
        let Some(item) = state.terminal.waiting.pop_front() else {
            break;
        };
        if matches!(item, Waiting::Line(_))
            && let Some(left) = state.terminal.group.as_mut()
        {
            *left = left.saturating_sub(1);
            if *left == 0 && state.terminal.held {
                effects.extend(ask(state, true));
            }
        }
        // A blank line counts, but is not spoken.
        if matches!(&item, Waiting::Line(line) if text::is_blank(line)) {
            continue;
        }
        let mark = state.allocate_mark();
        let (content, line) = match item {
            Waiting::Line(line) => (text::text_segments(&line, None), true),
            Waiting::Skipped(Skipped::Count(count)) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedLines(count),
                ))],
                false,
            ),
            Waiting::Skipped(Skipped::Uncounted) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedUncountedLines,
                ))],
                false,
            ),
            Waiting::Skipped(Skipped::MoreThan(count)) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedMoreThanLines(count),
                ))],
                false,
            ),
            Waiting::Cut => (
                vec![UtteranceSegment::new(SegmentContent::Message(
                    Message::TerminalLineCut,
                ))],
                false,
            ),
        };
        // A mark at the end too: playback reaching it says the utterance
        // has been heard.
        let end = state.allocate_mark();
        let mut segments = vec![UtteranceSegment::new(SegmentContent::Mark(mark))];
        segments.extend(content);
        segments.push(UtteranceSegment::new(SegmentContent::Mark(end)));
        let source = state
            .focus
            .as_ref()
            .map(|focus| crate::reduce::source_of(&focus.snapshot));
        effects.push(Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source,
            say_all: false,
            // Dropped once the focus leaves the terminal.
            validity: state.terminal.node.map(|node| FocusValidity {
                node,
                had_focus: true,
            }),
        }));
        state.terminal.ahead.push_back((mark, line));
        state.terminal.sounding = Some(end);
    }
    // Blank lines handed on without speech can end a group, or everything
    // waiting, with nothing sounding whose end would look at the backlog.
    let terminal = &mut state.terminal;
    if terminal.sounding.is_none()
        && terminal.ahead.is_empty()
        && terminal.group.is_some()
        && (terminal.group == Some(0) || terminal.waiting.is_empty())
    {
        if terminal.request.is_some() {
            terminal.deciding = true;
        } else {
            effects.extend(decide_and_pump(state));
        }
    }
    effects
}

/// Handles playback reaching an index mark. An utterance's opening mark:
/// it has started playing, and more is handed to speech, under the newest
/// output's trace. The closing mark of the last utterance handed to speech:
/// it has been heard, and when it ended the group, or everything waiting,
/// the backlog is looked at, once what Core asked for is in. A burst ends
/// only once its last line has been heard, never while output that has
/// already been written is still on its way (a burst's first line can
/// start playing before its second line arrives).
pub(crate) fn mark_reached(state: &mut SrState, mark: SpeechMark) -> Vec<Effect> {
    let terminal = &mut state.terminal;
    let Some(trace_id) = terminal.trace else {
        return Vec::new();
    };
    if terminal.ahead.iter().any(|(ahead, _)| *ahead == mark) {
        while terminal
            .ahead
            .front()
            .is_some_and(|(ahead, _)| *ahead <= mark)
        {
            terminal.ahead.pop_front();
        }
        return pump(state, trace_id);
    }
    if terminal.sounding != Some(mark) {
        return Vec::new();
    }
    terminal.sounding = None;
    if terminal.ahead.is_empty()
        && terminal.group.is_some()
        && (terminal.group == Some(0) || terminal.waiting.is_empty())
    {
        if terminal.request.is_some() {
            terminal.deciding = true;
            return Vec::new();
        }
        return decide_and_pump(state);
    }
    pump(state, trace_id)
}

/// The flood policy's decision ([`decide`]), then the next group handed to
/// speech; with nothing left waiting, reading goes live again.
fn decide_and_pump(state: &mut SrState) -> Vec<Effect> {
    let Some(trace_id) = state.terminal.trace else {
        return Vec::new();
    };
    decide(state);
    let mut effects = pump(state, trace_id);
    if state.terminal.waiting.is_empty() && state.terminal.held {
        effects.extend(ask(state, false));
    }
    effects
}

/// Speech was cut off (at `at_ms`, when known): output waiting or handed to
/// speech is dropped, and an outpost holding the terminal's changes reads to
/// the end without speaking, so nothing from before the cut is spoken.
pub(crate) fn cut(state: &mut SrState, at_ms: u64) -> Vec<Effect> {
    let terminal = &mut state.terminal;
    terminal.waiting.clear();
    terminal.ahead.clear();
    terminal.sounding = None;
    terminal.group = None;
    terminal.deciding = false;
    terminal.cut_at_ms = terminal.cut_at_ms.max(at_ms);
    cancel(state)
}

/// Tells an outpost that holds, or that Core asked, to read to the end
/// without speaking, and to read each change at once again. A request
/// still unanswered is overtaken: its answer still comes, ahead of the
/// cancel's, and may hold typing the terminal showed after the cut.
fn cancel(state: &mut SrState) -> Vec<Effect> {
    if !state.terminal.held && state.terminal.request.is_none() {
        return Vec::new();
    }
    let Some((query_id, effect)) = request(state, TextOp::TerminalCancel) else {
        return Vec::new();
    };
    state.terminal.held = false;
    state.terminal.overtaken = state.terminal.request.take();
    state.terminal.cancel = Some(query_id);
    vec![effect]
}

/// The focus moved to `node`: another node's output is forgotten.
pub(crate) fn focus_moved(state: &mut SrState, node: NodeId) {
    if state.terminal.node.is_some_and(|terminal| terminal != node) {
        state.terminal = TerminalSpeechState::default();
    }
}

/// Whether an effect cuts speech off, dropping output handed to speech.
pub(crate) fn cuts_speech(effect: &Effect) -> bool {
    match effect {
        Effect::StopSpeech => true,
        Effect::Speak(utterance) => utterance.priority == SpeechPriority::Interrupt,
        Effect::ForKey(_, effect) => cuts_speech(effect),
        _ => false,
    }
}

/// Drops terminal output still waiting to be spoken, as turning "Report new
/// output" off does, whether by its key or from the settings.
pub(crate) fn drop_waiting(state: &mut SrState) -> Vec<Effect> {
    state.terminal.waiting.clear();
    if state.terminal.ahead.is_empty() {
        state.terminal.group = None;
    }
    state.terminal.deciding = false;
    cancel(state)
}

/// Verbatim+5: toggles "Report new output", says its new value, and
/// reports the settings for the shell to save. Turning it off drops the
/// output still waiting.
pub(crate) fn toggle(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let on = !state.settings.report_terminal_output;
    state.settings.report_terminal_output = on;
    let mut effects = if on { Vec::new() } else { drop_waiting(state) };
    effects.extend([
        editing::speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Message(if on {
                Message::ReportNewOutputOn
            } else {
                Message::ReportNewOutputOff
            }))],
        ),
        Effect::SettingsChanged(state.settings),
    ]);
    effects
}

#[cfg(test)]
mod tests {
    use super::{caret_key_kind, typed_kind};
    use verbatim_model::{CaretKey, CaretMotion, TerminalKeyKind, TerminalUnit};

    #[test]
    fn keys_are_told_to_the_reading_rules_by_their_unit() {
        let key = |motion| CaretKey {
            motion,
            select: false,
        };
        assert_eq!(
            caret_key_kind(key(CaretMotion::NextCharacter)),
            TerminalKeyKind::Caret(TerminalUnit::Character)
        );
        assert_eq!(
            caret_key_kind(key(CaretMotion::PreviousWord)),
            TerminalKeyKind::Caret(TerminalUnit::Word)
        );
        assert_eq!(
            caret_key_kind(key(CaretMotion::PreviousLine)),
            TerminalKeyKind::Caret(TerminalUnit::Line)
        );
        assert_eq!(
            caret_key_kind(key(CaretMotion::Backspace)),
            TerminalKeyKind::Other
        );
        assert_eq!(typed_kind("a"), TerminalKeyKind::Typed("a".to_owned()));
        assert_eq!(typed_kind("\r"), TerminalKeyKind::Forget);
        assert_eq!(typed_kind("\t"), TerminalKeyKind::Forget);
        assert_eq!(typed_kind("\u{1}"), TerminalKeyKind::Other);
    }
}
