//! The outpost's decision to send a terminal's read to Core
//! ([`Terminal::sends`]), on the simulated terminal (`verbatim-term-sim`)
//! in place of UI Automation, with the reads it sends given to the reading
//! rules as Core gives them.

use verbatim_model::{TerminalSnapshot, TerminalText};
use verbatim_term_sim::{Flavor, Query, Range};
use verbatim_text::terminal_reading::{Settings, Speech, Step, TerminalReader};
use verbatim_text::terminal_rows::{Read, ReadText};

use super::Terminal;

/// The simulated terminal read as the outpost reads it, each read the
/// outpost sends given to the reading rules.
struct Watch {
    sim: verbatim_term_sim::Terminal,
    terminal: Terminal,
    reader: TerminalReader,
    start: Option<Range>,
    height: u32,
}

impl Watch {
    fn new() -> Self {
        Self {
            sim: verbatim_term_sim::Terminal::new(Flavor::ConsoleHost, 20, 4, 10),
            terminal: Terminal::new(),
            reader: TerminalReader::new(Settings {
                report_new_output: true,
                review_follows_caret: true,
            }),
            start: None,
            height: 0,
        }
    }

    /// Writes `bytes`, then a read begun at `at_ms`: the rules' step when
    /// the outpost sends it, `None` when it does not.
    fn write(&mut self, bytes: &str, at_ms: u64, waited: bool) -> Option<Step> {
        self.sim.write(bytes.as_bytes());
        let snapshot = self
            .sim
            .snapshot(&Query {
                start: self.start,
                screen_rows: self.height,
                whole: false,
                above: self.height,
            })
            .expect("a simulated read");
        self.start = Some(snapshot.next_start);
        let wire = TerminalSnapshot {
            text: TerminalText::unpadded(&snapshot.text),
            screen: TerminalText::unpadded(&snapshot.screen),
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: TerminalText::unpadded(&snapshot.first_row),
            caret_rows_to_end: snapshot.caret_rows_to_end,
            caret_line: TerminalText::unpadded(&snapshot.caret_line),
            caret_column: u32::try_from(snapshot.caret_column).expect("a short row"),
            above: snapshot.above,
            started_ms: at_ms,
            waited,
            colours: None,
            caret: None,
        };
        if !self.terminal.sends(&wire) {
            return None;
        }
        // As Core rebuilds the read.
        let read = Read {
            text: ReadText {
                text: wire.text.text.clone(),
                lengths: wire.text.lengths.clone(),
            },
            screen: wire.screen.padded(),
            from_start: wire.from_start,
            shrank: wire.shrank,
            at_document_start: wire.at_document_start,
            alternate: wire.alternate,
            first_row: wire.first_row.padded(),
            caret_rows_to_end: wire.caret_rows_to_end,
            caret_line: wire.caret_line.padded(),
            caret_column: wire.caret_column as usize,
            at_ms,
            above: wire.above as usize,
        };
        let step = self.reader.read(&read, waited, None);
        if let Some(outcome) = &step.outcome {
            self.height = u32::try_from(outcome.height).expect("a small screen");
        }
        Some(step)
    }
}

#[test]
fn a_read_identical_to_the_last_sent_is_not_sent_unless_it_waited() {
    let mut watch = Watch::new();
    assert!(watch.write("50%", 100, false).is_some());
    // The first read took the screen alone; this one a screen above it too.
    assert!(watch.write("", 200, false).is_some());
    assert!(watch.write("", 300, false).is_none());
    assert!(watch.write("", 400, true).is_some());
    assert!(watch.write("\r60%", 500, false).is_some());
}

/// A row changes at a read, a read identical to it follows that waited for
/// speech, and the row changes again at a read that waited: the identical
/// read is sent, so the two changes are not at reads in a row and the row
/// is not volatile ("Volatile rows").
#[test]
fn a_waited_read_identical_to_the_last_is_sent_so_a_row_changed_around_it_is_not_volatile() {
    let mut watch = Watch::new();
    watch.write("50%", 100, false);
    watch.write("", 200, false);
    let changed = watch.write("\r60%", 10_000, false).expect("sent");
    assert_eq!(
        changed.speech,
        vec![Speech::Changed {
            row: 0,
            text: "60%".to_owned(),
        }]
    );
    assert!(watch.write("", 15_000, true).is_some());
    let again = watch.write("\r70%", 20_000, true).expect("sent");
    assert_eq!(
        again.speech,
        vec![Speech::Changed {
            row: 0,
            text: "70%".to_owned(),
        }]
    );
    assert_eq!(again.withheld, Vec::new());
    assert_eq!(again.timer, None);
}

/// The read after the focus returns is sent though it is identical to the
/// last one sent: Core's reading rules take the first read after the focus
/// as the one that speaks nothing, so had it been dropped, the next read,
/// showing new output, would have said nothing of it.
#[test]
fn the_read_after_the_focus_returns_is_sent_though_identical() {
    let mut watch = Watch::new();
    watch.write("50%", 100, false);
    watch.write("", 200, false);
    assert!(watch.write("", 300, false).is_none());
    watch.terminal.focused();
    assert!(watch.write("", 400, false).is_some());
}
