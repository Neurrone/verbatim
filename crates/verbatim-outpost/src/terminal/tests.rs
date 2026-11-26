//! A measurement of what converting a terminal's read in one pass
//! ([`unpadded_utf16`]) saves, and of what its answer costs to write and
//! to parse. It is here, outside the text module, because it reads the
//! process's page faults from Windows.

use std::time::Instant;

use verbatim_model::batch::{Done, RangeText, StepOutcome, VisibleText};
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::request::{Answer, Outcome, Reply, RequestNumber};
use verbatim_model::{TerminalColourRun, TerminalText, TextAnchor};
use verbatim_process::message::{decode_message, encode_message, put_message};
use verbatim_process::transport::read_frame;

use crate::text::trim::unpadded_utf16;

/// What the outpost made of a read's UTF-16 text before it was converted
/// in one pass: a copy as UTF-8, then a copy of that without the padding.
fn two_copies(units: &[u16]) -> TerminalText {
    TerminalText::unpadded(&String::from_utf16_lossy(units))
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
/// each a numbered line in `words`' language, padded and ended by a line
/// break, as UTF-16.
fn padded_rows(rows: usize, (before, after): (&str, &str)) -> Vec<u16> {
    use std::fmt::Write as _;
    let mut text = String::new();
    for row in 0..rows {
        let line = format!("PS C> {before} {row} {after}");
        let _ = write!(text, "{line:<120}\r\n");
    }
    text.encode_utf16().collect()
}

/// A screen's colours at their most: 30 rows of 120 runs of one cell.
fn screen_colours() -> Vec<Vec<TerminalColourRun>> {
    let row: Vec<TerminalColourRun> = (0..120)
        .map(|column| TerminalColourRun {
            column,
            length: 1,
            foreground: column,
            background: 0,
        })
        .collect();
    vec![row; 30]
}

/// A whole read of `text` and `screen`, with `colours`, in the answer the
/// outpost sends Core: a batch's, whose steps answered the visible text, a
/// trimmed range and the screen's colours.
fn answer(
    text: TerminalText,
    screen: TerminalText,
    colours: Option<Vec<Vec<TerminalColourRun>>>,
) -> OutpostToCore {
    let mut steps = vec![
        StepOutcome::Done(Done::Visible(VisibleText {
            start: TextAnchor(1),
            end: TextAnchor(2),
            at_text_start: false,
            text: screen.text,
            padded: screen.lengths,
        })),
        StepOutcome::Done(Done::ReadRange(RangeText {
            text: text.text,
            truncated: false,
            padded: Some(text.lengths),
        })),
    ];
    steps.extend(colours.map(|rows| StepOutcome::Done(Done::ReadColours(rows))));
    OutpostToCore::Answer(Answer {
        number: RequestNumber(1),
        outcome: Outcome::Result(Reply::Steps(steps)),
        probes: Vec::new(),
        detail: String::new(),
        began_us: 0,
        ended_us: 0,
    })
}

/// The time and page faults of converting a terminal's read and writing
/// it in the answer the outpost sends Core, before and after the conversion
/// in one pass, and the time Core takes to parse that
/// answer's frame: run with `cargo test --release -p verbatim-outpost --lib
/// terminal::tests::measure_conversion -- --ignored --nocapture
/// --test-threads=1`. A whole document of a full history (9,031 rows and
/// a screen of 30) in Latin and in Cyrillic text, an ordinary read (two
/// screens), and an ordinary read with its screen's colours at their most
/// ([`screen_colours`]), each read 50 times after one to warm up.
/// "Before" converts with two copies and writes into a buffer that keeps
/// whatever it grew to, as the transport's did; "after" converts in one
/// pass into new buffers and writes into a buffer kept to
/// `WRITE_KEPT_BYTES` once each write is done, as the outpost's writer
/// does; "parse" decodes the frame's body and raw section as Core's
/// reducer does.
#[test]
#[ignore = "a measurement: run in a release build, by hand"]
fn measure_conversion() {
    const RUNS: u32 = 50;
    const LATIN: (&str, &str) = ("line", "of the history");
    const CYRILLIC: (&str, &str) = ("строка", "истории");
    use verbatim_process::overlapped::WRITE_KEPT_BYTES;
    for (what, units, colours) in [
        ("whole document", padded_rows(9_031, LATIN), None),
        (
            "whole document in Cyrillic",
            padded_rows(9_031, CYRILLIC),
            None,
        ),
        ("ordinary read", padded_rows(60, LATIN), None),
        (
            "ordinary read with colours",
            padded_rows(60, LATIN),
            Some(screen_colours()),
        ),
    ] {
        let screen: Vec<u16> = units[units.len() - 30 * 122..].to_vec();
        let wire = |text, screen| answer(text, screen, colours.clone());
        // Before: two copies, and a write buffer that keeps its size.
        let mut buffer = Vec::new();
        let mut before = || {
            let message = wire(two_copies(&units), two_copies(&screen));
            buffer.clear();
            put_message(&mut buffer, &message).expect("written");
        };
        before();
        let (faults, started) = (page_faults(), Instant::now());
        for _ in 0..RUNS {
            before();
        }
        let before_us = started.elapsed().as_micros() / u128::from(RUNS);
        let before_faults = (page_faults() - faults) / RUNS;
        let before_held = buffer.capacity();
        // After: one pass into new buffers, and a write buffer kept to its
        // bound.
        let mut buffer = Vec::new();
        let mut after = || {
            let message = wire(unpadded_utf16(&units), unpadded_utf16(&screen));
            buffer.clear();
            put_message(&mut buffer, &message).expect("written");
            if buffer.capacity() > WRITE_KEPT_BYTES {
                buffer.clear();
                buffer.shrink_to(WRITE_KEPT_BYTES);
            }
        };
        after();
        let (faults, started) = (page_faults(), Instant::now());
        for _ in 0..RUNS {
            after();
        }
        let after_us = started.elapsed().as_micros() / u128::from(RUNS);
        let after_faults = (page_faults() - faults) / RUNS;
        // Parse: the answer's frame, its body and raw section, read back.
        let message = wire(two_copies(&units), two_copies(&screen));
        let frame = encode_message(&message).expect("encoded");
        let frame = read_frame(&mut frame.as_slice())
            .expect("read")
            .expect("a frame");
        let parts = frame.parts();
        let raw = parts.raw.expect("such an answer has a raw section");
        let parse = || decode_message::<OutpostToCore>(parts).expect("parsed");
        assert_eq!(parse(), message, "the answer read back");
        let started = Instant::now();
        for _ in 0..RUNS {
            std::hint::black_box(parse());
        }
        let parse_us = started.elapsed().as_micros() / u128::from(RUNS);
        println!(
            "{what}: before {before_us} us and {before_faults} page faults a read, {before_held} \
             bytes held after; after {after_us} us and {after_faults} page faults a read, {} \
             bytes held after; parse {parse_us} us for a body of {} bytes and a raw section of \
             {}",
            buffer.capacity(),
            parts.body.len(),
            raw.len()
        );
    }
}
