//! `verbatim-replay`, run as `cargo xtask replay <flight-recorder.jsonl>
//! [--from <line>] [--to <line>]`: replays a run's flight-recorder dump through Core's reducer
//! from the state it starts at, and prints each input on one line with the
//! effects it produced, flagging every input whose effect count differs
//! from the one the dump recorded (`books/dev/src/how-to/e2e.md`, "Replaying a run").
//! It first prints the commit the dump's build and its own were made from,
//! with a warning when they differ or either had uncommitted changes.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use verbatim_core::{DumpContents, UNKNOWN_COMMIT, dump};
use verbatim_replay::{FIRST_INPUT_LINE, Renderer, replay_dump};

/// The command's arguments.
#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    dump: PathBuf,
    /// The first file line printed.
    from: Option<usize>,
    /// The last file line printed.
    to: Option<usize>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arguments = match parse(&args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("replay: {error}");
            eprintln!(
                "usage: cargo xtask replay <flight-recorder.jsonl> [--from <line>] [--to <line>]"
            );
            return ExitCode::from(2);
        }
    };
    let contents = match File::open(&arguments.dump)
        .map_err(|error| error.to_string())
        .and_then(|file| read(&mut BufReader::new(file)))
    {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!(
                "replay: could not read {}: {error}",
                arguments.dump.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let dump_source = Source {
        commit: &contents.header.commit,
        uncommitted_changes: contents.header.uncommitted_changes,
    };
    for line in source_lines(dump_source, own_source()) {
        println!("{line}");
    }
    let report = report(
        &contents,
        &Renderer::new(&sounds()),
        arguments.from,
        arguments.to,
    );
    for line in &report.lines {
        println!("{line}");
    }
    if report.differing == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The repository's sound files, which the built-in default theme plays.
fn sounds() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sounds")
}

/// The commit a build was made from, and whether its working tree had
/// uncommitted changes.
#[derive(Clone, Copy, Debug)]
struct Source<'a> {
    commit: &'a str,
    uncommitted_changes: bool,
}

/// The commit this replay was built from, captured by its build script.
fn own_source() -> Source<'static> {
    Source {
        commit: env!("VERBATIM_COMMIT"),
        uncommitted_changes: env!("VERBATIM_UNCOMMITTED_CHANGES") == "true",
    }
}

/// The lines naming the commits the dump's build and this replay were made
/// from, then a warning for each reason the replay's effect counts may
/// differ from the dump's other than a change in behaviour: different
/// commits, a commit not known (so a match cannot be shown), or
/// uncommitted changes in either.
fn source_lines(dump: Source, replay: Source) -> Vec<String> {
    const WHY: &str = "; effect counts may differ for that reason";
    let mut lines = vec![
        built_from("the dump", dump),
        built_from("this replay", replay),
    ];
    if dump.commit == UNKNOWN_COMMIT || replay.commit == UNKNOWN_COMMIT {
        lines.push(format!(
            "warning: the commit the dump or this replay was built from is unknown{WHY}"
        ));
    } else if dump.commit != replay.commit {
        lines.push(format!(
            "warning: this replay was built from a different commit than the dump{WHY}"
        ));
    }
    if dump.uncommitted_changes {
        lines.push(format!(
            "warning: the dump's build had uncommitted changes{WHY}"
        ));
    }
    if replay.uncommitted_changes {
        lines.push(format!(
            "warning: this replay's build had uncommitted changes{WHY}"
        ));
    }
    lines
}

fn built_from(whose: &str, source: Source) -> String {
    if source.commit == UNKNOWN_COMMIT {
        format!("{whose} was built from an unknown commit")
    } else if source.uncommitted_changes {
        format!(
            "{whose} was built from commit {}, with uncommitted changes",
            source.commit
        )
    } else {
        format!("{whose} was built from commit {}", source.commit)
    }
}

/// The dump `reader` holds, or why it cannot be read: which line is
/// malformed, or that the file ends before the state its inputs start
/// from.
fn read(reader: &mut impl BufRead) -> Result<DumpContents, String> {
    dump::read_dump(reader).map_err(|error| error.to_string())
}

fn parse(args: &[String]) -> Result<Arguments, String> {
    let mut dump = None;
    let mut from = None;
    let mut to = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" | "--to" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} needs a line number"))?;
                let line: usize = value
                    .parse()
                    .map_err(|_| format!("{arg} needs a line number, not {value:?}"))?;
                if arg == "--from" {
                    from = Some(line);
                } else {
                    to = Some(line);
                }
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path if dump.is_none() => dump = Some(PathBuf::from(path)),
            extra => return Err(format!("one dump only, but {extra:?} was given too")),
        }
    }
    Ok(Arguments {
        dump: dump.ok_or("no dump was given")?,
        from,
        to,
    })
}

/// What the command prints, and how many inputs' effect counts differ.
#[derive(Debug)]
struct Report {
    lines: Vec<String>,
    /// The inputs, among those printed, whose replay produced a different
    /// number of effects from the dump's.
    differing: usize,
}

/// Replays the whole of `contents`, since each input's effects depend on
/// every input before it, and renders the inputs on the file lines `from`
/// to `to`, then one line naming how many were replayed and how many
/// differ.
fn report(
    contents: &DumpContents,
    renderer: &Renderer,
    from: Option<usize>,
    to: Option<usize>,
) -> Report {
    let shown: Vec<_> = replay_dump(contents)
        .into_iter()
        .filter(|replayed| {
            from.is_none_or(|from| replayed.line >= from) && to.is_none_or(|to| replayed.line <= to)
        })
        .collect();
    let mut lines: Vec<String> = shown
        .iter()
        .map(|replayed| renderer.line(replayed))
        .collect();
    let differing = shown.iter().filter(|replayed| replayed.differs()).count();
    lines.push(match (shown.first(), shown.last()) {
        (Some(first), Some(last)) => format!(
            "replayed {} inputs, lines {} to {}: {}",
            shown.len(),
            first.line,
            last.line,
            match differing {
                0 => "none differs from the dump's effect counts".to_owned(),
                1 => "1 differs from the dump's effect count".to_owned(),
                count => format!("{count} differ from the dump's effect counts"),
            }
        ),
        _ => "no input is on the lines asked for".to_owned(),
    });
    if contents.truncated {
        lines.push(format!(
            "the dump ends mid-line: nothing after line {} was read",
            FIRST_INPUT_LINE + contents.inputs.len() - 1
        ));
    }
    Report { lines, differing }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../verbatim-core/tests/fixtures/focus_value_states.jsonl"),
        )
        .expect("reads the fixture")
    }

    fn report_of(text: &str, from: Option<usize>, to: Option<usize>) -> Report {
        let contents = read(&mut text.as_bytes()).expect("the dump parses");
        report(&contents, &Renderer::new(&sounds()), from, to)
    }

    /// Each input line of the fixture as it is replayed: the reader's
    /// start, a listener's focus tell, its read and completion, and two
    /// changes the outpost tells of the focus.
    const LINE_3: &str =
        "3: Queue trace 1, at_us 1000, input Process ListenerStarted 1. 1 effect: Queue";
    const LINE_4: &str = "4: Queue trace 2, at_us 2000, input ReaderReady generation 1, reader \"Listener\". 1 effect: Queue";
    const LINE_5: &str = "5: Queue trace 3, at_us 3000, input Tell from (generation 1, reader \"Listener\"), tell (application \"app\", at_us 3000, checked (keyboard_focus Answered true, probe (be... (749 characters). 2 effects: Queue Send (ask (number 1, request ReadObject descendants false, object Kept 1, purpose Foc... (144 characters); Queue WakeAt 503000";
    const LINE_6: &str = "6: Queue trace 4, at_us 4000, input Answer answer (number 1, outcome Result Object (object (window (handle 4001, process 40, root_... (134 characters), from (generation 1, reader \"Listener\"). 2 effects: Queue StartOutpost 40; Queue WakeAt 13003000";
    const LINE_7: &str = "7: Queue trace 5, at_us 5000, input Process OutpostStarted (generation 3, process 40). 1 effect: Queue WakeAt 9005000";
    const LINE_8: &str = "8: Queue trace 6, at_us 6000, input ReaderReady generation 3, reader Outpost 40. 2 effects: Queue Send (ask (number 2, request CompleteUia completes \"Focus\", read 1 items, route (end ... (144 characters); Queue WakeAt 506000";
    const LINE_9: &str = "9: Queue trace 7, at_us 7000, input Answer answer (number 2, outcome Result Focus (caret \"NoText\", class \"Plain\", focus (read (nod... (133 characters), from (generation 3, reader Outpost 40). 5 effects: Observed trace 7, source 40, backend \"Uia\", event FocusChanged node Slider \"Rate\", text \"NoText\", whole_window false, window (application \"app\", class \"AppPane\", foreground_window 4000, in_foreground true,... (128 characters); DropExpiredSpeech; Speak \"Rate slider 50\"; Queue Send (ask (number 3, request Follow node (group \"Base\", node 3:1), verdict \"Uia\", win... (143 characters); Queue WakeAt 506000";
    const LINE_10: &str = "10: Queue trace 8, at_us 8000, input Tell from (generation 3, reader Outpost 40), tell (application \"app\", at_us 8000, kind \"UiaPropertyChanged\", object Followed runti... (204 characters). 2 effects: Speak \"55\"; Queue WakeAt 506000";
    const LINE_11: &str = "11: Queue trace 9, at_us 9000, input Tell from (generation 3, reader Outpost 40), tell (application \"app\", at_us 9000, kind \"UiaPropertyChanged\", object Followed runti... (205 characters). 2 effects: Speak \"unavailable\"; Queue WakeAt 506000";

    /// Line `number` of the fixture, its recorded effect count `from`, set
    /// to `to`.
    fn with_count(text: &str, number: usize, from: usize, to: usize) -> String {
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let line = &mut lines[number - 1];
        let edited = line.replacen(
            &format!("\"effect_count\":{from}}}"),
            &format!("\"effect_count\":{to}}}"),
            1,
        );
        assert_ne!(*line, edited, "the edit applies");
        *line = edited;
        lines.join("\n") + "\n"
    }

    #[test]
    fn replaying_the_committed_fixture_prints_its_lines_and_flags_nothing() {
        let report = report_of(&fixture(), None, None);
        assert_eq!(
            report.lines,
            [
                LINE_3,
                LINE_4,
                LINE_5,
                LINE_6,
                LINE_7,
                LINE_8,
                LINE_9,
                LINE_10,
                LINE_11,
                "replayed 9 inputs, lines 3 to 11: none differs from the dump's effect counts",
            ]
        );
        assert_eq!(report.differing, 0);
    }

    #[test]
    fn a_dump_whose_recorded_count_was_edited_is_flagged() {
        let edited = with_count(&fixture(), 10, 2, 4);
        let report = report_of(&edited, Some(10), None);
        assert_eq!(
            report.lines,
            [
                "10: Queue trace 8, at_us 8000, input Tell from (generation 3, reader Outpost 40), tell (application \"app\", at_us 8000, kind \"UiaPropertyChanged\", object Followed runti... (204 characters). 2 effects (differs: the dump recorded 4): Speak \"55\"; Queue WakeAt 506000",
                LINE_11,
                "replayed 2 inputs, lines 10 to 11: 1 differs from the dump's effect count",
            ]
        );
        assert_eq!(report.differing, 1);
    }

    /// Only the lines from `--from` to `--to` are printed, and an input
    /// outside them whose count differs is not counted.
    #[test]
    fn a_line_range_prints_and_counts_only_its_own_inputs() {
        // The focus answer's recorded count, 5, becomes 6: it differs.
        let edited = with_count(&fixture(), 9, 5, 6);
        let report = report_of(&edited, Some(10), Some(10));
        assert_eq!(
            report.lines,
            [
                LINE_10,
                "replayed 1 inputs, lines 10 to 10: none differs from the dump's effect counts",
            ]
        );
        assert_eq!(report.differing, 0);

        let report = report_of(&edited, Some(9), Some(10));
        assert_eq!(
            report.lines,
            [
                "9: Queue trace 7, at_us 7000, input Answer answer (number 2, outcome Result Focus (caret \"NoText\", class \"Plain\", focus (read (nod... (133 characters), from (generation 3, reader Outpost 40). 5 effects (differs: the dump recorded 6): Observed trace 7, source 40, backend \"Uia\", event FocusChanged node Slider \"Rate\", text \"NoText\", whole_window false, window (application \"app\", class \"AppPane\", foreground_window 4000, in_foreground true,... (128 characters); DropExpiredSpeech; Speak \"Rate slider 50\"; Queue Send (ask (number 3, request Follow node (group \"Base\", node 3:1), verdict \"Uia\", win... (143 characters); Queue WakeAt 506000",
                LINE_10,
                "replayed 2 inputs, lines 9 to 10: 1 differs from the dump's effect count",
            ]
        );
        assert_eq!(report.differing, 1);
    }

    /// A complete line that is not an input is an error naming its line.
    #[test]
    fn a_malformed_line_is_an_error_naming_it() {
        let text = fixture();
        let mut lines: Vec<&str> = text.lines().collect();
        lines[3] = "{\"input\":";
        let malformed = lines.join("\n") + "\n";
        assert_eq!(
            read(&mut malformed.as_bytes()).map(|_| ()),
            Err(
                "flight-recorder dump line 4 is malformed: EOF while parsing a value at line 1 column 9"
                    .to_owned()
            )
        );
    }

    /// A dump cut off in its base state cannot be replayed; one cut off in
    /// an input is replayed up to the line before it, and says so.
    #[test]
    fn a_truncated_dump_is_an_error_or_replayed_up_to_the_cut() {
        let text = fixture();
        let base_ends = text
            .match_indices('\n')
            .nth(1)
            .expect("a base state line")
            .0;
        assert_eq!(
            read(&mut &text.as_bytes()[..base_ends - 10]).map(|_| ()),
            Err("flight-recorder dump ends before its base state line is complete".to_owned())
        );

        let last_starts = text.trim_end().rfind('\n').expect("an earlier line") + 1;
        let report = report_of(&text[..last_starts + 20], None, None);
        assert_eq!(
            report.lines,
            [
                LINE_3,
                LINE_4,
                LINE_5,
                LINE_6,
                LINE_7,
                LINE_8,
                LINE_9,
                LINE_10,
                "replayed 8 inputs, lines 3 to 10: none differs from the dump's effect counts",
                "the dump ends mid-line: nothing after line 10 was read",
            ]
        );
    }

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn clean(commit: &str) -> Source<'_> {
        Source {
            commit,
            uncommitted_changes: false,
        }
    }

    #[test]
    fn the_same_commit_with_no_uncommitted_changes_gives_no_warning() {
        assert_eq!(
            source_lines(clean(COMMIT), clean(COMMIT)),
            [
                format!("the dump was built from commit {COMMIT}"),
                format!("this replay was built from commit {COMMIT}"),
            ]
        );
    }

    #[test]
    fn a_different_commit_gives_a_warning() {
        assert_eq!(
            source_lines(clean(COMMIT), clean(OTHER)),
            [
                format!("the dump was built from commit {COMMIT}"),
                format!("this replay was built from commit {OTHER}"),
                "warning: this replay was built from a different commit than the dump; effect counts may differ for that reason".to_owned(),
            ]
        );
    }

    /// The committed fixture is a version 6 dump, so its commit is unknown.
    #[test]
    fn uncommitted_changes_or_an_unknown_commit_give_a_warning() {
        let contents = read(&mut fixture().as_bytes()).expect("the dump parses");
        let dump = Source {
            commit: &contents.header.commit,
            uncommitted_changes: contents.header.uncommitted_changes,
        };
        let replay = Source {
            commit: COMMIT,
            uncommitted_changes: true,
        };
        assert_eq!(
            source_lines(dump, replay),
            [
                "the dump was built from an unknown commit".to_owned(),
                format!("this replay was built from commit {COMMIT}, with uncommitted changes"),
                "warning: the commit the dump or this replay was built from is unknown; effect counts may differ for that reason".to_owned(),
                "warning: this replay's build had uncommitted changes; effect counts may differ for that reason".to_owned(),
            ]
        );
    }

    #[test]
    fn the_arguments_are_a_dump_and_its_line_range() {
        let args: Vec<String> = ["run.jsonl", "--from", "4", "--to", "9"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            parse(&args),
            Ok(Arguments {
                dump: PathBuf::from("run.jsonl"),
                from: Some(4),
                to: Some(9),
            })
        );
        assert_eq!(parse(&[]), Err("no dump was given".to_owned()));
    }
}
