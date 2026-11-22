//! The focused terminal read through the public reducer as the shell drives
//! it (`terminal_support`): the simulated terminal answering each step of
//! Core's window reads, review reads and searches as its provider does, and
//! what Core says of it (`books/dev/src/explanation/design/terminal-model.md`):
//! new output line by line, the review cursor on its row as output moves
//! it, a key's echo, "Report status bar", and the command marks.

mod desk;
#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;

use terminal_support::{Asked, Session, plain};
use verbatim_model::ReviewCommand;
use verbatim_term_sim::Flavor;

/// Lines `range`, each ended.
fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "line {line}\r\n");
        text
    })
}

#[test]
fn new_output_is_spoken_line_by_line() {
    let mut session = Session::new(Flavor::ConsoleHost, (20, 4, 100), "");
    session.write("alpha\r\nbeta\r\n", 100);
    assert_eq!(session.heard(), plain(&["alpha", "beta"]));
    assert_eq!(session.requests(), [Asked::Window]);
}

/// A four-row console host whose history holds four rows, full: the review
/// position on "line 11", then two lines of output rotate every row up two
/// places under the buffer's fixed positions. The review position moves
/// with its row (`books/dev/src/explanation/design/terminal-model.md`, "The
/// review cursor"; build 9's design, 5.6, F11), so the next command reads
/// "line 11" again, not the row now at its old position; each command reads
/// the window first.
#[test]
fn the_review_position_moves_with_its_row_as_a_full_history_rotates() {
    let mut session = Session::new(Flavor::ConsoleHost, (20, 4, 4), &numbered(1..=12));
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["line 12", "line 11"]));
    let _ = session.requests();
    session.write(&numbered(13..=14), 100);
    assert_eq!(session.heard(), plain(&["line 13", "line 14"]));
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["line 11"]));
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["line 12"]));
}
