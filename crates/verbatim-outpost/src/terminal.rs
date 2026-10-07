//! A terminal's new output (milestone M4 item 9; `phase6-design.md`, "How
//! the outpost finds new lines"), public so mockapp's tests drive it as the
//! worker does.
//!
//! The outpost keeps, for the focused terminal, an anchor at the start of
//! the last line it read and a [`Memory`]: that line's text and the text of
//! the line before it (the fingerprint), and the last lines it read, the
//! screen as it last saw it. On each change of the terminal's text, one
//! remote program ([`verbatim_uia_rops::terminal_tail`]) checks the
//! fingerprint at the anchor, searches upward for it when the text scrolled
//! beneath the anchor (a full scrollback discarding its oldest lines),
//! counts the lines from there to the end, and reads only the last of them.
//! [`after_anchor`] turns that into [`TerminalOutput`]:
//!
//! - The anchor's own line is compared character by character: a line that
//!   grew speaks what was added, one rewritten in place speaks from the
//!   start of the word where it first differs, and one that only got
//!   shorter (Backspace) speaks nothing.
//! - The lines after it are spoken, all of them up to the read limit, or the
//!   last ones with the rest counted as skipped.
//! - A redraw with the same text finds nothing new and sends nothing.
//!
//! When the fingerprint is not found, or the anchor can no longer be
//! compared with the text (a full-screen program switched screens), the
//! terminal is read afresh from the end of its document and
//! [`after_fresh`] compares the lines read with the screen last read, line
//! by line, as NVDA does for consoles that scroll: the lines after where
//! the old screen's end reappears at the new one's start, or else the lines
//! that differ in place, preceded by "skipped lines" without a count when
//! nothing matched and the text holds more than was read. Trailing padding
//! is removed from every line (`verbatim_text::trim_padding`, by Unicode's
//! `White_Space` property, whatever the language).
//!
//! A read the text changed under while it was read is set aside: when only
//! the last lines were being written to, it finds nothing and the next read
//! finds everything since; when the text scrolled beneath it, lines went by
//! unread, and it says so without a count and starts again from its own
//! last line.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};
use windows::core::AgileReference;

use verbatim_model::{LineChange, MAX_TERMINAL_LINE_BYTES, Skipped, TerminalOutput};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    Error as RopsError, Fingerprint, Found, Path, Tail, TailQuery, TailStart, terminal_tail,
};

use crate::text::TextError;

/// What the outpost remembers of a terminal's text between reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// The text of the line before the last line read, as the provider gave
    /// it; empty at the top.
    pub previous: String,
    /// The text of the last line read, as the provider gave it.
    pub line: String,
    /// The last lines read, without padding, oldest first: the screen as it
    /// was last seen, at most the read limit's lines.
    pub screen: Vec<String>,
}

/// A read of a terminal's tail, as text: [`Tail`] without its range, so the
/// diff can be tested on simulated text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailText {
    /// Where the fingerprint was found.
    pub found: Found,
    /// The line at the anchor now.
    pub line: String,
    /// The line before the anchor now.
    pub previous: String,
    /// The line where the fingerprint was found, as it is now.
    pub found_line: String,
    /// The lines after the anchor's line, or with no anchor, all of them.
    pub count: u32,
    /// How many of the last lines were read.
    pub rows: u32,
    /// The text of those lines, oldest first; a line the terminal wrapped
    /// is one line here.
    pub lines: Vec<String>,
    /// How many of the first lines after the anchor's were read too, when
    /// more followed it than the last lines read.
    pub head_rows: u32,
    /// The text of those first lines, oldest first.
    pub head: Vec<String>,
    /// The last line, read as a line: the next fingerprint's line.
    pub last_line: String,
    /// The line before it: the next fingerprint's line before.
    pub before_last: String,
    /// Whether the text held still while it was read; when it did not, the
    /// read is set aside, and the change that disturbed it causes another.
    pub settled: bool,
    /// Whether, unsettled, it was because the text scrolled beneath the
    /// read's ranges, so lines went by unread.
    pub scrolled: bool,
}

impl From<&Tail> for TailText {
    fn from(tail: &Tail) -> Self {
        Self {
            found: tail.found,
            line: tail.line.clone(),
            previous: tail.previous.clone(),
            found_line: tail.found_line.clone(),
            count: tail.count,
            rows: tail.rows,
            lines: tail.lines.clone(),
            head_rows: tail.head_rows,
            head: tail.head.clone(),
            last_line: tail.last_line.clone(),
            before_last: tail.before_last.clone(),
            settled: tail.settled,
            scrolled: tail.scrolled,
        }
    }
}

/// A line as it is spoken: without its padding and line break, and at most
/// [`MAX_TERMINAL_LINE_BYTES`].
#[must_use]
pub fn trimmed(raw: &str) -> String {
    let text = verbatim_text::trim_padding(raw);
    let mut end = text.len().min(MAX_TERMINAL_LINE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// How the last line read changed in place, `None` when it did not or only
/// got shorter: what it gained at its end, or, rewritten, the line from the
/// start of the word where it first differs. `old` and `new` are the lines
/// as the provider gave them; they are compared without their padding,
/// and the white space a grown line gained where `old` already had the same
/// white space (its padding, or its own trailing spaces, which cannot be
/// told apart) is counted as uncertain.
#[must_use]
pub fn line_change(old_raw: &str, new_raw: &str) -> Option<LineChange> {
    let old = trimmed(old_raw);
    let new = trimmed(new_raw);
    if old == new || old.starts_with(&new) {
        return None;
    }
    if let Some(added) = new.strip_prefix(&old) {
        let uncertain = added
            .chars()
            .zip(old_raw[old.len()..].chars())
            .take_while(|&(gained, had)| gained == had && gained.is_whitespace())
            .map(|(gained, _)| gained.len_utf8())
            .sum();
        return Some(LineChange {
            text: added.to_owned(),
            line: new.clone(),
            appended: true,
            uncertain,
        });
    }
    let differs = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((index, _), _)| index);
    let word = new[..differs]
        .rfind(char::is_whitespace)
        .map_or(0, |space| {
            space + new[space..].chars().next().map_or(1, char::len_utf8)
        });
    Some(LineChange {
        text: new[word..].to_owned(),
        line: new.clone(),
        appended: false,
        uncertain: 0,
    })
}

/// What [`after_anchor`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// The new output, and what to remember.
    Output(TerminalOutput, Memory),
    /// The anchor no longer marks where the text was read to: read the
    /// terminal afresh and compare screens ([`after_fresh`]).
    Afresh,
}

/// The output an anchored read found, or that the terminal must be read
/// afresh. The fingerprint counts as found at the anchor when the line
/// before it is unchanged and the anchor's own line is unchanged, grew,
/// got shorter, or is rewritten under a line that is not blank (a blank
/// line before it matches too easily to trust a rewrite).
#[must_use]
pub fn after_anchor(memory: &Memory, tail: &TailText, wanted: usize) -> Next {
    let (changed, line_now) = match tail.found {
        // Found above the anchor as it was read or grown since (the line
        // output was still being written to), under a line that is not
        // blank: what it gained is new.
        Found::Moved(_) => (
            line_change(&memory.line, &tail.found_line),
            tail.found_line.as_str(),
        ),
        Found::AtAnchor => {
            let changed = line_change(&memory.line, &tail.line);
            let trusted = changed.as_ref().is_none_or(|change| {
                change.appended || !trimmed(&memory.previous).trim().is_empty()
            });
            if !trusted {
                return Next::Afresh;
            }
            (changed, tail.line.as_str())
        }
        Found::NotFound | Found::Afresh => return Next::Afresh,
    };
    let lines: Vec<String> = tail.lines.iter().map(|line| trimmed(line)).collect();
    let head: Vec<String> = tail.head.iter().map(|line| trimmed(line)).collect();
    let unread = tail
        .count
        .saturating_sub(tail.rows)
        .saturating_sub(tail.head_rows);
    let skipped = (unread > 0).then_some(Skipped::Count(unread));
    let previous_now = if matches!(tail.found, Found::Moved(_)) {
        memory.previous.as_str()
    } else {
        tail.previous.as_str()
    };
    let (line, previous) = if tail.rows == 0 {
        (line_now.to_owned(), previous_now.to_owned())
    } else {
        (tail.last_line.clone(), tail.before_last.clone())
    };
    let mut screen = if skipped.is_some() {
        Vec::new()
    } else {
        // The anchor's line as it is now, however it changed.
        let mut screen = memory.screen.clone();
        if let Some(last) = screen.last_mut() {
            *last = trimmed(line_now);
        }
        screen.extend(head.iter().cloned());
        screen
    };
    screen.extend(lines.iter().cloned());
    keep_last(&mut screen, wanted);
    let output = TerminalOutput {
        changed,
        head,
        skipped,
        lines,
    };
    Next::Output(
        output,
        Memory {
            previous,
            line,
            screen,
        },
    )
}

/// Keeps the last `count` lines.
fn keep_last(lines: &mut Vec<String>, count: usize) {
    let excess = lines.len().saturating_sub(count);
    lines.drain(..excess);
}

/// The output a fresh read found, compared with what was remembered (none
/// for a first read, which only sets the baseline), and what to remember.
#[must_use]
pub fn after_fresh(
    memory: Option<&Memory>,
    tail: &TailText,
    wanted: usize,
) -> (TerminalOutput, Memory) {
    let screen: Vec<String> = tail.lines.iter().map(|line| trimmed(line)).collect();
    let (line, previous) = if tail.rows == 0 {
        (String::new(), String::new())
    } else {
        (tail.last_line.clone(), tail.before_last.clone())
    };
    let mut remembered = Memory {
        previous,
        line,
        screen,
    };
    keep_last(&mut remembered.screen, wanted);
    let Some(memory) = memory else {
        return (TerminalOutput::default(), remembered);
    };
    let old = &memory.screen;
    let new = &remembered.screen;
    let blank = |lines: &[String]| lines.iter().all(|line| line.trim().is_empty());
    // The old screen's end reappears in the new one, ending at `at`, its
    // last line as it was or grown since (the line output was still being
    // written to when it was read): as much of the old screen as fits
    // above `at`. When both screens hold as many lines, it reappears at the
    // new one's start once the text scrolled; an old screen of fewer lines
    // (an unsettled read remembers only the lines it read) reappears
    // further down. The lines that reappear as they were must not all be
    // blank, or a blank last line, which any line starts with, would match
    // anywhere.
    let reappears = |at: usize| {
        let m = old.len().min(at);
        let (end, start) = (&old[old.len() - m..], &new[at - m..at]);
        let (same, grown) = if end[m - 1] == start[m - 1] {
            (m, true)
        } else {
            (m - 1, start[m - 1].starts_with(end[m - 1].as_str()))
        };
        grown && end[..same] == start[..same] && !blank(&start[..same])
    };
    let output = if old == new {
        TerminalOutput::default()
    } else if let Some(at) = (!old.is_empty())
        .then(|| (1..=new.len()).rev().find(|&at| reappears(at)))
        .flatten()
    {
        // What the last line gained, and what follows, is new. The latest
        // place it reappears is taken, so nothing read before is new again.
        TerminalOutput {
            changed: line_change(&old[old.len() - 1], &new[at - 1]),
            lines: new[at..].to_vec(),
            ..TerminalOutput::default()
        }
    } else {
        let lines: Vec<String> = new
            .iter()
            .enumerate()
            .filter(|&(index, line)| old.get(index) != Some(line))
            .map(|(_, line)| line.clone())
            .collect();
        // A blank line in its place (the cursor's line, at the end of both)
        // says nothing about whether the text moved.
        let none_kept = !new
            .iter()
            .enumerate()
            .any(|(index, line)| !line.trim().is_empty() && old.get(index) == Some(line));
        let more = tail.count > tail.rows;
        TerminalOutput {
            skipped: (none_kept && more).then_some(Skipped::Uncounted),
            lines,
            ..TerminalOutput::default()
        }
    };
    (output, remembered)
}

/// What an unsettled read finds. When only the last lines were being written
/// to, nothing, and what was remembered stays, so the read that the
/// change's own event causes finds everything since. When the text scrolled
/// beneath the read (a flood in a full scrollback, which may keep every
/// read from settling until it ends), lines went by unread: "skipped lines"
/// without a count, and the read's own last lines are remembered, so the
/// next read starts from there rather than from an anchor the text has long
/// left, which identical later output could match by accident.
fn set_aside(tail: &TailText, memory: &Memory, wanted: usize) -> (TerminalOutput, Memory) {
    if !tail.scrolled {
        return (TerminalOutput::default(), memory.clone());
    }
    let skipped = TerminalOutput {
        skipped: Some(Skipped::Uncounted),
        ..TerminalOutput::default()
    };
    // A read that read no lines (none followed the line where the
    // fingerprint was found, which is the last line, the next anchor) has
    // no last line of its own to remember: the fingerprint stays, and it
    // describes that line still.
    if tail.rows == 0 {
        return (skipped, memory.clone());
    }
    let mut screen: Vec<String> = tail.lines.iter().map(|line| trimmed(line)).collect();
    keep_last(&mut screen, wanted);
    (
        skipped,
        Memory {
            previous: tail.before_last.clone(),
            line: tail.last_line.clone(),
            screen,
        },
    )
}

/// Where a terminal's tail is read from: the provider, or simulated text in
/// the unit tests.
pub trait TailSource {
    /// What a read fails with.
    type Error;

    /// Reads from the anchor, checking the fingerprint `memory` holds;
    /// `Ok(None)` when there is no anchor or it can no longer be compared
    /// with the text (a full-screen program switched screens).
    ///
    /// # Errors
    ///
    /// The source's error, when the terminal is gone or cannot be read.
    fn anchored(&mut self, memory: &Memory, wanted: u32) -> Result<Option<TailText>, Self::Error>;

    /// Reads afresh from the end of the document.
    ///
    /// # Errors
    ///
    /// The source's error, when the terminal is gone or cannot be read.
    fn fresh(&mut self, wanted: u32) -> Result<TailText, Self::Error>;
}

/// What is new in a terminal's text since `memory` was read, and what to
/// remember: from the anchor first, and afresh, comparing screens, when the
/// anchor no longer marks where the text was read to or found nothing new
/// after it (a full-screen program redrawing a line above the anchor). With
/// no memory, or with `baseline` (a focus arriving, whose earlier output is
/// not new to the user), the text is read afresh and nothing is new. A read
/// the text changed under ([`TailText::settled`] false: output scrolling a
/// full scrollback while its lines were read one by one) finds nothing and
/// keeps the memory and the anchor as they were, so the read that the
/// change's own event causes finds everything since; one the text scrolled
/// under says lines were skipped and starts again from it (`set_aside`).
///
/// # Errors
///
/// The source's error.
pub fn read_new<S: TailSource>(
    source: &mut S,
    memory: Option<&Memory>,
    baseline: bool,
    wanted: u32,
) -> Result<(TerminalOutput, Memory), S::Error> {
    let lines = usize::try_from(wanted).unwrap_or(usize::MAX);
    let mut anchored = None;
    if !baseline
        && let Some(memory) = memory
        && let Some(tail) = source.anchored(memory, wanted)?
    {
        // A read that did not find the fingerprint has nowhere to start
        // from, settled or not: the terminal is read afresh. Set aside, it
        // would count from the anchor, which a full scrollback keeps on
        // its last row, and so read no lines to take the next fingerprint
        // from.
        if !tail.settled && tail.found != Found::NotFound {
            return Ok(set_aside(&tail, memory, lines));
        }
        if let Next::Output(output, remembered) = after_anchor(memory, &tail, lines) {
            if !output.is_empty() {
                return Ok((output, remembered));
            }
            anchored = Some(remembered);
        }
    }
    let tail = source.fresh(wanted)?;
    if !tail.settled
        && !baseline
        && let Some(memory) = memory
    {
        return Ok(set_aside(&tail, anchored.as_ref().unwrap_or(memory), lines));
    }
    let earlier = if baseline {
        None
    } else {
        anchored.as_ref().or(memory)
    };
    Ok(after_fresh(earlier, &tail, lines))
}

/// A focused terminal's anchor and memory, kept by the worker between reads.
#[derive(Default)]
pub struct Terminal {
    anchor: Option<AgileReference<IUIAutomationTextRange>>,
    memory: Option<Memory>,
}

impl Terminal {
    /// What is remembered of the text, `None` before the first read.
    #[must_use]
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
    }
}

/// Whether a failed remote operation or classic read means the terminal
/// is gone.
fn is_gone(error: &RopsError) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}

/// A failed read as a text error.
fn text_error(error: &RopsError) -> TextError {
    if is_gone(error) {
        TextError::Gone
    } else {
        TextError::Failed(error.to_string())
    }
}

/// A terminal's tail read through UIA: the remote program when `remote`,
/// the classic implementation otherwise or when the program fails.
struct UiaTail<'a> {
    uia: &'a Uia,
    element: &'a IUIAutomationElement,
    pattern: &'a IUIAutomationTextPattern,
    anchor: Option<IUIAutomationTextRange>,
    remote: bool,
    /// The last line of the newest read, the next anchor.
    last: Option<IUIAutomationTextRange>,
    /// The path each read took.
    paths: Vec<Path>,
}

impl UiaTail<'_> {
    fn run(&mut self, query: &TailQuery<'_>) -> Result<TailText, RopsError> {
        let started = std::time::Instant::now();
        let calls_before = verbatim_uia::calls::peek().uia;
        let result = terminal_tail(self.uia, query, self.remote);
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let calls = verbatim_uia::calls::peek().uia.saturating_sub(calls_before);
        let start = match query.start {
            TailStart::Anchor { .. } => "anchor",
            TailStart::Document(_) | TailStart::Text { .. } => "fresh",
        };
        let (tail, path) = match result {
            Ok(answer) => answer,
            Err(error) => {
                tracing::debug!(start, elapsed_us, calls, %error, "terminal tail read failed");
                return Err(error);
            }
        };
        // What each read costs, which a flood repeats back to back
        // (`docs/performance.md`, "A terminal flood").
        tracing::debug!(
            start,
            path = path.name(),
            elapsed_us,
            calls,
            found = ?tail.found,
            count = tail.count,
            settled = tail.settled,
            scrolled = tail.scrolled,
            "terminal tail timing"
        );
        self.paths.push(path);
        let text = TailText::from(&tail);
        if !text.settled {
            tracing::debug!("a terminal's text changed while it was read; the read is set aside");
        }
        tracing::debug!(
            found = ?text.found,
            count = text.count,
            read = text.lines.len(),
            line = trimmed(&text.line),
            previous = trimmed(&text.previous),
            first = text.lines.first().map(|line| trimmed(line)),
            last = text.lines.last().map(|line| trimmed(line)),
            "terminal tail read"
        );
        if tail.settled || tail.scrolled {
            self.last = Some(tail.last);
        }
        Ok(text)
    }
}

impl TailSource for UiaTail<'_> {
    type Error = TextError;

    fn anchored(&mut self, memory: &Memory, wanted: u32) -> Result<Option<TailText>, TextError> {
        let Some(anchor) = self.anchor.clone() else {
            return Ok(None);
        };
        tracing::debug!(
            line = ?memory.line,
            previous = ?memory.previous,
            "terminal fingerprint sought"
        );
        let query = TailQuery {
            start: TailStart::Anchor {
                range: &anchor,
                fingerprint: Fingerprint {
                    line: &memory.line,
                    previous: &memory.previous,
                },
            },
            lines_wanted: wanted,
        };
        match self.run(&query) {
            Ok(tail) => Ok(Some(tail)),
            Err(error) if is_gone(&error) => Err(TextError::Gone),
            // A range from before a switch of screens no longer compares
            // with the text: read afresh.
            Err(error) => {
                tracing::debug!(%error, "a terminal's anchor could not be read");
                Ok(None)
            }
        }
    }

    fn fresh(&mut self, wanted: u32) -> Result<TailText, TextError> {
        let query = TailQuery {
            start: TailStart::Text {
                element: self.element,
                pattern: self.pattern,
            },
            lines_wanted: wanted,
        };
        self.run(&query).map_err(|error| text_error(&error))
    }
}

/// Reads what is new in a focused terminal's text since the last read
/// ([`read_new`]) through UIA, keeping the anchor and memory in `terminal`.
/// `remote` tries the remote program first. The answer is the output (empty
/// when nothing changed) and the path each read took, for the caller to log
/// a fallback and stop trying the remote program for a window whose import
/// failed.
///
/// # Errors
///
/// [`TextError::Gone`] when the terminal is gone, [`TextError::Failed`]
/// when it could not be read.
pub fn read(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    terminal: &mut Terminal,
    wanted: u32,
    remote: bool,
    baseline: bool,
) -> Result<(TerminalOutput, Vec<Path>), TextError> {
    let mut source = UiaTail {
        uia,
        element,
        pattern,
        anchor: terminal
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.resolve().ok()),
        remote,
        last: None,
        paths: Vec::new(),
    };
    let (output, memory) = read_new(&mut source, terminal.memory.as_ref(), baseline, wanted)?;
    if let Some(last) = &source.last {
        terminal.anchor = AgileReference::new(last).ok();
    }
    terminal.memory = Some(memory);
    Ok((output, source.paths))
}

#[cfg(test)]
mod tests;
