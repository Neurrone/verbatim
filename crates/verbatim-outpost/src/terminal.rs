//! A focused terminal's reading (`docs/design/terminal-model.md`, "The
//! read"), public so mockapp's tests drive it as the worker does.
//!
//! Every read is one snapshot of the terminal's text, from a screen above
//! where the last read said to start to the document's end, taken in one
//! remote program with the screen, the caret and its line, and, in the first
//! read after a key, the screen's colours
//! ([`verbatim_uia_rops::terminal_snapshot`]). The read is given to the
//! terminal's reading rules
//! ([`verbatim_text::terminal_reading::TerminalReader`]), which align it with
//! their memory of the terminal's rows and say what is new and what the keys
//! did; when they ask for the whole document, that read is taken at once and
//! given instead, so a change costs at most two reads. What they say is sent
//! to Core as [`TerminalSpeech`].
//!
//! When to read is decided by on-demand reading ([`reading`]); the review
//! cursor's commands are answered from the same reads ([`review`]).

pub mod reading;
pub mod review;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};
use windows::core::AgileReference;

use verbatim_model::{
    Skipped, TerminalItem, TerminalKey, TerminalKeyKind, TerminalSettings, TerminalSpeech,
    TerminalUnit,
};
use verbatim_text::terminal_reading::{
    ColourRun, Key, KeyKind, Settings, Speech, Step, TerminalReader, Unit,
};
use verbatim_text::terminal_rows::{Lost, Read, ReadText};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, Path, Snapshot, SnapshotQuery,
    terminal_snapshot,
};

use crate::text::TextError;
use reading::Reading;

/// The reading rules' settings from Core's ([`TerminalSettings`]).
#[must_use]
pub fn rules_settings(settings: TerminalSettings) -> Settings {
    Settings {
        report_new_output: settings.report_new_output,
        review_follows_caret: settings.review_follows_caret,
    }
}

/// A focused terminal's reading rules and what the reads need between
/// them, kept by the worker while the terminal exists, across focus
/// changes (`docs/design/terminal-model.md`, "The focus").
pub struct Terminal {
    reader: TerminalReader,
    /// The settings the reading rules were last given.
    settings: TerminalSettings,
    /// Where the next read starts ([`Snapshot::next_start`]).
    start: Option<AgileReference<IUIAutomationTextRange>>,
    /// The screen's height as the last read found it, in rows.
    screen_rows: u32,
    /// The screen's width as the last read found it, in cells.
    width: usize,
    /// Whether the next read takes the colours: a key reached the terminal
    /// since the last read ("A key's answer").
    colours_due: bool,
    /// On-demand reading.
    pub reading: Reading,
    /// When the last read began, in milliseconds on the shared clock: a
    /// text change observed before it is covered by that read
    /// ([`Terminal::covers`]).
    read_started_ms: u64,
    /// The caret last sent to Core for the terminal, as a caret report or
    /// in the answer to a caret key: a caret read that finds it unchanged,
    /// at the same place with the same line, is not sent again. Forgotten
    /// when the terminal gains the focus, and with the terminal when Core
    /// releases its node.
    pub sent_caret: Option<verbatim_model::CaretReport>,
    /// When the reading rules' quiet-period timer is due, in milliseconds
    /// on the shared clock ("Every time limit").
    due_ms: Option<u64>,
    /// The places the review cursor's answers minted, each a text anchor's
    /// number and the memory's place for it, oldest first ([`review`]).
    places: std::collections::VecDeque<(u64, u64)>,
    /// Whether the last read started at the document's start.
    at_document_start: bool,
    /// The caret as the last read found it.
    caret: Option<review::CaretAt>,
}

impl Terminal {
    /// A terminal before its first read, read with `settings`.
    #[must_use]
    pub fn new(settings: TerminalSettings) -> Self {
        Self {
            reader: TerminalReader::new(rules_settings(settings)),
            settings,
            start: None,
            screen_rows: 0,
            width: 0,
            colours_due: false,
            reading: Reading::default(),
            read_started_ms: 0,
            sent_caret: None,
            due_ms: None,
            places: std::collections::VecDeque::new(),
            at_document_start: false,
            caret: None,
        }
    }

    /// The reading rules, for the tests.
    #[must_use]
    pub fn reader(&self) -> &TerminalReader {
        &self.reader
    }

    /// When the quiet-period timer is due, `None` when nothing waits for
    /// it.
    #[must_use]
    pub fn due_ms(&self) -> Option<u64> {
        self.due_ms
    }

    /// Whether a text change observed at `observed_at_ms` (milliseconds on
    /// the shared clock) is covered by the last read, having been observed
    /// before that read began.
    #[must_use]
    pub fn covers(&self, observed_at_ms: u64) -> bool {
        observed_at_ms < self.read_started_ms
    }

    /// Gives the reading rules `settings` when they changed. Turning
    /// "report new output" off drops what output held.
    pub fn update_settings(&mut self, settings: TerminalSettings) {
        if settings != self.settings {
            self.settings = settings;
            let step = self.reader.set_settings(rules_settings(settings));
            self.due_ms = step.timer;
        }
    }

    /// Where the next read starts, for a read of the history above it.
    #[must_use]
    pub fn start_range(&self) -> Option<IUIAutomationTextRange> {
        self.start.as_ref().and_then(|range| range.resolve().ok())
    }

    /// The screen's height as the last read found it, in rows.
    #[must_use]
    pub fn screen_rows(&self) -> u32 {
        self.screen_rows
    }

    /// The focus arrived at the terminal at `at_ms`: the next read speaks
    /// nothing but puts the review cursor at the caret ("The focus").
    pub fn focus(&mut self, at_ms: u64) {
        let step = self.reader.focus(at_ms);
        self.due_ms = step.timer;
    }

    /// The focus left the terminal: what waited for the quiet period is
    /// dropped, and output written meanwhile is not heard on return.
    pub fn blur(&mut self) {
        let step = self.reader.blur();
        self.due_ms = step.timer;
    }

    /// A key reached the terminal ("Keys and their window"): its window opens, and the
    /// next read takes the colours.
    pub fn key(&mut self, key: &TerminalKey) -> TerminalSpeech {
        self.colours_due = true;
        let step = self.reader.key(rules_key(key));
        self.take(&step)
    }

    /// The caret as the last read found it.
    #[must_use]
    pub fn caret(&self) -> Option<review::CaretAt> {
        self.caret
    }

    /// The quiet-period timer came due at `now_ms`: what waited for it.
    pub fn timer(&mut self, now_ms: u64) -> TerminalSpeech {
        let step = self.reader.timer(now_ms);
        self.take(&step)
    }

    /// Keeps what the next read and the review need of `read`, which the
    /// reading rules handled with `step`, taking nothing again: the
    /// screen's size, the caret, whether it started at the document's
    /// start, when it began, and the timer.
    fn handled(&mut self, read: &Read, step: &Step) {
        if let Some(outcome) = &step.outcome {
            self.screen_rows = u32::try_from(outcome.height).unwrap_or(u32::MAX);
            self.width = outcome.width;
            self.caret = outcome.caret.map(|row| review::CaretAt {
                row,
                column: read.caret_column,
            });
        }
        self.at_document_start = read.at_document_start;
        self.read_started_ms = read.at_ms;
        self.due_ms = step.timer;
    }

    /// What `step` says, its timer kept.
    fn take(&mut self, step: &Step) -> TerminalSpeech {
        self.due_ms = step.timer;
        speech_of(step)
    }
}

/// What the reading rules said, as Core takes it.
#[must_use]
pub fn speech_of(step: &Step) -> TerminalSpeech {
    let items = step
        .speech
        .iter()
        .map(|speech| match speech {
            Speech::Line { text, .. } => TerminalItem::Output(text.to_string()),
            Speech::Changed { text, .. } | Speech::Held { text } => {
                TerminalItem::Output(text.clone())
            }
            // What a line gained, without the space that parts it from the
            // text it follows, which says nothing.
            Speech::Growth { text, .. } => TerminalItem::Growth(text.trim_start().to_owned()),
            // After a lost read, its lines follow a count of none of how
            // they are counted, to which each adds when Core skips it
            // ("Core's flood policy").
            Speech::LostRead(Lost::About) => TerminalItem::Skipped(Skipped::About(0)),
            Speech::LostRead(Lost::MoreThan) => TerminalItem::Skipped(Skipped::MoreThan(0)),
            Speech::LostRead(Lost::Uncounted) => TerminalItem::Skipped(Skipped::Uncounted),
            Speech::Echo { text, .. } => TerminalItem::Echo(text.clone()),
            Speech::Caret { unit, text, .. } => TerminalItem::Caret {
                unit: match unit {
                    Unit::Character => TerminalUnit::Character,
                    Unit::Word => TerminalUnit::Word,
                    Unit::Line => TerminalUnit::Line,
                },
                text: text.clone(),
            },
            Speech::Gained { text, .. } => TerminalItem::Gained(text.clone()),
            Speech::Lost { text, .. } => TerminalItem::Lost(text.clone()),
            Speech::Answer { text, .. } => TerminalItem::Answer(text.clone()),
            Speech::Suggestion { text, .. } => TerminalItem::Suggestion(text.clone()),
        })
        .collect();
    TerminalSpeech {
        items,
        followed: step.followed,
    }
}

/// The reading rules' form of a snapshot taken at `at_ms`: its text as
/// UTF-8, and the caret's row and its column in characters, from the caret
/// read with it.
#[must_use]
pub fn read_of(snapshot: &Snapshot, at_ms: u64) -> Read {
    let (caret_line, caret_column) = snapshot.caret.as_ref().map_or_else(
        || (String::new(), 0),
        |caret| caret_row(&caret.line.text, caret.line.offset),
    );
    Read {
        text: ReadText::unpadded(&String::from_utf16_lossy(&snapshot.text)),
        screen: String::from_utf16_lossy(&snapshot.screen),
        from_start: snapshot.from_start,
        shrank: snapshot.shrank,
        at_document_start: snapshot.at_document_start,
        alternate: snapshot.alternate,
        first_row: snapshot.first_row.clone(),
        caret_rows_to_end: snapshot.caret_rows_to_end,
        caret_line,
        caret_column,
        at_ms,
        above: usize::try_from(snapshot.above).unwrap_or(usize::MAX),
    }
}

/// The caret's row as UTF-8, and the caret's column in it in characters,
/// from the row's UTF-16 text and the code units before the caret.
fn caret_row(row: &[u16], offset: usize) -> (String, usize) {
    let before = String::from_utf16_lossy(&row[..offset.min(row.len())]);
    (String::from_utf16_lossy(row), before.chars().count())
}

/// The read program's colours as the reading rules take them.
fn colours_of(colours: &[Vec<verbatim_uia_rops::ColourRun>]) -> Vec<Vec<ColourRun>> {
    colours
        .iter()
        .map(|row| {
            row.iter()
                .map(|run| ColourRun {
                    column: run.column,
                    length: run.length,
                    foreground: run.foreground,
                    background: run.background,
                })
                .collect()
        })
        .collect()
}

/// Whether a failed remote operation or classic read means the terminal
/// is gone.
fn is_gone(error: &RopsError) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}

/// What [`read`] found.
pub struct ReadAnswer {
    /// What the reading rules said.
    pub speech: TerminalSpeech,
    /// The caret the read found.
    pub caret: Option<CaretAnswer>,
    /// The path the last snapshot took, for the caller to log a fallback.
    pub path: Option<Path>,
    /// When the read began, in milliseconds on the shared clock: what it
    /// found is as of then.
    pub started_ms: u64,
}

/// Reads `terminal` through UIA and gives the read to its reading rules,
/// with the caret and its line read in the same round trip (`caret`), and
/// the colours when a key reached the terminal since the last read. When
/// the rules ask for the whole document, it is read at once and given
/// instead ("The read"). `waited` says whether the read waited for Core's
/// speech queue ("Volatile rows"). `remote` tries the remote program first.
///
/// # Errors
///
/// [`TextError::Gone`] when the terminal is gone, [`TextError::Failed`]
/// when it could not be read.
pub fn read<'a>(
    uia: &'a Uia,
    (element, pattern): (&'a IUIAutomationElement, &'a IUIAutomationTextPattern),
    caret: CaretLineQuery<'a>,
    terminal: &mut Terminal,
    (remote, waited): (bool, bool),
) -> Result<ReadAnswer, TextError> {
    let started_ms = crate::protocol::now_us() / 1_000;
    let start = terminal
        .start
        .as_ref()
        .and_then(|range| range.resolve().ok());
    let colours = terminal.colours_due.then_some(terminal.width);
    let above = u32::try_from(terminal.reader.memory().above()).unwrap_or(u32::MAX);
    let mut whole = false;
    // A first read read again with its history block: the screen's height
    // it found.
    let mut history_block: Option<u32> = None;
    loop {
        let query = SnapshotQuery {
            element,
            pattern,
            start: if whole || history_block.is_some() {
                None
            } else {
                start.as_ref()
            },
            screen_rows: history_block.unwrap_or(terminal.screen_rows),
            whole,
            above: if whole {
                0
            } else {
                history_block.unwrap_or(above)
            },
            caret: Some(caret),
            colours,
        };
        let timer = std::time::Instant::now();
        let calls_before = verbatim_uia::calls::peek().uia;
        let result = terminal_snapshot(uia, &query, remote);
        let elapsed_us = u64::try_from(timer.elapsed().as_micros()).unwrap_or(u64::MAX);
        let calls = verbatim_uia::calls::peek().uia.saturating_sub(calls_before);
        let (mut snapshot, path) = match result {
            Ok(answer) => answer,
            Err(error) => {
                tracing::debug!(elapsed_us, calls, %error, "terminal snapshot failed");
                return Err(if is_gone(&error) {
                    TextError::Gone
                } else {
                    TextError::Failed(error.to_string())
                });
            }
        };
        // What each read costs (`docs/performance.md`, "A terminal
        // flood").
        tracing::debug!(
            path = path.name(),
            elapsed_us,
            calls,
            whole,
            above,
            text_units = snapshot.text.len(),
            from_start = snapshot.from_start,
            shrank = snapshot.shrank,
            alternate = snapshot.alternate,
            colours = snapshot.colours.is_some(),
            "terminal snapshot timing"
        );
        let read = read_of(&snapshot, started_ms);
        let read_colours = snapshot.colours.as_deref().map(colours_of);
        let step = terminal.reader.read(&read, waited, read_colours.as_deref());
        // The rules ask for the whole document at most once: a whole read
        // is never asked for again.
        if step.whole_document && !whole {
            whole = true;
            continue;
        }
        // A first read with no rows above its screen is read again from a
        // screen above it, once.
        if step.history_block && history_block.is_none() {
            history_block = Some(step.outcome.as_ref().map_or(0, |outcome| {
                u32::try_from(outcome.height).unwrap_or(u32::MAX)
            }));
            continue;
        }
        terminal.handled(&read, &step);
        // A read with no text leaves where the next read starts as the last
        // read placed it ("A read with no text").
        if !step.outcome.as_ref().is_some_and(|outcome| outcome.blank) {
            terminal.start = snapshot
                .next_start
                .take()
                .and_then(|range| AgileReference::new(&range).ok());
        }
        if colours.is_some() {
            terminal.colours_due = false;
        }
        return Ok(ReadAnswer {
            speech: speech_of(&step),
            caret: snapshot.caret.take(),
            path: Some(path),
            started_ms,
        });
    }
}

/// The text one message's terminal speech may take, well inside
/// [`crate::protocol::MAX_MESSAGE_BYTES`], since JSON's escaping can make a
/// line several times longer ([`split`]).
pub const MESSAGE_TEXT_BUDGET: usize = crate::protocol::MAX_MESSAGE_BYTES / 8;

/// `speech` split into parts that each take at most `budget` bytes of
/// text, in order, so that a message carrying one stays within
/// [`crate::protocol::MAX_MESSAGE_BYTES`]. A single item larger than the
/// budget is a part of its own; the review cursor's following goes with the
/// last part. Speech that fits is returned as it is.
#[must_use]
pub fn split(speech: TerminalSpeech, budget: usize) -> Vec<TerminalSpeech> {
    let size = |item: &TerminalItem| item.text().len() + 16;
    if speech.items.iter().map(size).sum::<usize>() <= budget {
        return vec![speech];
    }
    let mut parts: Vec<TerminalSpeech> = Vec::new();
    let mut current = TerminalSpeech::default();
    let mut used = 0;
    for item in speech.items {
        let cost = size(&item);
        if used + cost > budget && !current.items.is_empty() {
            parts.push(std::mem::take(&mut current));
            used = 0;
        }
        used += cost;
        current.items.push(item);
    }
    current.followed = speech.followed;
    parts.push(current);
    parts
}

/// The reading rules' key for what Core sent.
#[must_use]
pub fn rules_key(key: &TerminalKey) -> Key {
    Key {
        kind: match &key.kind {
            TerminalKeyKind::Typed(text) => KeyKind::Typed(text.clone()),
            TerminalKeyKind::Caret(unit) => KeyKind::Caret(match unit {
                TerminalUnit::Character => Unit::Character,
                TerminalUnit::Word => Unit::Word,
                TerminalUnit::Line => Unit::Line,
            }),
            TerminalKeyKind::Enter => KeyKind::Enter,
            TerminalKeyKind::Forget => KeyKind::Forget,
            TerminalKeyKind::Delete => KeyKind::Delete,
            TerminalKeyKind::Other => KeyKind::Other,
        },
        at_ms: key.at_ms,
    }
}

#[cfg(test)]
mod tests;
