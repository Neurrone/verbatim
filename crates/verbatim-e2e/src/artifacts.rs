//! Host-side artifacts one scenario run leaves behind under
//! [`artifacts_root`]: a [`ScenarioSummary`], the interleaved
//! [`crate::timeline::Timeline`] and Verbatim's captured stderr log, and a
//! reducer flight-recorder dump — all on every run, pass or fail. See
//! [`crate::scenario::Scenario::collect_run_artifacts`] for the timeline and
//! stderr pair, [`crate::scenario::Scenario::collect_flight_recorder`] for the
//! flight-recorder dump (taken before the clean quit so a passing run captures
//! it too), and [`crate::registry`] for where [`ScenarioSummary::write`] is
//! called.
//!
//! The commands that run scenarios as subprocesses (`cargo xtask e2e`,
//! `demo` and `nvda`) call the same [`artifacts_root`] and
//! [`scenario_dir`] functions a scenario subprocess used, so both sides
//! agree on where a run's artifacts are.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use verbatim_model::{DEFECT_TARGET, REMOTE_FALLBACK_TARGET, TraceId};

use crate::speech::LatencyRow;

/// Environment variable overriding [`artifacts_root`]'s default. Read by both
/// a scenario subprocess (`crates/verbatim-e2e`) and, when set in the same
/// shell, the xtask command that spawned it (inherited into the subprocesses
/// it spawns), so the two always agree on where artifacts land without any explicit
/// wiring between the two binaries.
pub const ARTIFACTS_DIR_ENV: &str = "VERBATIM_E2E_ARTIFACTS_DIR";

/// File name [`ScenarioSummary::write`] writes, inside [`scenario_dir`].
const SUMMARY_FILE_NAME: &str = "summary.txt";

/// The root directory every scenario's artifacts (summary, and on failure,
/// the timeline/stderr/flight-recorder trio) are written under:
/// [`ARTIFACTS_DIR_ENV`] when set, otherwise `target/e2e-artifacts` under the
/// workspace root (computed from this crate's own manifest directory, so it
/// does not depend on the caller's working directory — the same pattern
/// [`crate::scenario`]'s own `workspace_root` uses for `target/e2e-stage`).
#[must_use]
pub fn artifacts_root() -> PathBuf {
    resolve_artifacts_root(std::env::var(ARTIFACTS_DIR_ENV).ok())
}

/// The pure resolution logic behind [`artifacts_root`], split out so it can
/// be unit tested without mutating the real process environment:
/// `override_path`, when given, wins outright, otherwise the default under
/// the workspace root.
fn resolve_artifacts_root(override_path: Option<String>) -> PathBuf {
    if let Some(path) = override_path {
        return PathBuf::from(path);
    }
    crate::scenario::workspace_root()
        .join("target")
        .join("e2e-artifacts")
}

/// The directory one scenario's artifacts live under: `root` joined with the
/// scenario's own name, which is also its `#[test]` function name and its
/// name on `cargo xtask e2e` — one identifier used
/// everywhere, per [`crate::registry`]'s own doc comment.
#[must_use]
pub fn scenario_dir(root: &Path, scenario_name: &str) -> PathBuf {
    root.join(scenario_name)
}

/// The one-line-per-fact result of running a single scenario: written by
/// [`ScenarioSummary::write`] at the end of every run (pass or fail), for
/// people and the workflow's summary to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioSummary {
    /// The scenario's name, matching [`crate::registry::ScenarioDef::name`].
    pub name: String,
    /// Whether the scenario (setup, body, teardown, and the final clean
    /// quit) completed without error.
    pub passed: bool,
    /// How many of the run's utterances carry the time of the event behind
    /// them (the startup speech and the desktop's announcement carry none),
    /// the rows of `latency.csv`
    /// ([`crate::speech::SpeechCollector::latency_rows`]), read once
    /// Verbatim has quit. It counts utterances, not traces, so one trace's
    /// several utterances count several times. `None` when the run read no
    /// speech, as in compare mode.
    pub latency_records: Option<usize>,
    /// How many of those utterances started to play, the run's last one
    /// included.
    pub latency_reached_audio: Option<usize>,
    /// The worst event-to-queue latency (milliseconds) across those
    /// utterances, or `None` when there were none. This is the
    /// deterministic pipeline latency — event observed to speech queued —
    /// independent of the synthesizer.
    pub max_event_to_queue_ms: Option<u64>,
    /// The worst event-to-audio latency (milliseconds) across those
    /// utterances that played, or `None` when none did. An utterance's time
    /// to audio includes the time it waited while the speech queued before
    /// it played, so an utterance behind others in one step has a far
    /// larger figure than the first. Under the capture synth's instant sink
    /// this is close to the pipeline latency; under a real synth it
    /// includes synthesis and that waiting, and is the looser end-to-end
    /// smoke number.
    pub max_event_to_audio_ms: Option<u64>,
    /// The outposts that crashed during the run, whether or not its steps
    /// passed ([`crate::scenario::Scenario::outpost_crashes`]), or `None`
    /// when they could not be read. [`ScenarioSummary::new`] leaves it
    /// `None`; the run sets it once Verbatim has exited.
    pub outpost_crashes: Option<Vec<OutpostCrash>>,
    /// What became of each optional utterance the run asserted, in order,
    /// as [`crate::speech::OptionalOutcome`] displays it, such as
    /// `"Working on it..." after keys enter: cut off`.
    /// [`ScenarioSummary::new`] leaves it empty; the run sets it.
    pub optional_utterances: Vec<String>,
}

/// One outpost (a `verbatim-outpost.exe` or `verbatim-listener.exe`
/// process) that crashed during a scenario: it ended on an unhandled
/// exception or a fail-fast, which Windows reports as an abnormal exit or
/// an exit code that is an exception code, and the scenario did not end it
/// on purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutpostCrash {
    /// The process id it had.
    pub pid: u32,
    /// Its exit code, which for a crash is the exception code, such as
    /// `0xC0000005` for an access violation or `0xC0000409` for a
    /// fail-fast; `None` when it could not be read.
    pub exit_code: Option<i32>,
    /// Where its crash dump was copied on this machine, or `None` when no
    /// dump of it was written or it could not be copied.
    pub dump: Option<String>,
}

impl std::fmt::Display for OutpostCrash {
    /// `pid 1234, code 0xC0000005, dump <path>`, with `unknown` for an
    /// unread code and `none` for a missing dump: the form a summary line
    /// holds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pid {}, code ", self.pid)?;
        match self.exit_code {
            Some(code) => write!(f, "0x{:08X}", u32::from_ne_bytes(code.to_ne_bytes()))?,
            None => f.write_str("unknown")?,
        }
        write!(f, ", dump {}", self.dump.as_deref().unwrap_or("none"))
    }
}

impl ScenarioSummary {
    /// Builds a summary from a scenario's outcome and, when the run read
    /// its speech, the latency rows of every utterance it heard.
    #[must_use]
    pub fn new(name: &str, passed: bool, latency: Option<&[LatencyRow]>) -> Self {
        Self {
            name: name.to_owned(),
            passed,
            latency_records: latency.map(<[LatencyRow]>::len),
            latency_reached_audio: latency.map(|rows| {
                rows.iter()
                    .filter(|row| row.event_to_audio_ms.is_some())
                    .count()
            }),
            max_event_to_queue_ms: latency
                .and_then(|rows| rows.iter().map(|row| row.event_to_queue_ms).max()),
            max_event_to_audio_ms: latency
                .and_then(|rows| rows.iter().filter_map(|row| row.event_to_audio_ms).max()),
            outpost_crashes: None,
            optional_utterances: Vec::new(),
        }
    }

    /// Writes this summary to `dir` (created if missing) as
    /// [`SUMMARY_FILE_NAME`], plain `key: value` lines.
    ///
    /// # Errors
    ///
    /// Returns an error if `dir` cannot be created or the file cannot be
    /// written.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let mut text = String::new();
        let _ = writeln!(text, "name: {}", self.name);
        let _ = writeln!(
            text,
            "result: {}",
            if self.passed { "pass" } else { "fail" }
        );
        let _ = writeln!(
            text,
            "latency_records: {}",
            format_optional_count(self.latency_records)
        );
        let _ = writeln!(
            text,
            "latency_reached_audio: {}",
            format_optional_count(self.latency_reached_audio)
        );
        let _ = writeln!(
            text,
            "max_event_to_queue_ms: {}",
            format_optional_u64(self.max_event_to_queue_ms)
        );
        let _ = writeln!(
            text,
            "max_event_to_audio_ms: {}",
            format_optional_u64(self.max_event_to_audio_ms)
        );
        match &self.outpost_crashes {
            Some(crashes) => {
                let _ = writeln!(text, "outpost_crashes: {}", crashes.len());
                for crash in crashes {
                    let _ = writeln!(text, "outpost_crash: {crash}");
                }
            }
            None => {
                let _ = writeln!(text, "outpost_crashes: unknown");
            }
        }
        for outcome in &self.optional_utterances {
            let _ = writeln!(text, "optional_utterance: {outcome}");
        }
        fs::write(dir.join(SUMMARY_FILE_NAME), text)
    }
}

/// How many past runs of each scenario [`archive_run`] keeps.
const KEPT_RUNS_PER_SCENARIO: usize = 100;

/// Copies one finished scenario run's artifacts from `dir` into
/// `root\history\<scenario>\<UTC time>-<pass or fail>`, and removes all
/// but the newest [`KEPT_RUNS_PER_SCENARIO`] runs of that scenario. The
/// scenario's own directory under `root` always holds the latest run; the
/// history keeps the earlier ones, so an
/// intermittent failure is still there after later runs pass.
///
/// # Errors
///
/// Returns an error if the history directory cannot be created or a file
/// cannot be copied.
pub fn archive_run(root: &Path, scenario_name: &str, dir: &Path, passed: bool) -> io::Result<()> {
    let runs = root.join("history").join(scenario_name);
    let result = if passed { "pass" } else { "fail" };
    let target = runs.join(format!(
        "{}-{result}",
        utc_stamp(std::time::SystemTime::now())
    ));
    fs::create_dir_all(&target)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        // Videos are kept only for the latest run: at most one per scenario.
        let video = entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "mp4");
        if entry.file_type()?.is_file() && !video {
            fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    let mut kept: Vec<PathBuf> = fs::read_dir(&runs)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    // The names start with the time, so they sort oldest first.
    kept.sort();
    let excess = kept.len().saturating_sub(KEPT_RUNS_PER_SCENARIO);
    for old in &kept[..excess] {
        let _ = fs::remove_dir_all(old);
    }
    Ok(())
}

/// `time` as a sortable UTC timestamp, `2026-10-02T03-04-05.678Z`: hyphens
/// instead of colons, which Windows file names cannot hold.
fn utc_stamp(time: std::time::SystemTime) -> String {
    let since_epoch = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}.{:03}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        since_epoch.subsec_millis()
    )
}

/// The date of `time` in UTC, `2026-10-15`, for compare mode's report.
pub(crate) fn utc_date(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date `days` after 1970-01-01, by Howard
/// Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_index + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    })
    .unwrap_or(1);
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// How long before the failing step began the flight recorder's inputs
/// are shown in a failure's report whatever their trace, in microseconds:
/// an application's answer under another trace is often what decided the
/// failing step.
pub const RECORDER_WINDOW_US: u64 = 3_000_000;

/// The lines a failure's report shows from the run's logs collected into
/// `dir`, in time order: every line carrying `trace` in Verbatim's
/// captured log and the outposts', listener's and synthesizer host's logs
/// (every `.log` file, [`lines_carrying`]), each prefixed with its file's
/// name; and from Core's flight recorder (`flight-recorder.jsonl`), each
/// input carrying `trace` and every input Core received in `window_us`
/// (from and to, in microseconds on Verbatim's clock), replayed and
/// rendered on one line ([`recorder_lines`]). With no window, as when
/// Verbatim's clock is unknown, only the inputs carrying `trace` are.
///
/// # Errors
///
/// Returns an error if `dir` or a file in it cannot be read.
pub fn failure_lines(
    dir: &Path,
    trace: Option<TraceId>,
    window_us: Option<(u64, u64)>,
) -> io::Result<Vec<String>> {
    let mut sources = Vec::new();
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == FLIGHT_RECORDER_FILE {
            let text = String::from_utf8_lossy(&fs::read(&path)?).into_owned();
            found.extend(recorder_lines(&text, trace, window_us));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("log") {
            let text = String::from_utf8_lossy(&fs::read(&path)?).into_owned();
            sources.push((name.to_owned(), text));
        }
    }
    sources.sort();
    if let Some(trace) = trace {
        found.extend(lines_carrying(trace, &sources));
    }
    found.sort_by_key(|(time, order, index, _)| (*time, *order, *index));
    Ok(found.into_iter().map(|(.., line)| line).collect())
}

/// The flight recorder's file in a run's artifacts.
pub const FLIGHT_RECORDER_FILE: &str = "flight-recorder.jsonl";

/// A line found for a failure's report: its time in microseconds, its
/// file's place, its line's place in the file, and its text.
type Found = (Option<u64>, usize, usize, String);

/// The inputs of the flight-recorder dump `text` that carry `trace` or
/// that arrived in `window_us`, each replayed and rendered on one line,
/// marked `failing trace` for one carrying it, as
/// `verbatim_replay::failure_inputs` gives them
/// (`books/dev/src/reference/crates/verbatim-replay.md`), prefixed with the file's name. A
/// dump that cannot be read is one line saying why.
fn recorder_lines(text: &str, trace: Option<TraceId>, window_us: Option<(u64, u64)>) -> Vec<Found> {
    let contents = match verbatim_core::dump::read_dump(&mut text.as_bytes()) {
        Ok(contents) => contents,
        Err(error) => {
            return vec![(
                None,
                0,
                0,
                format!("{FLIGHT_RECORDER_FILE}: could not be read: {error}"),
            )];
        }
    };
    let renderer = verbatim_replay::Renderer::new(&sounds_dir());
    let replayed = verbatim_replay::replay_dump(&contents);
    // With no window, one no time falls in: an empty range.
    let (from_us, to_us) = window_us.unwrap_or((u64::MAX, 0));
    verbatim_replay::failure_inputs(&replayed, &renderer, trace, from_us, to_us)
        .into_iter()
        .map(|shown| {
            (
                Some(shown.time_us),
                0,
                shown.line,
                format!("{FLIGHT_RECORDER_FILE}: {}", shown.text),
            )
        })
        .collect()
}

/// The repository's sound files, which the built-in default theme plays,
/// so a rendered utterance names a sound as the suite hears it.
fn sounds_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sounds")
}

/// Every line of `sources` (each a file's name and text) carrying
/// `trace` as a field, `trace_id=` or `trace=`, with terminal colour
/// codes removed, with its time: a log line's leading UTC timestamp. A
/// line without a time of its own (a log message's continuation) takes the
/// time of the line before it in its file, so it stays after that line.
/// Each file's place is its index in `sources` plus one, after the flight
/// recorder's.
fn lines_carrying(trace: TraceId, sources: &[(String, String)]) -> Vec<Found> {
    let trace = trace.to_string();
    let mut found = Vec::new();
    for (order, (name, text)) in sources.iter().enumerate() {
        let mut time = None;
        for (index, raw) in text.lines().enumerate() {
            let line = without_colours(raw);
            time = line_time_us(&line).or(time);
            if carries_trace(&line, &trace) {
                found.push((time, order + 1, index, format!("{name}: {line}")));
            }
        }
    }
    found
}

/// Every line of the log `text`, a file of Verbatim's named `file`, that
/// states a defect of Verbatim's own, with terminal colour codes removed,
/// each prefixed with the file's name: a line logged to
/// [`DEFECT_TARGET`], or the Rust runtime's panic message, which every
/// process's panic hook writes (`thread '<name>' panicked at
/// <file>:<line>`), since a panic is a defect, caught or not. A run whose
/// logs hold one fails, so a defect does not pass unnoticed behind speech
/// that happened to come out right.
#[must_use]
pub fn defect_lines(file: &str, text: &str) -> Vec<String> {
    let target = format!(" {DEFECT_TARGET}: ");
    text.lines()
        .map(without_colours)
        .filter(|line| line.contains(&target) || line.contains("' panicked at "))
        .map(|line| format!("{file}: {line}"))
        .collect()
}

/// Every line of an outpost's log `text`, a file named `file`, that says
/// the outpost fell back from a remote operation to the classic calls or
/// marked a window for classic reading: a line logged to
/// [`REMOTE_FALLBACK_TARGET`], which names the program, its error and the
/// window, with terminal colour codes removed and prefixed with the file's
/// name. A run whose logs hold one fails unless its scenario expects one
/// ([`crate::registry::ScenarioDef::expects_fallback`]), so a fallback
/// does not pass unnoticed behind speech that came out right.
#[must_use]
pub fn fallback_lines(file: &str, text: &str) -> Vec<String> {
    let target = format!(" {REMOTE_FALLBACK_TARGET}: ");
    text.lines()
        .map(without_colours)
        .filter(|line| line.contains(&target))
        .map(|line| format!("{file}: {line}"))
        .collect()
}

/// What Verbatim's log `text` says of the native message box titled
/// `title`, in order: "shown" for each line saying it is about to show and
/// "returned" for each saying it returned (`verbatim_gui`'s
/// `log_native_modal`), read as [`defect_lines`] reads a log, with terminal
/// colour codes removed.
#[must_use]
pub fn native_modal_lines(text: &str, title: &str) -> Vec<&'static str> {
    let titled = format!("title=\"{title}\"");
    text.lines()
        .map(without_colours)
        .filter(|line| line.contains(&titled))
        .filter_map(|line| {
            if line.contains("a native message box is about to show") {
                Some("shown")
            } else if line.contains("a native message box returned") {
                Some("returned")
            } else {
                None
            }
        })
        .collect()
}

/// `line` without the escape sequences a terminal colours text with.
fn without_colours(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // `ESC [`, parameters, and a final letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `line` holds `trace` (its decimal form) as a trace field:
/// `trace_id=` or `trace=` in a log line, `"trace_id":` or `"trace":` in
/// an input's JSON.
fn carries_trace(line: &str, trace: &str) -> bool {
    ["trace_id=", "trace=", "\"trace_id\":", "\"trace\":"]
        .iter()
        .any(|key| {
            line.match_indices(key).any(|(at, _)| {
                let value = &line[at + key.len()..];
                value.starts_with(trace)
                    && !value[trace.len()..].starts_with(|c: char| c.is_ascii_digit())
            })
        })
}

/// When `line` was written, in microseconds since the Unix epoch: a log
/// line's leading timestamp (`2026-10-09T10:42:19.268605Z`).
fn line_time_us(line: &str) -> Option<u64> {
    parse_utc_us(line.split_whitespace().next()?)
}

/// `2026-10-09T10:42:19.268605Z` as microseconds since the Unix epoch.
fn parse_utc_us(stamp: &str) -> Option<u64> {
    let stamp = stamp.strip_suffix('Z')?;
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.split(':').map(str::parse::<u64>);
    let (hours, minutes, seconds) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let micros: u64 = format!("{fraction:0<6}").get(..6)?.parse().ok()?;
    let days = u64::try_from(days_from_civil(year, month, day)).ok()?;
    Some((((days * 24 + hours) * 60 + minutes) * 60 + seconds) * 1_000_000 + micros)
}

/// The days from 1970-01-01 to the proleptic Gregorian date given, by
/// Howard Hinnant's `days_from_civil` algorithm, the inverse of
/// [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn format_optional_count(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |count| count.to_string())
}

fn format_optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |ms| ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-09T10:00:00Z, in milliseconds since the Unix epoch.
    const TEN_O_CLOCK_MS: u64 = 1_791_540_000_000;

    /// A dump of `arrivals`, each with the effect count reducing it gives,
    /// from a new state.
    fn dump_of(arrivals: Vec<verbatim_model::Arrival>) -> String {
        let mut state = verbatim_core::SrState::new();
        let mut runtime = verbatim_core::Runtime::default();
        let recorded: Vec<verbatim_core::RecordedInput> = arrivals
            .into_iter()
            .map(|arrival| verbatim_core::RecordedInput {
                effect_count: verbatim_core::reduce(&mut state, &mut runtime, &arrival).len(),
                arrival,
            })
            .collect();
        let mut out = Vec::new();
        verbatim_core::dump::write_dump(
            &mut out,
            "0.1.0",
            verbatim_core::UNKNOWN_COMMIT,
            false,
            "2026-10-09T10:00:05Z",
            &verbatim_core::SrState::new(),
            &recorded,
        )
        .expect("writes the dump");
        String::from_utf8(out).expect("a dump is UTF-8")
    }

    /// A key typing `text` at `at_ms`, as the hook reports it to the
    /// event queue.
    fn typed(trace: TraceId, text: &str, at_ms: u64) -> verbatim_model::Arrival {
        verbatim_model::Arrival {
            at_us: at_ms * 1_000,
            input: verbatim_model::Input::Queue {
                trace_id: trace,
                input: verbatim_model::request::QueueInput::Press(Box::new(
                    verbatim_model::request::Press {
                        key_seq: verbatim_model::KeySeq::new(1),
                        pressed_at_us: at_ms * 1_000,
                        action: verbatim_model::request::PressAction::Typed {
                            text: text.to_owned(),
                        },
                    },
                )),
            },
        }
    }

    /// The lines Verbatim's GUI logs around a native message box, coloured
    /// as a terminal colours them, are read by title, in order.
    #[test]
    fn a_native_message_boxs_lines_are_read_by_its_title() {
        let log = "2026-11-21T08:20:25.507447Z \u{1b}[32m INFO\u{1b}[0m verbatim_gui: a native message box is about to show title=\"Remove Theme\"\n\
                   2026-11-21T08:20:26.000000Z  INFO verbatim_gui: a native message box is about to show title=\"Error\"\n\
                   2026-11-21T08:20:30.675135Z  INFO verbatim_gui: a native message box returned title=\"Remove Theme\" returned_after_ms=5168\n";
        assert_eq!(
            native_modal_lines(log, "Remove Theme"),
            ["shown", "returned"]
        );
        assert_eq!(native_modal_lines(log, "Error"), ["shown"]);
    }

    #[test]
    fn a_failure_shows_its_trace_s_lines_and_the_recorder_s_window_in_time_order() {
        let trace = TraceId::mint();
        let other = TraceId::mint();
        let dir = temp_dir("failure-lines");
        fs::create_dir_all(&dir).expect("creates the directory");
        // The window is 09:59:57 to the failure at 10:00:00. Each line
        // carrying the trace, and each input received in the window, is
        // shown; the other trace's
        // lines and its inputs before then, or after the failure, are not.
        fs::write(
            dir.join("stderr.log"),
            format!(
                "2026-10-09T09:59:55.500000Z  INFO verbatim: gesture \u{1b}[3mtrace_id\u{1b}[0m={trace}\n\
                 2026-10-09T09:59:58.000000Z  INFO verbatim: gesture trace_id={other}\n\
                 2026-10-09T09:59:58.500000Z  INFO verbatim: gesture trace_id={trace}9\n"
            ),
        )
        .expect("writes");
        fs::write(
            dir.join("outpost-conhost-7.log"),
            format!(
                "2026-10-09T09:59:58.200000Z DEBUG verbatim_outpost: a watch is open trace={trace}\n"
            ),
        )
        .expect("writes");
        fs::write(
            dir.join(FLIGHT_RECORDER_FILE),
            dump_of(vec![
                typed(other, "a", TEN_O_CLOCK_MS - 5_000),
                typed(trace, "b", TEN_O_CLOCK_MS - 4_000),
                typed(other, "c", TEN_O_CLOCK_MS - 2_000),
                verbatim_model::Arrival {
                    at_us: (TEN_O_CLOCK_MS - 1_000) * 1_000,
                    input: verbatim_model::Input::Tick,
                },
                typed(other, "d", TEN_O_CLOCK_MS + 1_000),
            ]),
        )
        .expect("writes");
        fs::write(dir.join("timeline.txt"), format!("trace_id={trace}\n")).expect("writes");

        let window = Some((
            TEN_O_CLOCK_MS * 1_000 - RECORDER_WINDOW_US,
            TEN_O_CLOCK_MS * 1_000,
        ));
        let lines = failure_lines(&dir, Some(trace), window).expect("reads the logs");
        let typed_at = |line: usize, trace: TraceId, text: &str, at_ms: u64| {
            let at_us = at_ms * 1_000;
            format!(
                "{line}: at_us {at_us}, Queue trace {trace}, input Press pressed_at_us {at_us}, action Typed text \"{text}\", key_seq 1. 1 effect: Queue"
            )
        };
        assert_eq!(
            lines,
            [
                format!(
                    "stderr.log: 2026-10-09T09:59:55.500000Z  INFO verbatim: gesture trace_id={trace}"
                ),
                format!(
                    "flight-recorder.jsonl: (failing trace) {}",
                    typed_at(4, trace, "b", TEN_O_CLOCK_MS - 4_000)
                ),
                format!(
                    "flight-recorder.jsonl: {}",
                    typed_at(5, other, "c", TEN_O_CLOCK_MS - 2_000)
                ),
                format!(
                    "outpost-conhost-7.log: 2026-10-09T09:59:58.200000Z DEBUG verbatim_outpost: a watch is open trace={trace}"
                ),
                format!(
                    "flight-recorder.jsonl: 6: at_us {}, Tick. 0 effects",
                    (TEN_O_CLOCK_MS - 1_000) * 1_000
                ),
            ]
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_utc_stamp_names_the_date_and_time() {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_790_907_458_585);
        assert_eq!(utc_stamp(time), "2026-10-02T02-17-38.585Z");
        assert_eq!(utc_stamp(std::time::UNIX_EPOCH), "1970-01-01T00-00-00.000Z");
    }

    #[test]
    fn archiving_keeps_each_run_and_only_the_newest_hundred() {
        let root = std::env::temp_dir().join(format!(
            "verbatim-e2e-archive-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let dir = root.join("demo");
        fs::create_dir_all(&dir).expect("creates the scenario directory");
        fs::write(dir.join("summary.txt"), "name: demo").expect("writes");
        let runs = root.join("history").join("demo");
        for old in 0..KEPT_RUNS_PER_SCENARIO {
            fs::create_dir_all(runs.join(format!("2000-01-01T00-00-{old:02}.000Z-pass")))
                .expect("creates an old run");
        }

        archive_run(&root, "demo", &dir, false).expect("archives");

        let names: Vec<String> = fs::read_dir(&runs)
            .expect("lists")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names.len(), KEPT_RUNS_PER_SCENARIO);
        assert!(!names.contains(&"2000-01-01T00-00-00.000Z-pass".to_owned()));
        let newest = names
            .iter()
            .find(|name| name.ends_with("-fail"))
            .expect("the new run is kept");
        assert!(runs.join(newest).join("summary.txt").exists());

        fs::remove_dir_all(&root).ok();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("verbatim-e2e-artifacts-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn summary_round_trips_its_outpost_crashes() {
        let dir = temp_dir("round-trip-crashes");
        let mut summary = ScenarioSummary::new("conhost_leave_flood", false, None);
        summary.outpost_crashes = Some(vec![
            OutpostCrash {
                pid: 1234,
                exit_code: Some(i32::from_ne_bytes(0xC000_0005_u32.to_ne_bytes())),
                dump: Some(r"C:\artifacts\verbatim-outpost.exe.1234.dmp".to_owned()),
            },
            OutpostCrash {
                pid: 5678,
                exit_code: None,
                dump: None,
            },
        ]);
        summary.write(&dir).expect("writes the summary");

        let text = fs::read_to_string(dir.join(SUMMARY_FILE_NAME)).expect("reads the file");
        assert!(text.ends_with(concat!(
            "outpost_crashes: 2\n",
            r"outpost_crash: pid 1234, code 0xC0000005, dump C:\artifacts\verbatim-outpost.exe.1234.dmp",
            "\noutpost_crash: pid 5678, code unknown, dump none\n",
        )));
    }

    #[test]
    fn summary_round_trips_its_optional_utterances() {
        let dir = temp_dir("round-trip-optional");
        let mut summary = ScenarioSummary::new("explorer_folder_window", true, None);
        summary.optional_utterances = vec![
            "\"Working on it...\" after keys enter: cut off".to_owned(),
            "\"Working on it...\" after keys enter: absent".to_owned(),
        ];
        summary.write(&dir).expect("writes the summary");

        let text = fs::read_to_string(dir.join(SUMMARY_FILE_NAME)).expect("reads the file");
        assert!(text.ends_with(concat!(
            "optional_utterance: \"Working on it...\" after keys enter: cut off\n",
            "optional_utterance: \"Working on it...\" after keys enter: absent\n",
        )));
    }

    #[test]
    fn new_counts_reached_audio_among_the_records() {
        let records = sample_records();
        let summary = ScenarioSummary::new("scenario", true, Some(&records));
        assert_eq!(summary.latency_records, Some(3));
        assert_eq!(summary.latency_reached_audio, Some(2));
    }

    fn sample_records() -> Vec<LatencyRow> {
        vec![
            latency_row(Some(10)),
            latency_row(Some(20)),
            latency_row(None),
        ]
    }

    fn latency_row(event_to_audio_ms: Option<u64>) -> LatencyRow {
        LatencyRow {
            step: None,
            text: "OK button".to_owned(),
            event_to_queue_ms: 5,
            event_to_audio_ms,
        }
    }

    #[test]
    fn a_log_s_defect_lines_are_found_with_their_file_and_nothing_else() {
        // As Verbatim writes its log: coloured, with a warning about an
        // application and an ordinary line beside the defects.
        let log = "\u{1b}[2m2026-11-17T10:48:11.261423Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \u{1b}[2mverbatim::defect\u{1b}[0m\u{1b}[2m:\u{1b}[0m an answer did not decode as its request's reply, the reader's fault \u{1b}[3mnumber\u{1b}[0m\u{1b}[2m=\u{1b}[0mrequest 4\n\
                   2026-11-17T10:48:12.000000Z  WARN verbatim_outpost::outpost: the target application's exit is not watched\n\
                   2026-11-17T10:48:13.000000Z  INFO verbatim: gesture trace_id=7\n\
                   thread 'worker' panicked at crates/verbatim-outpost/src/outpost/read.rs:10:5:\n\
                   2026-11-17T10:48:14.000000Z ERROR verbatim::defect: Core's front entry could not be rebuilt from its checkpoint, a defect in Core replayed=1 original=2\n";
        assert_eq!(
            defect_lines("stderr.log", log),
            vec![
                "stderr.log: 2026-11-17T10:48:11.261423Z  WARN verbatim::defect: an answer did not decode as its request's reply, the reader's fault number=request 4".to_owned(),
                "stderr.log: thread 'worker' panicked at crates/verbatim-outpost/src/outpost/read.rs:10:5:".to_owned(),
                "stderr.log: 2026-11-17T10:48:14.000000Z ERROR verbatim::defect: Core's front entry could not be rebuilt from its checkpoint, a defect in Core replayed=1 original=2".to_owned(),
            ]
        );
    }

    #[test]
    fn a_log_with_no_defect_has_no_defect_lines() {
        let log = "2026-11-17T10:48:12.000000Z  WARN verbatim_outpost::outpost: the target application's exit is not watched\n\
                   2026-11-17T10:48:13.000000Z  WARN verbatim_listener: a bar's read did not return in time; its reader is abandoned pid=4\n";
        assert_eq!(defect_lines("listener.log", log), Vec::<String>::new());
    }

    /// The fallback lines of an outpost's log, coloured as it writes them:
    /// the program's fallback, with its program, window and error, and the
    /// window marked for classic reading; a defect line, an ordinary
    /// warning, and a line naming the target in its text are none.
    #[test]
    fn an_outpost_log_s_fallback_lines_are_found_with_their_file_and_nothing_else() {
        let log = "\u{1b}[2m2026-11-21T08:14:52.466965Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \u{1b}[2mverbatim::remote_fallback\u{1b}[0m\u{1b}[2m:\u{1b}[0m a remote operation failed; read the classic way \u{1b}[3mprogram\u{1b}[0m\u{1b}[2m=\u{1b}[0m\"navigation step\" \u{1b}[3mwindow\u{1b}[0m\u{1b}[2m=\u{1b}[0mSome(393818) \u{1b}[3merror\u{1b}[0m\u{1b}[2m=\u{1b}[0mimporting into a remote operation failed: Catastrophic failure (0x8000FFFF)\n\
                   2026-11-21T08:14:52.467000Z  WARN verbatim::remote_fallback: a window is marked for classic reading window=393818\n\
                   2026-11-21T08:14:52.468000Z ERROR verbatim::defect: an answer did not decode as its request's reply\n\
                   2026-11-21T08:14:52.469000Z  WARN verbatim_outpost::outpost: the target application's exit is not watched\n\
                   2026-11-21T08:14:52.470000Z  WARN verbatim_outpost::outpost: a message naming verbatim::remote_fallback in its text\n";
        assert_eq!(
            fallback_lines("outpost-mockapp-4.log", log),
            vec![
                "outpost-mockapp-4.log: 2026-11-21T08:14:52.466965Z  WARN verbatim::remote_fallback: a remote operation failed; read the classic way program=\"navigation step\" window=Some(393818) error=importing into a remote operation failed: Catastrophic failure (0x8000FFFF)".to_owned(),
                "outpost-mockapp-4.log: 2026-11-21T08:14:52.467000Z  WARN verbatim::remote_fallback: a window is marked for classic reading window=393818".to_owned(),
            ]
        );
    }

    /// Whatever its message, a line logged to the defect target is a
    /// defect, and a line of another target that names the target in its
    /// text is not.
    #[test]
    fn a_line_logged_to_the_defect_target_is_a_defect() {
        let defect =
            format!("2026-11-17T10:48:13.000000Z ERROR {DEFECT_TARGET}: any message at all");
        assert_eq!(
            defect_lines("outpost-notepad-4.log", &defect),
            vec![format!("outpost-notepad-4.log: {defect}")]
        );
        let other = format!(
            "2026-11-17T10:48:13.000000Z ERROR verbatim_outpost: a message naming {DEFECT_TARGET} in its text"
        );
        assert_eq!(
            defect_lines("outpost-notepad-4.log", &other),
            Vec::<String>::new()
        );
    }
}
