//! The outpost's decision to send a terminal's read to Core
//! ([`Terminal::sends`]), on the simulated terminal (`verbatim-term-sim`)
//! in place of UI Automation, with the reads it sends given to the reading
//! rules as Core gives them; and a read's text converted in one pass
//! ([`super::unpadded_utf16`]) exactly as with two copies, with a
//! measurement of what that saves.

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

/// What the outpost made of a read's UTF-16 text before it was converted
/// in one pass: a copy as UTF-8, then a copy of that without the padding.
fn two_copies(units: &[u16]) -> TerminalText {
    TerminalText::unpadded(&String::from_utf16_lossy(units))
}

/// `units` converted in one pass into `reused`, buffers a read used
/// before, checked against the two copies field by field and as the bytes
/// sent; the buffers are returned for the next.
fn converted_alike(units: &[u16], reused: TerminalText, what: &str) -> TerminalText {
    let one_pass = super::unpadded_utf16(units, reused);
    let expected = two_copies(units);
    assert_eq!(one_pass, expected, "{what}");
    assert_eq!(
        serde_json::to_vec(&one_pass).expect("serializes"),
        serde_json::to_vec(&expected).expect("serializes"),
        "{what}"
    );
    one_pass
}

/// Every corpus stream's whole document and screen, after each chunk, in
/// both terminals, converts in one pass exactly as with two copies, into
/// buffers reused from one read to the next.
#[test]
fn a_reads_text_converted_in_one_pass_is_the_two_copies_exactly() {
    use verbatim_term_sim::conformance::{history, streams};
    let mut reused = TerminalText::default();
    for (name, stream) in streams() {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let mut terminal = verbatim_term_sim::Terminal::new(flavor, 120, 30, history(flavor));
            for (chunk, bytes) in stream.chunks.iter().enumerate() {
                terminal.write(bytes.as_bytes());
                let snapshot = terminal
                    .snapshot(&Query {
                        start: None,
                        screen_rows: 30,
                        whole: true,
                        above: 0,
                    })
                    .expect("a simulated read");
                for (part, text) in [("text", &snapshot.text), ("screen", &snapshot.screen)] {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    reused = converted_alike(
                        &units,
                        reused,
                        &format!("{name} in {flavor:?}, chunk {chunk}, {part}"),
                    );
                }
            }
        }
    }
}

/// Unpaired surrogates become U+FFFD as with two copies, wherever they
/// are, and line breaks, lone carriage returns and line feeds, padding and
/// other white space are kept or left out alike.
#[test]
fn unpaired_surrogates_and_line_ends_convert_as_the_two_copies() {
    let cases: [&[u16]; 14] = [
        &[],
        &[0xD800],
        &[0xDC00, 0x61],
        &[0x61, 0xD83D, 0x20, 0x20, 0x0D, 0x0A, 0x62],
        &[0x61, 0x20, 0xD800, 0x20, 0x0D, 0x0A],
        &[0xD83D, 0xDE00, 0x20, 0x20, 0x0D, 0x0A, 0x20, 0x20],
        &[0x0D],
        &[0x20, 0x0D],
        &[0x0D, 0x0D, 0x0A, 0x0A],
        &[0x61, 0x20, 0x0A, 0x20, 0x0D, 0x0A],
        &[0x0D, 0x0A, 0x0D, 0x0A],
        &[0x61, 0x09, 0x20, 0xA0, 0x20, 0x0D, 0x0A, 0x62, 0x20],
        &[0x20, 0x20, 0x20, 0xDBFF, 0x0D, 0x20, 0x0A],
        &[
            0x6F, 0x6E, 0x65, 0x20, 0x20, 0x0D, 0x0A, 0x74, 0x77, 0x6F, 0x20, 0x20,
        ],
    ];
    let mut reused = TerminalText::default();
    for (case, units) in cases.iter().enumerate() {
        reused = converted_alike(units, reused, &format!("case {case}: {units:04X?}"));
    }
}

/// `K32GetProcessMemoryInfo`'s counters, of which the measurement reads
/// the page faults and the working set.
#[repr(C)]
#[derive(Default)]
struct MemoryCounters {
    size: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged_pool: usize,
    quota_paged_pool: usize,
    quota_peak_non_paged_pool: usize,
    quota_non_paged_pool: usize,
    pagefile: usize,
    peak_pagefile: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut MemoryCounters, size: u32) -> i32;
}

/// This process's page faults so far.
fn page_faults() -> u32 {
    let mut counters = MemoryCounters {
        size: u32::try_from(std::mem::size_of::<MemoryCounters>()).expect("small"),
        ..MemoryCounters::default()
    };
    // SAFETY: the pseudo handle of the current process needs no closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: the counters are a writable struct of the size given.
    let ok = unsafe { K32GetProcessMemoryInfo(process, &raw mut counters, counters.size) };
    assert_ne!(ok, 0, "the process's memory counters");
    counters.page_faults
}

/// A terminal's text as its provider gives it: `rows` rows of 120 cells,
/// each padded and ended by a line break, as UTF-16.
fn padded_rows(rows: usize) -> Vec<u16> {
    use std::fmt::Write as _;
    let mut text = String::new();
    for row in 0..rows {
        let line = format!("PS C> line {row} of the history");
        let _ = write!(text, "{line:<120}\r\n");
    }
    text.encode_utf16().collect()
}

/// The time and page faults of converting and writing a terminal's read,
/// before and after the conversion in one pass with buffers kept: run with
/// `cargo test --release -p verbatim-outpost --lib
/// terminal::tests::measure_conversion -- --ignored --nocapture
/// --test-threads=1`. A whole document of a full history (9,001 rows and
/// a screen of 30) and an ordinary read (two screens), each read 50 times
/// after one to warm up; "before" converts with two copies and writes into
/// a buffer that keeps whatever it grew to, as the transport's did, "after"
/// converts in one pass into the spare buffers ([`super::recycle`]) and
/// writes into a buffer kept to `WRITE_KEPT_BYTES` once each write is done.
#[test]
#[ignore = "a measurement: run in a release build, by hand"]
fn measure_conversion() {
    const RUNS: u32 = 50;
    use verbatim_process::overlapped::WRITE_KEPT_BYTES;
    for (what, units) in [
        ("whole document", padded_rows(9_031)),
        ("ordinary read", padded_rows(60)),
    ] {
        let screen: Vec<u16> = units[units.len() - 30 * 122..].to_vec();
        let wire = |text: TerminalText, screen: TerminalText| TerminalSnapshot {
            text,
            screen,
            from_start: true,
            shrank: false,
            at_document_start: false,
            alternate: false,
            first_row: TerminalText::default(),
            caret_rows_to_end: 1,
            caret_line: TerminalText::default(),
            caret_column: 0,
            above: 30,
            started_ms: 0,
            waited: false,
            colours: None,
            caret: None,
        };
        // Before: two copies, and a write buffer that keeps its size.
        let mut buffer = Vec::new();
        let mut before = || {
            let snapshot = wire(two_copies(&units), two_copies(&screen));
            buffer.clear();
            crate::protocol::put_message(&mut buffer, &snapshot).expect("written");
        };
        before();
        let (faults, started) = (page_faults(), std::time::Instant::now());
        for _ in 0..RUNS {
            before();
        }
        let before_us = started.elapsed().as_micros() / u128::from(RUNS);
        let before_faults = (page_faults() - faults) / RUNS;
        let before_held = buffer.capacity();
        // After: one pass into spare buffers, given back once written, and
        // a write buffer kept to its bound.
        let mut buffer = Vec::new();
        let mut after = || {
            let (text, spare_screen) = super::spare();
            let snapshot = wire(
                super::unpadded_utf16(&units, text),
                super::unpadded_utf16(&screen, spare_screen),
            );
            buffer.clear();
            crate::protocol::put_message(&mut buffer, &snapshot).expect("written");
            if buffer.capacity() > WRITE_KEPT_BYTES {
                buffer.clear();
                buffer.shrink_to(WRITE_KEPT_BYTES);
            }
            super::recycle(snapshot);
        };
        after();
        let (faults, started) = (page_faults(), std::time::Instant::now());
        for _ in 0..RUNS {
            after();
        }
        let after_us = started.elapsed().as_micros() / u128::from(RUNS);
        let after_faults = (page_faults() - faults) / RUNS;
        let spare: usize = super::SPARE
            .lock()
            .expect("the spare buffers")
            .iter()
            .map(|(text, screen)| text.text.capacity() + screen.text.capacity())
            .sum();
        println!(
            "{what}: before {before_us} us and {before_faults} page faults a read, {before_held} \
             bytes held after; after {after_us} us and {after_faults} page faults a read, {} \
             bytes held after ({} written, {spare} spare)",
            buffer.capacity() + spare,
            buffer.capacity()
        );
    }
}
