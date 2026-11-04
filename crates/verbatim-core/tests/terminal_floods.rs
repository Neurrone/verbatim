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
    outpost
        .sim
        .write(format!("{text}{PROMPT}command").as_bytes());
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
    session.write(
        &format!("{}{PROMPT}", written("flood line", 13..=2000)),
        200,
    );
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
    session.write(
        &format!("{}{PROMPT}", written("after line", 1..=after)),
        200,
    );
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
    past_history(
        Flavor::WindowsTerminal,
        "Phrase(SkippedMoreThanLines(9016))",
    );
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
    session.write(
        &format!("{}{PROMPT}", written("flood line", 33..=2000)),
        200,
    );
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

/// A full-screen program drawing its 30 rows on the alternate screen in one
/// write, at limits of 10: the new screen's rows are a burst like any
/// other, its first 10 heard, the 10 between skipped and its last 10
/// heard; closing it brings the main screen back as a new screen, whose
/// rows are spoken whole.
fn redraw_limit(flavor: Flavor) {
    let mut session = terminal(flavor, "", 10);
    let rows = (1..=30)
        .map(|row| format!("screen row {row}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    session.write(&format!("\r\n\x1b[?1049h\x1b[H\x1b[2J{rows}"), 100);
    session.play();
    let mut speech = heard("screen row", 1..=10);
    speech.push(said(None, "Phrase(SkippedLines(10))"));
    speech.extend(heard("screen row", 21..=30));
    assert_eq!(session.heard(), speech);
    assert_eq!(session.requests(), requests(false));
    session.write(&format!("\x1b[?1049lclosed\r\n{PROMPT}"), 200);
    session.play();
    assert_eq!(
        session.heard(),
        vec![
            said(None, &format!("{PROMPT}command")),
            said(None, "closed"),
            said(None, PROMPT.trim_end()),
        ]
    );
    assert_eq!(session.requests(), Vec::new());
}

#[test]
fn a_full_screen_redraw_goes_through_the_limits_in_the_console_host() {
    redraw_limit(Flavor::ConsoleHost);
}

#[test]
fn a_full_screen_redraw_goes_through_the_limits_in_windows_terminal() {
    redraw_limit(Flavor::WindowsTerminal);
}

/// Windows PowerShell's `Clear-Host` in Windows Terminal, as it writes it:
/// the caret homed, then the screen and the history erased.
const CLEAR_HOST: &str = "\x1b[H\x1b[2J\x1b[3J";

/// In Windows Terminal, 200 lines of history and a group's worth of lines
/// at limits of 10, then, once the first has been heard, the screen and
/// history cleared and 100 lines written while the outpost holds, as
/// `clear; dir` in one line: the read at the group's end finds the document
/// shorter than the last read's, its start refused and nothing of it kept,
/// so the whole document is read and is all new, its 101 lines counted as
/// about, less the 10 heard last.
#[test]
fn output_after_windows_terminals_clear_is_all_new_and_counted_as_about() {
    let mut session = terminal(Flavor::WindowsTerminal, &written("old line", 1..=200), 10);
    session.write(&format!("\r\n{}", written("before line", 1..=10)), 100);
    assert!(session.play_one());
    assert_eq!(session.heard(), heard("before line", 1..=1));
    session.write(
        &format!("{CLEAR_HOST}{}{PROMPT}", written("cleared line", 1..=100)),
        200,
    );
    session.play();
    assert_eq!(
        session.heard(),
        burst(
            heard("before line", 2..=10),
            "Phrase(SkippedAboutLines(91))",
            heard("cleared line", 92..=100)
        )
    );
    assert_eq!(session.requests(), requests(true));
}

/// In Windows Terminal at limits of 15, a group's worth of lines; once the
/// first has been heard, the screen and history cleared and a group's worth
/// written with no line break after the last, so the screen starts at the
/// document's start; once the first of those has been heard, 9,100 lines
/// written while the outpost holds. The read at the cleared group's end
/// keeps nothing of the screen before, which the clear set aside and which
/// is no longer in the buffer, so it is a lost read, and the whole
/// document, Windows Terminal's 9,031 rows, holds nothing of the last
/// read: more went by than it holds, less the 15 heard last.
#[test]
fn a_flood_after_windows_terminals_clear_says_more_than_it_holds() {
    let mut session = terminal(Flavor::WindowsTerminal, "", 15);
    session.write(&format!("\r\n{}", written("before line", 1..=15)), 100);
    assert!(session.play_one());
    assert_eq!(session.heard(), heard("before line", 1..=1));
    let cleared = written("cleared line", 1..=15);
    session.write(
        &format!("{CLEAR_HOST}{}", cleared.trim_end_matches("\r\n")),
        200,
    );
    for _ in 0..15 {
        assert!(session.play_one());
    }
    let mut first = heard("before line", 2..=15);
    first.extend(heard("cleared line", 1..=1));
    assert_eq!(session.heard(), first);
    session.write(
        &format!("\r\n{}{PROMPT}", written("fill line", 1..=9100)),
        300,
    );
    session.play();
    assert_eq!(
        session.heard(),
        burst(
            heard("cleared line", 2..=15),
            "Phrase(SkippedMoreThanLines(9016))",
            heard("fill line", 9087..=9100)
        )
    );
    // The read at the before lines' group's end, holding again for the
    // cleared lines' group, then that group's requests, its read lost.
    let mut both = vec![TextOp::TerminalHold, TextOp::TerminalRead { hold: true }];
    both.extend(requests(true).into_iter().skip(1));
    assert_eq!(session.requests(), both);
}

/// A fresh console host, with no history, taking 9,100 lines in one write
/// before its first read, at limits of 10: its 9,001 rows keep fill lines
/// 101 to 9,100 and the prompt, the read keeps nothing of the screen before
/// and no screen is aside, so it is a lost read, and the whole document
/// holds nothing of the last read: its first 10 lines are heard, then more
/// than it holds less the 20 heard went by.
#[test]
fn a_flood_onto_a_fresh_console_host_says_more_than_it_holds() {
    let mut session = terminal(Flavor::ConsoleHost, "", 10);
    session.write(
        &format!("\r\n{}{PROMPT}", written("fill line", 1..=9100)),
        100,
    );
    session.play();
    assert_eq!(
        session.heard(),
        burst(
            heard("fill line", 101..=110),
            "Phrase(SkippedMoreThanLines(8981))",
            heard("fill line", 9092..=9100)
        )
    );
    let mut lost = vec![TextOp::TerminalReadNow(TerminalScope::Whole)];
    lost.extend(requests(false));
    assert_eq!(session.requests(), lost);
}

/// The console host with the screen cleared, its last row a footer below a
/// scroll region over the rows above, at limits of 15: a group's worth of
/// lines written into the region, then, once the first has been heard,
/// lines to 12,000 and the footer redrawn while the outpost holds. The read
/// at the group's end finds its start gone with the rotating history, the
/// whole document holds nothing of the last read but the footer at its
/// row, so the lines above it are all new: more than the console host's
/// 9,001 rows less the 15 heard last went by, then the footer's new text.
/// Removing the region and clearing the screen speaks what follows. Each
/// line starts with a carriage return and a line feed, as the console
/// host's default output mode turns a line feed into both.
#[test]
fn a_flood_above_a_footer_past_the_console_hosts_history_says_more_than_it_holds() {
    let mut session = terminal(Flavor::ConsoleHost, "", 15);
    session.write("\r\n\x1b[2J\x1b[1;29r\x1b[30;1Hstatus: busy\x1b[29;1H", 100);
    session.play();
    assert_eq!(session.heard(), vec![said(None, "status: busy")]);
    assert_eq!(session.requests(), Vec::new());
    let lines = |range: std::ops::RangeInclusive<u32>| {
        range.fold(String::new(), |mut text, line| {
            let _ = write!(text, "\r\nflood line {line}");
            text
        })
    };
    session.write(&lines(1..=15), 200);
    assert!(session.play_one());
    assert_eq!(session.heard(), heard("flood line", 1..=1));
    session.write(
        &format!("{}\x1b[s\x1b[30;1Hstatus: done\x1b[u", lines(16..=12_000)),
        300,
    );
    session.play();
    let mut speech = heard("flood line", 2..=15);
    speech.push(said(None, "Phrase(SkippedMoreThanLines(8986))"));
    speech.extend(heard("flood line", 11_987..=12_000));
    speech.push(said(None, "status: done"));
    assert_eq!(session.heard(), speech);
    assert_eq!(session.requests(), requests(true));
    session.write(&format!("\x1b[r\x1b[2J\x1b[Hfooter gone\r\n{PROMPT}"), 400);
    session.play();
    assert_eq!(
        session.heard(),
        vec![said(None, "footer gone"), said(None, PROMPT.trim_end())]
    );
    assert_eq!(session.requests(), Vec::new());
}
