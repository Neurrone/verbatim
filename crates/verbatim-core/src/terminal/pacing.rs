//! The pacing (`books/dev/src/explanation/design/terminal-model.md`,
//! "Core's flood policy"): how the focused terminal's output is handed to
//! speech. Lines go a few at a time, each between two index marks, and the
//! rest wait here, so the backlog not yet spoken is known. The first "Lines
//! spoken in full" lines of a burst are spoken whole; once a group's last
//! line has been heard, the look at the backlog replaces all but the newest
//! "Last lines to speak" with "skipped N lines" when more than "Lines
//! spoken in full" wait, and what is left is the next group. A cut drops
//! everything waiting. Whether the watch's reads are held is derived from
//! the pacing ([`Pacing::holds`]), never stored.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    Effect, KeySeq, Message, Phrase, SegmentContent, SpeechMark, TraceId, Utterance,
    UtteranceSegment,
};

use crate::text;

/// How many utterances of output are handed to speech before their marks
/// are reached: two queued behind the one playing, so there is no gap
/// between lines and the backlog stays here, where it can be trimmed.
const AHEAD: usize = 2;

/// The most bytes of terminal output waiting to be spoken (Dickson,
/// 2026-10-07): when the bound is reached the oldest waiting lines join
/// the skipped count, and a single line larger than the whole bound is cut
/// on a grapheme boundary, saying so.
pub(crate) const MAX_WAITING_BYTES: usize = 10 * 1024 * 1024;

/// How many lines went by unread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Skipped {
    /// Exactly this many.
    Count(u32),
    /// Some, but how many is not known: "skipped lines".
    Uncounted,
    /// More than this many: the history rotated past the last read's end,
    /// and this many are the rows it still holds that are not spoken.
    MoreThan(u32),
    /// About this many: the document shrank under a clear, and these are
    /// its rows.
    About(u32),
}

impl Skipped {
    /// This count followed by the `newer` one. Counts add, exact only when
    /// both are; a newer [`Skipped::MoreThan`] stands for everything before
    /// it, and lines counted after one add to it.
    #[must_use]
    pub(crate) fn plus(self, newer: Self) -> Self {
        match (self, newer) {
            (Self::Count(a), Self::Count(b)) => Self::Count(a.saturating_add(b)),
            (_, Self::MoreThan(b)) => Self::MoreThan(b),
            (Self::MoreThan(a), Self::Count(b) | Self::About(b)) => {
                Self::MoreThan(a.saturating_add(b))
            }
            (Self::About(a) | Self::Count(a), Self::About(b))
            | (Self::About(a), Self::Count(b)) => Self::About(a.saturating_add(b)),
            (_, Self::Uncounted) | (Self::Uncounted, Self::Count(_) | Self::About(_)) => {
                Self::Uncounted
            }
        }
    }
}

/// One piece of output waiting to be spoken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Waiting {
    /// A line, or what changed of one; a blank one counts but is not
    /// spoken (Q1).
    Line(String),
    /// Lines skipped as too much to read.
    Skipped(Skipped),
    /// The line before was longer than the bound and was cut: "line cut".
    /// Not a line.
    Cut,
}

impl Waiting {
    fn is_line(&self) -> bool {
        matches!(self, Self::Line(_))
    }
}

/// What output handed to speech is spoken under: the newest output's trace
/// and its entry's fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Under {
    pub(crate) trace: TraceId,
    pub(crate) fence: Option<KeySeq>,
}

/// A group being spoken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Group {
    /// Lines the group still hands on; once 0 the hold lifts, and the
    /// group's last mark reached makes the pacing [`Pacing::Due`].
    left: usize,
    /// Output not yet handed to speech, oldest first.
    waiting: VecDeque<Waiting>,
    /// The opening marks of output handed to speech not yet reached, with
    /// whether each is a line.
    ahead: VecDeque<(SpeechMark, bool)>,
    /// The closing mark of the last utterance handed to speech, until
    /// playback reaches it.
    sounding: Option<SpeechMark>,
    under: Under,
}

/// The pacing's state (build 9's design, 5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pacing {
    /// Nothing is being spoken or waits.
    #[default]
    Idle,
    /// A group is being spoken.
    Speaking(Group),
    /// The group's last mark was reached and the look at what waits has
    /// not run, waiting for the watch's output read to answer (Q9).
    Due {
        waiting: VecDeque<Waiting>,
        under: Under,
    },
}

/// What the pacing needs of Core to hand output on: the limits, the index
/// marks, and the utterance's source and validity, which are the focus's.
pub(crate) struct Voice<'a> {
    /// "Lines spoken in full".
    pub(crate) full: usize,
    /// "Last lines to speak".
    pub(crate) last: usize,
    /// The next index mark's number.
    pub(crate) next_mark: &'a mut u64,
    /// An utterance of output, its segments to be filled.
    pub(crate) utterance: &'a dyn Fn(TraceId, Vec<UtteranceSegment>) -> Utterance,
}

impl Voice<'_> {
    fn mark(&mut self) -> SpeechMark {
        let mark = SpeechMark(*self.next_mark);
        *self.next_mark = self.next_mark.saturating_add(1);
        mark
    }
}

impl Pacing {
    /// Whether the watch's reads are held: a group still hands on lines and
    /// at least "Lines spoken in full" lines wait beyond it (TM rule 1).
    pub(crate) fn holds(&self, full: usize) -> bool {
        match self {
            Self::Speaking(group) => {
                group.left > 0 && lines(&group.waiting).saturating_sub(group.left) >= full
            }
            Self::Idle | Self::Due { .. } => false,
        }
    }

    /// Whether the group being spoken has handed on its last line: a read
    /// sent now is the read at the group's last line, which waited for
    /// speech.
    pub(crate) fn released(&self) -> bool {
        match self {
            Self::Speaking(group) => group.left == 0,
            Self::Due { .. } => true,
            Self::Idle => false,
        }
    }

    /// Column G: an entry's turn with `output`, in order, under `under`.
    pub(crate) fn take(
        &mut self,
        output: Vec<Waiting>,
        under: Under,
        voice: &mut Voice<'_>,
    ) -> Vec<Effect> {
        let keep = voice.full.max(voice.last);
        let group = match std::mem::take(self) {
            Self::Idle => Group {
                left: voice.full,
                waiting: VecDeque::new(),
                ahead: VecDeque::new(),
                sounding: None,
                under,
            },
            Self::Speaking(group) => group,
            // Appended to what waits for the look.
            Self::Due { mut waiting, .. } => {
                append(&mut waiting, output, keep, 0);
                *self = Self::Due { waiting, under };
                return Vec::new();
            }
        };
        let mut group = Group { under, ..group };
        append(&mut group.waiting, output, keep, group.left);
        *self = Self::Speaking(group);
        self.pump(voice)
    }

    /// Columns H and I: playback reached `mark`.
    pub(crate) fn mark_reached(&mut self, mark: SpeechMark, voice: &mut Voice<'_>) -> Vec<Effect> {
        let Self::Speaking(group) = self else {
            return Vec::new();
        };
        if group.ahead.iter().any(|(ahead, _)| *ahead == mark) {
            while group.ahead.front().is_some_and(|(ahead, _)| *ahead <= mark) {
                group.ahead.pop_front();
            }
            return self.pump(voice);
        }
        if group.sounding != Some(mark) {
            return Vec::new();
        }
        group.sounding = None;
        self.pump(voice)
    }

    /// Whether an utterance cut off before playback reached `marks` was
    /// output handed on: speech was cut off (column J).
    pub(crate) fn cancelled(&self, marks: &[SpeechMark]) -> bool {
        match self {
            Self::Speaking(group) => {
                group.sounding.is_some_and(|mark| marks.contains(&mark))
                    || group.ahead.iter().any(|(mark, _)| marks.contains(mark))
            }
            Self::Idle | Self::Due { .. } => false,
        }
    }

    /// Column K2: "report new output" turned off drops what waits.
    pub(crate) fn drop_waiting(&mut self) {
        match self {
            Self::Speaking(group) => {
                group.waiting.clear();
                if group.ahead.is_empty() && group.sounding.is_none() {
                    *self = Self::Idle;
                }
            }
            Self::Due { .. } => *self = Self::Idle,
            Self::Idle => {}
        }
    }

    /// Column E for [`Pacing::Due`]: unless `reading`, the watch having an
    /// output read out, the look at what waits (F1): more than "Lines
    /// spoken in full" lines, those skipped counted, leave the newest "Last
    /// lines to speak" behind one skipped count, and what waits is the next
    /// group; with nothing waiting, the burst is over.
    pub(crate) fn look(&mut self, reading: bool, voice: &mut Voice<'_>) -> Vec<Effect> {
        if reading || !matches!(self, Self::Due { .. }) {
            return Vec::new();
        }
        let Self::Due { mut waiting, under } = std::mem::take(self) else {
            return Vec::new();
        };
        if waiting.is_empty() {
            return Vec::new();
        }
        let mut left = lines(&waiting);
        if skips(&waiting, voice.full) {
            skip_all_but(&mut waiting, 0, voice.last);
            left = left.min(voice.last);
        }
        *self = Self::Speaking(Group {
            left: voice.full.max(left),
            waiting,
            ahead: VecDeque::new(),
            sounding: None,
            under,
        });
        self.pump(voice)
    }

    /// Hands waiting output on until enough is ahead of playback, as far as
    /// the group goes; with nothing sounding once the group, or everything
    /// waiting, is handed on, the pacing is due.
    fn pump(&mut self, voice: &mut Voice<'_>) -> Vec<Effect> {
        let Self::Speaking(group) = self else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        while group.ahead.len() < AHEAD {
            // A group ends after its last line and the note that it was cut.
            if group.left == 0 && group.waiting.front() != Some(&Waiting::Cut) {
                break;
            }
            let Some(item) = group.waiting.pop_front() else {
                break;
            };
            let line = item.is_line();
            if line {
                group.left = group.left.saturating_sub(1);
            }
            let content = match item {
                // A blank line counts, but is not spoken.
                Waiting::Line(line) if text::is_blank(&line) => continue,
                Waiting::Line(line) => text::text_segments(&line, None),
                Waiting::Skipped(skipped) => vec![UtteranceSegment::new(SegmentContent::Phrase(
                    match skipped {
                        Skipped::Count(count) => Phrase::SkippedLines(count),
                        Skipped::Uncounted => Phrase::SkippedUncountedLines,
                        Skipped::About(count) => Phrase::SkippedAboutLines(count),
                        Skipped::MoreThan(count) => Phrase::SkippedMoreThanLines(count),
                    },
                ))],
                Waiting::Cut => vec![UtteranceSegment::new(SegmentContent::Message(
                    Message::TerminalLineCut,
                ))],
            };
            let (opening, closing) = (voice.mark(), voice.mark());
            let mut segments = vec![UtteranceSegment::new(SegmentContent::Mark(opening))];
            segments.extend(content);
            segments.push(UtteranceSegment::new(SegmentContent::Mark(closing)));
            effects.push(Effect::Speak(
                (voice.utterance)(group.under.trace, segments),
                group.under.fence,
            ));
            group.ahead.push_back((opening, line));
            group.sounding = Some(closing);
        }
        if group.sounding.is_none()
            && group.ahead.is_empty()
            && (group.left == 0 || group.waiting.is_empty())
        {
            let waiting = std::mem::take(&mut group.waiting);
            *self = Self::Due {
                waiting,
                under: group.under,
            };
        }
        effects
    }
}

/// The number of lines in `waiting`.
fn lines(waiting: &VecDeque<Waiting>) -> usize {
    waiting.iter().filter(|item| item.is_line()).count()
}

/// Adds `output` to `waiting`, each line within the bound, then keeps the
/// backlog past the group's `group_left` lines to at most `keep` lines,
/// older ones counted as skipped, as the look would, and everything waiting
/// within [`MAX_WAITING_BYTES`].
fn append(waiting: &mut VecDeque<Waiting>, output: Vec<Waiting>, keep: usize, group_left: usize) {
    for item in output {
        match item {
            Waiting::Line(line) => {
                let (line, cut) = within_bound(line);
                waiting.push_back(Waiting::Line(line));
                if cut {
                    waiting.push_back(Waiting::Cut);
                }
            }
            Waiting::Skipped(skipped) => match waiting.back_mut() {
                Some(Waiting::Skipped(last)) => *last = last.plus(skipped),
                _ => waiting.push_back(Waiting::Skipped(skipped)),
            },
            Waiting::Cut => waiting.push_back(Waiting::Cut),
        }
    }
    let start = backlog_start(waiting, group_left);
    if waiting
        .iter()
        .skip(start)
        .filter(|item| item.is_line())
        .count()
        > keep
    {
        skip_all_but(waiting, start, keep);
    }
    bound(waiting);
}

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

/// Where the backlog starts in `waiting`: past the `group_left` lines the
/// group still hands on, and the note that the last of them was cut.
fn backlog_start(waiting: &VecDeque<Waiting>, group_left: usize) -> usize {
    if group_left == 0 {
        return 0;
    }
    let mut seen: usize = 0;
    for (index, item) in waiting.iter().enumerate() {
        if item.is_line() {
            seen = seen.saturating_add(1);
            if seen == group_left {
                let after = index.saturating_add(1);
                return if waiting.get(after) == Some(&Waiting::Cut) {
                    after.saturating_add(1)
                } else {
                    after
                };
            }
        }
    }
    waiting.len()
}

/// Whether the look skips what waits: more lines than `full`, those
/// already skipped included.
fn skips(waiting: &VecDeque<Waiting>, full: usize) -> bool {
    let mut count = 0usize;
    let mut skipped = Skipped::Count(0);
    for item in waiting {
        match item {
            Waiting::Line(_) => count = count.saturating_add(1),
            Waiting::Skipped(more) => skipped = skipped.plus(*more),
            Waiting::Cut => {}
        }
    }
    match skipped {
        Skipped::Count(more) => {
            count.saturating_add(usize::try_from(more).unwrap_or(usize::MAX)) > full
        }
        Skipped::Uncounted | Skipped::MoreThan(_) | Skipped::About(_) => true,
    }
}

/// Replaces everything in `waiting` from `start` on but its newest `keep`
/// lines with one skipped count, which then comes first among them; a line
/// goes with the note that it was cut.
fn skip_all_but(waiting: &mut VecDeque<Waiting>, start: usize, keep: usize) {
    let total = waiting
        .iter()
        .skip(start)
        .filter(|item| item.is_line())
        .count();
    let mut dropping = total.saturating_sub(keep);
    let mut skipped: Option<Skipped> = None;
    let mut in_line = false;
    while let Some(item) = waiting.get(start) {
        let drop = match item {
            Waiting::Skipped(_) => true,
            Waiting::Cut => in_line || dropping > 0,
            Waiting::Line(_) => dropping > 0,
        };
        if !drop {
            break;
        }
        match waiting.remove(start) {
            Some(Waiting::Line(_)) => {
                let one = Skipped::Count(1);
                skipped = Some(skipped.map_or(one, |count| count.plus(one)));
                dropping = dropping.saturating_sub(1);
                in_line = true;
            }
            Some(Waiting::Skipped(count)) => {
                skipped = Some(skipped.map_or(count, |before| before.plus(count)));
                in_line = false;
            }
            Some(Waiting::Cut) | None => {}
        }
    }
    if let Some(skipped) = skipped.filter(|skipped| *skipped != Skipped::Count(0)) {
        waiting.insert(start, Waiting::Skipped(skipped));
    }
}

/// Keeps everything waiting at most [`MAX_WAITING_BYTES`], the oldest
/// lines joining a skipped count in their place.
fn bound(waiting: &mut VecDeque<Waiting>) {
    let mut bytes: usize = waiting
        .iter()
        .map(|item| match item {
            Waiting::Line(line) => line.len(),
            Waiting::Skipped(_) | Waiting::Cut => 0,
        })
        .sum();
    while bytes > MAX_WAITING_BYTES {
        let Some(index) = waiting.iter().position(Waiting::is_line) else {
            break;
        };
        if let Some(Waiting::Line(line)) = waiting.remove(index) {
            bytes = bytes.saturating_sub(line.len());
        }
        if waiting.get(index) == Some(&Waiting::Cut) {
            waiting.remove(index);
        }
        let one = Skipped::Count(1);
        match index
            .checked_sub(1)
            .and_then(|before| waiting.get_mut(before))
        {
            Some(Waiting::Skipped(count)) => *count = count.plus(one),
            _ => waiting.insert(index, Waiting::Skipped(one)),
        }
    }
}

#[cfg(test)]
#[path = "pacing_tests.rs"]
mod tests;
