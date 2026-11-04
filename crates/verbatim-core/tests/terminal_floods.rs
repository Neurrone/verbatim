//! Floods of terminal output through Core's terminal model and its flood
//! policy together (`docs/design/terminal-model.md`, "Core's flood policy"
//! and "A lost read"), driven through `reduce` with the simulated terminal
//! at the size the harness opens each terminal, 120 by 30 with its default
//! history (`verbatim_term_sim::conformance::history`), read as the outpost
//! reads it for Core and held while Core's queue is full
//! (`terminal_support`).

mod terminal_support;

use std::fmt::Write as _;

use terminal_support::{Outpost, Said, Session, said};
use verbatim_model::{Input, ReaderSettings, TerminalScope, TextOp};
use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::history;

/// The shell's prompt, with the caret after it.
const PROMPT: &str = "ready> ";

/// Lines `range` named `name`, each ended.
fn written(name: &str, range: std::ops::RangeInclusive<u32>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "{name} {line}\r\n");
        text
    })
}

/// Lines `range` named `name`, each heard with no key.
fn heard(name: &str, range: std::ops::RangeInclusive<u32>) -> Vec<Said> {
    range
        .map(|line| said(None, &format!("{name} {line}")))
        .collect()
}

/// A terminal of `flavor` at the harness's size, showing `text` with the
/// command just entered after it, and both flood limits at `limit`; what
/// Core queues is played only when the test plays it.
fn terminal(flavor: Flavor, text: &str, limit: u16) -> Session {
    let mut outpost = Outpost::new(flavor, 120, 30, history(flavor));
    outpost.sim.write(format!("{text}{PROMPT}command").as_bytes());
    let mut session = Session::with(outpost);
    session.input(&Input::Settings(ReaderSettings {
        terminal_full_lines: limit,
        terminal_last_lines: limit,
        ..ReaderSettings::default()
    }));
    session.manual_play = true;
    session
}

/// The requests of a burst read in two groups: the hold once a group's
/// worth waits, a read as each group's last line is handed to speech, the
/// whole document when `lost`, after the first, and the last read letting
/// the outpost read live again.
fn requests(lost: bool) -> Vec<TextOp> {
    let read = |hold| TextOp::TerminalRead { hold };
    let mut requests = vec![TextOp::TerminalHold, read(true)];
    if lost {
        requests.push(TextOp::TerminalReadNow(TerminalScope::Whole));
    }
    requests.extend([read(true), read(false)]);
    requests
}

/// The speech of a burst: `first` heard in full, the skip `phrase`, then
/// `last` and the prompt.
fn burst(first: Vec<Said>, phrase: &str, last: Vec<Said>) -> Vec<Said> {
    let mut speech = first;
    speech.push(said(None, phrase));
    speech.extend(last);
    speech.push(said(None, PROMPT.trim_end()));
    speech
}

/// A flood of 2,000 lines into a history already full, at limits of 10:
/// the first read takes 12 lines and Core holds the terminal; the rest is
/// written while it holds, carrying the first read's rows out of the
/// buffer, so the read Core asks for at the group's end finds its start
/// gone and Core reads the whole document, finding the last read's rows by
/// their text. The count is "about", the burst's 2,001 lines less the 10
/// heard each side.
fn full_history_flood(flavor: Flavor) {
    let mut session = terminal(flavor, &written("fill line", 1..=9100), 10);
    session.write(&format!("\r\n{}", written("flood line", 1..=12)), 100);
    session.write(&format!("{}{PROMPT}", written("flood line", 13..=2000)), 200);
    session.play();
    assert_eq!(
        session.heard(),
        burst(
            heard("flood line", 1..=10),
            "Phrase(SkippedAboutLines(1981))",
            heard("flood line", 1992..=2000)
        )
    );
    assert_eq!(session.requests(), requests(true));
}

#[test]
fn a_flood_through_a_full_history_is_counted_by_text_in_the_console_host() {
    full_history_flood(Flavor::ConsoleHost);
}

#[test]
fn a_flood_through_a_full_history_is_counted_by_text_in_windows_terminal() {
    full_history_flood(Flavor::WindowsTerminal);
}

/// A group's worth of lines at limits of 15, then `after` lines and the
/// prompt written once the first line has been heard, while the outpost
/// holds; returns the session with everything played.
fn during_group(flavor: Flavor, after: u32) -> Session {
    let mut session = terminal(flavor, "", 15);
    session.write(&format!("\r\n{}", written("before line", 1..=15)), 100);
    assert!(session.play_one());
    assert_eq!(session.heard(), heard("before line", 1..=1));
    session.write(&format!("{}{PROMPT}", written("after line", 1..=after)), 200);
    session.play();
    session
}

/// 5,000 lines landing while a group plays, within the history: the read
/// at the group's end starts at the rows the last read kept, so the count
/// is exact, the burst's 5,016 lines less the 15 heard each side.
fn within_history(flavor: Flavor) {
    let mut session = during_group(flavor, 5000);
    assert_eq!(
        session.heard(),
        burst(
            heard("before line", 2..=15),
            "Phrase(SkippedLines(4986))",
            heard("after line", 4987..=5000)
        )
    );
    assert_eq!(session.requests(), requests(false));
}

#[test]
fn a_flood_within_the_history_while_a_group_plays_is_counted_exactly_in_the_console_host() {
    within_history(Flavor::ConsoleHost);
}

#[test]
fn a_flood_within_the_history_while_a_group_plays_is_counted_exactly_in_windows_terminal() {
    within_history(Flavor::WindowsTerminal);
}

/// 12,000 lines landing while a group plays, more than the buffer holds:
/// the read at the group's end finds its start gone, and the lines skipped
/// are more than the buffer's rows less the 15 heard last, `phrase`.
fn past_history(flavor: Flavor, phrase: &str) {
    let mut session = during_group(flavor, 12_000);
    assert_eq!(
        session.heard(),
        burst(
            heard("before line", 2..=15),
            phrase,
            heard("after line", 11_987..=12_000)
        )
    );
    assert_eq!(session.requests(), requests(true));
}

#[test]
fn a_flood_past_the_history_while_a_group_plays_says_more_than_the_console_host_holds() {
    past_history(Flavor::ConsoleHost, "Phrase(SkippedMoreThanLines(8986))");
}

#[test]
fn a_flood_past_the_history_while_a_group_plays_says_more_than_windows_terminal_holds() {
    past_history(Flavor::WindowsTerminal, "Phrase(SkippedMoreThanLines(9016))");
}

/// `count` lines that read alike, each ended.
fn same(count: usize) -> String {
    "the same line\r\n".repeat(count)
}

/// 2,000 lines that read alike, the first 12 in a read of their own and the
/// rest while the outpost holds, at limits of 10: the read at the group's
/// end starts at the rows the last read kept, every one of which reads as
/// the lines below them do, and still counts exactly the lines below them.
#[test]
fn identical_lines_below_the_last_read_are_counted_exactly() {
    let mut session = terminal(Flavor::ConsoleHost, "", 10);
    session.write(&format!("\r\n{}", same(12)), 100);
    session.write(&format!("{}{PROMPT}", same(1988)), 200);
    session.play();
    let line = said(None, "the same line");
    assert_eq!(
        session.heard(),
        burst(
            vec![line.clone(); 10],
            "Phrase(SkippedLines(1981))",
            vec![line; 9]
        )
    );
    assert_eq!(session.requests(), requests(false));
}

/// A flood of 2,000 lines at the shipped limits of 30 starting under a line
/// of 40 words, wrapped onto three of the screen's 120-cell rows: the first
/// read takes 32 lines and Core holds the terminal, the rest is written
/// while it holds, and the read at the group's end counts the screen's
/// rows as rows, so none of the wrapped line's rows is counted as a line
/// that went by unread, and the count is exact, the burst's 2,001 lines
/// less the 30 heard each side.
#[test]
fn a_flood_that_starts_under_a_wrapped_line_counts_its_rows_once() {
    let words = (1..=40)
        .map(|n| format!("word{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut session = terminal(Flavor::ConsoleHost, &format!("{words}\r\n"), 30);
    session.write(&format!("\r\n{}", written("flood line", 1..=32)), 100);
    session.write(&format!("{}{PROMPT}", written("flood line", 33..=2000)), 200);
    session.play();
    assert_eq!(
        session.heard(),
        burst(
            heard("flood line", 1..=30),
            "Phrase(SkippedLines(1941))",
            heard("flood line", 1972..=2000)
        )
    );
    assert_eq!(session.requests(), requests(false));
}
