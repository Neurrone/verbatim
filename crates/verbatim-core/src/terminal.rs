//! What a focused terminal says (`docs/design/terminal-model.md`, "Core's
//! flood policy"): the terminal's outpost reads it, Core's terminal model
//! (`crate::terminal_model`) gives each read to the reading rules, and what
//! they say is spoken here:
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
//! - On-demand reading (`docs/design/terminal-model.md`, "When to read"): once
//!   as many lines wait as a group speaks, the outpost is told to hold
//!   ([`TextOp::TerminalHold`]), and only notes that the terminal changed.
//!   When the group's last line is handed to speech, Core asks for what is
//!   new ([`TextOp::TerminalRead`]), so the answer is ready before that line
//!   ends; the backlog is looked at once the line plays and the answer is
//!   in, never before. Once nothing waits, reading goes live again. A cut
//!   while holding tells the outpost to read to the end without speaking
//!   ([`TextOp::TerminalCancel`]); of what was read before a cut, still on
//!   its way, only the answers to keys are spoken.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    CaretKey, CaretMotion, Effect, FocusValidity, Message, NodeId, Phrase, QueryId, SegmentContent,
    Skipped, SpeechMark, SpeechPriority, TerminalItem, TerminalKeyKind, TerminalUnit, TextOp,
    TextReply, TextRequest, TraceId, Utterance, UtteranceSegment,
};

use verbatim_text::terminal_rows::Text;

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
    /// A line, or what changed of one, and what a skipped count makes of
    /// it.
    Line(String, LineKind),
    /// Lines skipped as too much to read.
    Skipped(Skipped),
    /// The line before was longer than all the output kept waiting
    /// ([`MAX_WAITING_BYTES`]) and was cut: "line cut". Not a line.
    Cut,
}

/// What a waiting line is to a skipped count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LineKind {
    /// A terminal line, counted once, as `Counted` says.
    Line(Counted),
    /// What a growing line gained ([`TerminalItem::Growth`]): it belongs to
    /// the line before it, with which it counts and is dropped as one.
    Growth,
}

/// How a skipped count that takes a line counts it. A lost read's lines
/// keep the read's kind, so that any count that skips one of them says so
/// ("skipped about N lines", "skipped more than N lines"), however many
/// reads later the skip comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Counted {
    /// Counted exactly.
    Exactly,
    /// Read after a lost read that found the last read's rows by their
    /// text, so counted from text ([`Skipped::About`]).
    About,
    /// Read after a lost read that found none of the last read's rows: the
    /// lines a full history held, after lines that went by unread
    /// ([`Skipped::MoreThan`]).
    MoreThan,
}

/// `skipped`, the count so far of lines being skipped, with one more line
/// that counts as `counted`. After a "more than" count every line adds to
/// it; a line past a full history makes the count "more than", standing
/// for what went before it, as [`Skipped::plus`] does; a line counted from
/// text makes it "about".
fn with_line(skipped: Option<Skipped>, counted: Counted) -> Skipped {
    match (skipped, counted) {
        (Some(more @ Skipped::MoreThan(_)), _) => more.plus(Skipped::Count(1)),
        (skipped, Counted::MoreThan) => skipped
            .unwrap_or(Skipped::Count(0))
            .plus(Skipped::MoreThan(1)),
        (skipped, Counted::About) => {
            skipped.map_or(Skipped::About(1), |skipped| skipped.plus(Skipped::About(1)))
        }
        (skipped, Counted::Exactly) => {
            skipped.map_or(Skipped::Count(1), |skipped| skipped.plus(Skipped::Count(1)))
        }
    }
}

/// The most bytes of terminal output waiting to be spoken (Dickson,
/// 2026-10-07): extremely long output is not cut, but when the bound is
/// reached the oldest waiting lines join the skipped count, and a single
/// line larger than the whole bound is cut on a grapheme boundary, saying
/// so.
pub(crate) const MAX_WAITING_BYTES: usize = 10 * 1024 * 1024;

/// `line` cut to [`MAX_WAITING_BYTES`] on a grapheme boundary, and whether
/// it was.
fn within_bound(mut line: String) -> (String, bool) {
    if line.len() <= MAX_WAITING_BYTES {
        return (line, false);
    }
    let end = verbatim_text::graphemes(&line)
        .into_iter()
        .take_while(|grapheme| grapheme.end <= MAX_WAITING_BYTES)
        .last()
        .map_or(0, |grapheme| grapheme.end);
    line.truncate(end);
    (line, true)
}

/// What a terminal's reading rules said, as the flood policy takes it in
/// ([`speech`]): new lines keep the rules' text, shared with the read, until
/// they wait, so a line the backlog's bound skips is never copied
/// ([`queue`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Said {
    /// What to say, in the order it is spoken.
    pub(crate) items: Vec<SaidItem>,
    /// Whether the review cursor followed the caret: a key moved the caret
    /// within its window with "review follows caret" on, so the next review
    /// command reads at the caret.
    pub(crate) followed: bool,
}

/// One thing a terminal said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaidItem {
    /// Anything but a new line.
    Item(TerminalItem),
    /// A new line of output, as [`TerminalItem::Output`] is, its text the
    /// reading rules'.
    Line(Text),
}

impl SaidItem {
    /// Whether it is output ([`TerminalItem::is_output`]).
    fn is_output(&self) -> bool {
        match self {
            Self::Item(item) => item.is_output(),
            Self::Line(_) => true,
        }
    }
}

/// Whether the focus is a live terminal.
pub(crate) fn terminal_focus(state: &SrState) -> Option<NodeId> {
    state
        .focus
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.role == verbatim_model::Role::Terminal)
        .map(|focus| focus.snapshot.id)
}

/// What a caret key is to the reading rules: one Verbatim knows, by the
/// unit it moves by, or any other ("Keys and their window").
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
        CaretMotion::Delete => TerminalKeyKind::Delete,
        _ => TerminalKeyKind::Other,
    }
}

/// What text typed is to the reading rules: Enter, which forgets the
/// typing not yet shown and is answered only by new output; Tab, which
/// forgets it too; a control character, which types nothing the terminal
/// shows as itself; and anything else, typed.
#[must_use]
pub fn typed_kind(typed: &str) -> TerminalKeyKind {
    if typed.chars().any(|c| matches!(c, '\r' | '\n')) {
        TerminalKeyKind::Enter
    } else if typed.contains('\t') {
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
    speech: Said,
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
        TerminalItem::Output(_)
        | TerminalItem::Growth(_)
        | TerminalItem::Skipped(_)
        | TerminalItem::Echo(_) => return None,
    })
}

/// Takes in what the focused terminal said: speaks the answers to keys,
/// then, unless the output is `before_cut` (read before a cut), queues it
/// by the flood policy and hands the oldest to speech.
fn take(state: &mut SrState, trace_id: TraceId, speech: Said, before_cut: bool) -> Vec<Effect> {
    let mut effects = Vec::new();
    for item in &speech.items {
        let SaidItem::Item(item) = item else {
            continue;
        };
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
    let output = speech.items.iter().any(SaidItem::is_output);
    if !state.settings.report_terminal_output || before_cut || !output {
        return effects;
    }
    state.terminal.trace = Some(trace_id);
    if state.terminal.group.is_none() {
        state.terminal.group = Some(state.settings.full_lines());
    }
    queue(state, speech.items);
    bound(state);
    effects.extend(pump(state, trace_id));
    effects.extend(hold_if_full(state));
    effects
}

/// Whether `item` is a line that counts: a line and its growth count, and
/// are dropped, as one ("Core's flood policy").
fn is_line(item: &Waiting) -> bool {
    matches!(item, Waiting::Line(_, LineKind::Line(_)))
}

/// Whether `item` belongs to the line before it: its growth, or the note
/// that it was cut.
fn belongs(item: &Waiting) -> bool {
    matches!(item, Waiting::Line(_, LineKind::Growth) | Waiting::Cut)
}

/// The number of lines waiting.
fn waiting_lines(waiting: &VecDeque<Waiting>) -> usize {
    waiting.iter().filter(|item| is_line(item)).count()
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
    reply: &TextReply,
) -> Vec<Effect> {
    // The answer is a read Core's terminal model aligns, spoken under the
    // read's speech key.
    let (output, key, mut effects) =
        crate::terminal_model::answered_read(state, reply).unwrap_or_default();
    effects.extend(answered(state, trace_id, query_id, output, key));
    effects
}

/// Takes in what the read answering terminal request `query_id` said,
/// `output`, spoken under the read's speech key `key` when it has one: see
/// [`reply`].
pub(crate) fn answered(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    output: Said,
    key: Option<TraceId>,
) -> Vec<Effect> {
    let trace_id = key.unwrap_or(trace_id);
    let spoken = if state.terminal.cancel == Some(query_id) {
        state.terminal.cancel = None;
        take(state, trace_id, output, true)
    } else if state.terminal.overtaken == Some(query_id) {
        state.terminal.overtaken = None;
        take(state, trace_id, output, true)
    } else {
        state.terminal.request = None;
        let mut spoken = take(state, trace_id, output, false);
        if std::mem::take(&mut state.terminal.deciding) {
            spoken.extend(decide_and_pump(state));
        }
        spoken
    };
    crate::terminal_model::under(key, spoken)
}

/// Adds the output among `items` to the waiting queue, in the order it is
/// spoken, its text moved, and keeps the backlog behind the group being
/// spoken to at most as many lines as either limit could keep, older ones
/// counted as skipped, as [`skip_all_but`] would once they were added:
/// output past that bound is counted as it is added, and never added, so a
/// line of the reading rules' is made a string of its own only once it
/// waits. Which of them is spoken is decided once the group is heard
/// ([`decide`]).
fn queue(state: &mut SrState, items: Vec<SaidItem>) {
    let keep = state.settings.full_lines().max(state.settings.last_lines());
    let terminal = &mut state.terminal;
    let group_left = terminal.group.unwrap_or(0);
    // The backlog once the output is added: the lines past the group's
    // ([`backlog_start`]).
    let lines = waiting_lines(&terminal.waiting)
        + items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    SaidItem::Item(TerminalItem::Output(_)) | SaidItem::Line(_)
                )
            })
            .count();
    let backlog = match group_left {
        0 => lines,
        group => lines.saturating_sub(group),
    };
    let waiting = &mut terminal.waiting;
    if backlog <= keep {
        added(items, |item| match item {
            Adding::Waiting(Waiting::Skipped(skipped)) => push_skipped(waiting, skipped),
            item => waiting.push_back(item.waiting()),
        });
        return;
    }
    // The waiting items go round once, each taken from the front and given
    // back at the back unless it is skipped, followed by the output's.
    let mut bounded = Bounded {
        group_left: (group_left > 0).then_some(group_left),
        skipping: group_left == 0,
        dropping: backlog - keep,
        skipped: None,
        in_line: false,
    };
    // The item before the next, held until the next is known: a skipped
    // count of the output merges with a skipped count before it, as
    // [`push_skipped`] merges it with the last waiting.
    let mut held: Option<Adding> = None;
    for _ in 0..waiting.len() {
        if let Some(item) = waiting.pop_front()
            && let Some(before) = held.replace(Adding::Waiting(item))
        {
            bounded.add(waiting, before);
        }
    }
    added(items, |item| match (item, &mut held) {
        (
            Adding::Waiting(Waiting::Skipped(skipped)),
            Some(Adding::Waiting(Waiting::Skipped(last))),
        ) => *last = last.plus(skipped),
        (item, held) => {
            if let Some(before) = held.replace(item) {
                bounded.add(waiting, before);
            }
        }
    });
    if let Some(before) = held {
        bounded.add(waiting, before);
    }
    bounded.end(waiting);
}

/// A waiting item as it is added ([`queue`]): one of the reading rules'
/// lines keeps their text until it waits.
enum Adding {
    /// An item as it waits.
    Waiting(Waiting),
    /// A line no larger than the bound, of the kind given, its text the
    /// reading rules'.
    Line(Text, LineKind),
}

impl Adding {
    /// What it waits as.
    fn waiting(self) -> Waiting {
        match self {
            Self::Waiting(item) => item,
            Self::Line(text, kind) => Waiting::Line(text.to_string(), kind),
        }
    }

    /// The line's kind when it is a line, or else the item, with no text.
    fn kind(&self) -> Result<LineKind, &Waiting> {
        match self {
            Self::Waiting(Waiting::Line(_, kind)) | Self::Line(_, kind) => Ok(*kind),
            Self::Waiting(item) => Err(item),
        }
    }

    /// Whether it is a line that counts ([`is_line`]).
    fn is_line(&self) -> bool {
        matches!(self.kind(), Ok(LineKind::Line(_)))
    }

    /// Whether it belongs to the line before it ([`belongs`]).
    fn belongs(&self) -> bool {
        matches!(self.kind(), Ok(LineKind::Growth) | Err(Waiting::Cut))
    }
}

/// The waiting items the output among `items` makes, in order, each given
/// to `add`, a skipped count as itself for `add` to merge with a count
/// before it. A line larger than the bound is cut and followed by
/// [`Waiting::Cut`].
fn added(items: Vec<SaidItem>, mut add: impl FnMut(Adding)) {
    // Every line counts, blank ones included (Dickson, 2026-10-07, and the
    // coherence review of 2026-10-09): a blank line waits like any other
    // and is counted when skipped, but is never spoken ([`pump`]).
    // A lost read puts a count of none ahead of its lines, and they keep
    // its kind ([`Counted`]). An "about" count of none says nothing itself,
    // so only its lines carry it; a "more than" count of none waits ahead
    // of its lines, and says "skipped lines" unless a skip after it will
    // say "more than" ([`pump`]).
    let mut counted = Counted::Exactly;
    for item in items {
        let (line, kind) = match item {
            SaidItem::Line(text) if text.len() <= MAX_WAITING_BYTES => {
                add(Adding::Line(text, LineKind::Line(counted)));
                continue;
            }
            SaidItem::Line(text) => (text.to_string(), LineKind::Line(counted)),
            SaidItem::Item(TerminalItem::Output(line)) => (line, LineKind::Line(counted)),
            SaidItem::Item(TerminalItem::Growth(line)) => (line, LineKind::Growth),
            SaidItem::Item(TerminalItem::Skipped(Skipped::About(0))) => {
                counted = Counted::About;
                continue;
            }
            SaidItem::Item(TerminalItem::Skipped(skipped)) => {
                if skipped == Skipped::MoreThan(0) {
                    counted = Counted::MoreThan;
                }
                add(Adding::Waiting(Waiting::Skipped(skipped)));
                continue;
            }
            SaidItem::Item(_) => continue,
        };
        let (line, cut) = within_bound(line);
        add(Adding::Waiting(Waiting::Line(line, kind)));
        if cut {
            add(Adding::Waiting(Waiting::Cut));
        }
    }
}

/// The waiting queue rebuilt in order while the backlog is over its bound
/// ([`queue`]): the group being spoken and what belongs to its last line
/// kept, then, from the backlog's start, what [`skip_all_but`] drops
/// counted into one skipped count in its place, then the rest kept.
struct Bounded {
    /// Lines of the group still to come; `None` once they and what belongs
    /// to the last of them have.
    group_left: Option<usize>,
    /// Whether the backlog has started and nothing in it is kept yet.
    skipping: bool,
    /// Lines still to drop.
    dropping: usize,
    /// The count of what was dropped.
    skipped: Option<Skipped>,
    /// Whether the last item dropped was a line, whose growth goes with it.
    in_line: bool,
}

impl Bounded {
    /// The next item, kept at the back of `waiting`, or dropped and
    /// counted.
    fn add(&mut self, waiting: &mut VecDeque<Waiting>, item: Adding) {
        if let Some(left) = &mut self.group_left {
            if *left > 0 {
                if item.is_line() {
                    *left -= 1;
                }
                waiting.push_back(item.waiting());
                return;
            }
            // What belongs to the group's last line is the group's.
            if item.belongs() {
                waiting.push_back(item.waiting());
                return;
            }
            self.group_left = None;
            self.skipping = true;
        }
        if self.skipping {
            let drop = match item.kind() {
                Err(Waiting::Skipped(_)) => true,
                _ if item.belongs() => self.in_line || self.dropping > 0,
                _ => self.dropping > 0,
            };
            if drop {
                match item.kind() {
                    Ok(LineKind::Line(counted)) => {
                        self.skipped = Some(with_line(self.skipped, counted));
                        self.dropping = self.dropping.saturating_sub(1);
                        self.in_line = true;
                    }
                    Err(Waiting::Skipped(count)) => {
                        let count = *count;
                        self.skipped = Some(self.skipped.map_or(count, |s| s.plus(count)));
                        self.in_line = false;
                    }
                    // A dropped line's growth, and its note that it was cut,
                    // go with it.
                    Ok(LineKind::Growth) | Err(_) => {}
                }
                return;
            }
            self.end(waiting);
        }
        waiting.push_back(item.waiting());
    }

    /// Puts the count of what was dropped where it was, once nothing more
    /// is: a line's growth alone counts no line, and leaves no count.
    fn end(&mut self, waiting: &mut VecDeque<Waiting>) {
        if std::mem::take(&mut self.skipping)
            && let Some(skipped) = self
                .skipped
                .take()
                .filter(|skipped| *skipped != Skipped::Count(0))
        {
            waiting.push_back(Waiting::Skipped(skipped));
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
/// spoken still hands to speech, `group_left` of them, anything among them,
/// and what belongs to the last of them.
fn backlog_start(waiting: &VecDeque<Waiting>, group_left: usize) -> usize {
    if group_left == 0 {
        return 0;
    }
    let mut lines = 0;
    for (index, item) in waiting.iter().enumerate() {
        if is_line(item) {
            lines += 1;
            if lines == group_left {
                return index
                    + 1
                    + waiting
                        .iter()
                        .skip(index + 1)
                        .take_while(|item| belongs(item))
                        .count();
            }
        }
    }
    waiting.len()
}

/// Replaces everything in `waiting` from `start` on but its newest `keep`
/// lines with one skipped count, which then comes first among them. A line
/// is dropped with what belongs to it, so the lines kept never start with a
/// bare growth.
fn skip_all_but(waiting: &mut VecDeque<Waiting>, start: usize, keep: usize) {
    let lines = waiting
        .iter()
        .skip(start)
        .filter(|item| is_line(item))
        .count();
    let mut dropping = lines.saturating_sub(keep);
    let mut skipped: Option<Skipped> = None;
    let index = start;
    // Whether the last item dropped was a line, whose growth goes with it.
    let mut in_line = false;
    while let Some(item) = waiting.get(index) {
        let drop = match item {
            Waiting::Skipped(_) => true,
            item if belongs(item) => in_line || dropping > 0,
            _ => dropping > 0,
        };
        if !drop {
            break;
        }
        match waiting.remove(index) {
            Some(Waiting::Line(_, LineKind::Line(counted))) => {
                skipped = Some(with_line(skipped, counted));
                dropping = dropping.saturating_sub(1);
                in_line = true;
            }
            Some(Waiting::Skipped(count)) => {
                skipped = Some(skipped.map_or(count, |s| s.plus(count)));
                in_line = false;
            }
            // A dropped line's growth, and its note that it was cut, go with
            // it.
            Some(Waiting::Line(_, LineKind::Growth) | Waiting::Cut) => {}
            None => break,
        }
    }
    // A line's growth alone counts no line, and leaves no count.
    if let Some(skipped) = skipped.filter(|skipped| *skipped != Skipped::Count(0)) {
        waiting.insert(start, Waiting::Skipped(skipped));
    }
}

/// Keeps everything waiting at most [`MAX_WAITING_BYTES`], the oldest
/// lines counted as skipped beyond it, as [`queue`] keeps the backlog to
/// as many lines as either limit could keep.
fn bound(state: &mut SrState) {
    let terminal = &mut state.terminal;
    // The oldest waiting lines join the skipped count until what waits fits
    // the bound, the backlog's first, then the group's.
    let mut bytes: usize = terminal
        .waiting
        .iter()
        .map(|item| match item {
            Waiting::Line(line, _) => line.len(),
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
            .find(|(_, item)| matches!(item, Waiting::Line(..)))
            .map(|(index, _)| index);
        let Some(index) = index else {
            break;
        };
        if index < start
            && let Some(left) = terminal.group.as_mut()
        {
            *left = left.saturating_sub(1);
        }
        let counted = match terminal.waiting.get(index) {
            Some(Waiting::Line(_, LineKind::Line(counted))) => Some(*counted),
            _ => None,
        };
        // The count just before it, which a skipped line joins.
        let before = index
            .checked_sub(1)
            .and_then(|before| match terminal.waiting.get(before) {
                Some(Waiting::Skipped(count)) => Some(*count),
                _ => None,
            });
        // The line goes with what belongs to it.
        let mut first = true;
        while let Some(item) = terminal.waiting.get(index)
            && (first || (counted.is_some() && belongs(item)))
        {
            if let Some(Waiting::Line(line, _)) = terminal.waiting.remove(index) {
                bytes -= line.len();
            }
            first = false;
        }
        if let Some(counted) = counted {
            let count = Waiting::Skipped(with_line(before, counted));
            if before.is_some() {
                terminal.waiting[index - 1] = count;
            } else {
                terminal.waiting.insert(index, count);
            }
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
    let mut lines = waiting_lines(&terminal.waiting);
    if skips(&terminal.waiting, 0, full) {
        skip_all_but(&mut terminal.waiting, 0, last);
        lines = lines.min(last);
    }
    terminal.group = Some(full.max(lines));
}

/// Whether the flood policy skips what waits in `waiting` from `start` on
/// once it is looked at: more lines than "Lines spoken in full" (`full`),
/// those skipped already included. A lost read's "more than" count of none
/// ([`is_mark`]) counts no line.
fn skips(waiting: &VecDeque<Waiting>, start: usize, full: usize) -> bool {
    let mut lines = 0usize;
    let mut skipped = Skipped::Count(0);
    for item in waiting.iter().skip(start) {
        match item {
            item if is_mark(item) => {}
            Waiting::Skipped(count) => skipped = skipped.plus(*count),
            item if is_line(item) => lines += 1,
            _ => {}
        }
    }
    match skipped {
        Skipped::Count(count) => {
            lines.saturating_add(usize::try_from(count).unwrap_or(usize::MAX)) > full
        }
        Skipped::Uncounted | Skipped::MoreThan(_) | Skipped::About(_) => true,
    }
}

/// Whether `item` is a lost read's "more than" count of none: the lines
/// after it are what a full history held, and how many went by before
/// them is not known.
fn is_mark(item: &Waiting) -> bool {
    matches!(item, Waiting::Skipped(Skipped::MoreThan(0)))
}

/// Whether, once the group being spoken (`group_left` more lines of
/// `waiting`) is heard, the flood policy, with what waits now, skips the
/// backlog's first item and that item says "more than": a line read past
/// a full history, or a "more than" count. A lost read's "more than" count
/// of none reaching speech is then silent, the skip saying it instead.
fn skip_says_more_than(
    waiting: &VecDeque<Waiting>,
    group_left: usize,
    full: usize,
    last: usize,
) -> bool {
    let start = backlog_start(waiting, group_left);
    match waiting.get(start) {
        Some(Waiting::Skipped(Skipped::MoreThan(_))) => skips(waiting, start, full),
        // The policy keeps the newest "Last lines to speak": the first line
        // is skipped only when more wait.
        Some(Waiting::Line(_, LineKind::Line(Counted::MoreThan))) => {
            skips(waiting, start, full)
                && waiting
                    .iter()
                    .skip(start)
                    .filter(|item| is_line(item))
                    .count()
                    > last
        }
        _ => false,
    }
}

/// Hands waiting output to speech until enough is ahead of playback, as
/// far as the group being spoken goes. Handing the group's last line to
/// speech while the outpost holds asks it for what is new.
fn pump(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let mut effects = Vec::new();
    while state.terminal.ahead.len() < AHEAD {
        let terminal = &state.terminal;
        // A group ends after its last line and what belongs to it.
        if terminal.group == Some(0) && !terminal.waiting.front().is_some_and(belongs) {
            break;
        }
        let Some(item) = state.terminal.waiting.pop_front() else {
            break;
        };
        if is_line(&item)
            && let Some(left) = state.terminal.group.as_mut()
        {
            *left = left.saturating_sub(1);
            if *left == 0 && state.terminal.held {
                effects.extend(ask(state, true));
            }
        }
        // A blank line counts, but is not spoken; a lost read's "more than"
        // count of none says nothing when a skip after it will say "more
        // than", which is decided once, as it reaches speech.
        if matches!(&item, Waiting::Line(line, _) if text::is_blank(line))
            || (is_mark(&item)
                && skip_says_more_than(
                    &state.terminal.waiting,
                    state.terminal.group.unwrap_or(0),
                    state.settings.full_lines(),
                    state.settings.last_lines(),
                ))
        {
            continue;
        }
        let mark = state.allocate_mark();
        let counts = is_line(&item);
        let (content, line) = match item {
            Waiting::Line(line, _) => (text::text_segments(&line, None), counts),
            Waiting::Skipped(Skipped::Count(count)) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedLines(count),
                ))],
                false,
            ),
            // An overflow none of whose lines were skipped: how many went
            // by is not known.
            Waiting::Skipped(Skipped::Uncounted | Skipped::MoreThan(0)) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedUncountedLines,
                ))],
                false,
            ),
            Waiting::Skipped(Skipped::About(count)) => (
                vec![UtteranceSegment::new(SegmentContent::Phrase(
                    Phrase::SkippedAboutLines(count),
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

/// An utterance cut off at `at_ms` before playback reached `marks`: when
/// output handed to speech waits for one of them, speech was cut off, and
/// the output still waiting is dropped, as a key's cut drops it.
pub(crate) fn utterance_cancelled(
    state: &mut SrState,
    marks: &[SpeechMark],
    at_ms: u64,
) -> Vec<Effect> {
    let terminal = &state.terminal;
    let waits = terminal.sounding.is_some_and(|mark| marks.contains(&mark))
        || terminal.ahead.iter().any(|(mark, _)| marks.contains(mark));
    if waits { cut(state, at_ms) } else { Vec::new() }
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
mod tests;
