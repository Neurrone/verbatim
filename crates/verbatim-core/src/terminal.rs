//! Terminal output (milestone M4 item 9; `phase6-design.md`, "Terminal
//! output: notifications or diffing" and "The flood policy, reconsidered").
//!
//! The focused terminal's outpost diffs its text and sends what is new as
//! `NormalizedEvent::TerminalOutput`. Here it is spoken:
//!
//! - In order, queued, as it arrives, one line per utterance; blank lines are
//!   dropped. Newer output never cancels older output still waiting.
//! - Lines are handed to speech a few at a time, each starting with an index
//!   mark, and the rest wait here, so the backlog of output not yet spoken
//!   is known. When more lines wait than "Lines spoken in full", the oldest
//!   are replaced by "skipped N lines" ("skipped lines" when the outpost
//!   could not count them) and the newest "Last lines to speak" are kept.
//!   Output shorter than the limit is never touched, however many batches
//!   it arrives in. Nothing is lost for good: every line stays reachable
//!   with the review cursor.
//! - The last line read, changed in place (a prompt that grew, a progress
//!   bar rewritten), speaks what changed; while an earlier version of that
//!   line is still waiting, the newer one takes its place, so a line
//!   rewritten quickly is spoken once.
//! - Typing held for the terminal (the password rule, in `editing`) is
//!   echoed when the terminal shows it at the end of the line, and that
//!   text is not spoken again as output. White space the line may have had
//!   already (a prompt's trailing space, which the outpost cannot tell from
//!   padding) is matched only as far as the typing starts with it. When the terminal shows something
//!   else (a password prompt's asterisks), what was held is never spoken.
//! - Anything that cuts speech off (a key, a focus change, an interrupting
//!   utterance) drops the output still waiting, as a key press does in
//!   NVDA. "Report new output" off (Verbatim+5) speaks no output at all.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    Effect, FocusValidity, LineChange, Message, NodeId, Phrase, SegmentContent, Skipped,
    SpeechMark, SpeechPriority, TerminalOutput, TraceId, Utterance, UtteranceSegment,
};

use crate::editing::{self, MAX_HELD_TYPING};
use crate::state::SrState;
use crate::text;

/// How many utterances of output are handed to speech before their marks
/// are reached: one playing and one ready behind it, so there is no gap
/// between lines and the backlog stays here, where it can be trimmed.
const AHEAD: usize = 2;

/// The focused terminal's output still to be spoken, and its typing that
/// was echoed before the terminal showed it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalSpeech {
    /// The terminal this is about.
    pub(crate) node: Option<NodeId>,
    /// Output not yet handed to speech, oldest first; at most
    /// `MAX_TERMINAL_LINES` lines and one skipped count.
    pub(crate) waiting: VecDeque<Waiting>,
    /// The marks of output handed to speech whose playback has not reached
    /// them yet, oldest first, with whether each is a line.
    pub(crate) ahead: VecDeque<(SpeechMark, bool)>,
    /// Whether the newest waiting item is the terminal's last line read, so
    /// a change to that line replaces it.
    pub(crate) last_line_waiting: bool,
    /// Characters typed and echoed at once ("speak passwords" on) that the
    /// terminal has not shown yet; when it shows them, they are not spoken
    /// again as output. At most `MAX_HELD_TYPING` bytes.
    pub(crate) echoed_typing: String,
    /// The trace of the newest output, which output handed to speech when
    /// a mark is reached is spoken under.
    pub(crate) trace: Option<TraceId>,
}

/// One piece of output waiting to be spoken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Waiting {
    /// A line, or what changed of one.
    Line(String),
    /// Lines skipped as too much to read.
    Skipped(Skipped),
}

/// Handles new output in `node`, the focus: echoes typing it shows, then
/// queues the rest by the flood policy and hands the oldest to speech.
pub(crate) fn output(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    output: &TerminalOutput,
) -> Vec<Effect> {
    if !state.focus_matches(node) {
        return Vec::new();
    }
    focus_moved(state, node);
    state.terminal.node = Some(node);
    let mut effects = Vec::new();
    let mut changed = output.changed.clone();
    if let Some(change) = changed.as_mut() {
        effects.extend(typing_shown(state, trace_id, change));
    }
    if !state.settings.report_terminal_output {
        return effects;
    }
    state.terminal.trace = Some(trace_id);
    queue(state, changed.as_ref(), output);
    trim(state);
    effects.extend(pump(state, trace_id));
    effects
}

/// The length in bytes of the longest common prefix of `a` and `b`, at a
/// character boundary of both.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or_else(|| a.len().min(b.len()), |((index, _), _)| index)
}

/// Echoes the typing the terminal now shows at the end of its line, and
/// takes it out of `change`, so it is not spoken again as output. Typing
/// held while the line changed some other way is dropped unspoken: the
/// terminal did not show it (a password prompt that prints asterisks). A
/// line rewritten while typing was held or echoed is taken as the typing
/// showing: the typing is echoed and the rewrite is not spoken.
fn typing_shown(state: &mut SrState, trace_id: TraceId, change: &mut LineChange) -> Vec<Effect> {
    // Control characters (Backspace, Tab) never show as themselves.
    let visible = state.held_typing.trim_start_matches(char::is_control).len();
    let start = state.held_typing.len() - visible;
    state.held_typing.drain(..start);
    if !change.appended {
        let typing = !state.held_typing.is_empty() || !state.terminal.echoed_typing.is_empty();
        state.terminal.echoed_typing.clear();
        if typing {
            change.text.clear();
        }
        let held = std::mem::take(&mut state.held_typing);
        return editing::echo(state, trace_id, &held);
    }
    let typing = if state.held_typing.is_empty() {
        &state.terminal.echoed_typing
    } else {
        &state.held_typing
    };
    let already = already_there(change, typing);
    change.text.drain(..already);
    let echoed = common_prefix(&state.terminal.echoed_typing, &change.text);
    if echoed > 0 {
        state.terminal.echoed_typing.drain(..echoed);
        change.text.drain(..echoed);
    } else if !change.text.is_empty() {
        state.terminal.echoed_typing.clear();
    }
    let shown = common_prefix(&state.held_typing, &change.text);
    let held: String = state.held_typing.drain(..shown).collect();
    change.text.drain(..shown);
    if !change.text.is_empty() {
        // The terminal showed something other than what was typed.
        state.held_typing.clear();
    }
    editing::echo(state, trace_id, &held)
}

/// The length in bytes of the white space at the start of a grown line's
/// `change` that was on the line already, as far as `typing` tells: the
/// uncertain white space (a prompt's trailing space, which the outpost
/// cannot tell from padding) beyond what the typing itself starts with.
/// Without typing, nothing is taken away.
fn already_there(change: &LineChange, typing: &str) -> usize {
    if typing.is_empty() {
        return 0;
    }
    let typed_space = typing.len() - typing.trim_start().len();
    let uncertain = change.uncertain.min(change.text.len());
    let mut already = 0;
    for character in change.text.chars() {
        if uncertain.saturating_sub(already) <= typed_space || !character.is_whitespace() {
            break;
        }
        already += character.len_utf8();
    }
    already
}

/// Notes characters typed into a terminal and echoed at once, so the
/// terminal showing them is not spoken again; Enter forgets them.
pub(crate) fn typed(state: &mut SrState, typed: &str) {
    let echoed = &mut state.terminal.echoed_typing;
    if typed.chars().any(|c| c == '\r' || c == '\n') {
        echoed.clear();
    } else if echoed.len() + typed.len() <= MAX_HELD_TYPING {
        echoed.push_str(typed.trim_start_matches(char::is_control));
    }
}

/// Adds the output to the waiting queue, in the order it is spoken.
fn queue(state: &mut SrState, changed: Option<&LineChange>, output: &TerminalOutput) {
    let terminal = &mut state.terminal;
    if let Some(change) = changed.filter(|change| !text::is_blank(&change.text)) {
        if terminal.last_line_waiting
            && let Some(Waiting::Line(waiting)) = terminal.waiting.back_mut()
        {
            // The earlier version was never spoken: the whole line now
            // takes its place.
            waiting.clone_from(&change.line);
        } else {
            terminal
                .waiting
                .push_back(Waiting::Line(change.text.clone()));
        }
        terminal.last_line_waiting = true;
    }
    if let Some(skipped) = output.skipped {
        push_skipped(&mut terminal.waiting, skipped);
        terminal.last_line_waiting = false;
    }
    for line in &output.lines {
        let blank = text::is_blank(line);
        if !blank {
            terminal.waiting.push_back(Waiting::Line(line.clone()));
        }
        terminal.last_line_waiting = !blank;
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

/// The flood policy: with more lines waiting than "Lines spoken in full",
/// everything before the newest "Last lines to speak" becomes one skipped
/// count.
fn trim(state: &mut SrState) {
    let full = state.settings.full_lines();
    let last = state.settings.last_lines();
    let terminal = &mut state.terminal;
    let lines = |queue: &VecDeque<Waiting>| {
        queue
            .iter()
            .filter(|item| matches!(item, Waiting::Line(_)))
            .count()
    };
    let ahead = terminal.ahead.iter().filter(|(_, line)| *line).count();
    let waiting = lines(&terminal.waiting);
    if waiting + ahead <= full || waiting <= last {
        return;
    }
    let mut dropped = waiting - last;
    let mut skipped = Skipped::Count(0);
    while dropped > 0 || matches!(terminal.waiting.front(), Some(Waiting::Skipped(_))) {
        match terminal.waiting.pop_front() {
            Some(Waiting::Line(_)) => {
                skipped = skipped.plus(Skipped::Count(1));
                dropped -= 1;
            }
            Some(Waiting::Skipped(count)) => skipped = skipped.plus(count),
            None => break,
        }
    }
    terminal.waiting.push_front(Waiting::Skipped(skipped));
}

/// Hands waiting output to speech until enough is ahead of playback.
fn pump(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let mut effects = Vec::new();
    while state.terminal.ahead.len() < AHEAD {
        let Some(item) = state.terminal.waiting.pop_front() else {
            break;
        };
        if state.terminal.waiting.is_empty() {
            state.terminal.last_line_waiting = false;
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
        };
        let mut segments = vec![UtteranceSegment::new(SegmentContent::Mark(mark))];
        segments.extend(content);
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
    }
    effects
}

/// Handles playback reaching an index mark: output up to it has started
/// playing, and more is handed to speech, under the newest output's trace.
pub(crate) fn mark_reached(state: &mut SrState, mark: SpeechMark) -> Vec<Effect> {
    let terminal = &mut state.terminal;
    let Some(trace_id) = terminal.trace else {
        return Vec::new();
    };
    if !terminal.ahead.iter().any(|(ahead, _)| *ahead == mark) {
        return Vec::new();
    }
    while terminal
        .ahead
        .front()
        .is_some_and(|(ahead, _)| *ahead <= mark)
    {
        terminal.ahead.pop_front();
    }
    pump(state, trace_id)
}

/// Speech was cut off: output waiting or handed to speech is dropped.
pub(crate) fn cut(state: &mut SrState) {
    let terminal = &mut state.terminal;
    terminal.waiting.clear();
    terminal.ahead.clear();
    terminal.last_line_waiting = false;
}

/// The focus moved to `node`: another node's output is forgotten.
pub(crate) fn focus_moved(state: &mut SrState, node: NodeId) {
    if state.terminal.node.is_some_and(|terminal| terminal != node) {
        state.terminal = TerminalSpeech::default();
    }
}

/// Whether an effect cuts speech off, dropping output handed to speech.
pub(crate) fn cuts_speech(effect: &Effect) -> bool {
    match effect {
        Effect::StopSpeech => true,
        Effect::Speak(utterance) => utterance.priority == SpeechPriority::Interrupt,
        _ => false,
    }
}

/// Drops terminal output still waiting to be spoken, as turning "Report new
/// output" off does, whether by its key or from the settings.
pub(crate) fn drop_waiting(state: &mut SrState) {
    state.terminal.waiting.clear();
    state.terminal.last_line_waiting = false;
}

/// Verbatim+5: toggles "Report new output", says its new value, and
/// reports the settings for the shell to save. Turning it off drops the
/// output still waiting.
pub(crate) fn toggle(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let on = !state.settings.report_terminal_output;
    state.settings.report_terminal_output = on;
    if !on {
        drop_waiting(state);
    }
    vec![
        editing::speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Message(if on {
                Message::ReportNewOutputOn
            } else {
                Message::ReportNewOutputOff
            }))],
        ),
        Effect::SettingsChanged(state.settings),
    ]
}

#[cfg(test)]
mod tests {
    use super::common_prefix;

    #[test]
    fn the_common_prefix_ends_at_a_character_boundary() {
        assert_eq!(common_prefix("abc", "abd"), 2);
        assert_eq!(common_prefix("ab", "abc"), 2);
        assert_eq!(common_prefix("", "x"), 0);
        assert_eq!(common_prefix("é1", "é2"), "é".len());
    }
}
