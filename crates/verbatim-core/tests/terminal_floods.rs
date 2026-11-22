//! Floods of terminal output through Core's window reads, its places and
//! its pacing together (`books/dev/src/explanation/design/terminal-model.md`,
//! "Core's flood policy"; build 9's design, 5.3 and 5.6), driven through
//! the public reducer with the simulated terminal at the size the harness
//! opens each terminal, 120 by 30 with its default history
//! (`verbatim_term_sim::conformance::history`), each step of Core's reads
//! answered as its provider answers them (`terminal_support`). No read is
//! sent while the pacing holds, so output written then is found by the read
//! at the group's last line.

mod desk;
#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;

use terminal_support::{Asked, Said, Session, said};
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
    let mut session = Session::new(
        flavor,
        (120, 30, history(flavor)),
        &format!("{text}{PROMPT}command"),
    );
    session.limits(limit);
    session.manual_play = true;
    session
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

/// A group's worth of lines at limits of 15, then `after` lines and the
/// prompt written once the first line has been heard, while Core holds;
/// returns the session with everything played.
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
/// at the group's last line counts the rows written past its end mark by
/// place while the document grows (5.6, "no rotation"), so the count is
/// exact, the burst's 5,016 lines less the 15 heard each side.
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
    assert_eq!(session.requests(), [Asked::Window, Asked::Window]);
}

#[test]
fn a_flood_within_the_history_while_a_group_plays_is_counted_exactly_in_the_console_host() {
    within_history(Flavor::ConsoleHost);
}

#[test]
fn a_flood_within_the_history_while_a_group_plays_is_counted_exactly_in_windows_terminal() {
    within_history(Flavor::WindowsTerminal);
}

/// A flood of 2,000 lines at the shipped limits of 30 starting under a line
/// of 40 words, wrapped onto three of the screen's 120-cell rows: the first
/// read takes 32 lines and Core holds the terminal, the rest is written
/// while it holds, and the read at the group's last line counts rows, so
/// none of the wrapped line's rows is counted as a line that went by
/// unread, and the count is exact, the burst's 2,001 lines less the 30
/// heard each side.
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
    assert_eq!(session.requests(), [Asked::Window, Asked::Window]);
}

/// A full-screen program drawing its 30 rows on the alternate screen in one
/// write, at limits of 10: the new screen's rows are a burst like any
/// other, its first 10 heard, the 10 between skipped and its last 10
/// heard (TM rule 7).
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
    assert_eq!(session.requests(), [Asked::Window]);
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
/// history cleared and 100 lines written while Core holds, as `clear; dir`
/// in one line: the read at the group's last line finds its end mark past
/// the shorter document, refused, so the whole document is new, its 101
/// lines counted as about, less the 10 heard last (5.6, "the mark
/// refused").
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
    assert_eq!(session.requests(), [Asked::Window, Asked::Window]);
}
