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

use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, IUIAutomationTextRange};
use windows::core::AgileReference;

use verbatim_model::{LineChange, MAX_TERMINAL_LINE_BYTES, Skipped, TerminalOutput};
use verbatim_uia::Uia;
use verbatim_uia::text::TextPatternExt;
use verbatim_uia_rops::{
    Error as RopsError, Fingerprint, Found, Path, SEARCH_LINES, Tail, TailQuery, TailStart,
    terminal_tail,
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
    /// The lines after the anchor's line, or with no anchor, all of them.
    pub count: u32,
    /// The last lines, oldest first.
    pub lines: Vec<String>,
    /// The line above the first of `lines`.
    pub above: String,
}

impl From<&Tail> for TailText {
    fn from(tail: &Tail) -> Self {
        Self {
            found: tail.found,
            line: tail.line.clone(),
            previous: tail.previous.clone(),
            count: tail.count,
            lines: tail.lines.clone(),
            above: tail.above.clone(),
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
/// start of the word where it first differs.
#[must_use]
pub fn line_change(old: &str, new: &str) -> Option<LineChange> {
    let old = trimmed(old);
    let new = trimmed(new);
    if old == new || old.starts_with(&new) {
        return None;
    }
    if let Some(added) = new.strip_prefix(&old) {
        return Some(LineChange {
            text: added.to_owned(),
            line: new.clone(),
            appended: true,
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
        Found::Moved(_) => (None, memory.line.as_str()),
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
    let count = usize::try_from(tail.count).unwrap_or(usize::MAX);
    let lines: Vec<String> = tail.lines.iter().map(|line| trimmed(line)).collect();
    let unread = count.saturating_sub(lines.len());
    let skipped = (unread > 0).then(|| Skipped::Count(u32::try_from(unread).unwrap_or(u32::MAX)));
    let previous_now = if matches!(tail.found, Found::Moved(_)) {
        memory.previous.as_str()
    } else {
        tail.previous.as_str()
    };
    let (line, previous) = match tail.lines.as_slice() {
        [] => (line_now.to_owned(), previous_now.to_owned()),
        [only] if count == 1 => (only.clone(), line_now.to_owned()),
        [only] => (only.clone(), tail.above.clone()),
        [.., before, last] => (last.clone(), before.clone()),
    };
    let mut screen = if skipped.is_some() {
        Vec::new()
    } else {
        // The anchor's line as it is now, however it changed.
        let mut screen = memory.screen.clone();
        if let Some(last) = screen.last_mut() {
            *last = trimmed(line_now);
        }
        screen
    };
    screen.extend(lines.iter().cloned());
    keep_last(&mut screen, wanted);
    let output = TerminalOutput {
        changed,
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
    let (line, previous) = match tail.lines.as_slice() {
        [] => (String::new(), String::new()),
        [only] => (only.clone(), tail.above.clone()),
        [.., before, last] => (last.clone(), before.clone()),
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
    let output = if old == new {
        TerminalOutput::default()
    } else if let Some(overlap) = (1..=old.len().min(new.len()))
        .rev()
        .find(|&m| old[old.len() - m..] == new[..m] && !blank(&new[..m]))
    {
        // The old screen's end reappears at the new one's start: it
        // scrolled, and what follows is new.
        TerminalOutput {
            lines: new[overlap..].to_vec(),
            ..TerminalOutput::default()
        }
    } else {
        let lines: Vec<String> = new
            .iter()
            .enumerate()
            .filter(|&(index, line)| old.get(index) != Some(line))
            .map(|(_, line)| line.clone())
            .collect();
        let none_kept = lines.len() == new.len();
        let more = usize::try_from(tail.count).unwrap_or(usize::MAX) > new.len();
        TerminalOutput {
            skipped: (none_kept && more).then_some(Skipped::Uncounted),
            lines,
            ..TerminalOutput::default()
        }
    };
    (output, remembered)
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
/// not new to the user), the text is read afresh and nothing is new.
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
        && let Next::Output(output, remembered) = after_anchor(memory, &tail, lines)
    {
        if !output.is_empty() {
            return Ok((output, remembered));
        }
        anchored = Some(remembered);
    }
    let tail = source.fresh(wanted)?;
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
        let (tail, path) = terminal_tail(self.uia, query, self.remote)?;
        self.paths.push(path);
        let text = TailText::from(&tail);
        self.last = Some(tail.last);
        Ok(text)
    }
}

impl TailSource for UiaTail<'_> {
    type Error = TextError;

    fn anchored(&mut self, memory: &Memory, wanted: u32) -> Result<Option<TailText>, TextError> {
        let Some(anchor) = self.anchor.clone() else {
            return Ok(None);
        };
        let query = TailQuery {
            start: TailStart::Anchor {
                range: &anchor,
                fingerprint: Fingerprint {
                    line: &memory.line,
                    previous: &memory.previous,
                },
            },
            lines_wanted: wanted,
            search_lines: SEARCH_LINES,
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
        let document = self.pattern.document_range().map_err(|error| {
            if verbatim_uia::element_is_gone(&error) {
                TextError::Gone
            } else {
                TextError::Failed(error.to_string())
            }
        })?;
        let query = TailQuery {
            start: TailStart::Document(&document),
            lines_wanted: wanted,
            search_lines: SEARCH_LINES,
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
    pattern: &IUIAutomationTextPattern,
    terminal: &mut Terminal,
    wanted: u32,
    remote: bool,
    baseline: bool,
) -> Result<(TerminalOutput, Vec<Path>), TextError> {
    let mut source = UiaTail {
        uia,
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
