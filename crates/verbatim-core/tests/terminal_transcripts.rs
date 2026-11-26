//! What Core says of each recorded terminal session, pinned: every
//! recording of the conformance corpus (the streams with recorded times:
//! Pi, Claude Code, cargo, bash, `PSReadLine`, fzf, tig, Microsoft Edit,
//! less, vim and the streaming-output series) replayed through `reduce`
//! with the simulated terminal (`terminal_support`), in both terminals at
//! the size they were recorded, and everything heard compared with the
//! transcript kept in `tests/fixtures/terminal_transcripts/`, one file a
//! terminal, one section a recording.
//!
//! The transcripts were taken from the terminal reading as it stood on
//! 2026-11-26, before the terminal moves onto the batch protocol, and are
//! what that reading says, right or wrong: a line that changes is a
//! change in what users hear, to be decided, not pinned again unread
//! (Dickson, 2026-11-26, `docs/simplification-review/builds.md`).
//!
//! The replay: each chunk is written at its recorded time and told to
//! Core, which reads it at once; the quiet-period timer is fired whenever
//! it falls due before the next chunk, and until it rests after the last;
//! speech plays as soon as it is queued. The keys of the three recordings
//! made with keys (fzf, tig's log and Microsoft Edit) are pressed at the
//! times their recipes give
//! (`crates/verbatim-term-sim/conformance/recipes`); the other recordings
//! carry no keys, so their typing is read as output.
//!
//! A transcript line is the time in milliseconds of the write, key or
//! timer that caused the utterance, the key's number when it was said for
//! a key, and the utterance's words, escaped. A recording's own profile
//! path is written without the account's name ([`without_account`]).
//! The transcripts are one file a terminal, compiled into the test, so
//! that the test reads no file where the determinism rule allows none
//! (Claude, 2026-11-26).

#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;

use terminal_support::{Key, Said, Session, caret};
use verbatim_model::request::{PressAction, QueueInput};
use verbatim_model::{CaretKey, CaretMotion, TraceId};
use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::{Stream, history, streams};

/// A key of a recording: when it reached the program, in milliseconds
/// from the program's start, and what the hook reports of it.
type Pressed = (u64, Action);

/// What a recording's key is to Core.
#[derive(Clone, Copy)]
enum Action {
    /// A character typed, a control character for a Control chord or
    /// Backspace, a carriage return for Enter.
    Typed(&'static str),
    /// Escape.
    Escape,
    /// A chord that types nothing, such as Alt+F.
    Chord,
    /// A caret key the hook passes at once.
    Caret(CaretMotion),
}

/// The keys of the recordings made with keys, from their recipes: each
/// key's time is 50 milliseconds before the chunk the program answered it
/// with, and the recipe's own time where it answered with none.
fn keys(stream: &str) -> Vec<Pressed> {
    use Action::{Caret, Chord, Escape, Typed};
    let before = |chunk: u64| chunk - 50;
    match stream {
        // a, n, Control+P twice, Control+N, Backspace, Control+P twice,
        // Escape.
        "fzf-full" | "fzf-inline" => {
            let chunks: [u64; 9] = if stream == "fzf-full" {
                [1651, 2474, 3298, 3906, 4521, 5153, 5971, 6587, 7206]
            } else {
                [1648, 2468, 3288, 3906, 4521, 5166, 6003, 6617, 7539]
            };
            let actions = [
                Typed("a"),
                Typed("n"),
                Typed("\u{10}"),
                Typed("\u{10}"),
                Typed("\u{e}"),
                Typed("\u{8}"),
                Typed("\u{10}"),
                Typed("\u{10}"),
                Escape,
            ];
            chunks.map(before).into_iter().zip(actions).collect()
        }
        // j three times, k, Enter, j twice, q twice.
        "tig-log" => vec![
            (before(2366), Typed("j")),
            (before(2985), Typed("j")),
            (before(3586), Typed("j")),
            (before(4203), Typed("k")),
            (before(4838), Typed("\r")),
            (before(5898), Typed("j")),
            (before(6516), Typed("j")),
            (before(7133), Typed("q")),
            (before(7958), Typed("q")),
        ],
        // Down Arrow twice, End, Alt+F, Down Arrow twice, Escape,
        // Control+O, Down Arrow twice and Up Arrow (answered with no
        // chunk), Escape, Control+Q.
        "edit-files" => vec![
            (before(1530), Caret(CaretMotion::NextLine)),
            (before(2148), Caret(CaretMotion::NextLine)),
            (before(2765), Caret(CaretMotion::EndOfLine)),
            (before(3383), Chord),
            (before(4204), Caret(CaretMotion::NextLine)),
            (before(4823), Caret(CaretMotion::NextLine)),
            (before(5553), Escape),
            (before(6383), Typed("\u{f}")),
            (7300, Caret(CaretMotion::NextLine)),
            (7900, Caret(CaretMotion::NextLine)),
            (8500, Caret(CaretMotion::PreviousLine)),
            (before(9537), Escape),
            (before(10_360), Typed("\u{11}")),
        ],
        _ => Vec::new(),
    }
}

/// More keys than any recording has.
const MOST_KEYS: u64 = 20;

/// The key `number` of a recording, pressed at `at_ms`, as the hook
/// reports it.
fn key(number: u64, at_ms: u64, action: Action) -> Key {
    let trace_id = TraceId::from_raw(number);
    let action = match action {
        Action::Typed(text) => PressAction::Typed {
            text: text.to_owned(),
        },
        Action::Escape => PressAction::Clearing,
        Action::Chord => PressAction::EditingChord,
        Action::Caret(motion) => {
            return caret(
                trace_id,
                CaretKey {
                    motion,
                    select: false,
                },
                at_ms,
                false,
            );
        }
    };
    Key {
        trace_id,
        at_ms,
        action,
    }
}

/// `words` with the account's name after a `Users` folder, in a Windows
/// or a POSIX path, replaced by "user": four recordings were made in a
/// profile folder, and a transcript does not copy whose.
fn without_account(words: &str) -> String {
    let mut clean = String::with_capacity(words.len());
    let mut rest = words;
    while let Some(at) = rest.find("Users") {
        let (before, after) = rest.split_at(at + "Users".len());
        clean.push_str(before);
        let mut characters = after.chars();
        match characters.next() {
            Some(separator @ ('\\' | '/')) => {
                clean.push(separator);
                let name = characters.as_str();
                let end = name
                    .find(['\\', '/', '>', ' ', '"', '\''])
                    .unwrap_or(name.len());
                if end > 0 {
                    clean.push_str("user");
                }
                rest = &name[end..];
            }
            _ => rest = after,
        }
    }
    clean.push_str(rest);
    clean
}

/// Adds what `session` said since it was last asked to `transcript`, as
/// caused at `at_ms`.
fn take(session: &mut Session, at_ms: u64, transcript: &mut String) {
    for Said { key, words } in session.heard() {
        let words = without_account(&words);
        let _ = match key {
            Some(key) => writeln!(
                transcript,
                "{at_ms} ms, key {}: {}",
                (1..=MOST_KEYS)
                    .find(|&number| TraceId::from_raw(number) == key)
                    .expect("a key of the recording"),
                words.escape_debug()
            ),
            None => writeln!(transcript, "{at_ms} ms: {}", words.escape_debug()),
        };
    }
}

/// Fires the quiet-period timer for as long as it is due at or before
/// `until`, or until it rests when `until` is `None`.
fn fire_timers(session: &mut Session, until: Option<u64>, transcript: &mut String) {
    // More firings than any recording's replay makes: a timer that armed
    // itself again for ever would otherwise never end the replay.
    for _ in 0..10_000 {
        let Some(due) = session
            .armed()
            .filter(|&due| until.is_none_or(|until| due <= until))
        else {
            return;
        };
        session.at_ms = session.at_ms.max(due);
        session.queue(QueueInput::TerminalDue { at_ms: due });
        take(session, due, transcript);
    }
    panic!("the quiet-period timer never rested");
}

/// What Core says of the recording `stream`, named `name`, in `flavor`.
fn transcript(name: &str, stream: &Stream, flavor: Flavor) -> String {
    let mut session = Session::new(flavor, 120, 30, history(flavor));
    let mut transcript = String::new();
    let mut keys = keys(name).into_iter().zip(1..).peekable();
    let press = |session: &mut Session, transcript: &mut String, (at_ms, action), number| {
        fire_timers(session, Some(at_ms), transcript);
        session.at_ms = session.at_ms.max(at_ms);
        session.press(&key(number, at_ms, action));
        take(session, at_ms, transcript);
    };
    for (chunk, &at_ms) in stream.chunks.iter().zip(&stream.at_ms) {
        while let Some(&(pressed, number)) = keys.peek().filter(|((key_ms, _), _)| *key_ms <= at_ms)
        {
            press(&mut session, &mut transcript, pressed, number);
            keys.next();
        }
        fire_timers(&mut session, Some(at_ms), &mut transcript);
        session.outpost.sim.write(chunk.as_bytes());
        session.live(at_ms);
        take(&mut session, at_ms, &mut transcript);
    }
    for (pressed, number) in keys {
        press(&mut session, &mut transcript, pressed, number);
    }
    fire_timers(&mut session, None, &mut transcript);
    transcript
}

/// The sections of a pinned file: each recording's name and its
/// transcript, in the file's order.
fn sections(pinned: &str) -> Vec<(&str, String)> {
    let mut sections: Vec<(&str, String)> = Vec::new();
    for line in pinned.lines() {
        if let Some(name) = line.strip_prefix("# recording ") {
            sections.push((name, String::new()));
        } else if let Some((_, transcript)) = sections.last_mut() {
            transcript.push_str(line);
            transcript.push('\n');
        } else {
            panic!("a line before the first recording's heading: {line:?}");
        }
    }
    sections
}

/// Replays every recording in `flavor` and compares each transcript with
/// its section of `pinned`, and the recordings with the sections. A
/// transcript that differs fails with its first differing line and prints
/// the whole section as it is now.
fn compare(flavor: Flavor, pinned: &str) {
    let recordings: Vec<(String, Stream)> = streams()
        .into_iter()
        .filter(|(_, stream)| !stream.at_ms.is_empty())
        .collect();
    let sections = sections(pinned);
    assert_eq!(
        sections.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        recordings
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        "the pinned sections are the corpus's recordings, in name order"
    );
    let mut failures = Vec::new();
    for ((name, stream), (_, expected)) in recordings.iter().zip(&sections) {
        assert_eq!(
            stream.chunks.len(),
            stream.at_ms.len(),
            "{name}: a time for each chunk"
        );
        let actual = transcript(name, stream, flavor);
        if actual == *expected {
            continue;
        }
        let line = actual
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
        failures.push(format!(
            "{name} in {flavor:?} differs at line {}: pinned {:?}, now {:?}",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        ));
        println!("# recording {name}\n{actual}");
    }
    assert!(
        failures.is_empty(),
        "{} transcripts differ; each section as it is now is printed above:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_recordings_say_what_is_pinned_in_the_console_host() {
    compare(
        Flavor::ConsoleHost,
        include_str!("fixtures/terminal_transcripts/console-host.txt"),
    );
}

#[test]
fn the_recordings_say_what_is_pinned_in_windows_terminal() {
    compare(
        Flavor::WindowsTerminal,
        include_str!("fixtures/terminal_transcripts/windows-terminal.txt"),
    );
}

#[test]
fn a_profile_path_is_written_without_the_accounts_name() {
    assert_eq!(
        without_account(r"PS C:\Users\someone> cd C:\Users\someone\src /c/Users/someone/x"),
        r"PS C:\Users\user> cd C:\Users\user\src /c/Users/user/x"
    );
    assert_eq!(without_account("Users and Users\\"), "Users and Users\\");
}
