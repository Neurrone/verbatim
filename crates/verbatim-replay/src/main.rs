//! `verbatim-replay`, run as `cargo xtask replay <flight-recorder.jsonl>
//! [--from <line>] [--to <line>]`: replays a run's flight-recorder dump through Core's reducer
//! from the state it starts at, and prints each input on one line with the
//! effects it produced, flagging every input whose effect count differs
//! from the one the dump recorded (`docs/tooling/e2e.md`, "Replaying a run").

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use verbatim_core::{DumpContents, dump};
use verbatim_replay::{Renderer, replay_dump};

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
        .and_then(|file| {
            dump::read_dump(&mut BufReader::new(file)).map_err(|error| error.to_string())
        }) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!(
                "replay: could not read {}: {error}",
                arguments.dump.display()
            );
            return ExitCode::FAILURE;
        }
    };
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
        lines.push("the dump ends mid-line: its last input was cut off".to_owned());
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
        let contents = dump::read_dump(&mut text.as_bytes()).expect("the dump parses");
        report(&contents, &Renderer::new(&sounds()), from, to)
    }

    const FOCUS: &str = "3: Event FocusChanged, node Slider \"Rate\", whole_window false, trace 1, source 700, observed_at_us 0, backend \"Uia\". 2 effects: DropExpiredSpeech; Speak \"Rate slider 50\"";
    const VALUE: &str = "4: Event ValueChanged, node_id 0:1, value \"55\", trace 2, source 700, observed_at_us 0, backend \"Uia\". 1 effect: Speak \"55\"";
    const STATES: &str = "5: Event PropertyChanged, node_id 0:1, change States 64, trace 3, source 700, observed_at_us 0, backend \"Uia\". 1 effect: Speak \"unavailable\"";

    #[test]
    fn replaying_the_committed_fixture_prints_its_lines_and_flags_nothing() {
        let report = report_of(&fixture(), None, None);
        assert_eq!(
            report.lines,
            [
                FOCUS,
                VALUE,
                STATES,
                "replayed 3 inputs, lines 3 to 5: none differs from the dump's effect counts",
            ]
        );
        assert_eq!(report.differing, 0);
    }

    #[test]
    fn a_dump_whose_recorded_count_was_edited_is_flagged() {
        let text = fixture();
        let edited = text.replacen("\"effect_count\":1}", "\"effect_count\":4}", 1);
        assert_ne!(edited, text, "the edit applies");
        let report = report_of(&edited, Some(4), None);
        assert_eq!(
            report.lines,
            [
                "4: Event ValueChanged, node_id 0:1, value \"55\", trace 2, source 700, observed_at_us 0, backend \"Uia\". 1 effect (differs: the dump recorded 4): Speak \"55\"",
                STATES,
                "replayed 2 inputs, lines 4 to 5: 1 differs from the dump's effect count",
            ]
        );
        assert_eq!(report.differing, 1);
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
