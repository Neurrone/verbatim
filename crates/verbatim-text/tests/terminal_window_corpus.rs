//! The conformance corpus's recorded streams replayed through
//! `verbatim_text::terminal_window` read by read, as Core's watch would
//! call it: each read the real terminal answered after a chunk, cut to the
//! window `TerminalRead` reads (the rows from two screens above the
//! document's end to its end, whole lines), its screen's top found from
//! the screen's text, given as a `Window` at the chunk's time and diffed
//! against the read before, and the held items' due times run once the
//! stream ends. What each read hands on (the echo, the keys' answers, the
//! output for the pacing, whether it kept a line, whether it was dropped)
//! is pinned whole for each stream in both terminals, in
//! `tests/fixtures/terminal_window/`.
//!
//! Keys are passed only in the three streams the keys' answers are judged
//! by (fzf on its alternate screen and inline, tig's log, Microsoft
//! Edit), each at the time it reached the program in the recording's
//! recipe (`crates/verbatim-term-sim/conformance/recipes`), with the
//! colours read for every read in a key's window, as Core asks for them.
//! In the other streams typing is output, since the streams carry no keys.

use std::fmt::Write as _;
use std::sync::Arc;

use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::{Answer as Recorded, expectation, streams};
use verbatim_text::terminal_window::{
    Answer, Caret, ColourRun, KeyKind, Read, Reading, Unit, Window,
};

/// The streams the design's replay names (build 9, section 9).
const NAMED: &[&str] = &[
    "bash",
    "claude-code-reply",
    "edit-files",
    "fzf-full",
    "fzf-inline",
    "pi-changed-above",
    "progress",
    "psreadline",
    "tig-log",
];

/// The time between reads of a stream written by hand, which records none.
const HAND_STEP_MS: u64 = 10;

/// How long after its chunk a read ends.
const READ_LAG_US: u64 = 5_000;

/// A key of a recipe: when it reached the program, in milliseconds from
/// the program's start, what kind it is, and whether it is a typed
/// character held for its echo.
type Key = (u64, KeyKind, Option<&'static str>);

/// The keys of the streams judged by their keys, from their recipes: each
/// key's time is its chunk's less 50 milliseconds where the recording has
/// a chunk for it, and the recipe's own time where it has none.
fn keys(stream: &str) -> Vec<Key> {
    let typed = |at, text| (at, KeyKind::Typed, Some(text));
    let other = |at| (at, KeyKind::Other, None);
    let line = |at| (at, KeyKind::Caret(Unit::Line), None);
    match stream {
        // a, n, Control+P twice, Control+N, Backspace, Control+P twice,
        // Escape.
        "fzf-full" | "fzf-inline" => {
            let chunks: &[u64] = if stream == "fzf-full" {
                &[1651, 2474, 3298, 3906, 4521, 5153, 5971, 6587, 7206]
            } else {
                &[1648, 2468, 3288, 3906, 4521, 5166, 6003, 6617, 7539]
            };
            let at = |index: usize| chunks[index] - 50;
            vec![
                typed(at(0), "a"),
                typed(at(1), "n"),
                other(at(2)),
                other(at(3)),
                other(at(4)),
                other(at(5)),
                other(at(6)),
                other(at(7)),
                (at(8), KeyKind::Forget, None),
            ]
        }
        // j three times, k, Enter, j twice, q twice.
        "tig-log" => {
            let at = |chunk: u64| chunk - 50;
            vec![
                typed(at(2366), "j"),
                typed(at(2985), "j"),
                typed(at(3586), "j"),
                typed(at(4203), "k"),
                (at(4838), KeyKind::Enter, None),
                typed(at(5898), "j"),
                typed(at(6516), "j"),
                typed(at(7133), "q"),
                typed(at(7958), "q"),
            ]
        }
        // Down Arrow twice, End, Alt+F, Down Arrow twice, Escape,
        // Control+O, Down Arrow twice and Up Arrow (no chunk), Escape,
        // Control+Q.
        "edit-files" => {
            let at = |chunk: u64| chunk - 50;
            vec![
                line(at(1530)),
                line(at(2148)),
                (at(2765), KeyKind::Caret(Unit::Character), None),
                other(at(3383)),
                line(at(4204)),
                line(at(4823)),
                (at(5553), KeyKind::Forget, None),
                other(at(6383)),
                line(7300),
                line(7900),
                line(8500),
                (at(9537), KeyKind::Forget, None),
                other(at(10360)),
            ]
        }
        _ => Vec::new(),
    }
}

/// How many characters `line` takes with its padding, and its text without
/// it.
fn unpad(line: &str) -> (u32, String) {
    (
        u32::try_from(line.chars().count()).unwrap(),
        line.trim_end_matches(' ').to_owned(),
    )
}

/// The screen's text up to the document's end, which the read's text ends
/// with, or which ends with the read's text when the read started inside
/// the screen (`verbatim-term-sim`'s
/// `no_document_row_lies_below_a_reads_screen`).
fn screen_to_end<'a>(text: &str, screen: &'a str) -> &'a str {
    let blank_from = screen.trim_end_matches([' ', '\r', '\n']).len();
    (blank_from..=screen.len())
        .rev()
        .filter(|&end| screen.is_char_boundary(end))
        .map(|end| &screen[..end])
        .find(|within| text.ends_with(within) || within.ends_with(text))
        .expect("the screen meets the document's end")
}

/// The screen's top in rows above the document's end, from the screen's
/// text up to that end.
fn screen_top(within: &str, width: usize) -> u32 {
    let within = within.strip_suffix("\r\n").unwrap_or(within);
    if within.is_empty() {
        return 0;
    }
    within
        .split("\r\n")
        .map(|line| u32::try_from(line.chars().count().div_ceil(width).max(1)).unwrap())
        .sum()
}

/// The recorded read `answer` as `TerminalRead` reads it, at `ended_us`,
/// with its colours when `colours`.
fn window(
    answer: &Recorded,
    (width, height): (usize, usize),
    ended_us: u64,
    colours: bool,
) -> Window {
    // A read that started inside the screen holds less than the window,
    // which always holds the screen.
    let within = screen_to_end(&answer.text, &answer.screen);
    let full = if within.len() > answer.text.len() {
        within
    } else {
        answer.text.as_str()
    };
    let mut lines: Vec<&str> = full.split("\r\n").collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    // The window: whole lines from the end, two screens of rows.
    let mut rows = 0;
    let mut first = lines.len();
    while first > 0 && rows < 2 * height {
        first -= 1;
        rows += lines[first].chars().count().div_ceil(width).max(1);
    }
    let mut text = String::new();
    let mut lengths: Vec<(u32, u32)> = Vec::new();
    for line in &lines[first..] {
        let (length, unpadded) = unpad(line);
        text.push_str(&unpadded);
        text.push_str("\r\n");
        match lengths.last_mut() {
            Some((count, last)) if *last == length => *count += 1,
            _ => lengths.push((1, length)),
        }
    }
    let colours = colours.then(|| {
        answer
            .colours
            .iter()
            .map(|row| {
                row.iter()
                    .map(|run| ColourRun {
                        column: u32::try_from(run.column).unwrap(),
                        length: u32::try_from(run.length).unwrap(),
                        foreground: run.foreground,
                        background: run.background,
                    })
                    .collect()
            })
            .collect()
    });
    Window::new(Read {
        text: &text,
        lengths: &lengths,
        width: u32::try_from(width).unwrap(),
        screen_top: screen_top(within, width),
        screen_rows: u32::try_from(height).unwrap(),
        caret: (answer.caret_rows_to_end > 0).then_some(Caret {
            rows_to_end: answer.caret_rows_to_end,
            column: answer.caret_column,
        }),
        colours,
        ended_us,
    })
}

/// One answer as a transcript line, or none when it hands on nothing.
fn line(at: &str, answer: &Answer) -> Option<String> {
    let mut parts = Vec::new();
    if answer.dropped {
        parts.push("dropped".to_owned());
    }
    if !answer.echo.is_empty() {
        parts.push(format!("echo {:?}", answer.echo.concat()));
    }
    for said in &answer.keys {
        parts.push(format!("key {:?} {:?}", said.unit, said.text));
    }
    for output in &answer.output {
        parts.push(format!("out {output:?}"));
    }
    if parts.is_empty() {
        return None;
    }
    let shared = if answer.shares_line { "" } else { " (new)" };
    Some(scrub(&format!("{at}{shared}: {}", parts.join("; "))))
}

/// `line` with the recording machine's paths left out, so that no
/// transcript holds a user's name or the repository's path, which some
/// recorded screens show: a profile's folder name after `Users\` becomes
/// `%USERNAME%`, and a path from the home folder, `~\`, becomes `~\%PATH%`.
/// The line is in `Debug` form, each backslash doubled.
fn scrub(line: &str) -> String {
    const PROFILE: &str = "Users\\\\";
    const HOME: &str = "~\\\\";
    let mut out = String::new();
    let mut rest = line;
    loop {
        let next = [PROFILE, HOME]
            .into_iter()
            .filter_map(|marker| rest.find(marker).map(|at| (at, marker)))
            .min();
        let Some((at, marker)) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at + marker.len()]);
        rest = &rest[at + marker.len()..];
        if marker == PROFILE {
            out.push_str("%USERNAME%");
            rest = &rest[rest.find('\\').unwrap_or(rest.len())..];
        } else {
            out.push_str("%PATH%");
            rest = &rest[rest.find(['"', ' ']).unwrap_or(rest.len())..];
        }
    }
}

/// What the stream `name` hands on in `flavor`, one line a read that says
/// anything.
fn transcript(name: &str, flavor: Flavor, at_ms: &[u64]) -> String {
    let expected = expectation(name, flavor);
    let size = (expected.width, expected.height);
    let mut keys = keys(name).into_iter().peekable();
    let mut reading = Reading::new();
    let mut old = Arc::new(Window::empty());
    let mut out = String::new();
    let mut key_until = 0u64;
    for (index, answer) in expected.answers.iter().enumerate() {
        let at = at_ms
            .get(index)
            .copied()
            .unwrap_or(index as u64 * HAND_STEP_MS);
        let ended = at * 1000 + READ_LAG_US;
        // Whatever comes due before this read, then each key before it.
        while let Some(due) = reading.due().filter(|&due| due < ended) {
            if let Some(text) = line(&format!("timer {}", due / 1000), &reading.timer(due)) {
                writeln!(out, "{text}").unwrap();
            }
        }
        while let Some((key_at, kind, typed)) = keys.next_if(|(key_at, ..)| key_at * 1000 < ended) {
            if let Some(text) = typed {
                reading.hold_typed(text, key_at * 1000);
            }
            reading.key(kind, key_at * 1000, &old);
            key_until = key_at * 1000 + verbatim_text::terminal_window::KEY_WINDOW_US;
        }
        let new = window(answer, size, ended, ended <= key_until);
        let said = if index == 0 {
            Answer::default()
        } else {
            reading.answer(&old, &new)
        };
        if let Some(text) = line(&format!("{index}"), &said) {
            writeln!(out, "{text}").unwrap();
        }
        if !said.dropped {
            old = Arc::new(new);
        }
    }
    while let Some(due) = reading.due() {
        if let Some(text) = line(&format!("timer {}", due / 1000), &reading.timer(due)) {
            writeln!(out, "{text}").unwrap();
        }
    }
    out
}

/// The pinned transcripts, by file name.
fn pinned(file: &str) -> Option<&'static str> {
    macro_rules! files {
        ($($name:literal),* $(,)?) => {
            match file {
                $($name => Some(include_str!(concat!("fixtures/terminal_window/", $name))),)*
                _ => None,
            }
        };
    }
    files!(
        "bash.console-host.txt",
        "bash.windows-terminal.txt",
        "claude-code-reply.console-host.txt",
        "claude-code-reply.windows-terminal.txt",
        "edit-files.console-host.txt",
        "edit-files.windows-terminal.txt",
        "fzf-full.console-host.txt",
        "fzf-full.windows-terminal.txt",
        "fzf-inline.console-host.txt",
        "fzf-inline.windows-terminal.txt",
        "pi-changed-above.console-host.txt",
        "pi-changed-above.windows-terminal.txt",
        "progress.console-host.txt",
        "progress.windows-terminal.txt",
        "psreadline.console-host.txt",
        "psreadline.windows-terminal.txt",
        "stream.console-host.txt",
        "stream.windows-terminal.txt",
        "tig-log.console-host.txt",
        "tig-log.windows-terminal.txt",
    )
}

/// Every named stream, and every `stream-` stream together, hands on in
/// both terminals exactly what its pinned transcript says.
#[test]
fn the_corpus_hands_on_what_its_transcripts_pin() {
    let streams = streams();
    let mut failures = Vec::new();
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        let suffix = match flavor {
            Flavor::ConsoleHost => "console-host",
            Flavor::WindowsTerminal => "windows-terminal",
        };
        let mut together = String::new();
        let mut files: Vec<(String, String)> = Vec::new();
        for (name, stream) in &streams {
            if name.starts_with("stream-") {
                writeln!(together, "## {name}").unwrap();
                together.push_str(&transcript(name, flavor, &stream.at_ms));
            } else if NAMED.contains(&name.as_str()) {
                files.push((
                    format!("{name}.{suffix}.txt"),
                    transcript(name, flavor, &stream.at_ms),
                ));
            }
        }
        files.push((format!("stream.{suffix}.txt"), together));
        for (file, actual) in files {
            if pinned(&file) != Some(actual.as_str()) {
                failures.push(format!("=== {file}\n{actual}=== end\n"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.concat());
}
