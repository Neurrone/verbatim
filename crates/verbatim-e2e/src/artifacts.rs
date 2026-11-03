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
//! This module is also the seam `cargo xtask vm test` reads through: it
//! calls exactly the same [`artifacts_root`] and [`scenario_dir`] functions a
//! scenario subprocess used to decide where to look, and
//! [`ScenarioSummary::read`] to parse what that subprocess left behind —
//! rather than scraping the subprocess's own stdout — so the run summary,
//! one line per scenario, is built from a small, deliberately-written file
//! instead of fragile text parsing.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use verbatim_model::TraceId;

use crate::speech::LatencyRow;

/// Environment variable overriding [`artifacts_root`]'s default. Read by both
/// a scenario subprocess (`crates/verbatim-e2e`) and, when set in the same
/// shell, `cargo xtask vm test` (inherited into the subprocesses it spawns),
/// so the two always agree on where artifacts land without any explicit
/// wiring between the two binaries.
pub const ARTIFACTS_DIR_ENV: &str = "VERBATIM_E2E_ARTIFACTS_DIR";

/// File name [`ScenarioSummary::write`] and [`ScenarioSummary::read`] agree
/// on, inside [`scenario_dir`].
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
/// `cargo xtask vm test --scenario` selector — one identifier used
/// everywhere, per [`crate::registry`]'s own doc comment.
#[must_use]
pub fn scenario_dir(root: &Path, scenario_name: &str) -> PathBuf {
    root.join(scenario_name)
}

/// The one-line-per-fact result of running a single scenario: written by
/// [`ScenarioSummary::write`] at the end of every run (pass or fail) and read
/// back by `cargo xtask vm test` via [`ScenarioSummary::read`] to build the
/// final run summary, without needing to parse the scenario subprocess's own
/// stdout.
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
    /// holds and [`parse_outpost_crash`] reads back.
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
    /// [`SUMMARY_FILE_NAME`], plain `key: value` lines — a format
    /// [`ScenarioSummary::read`] parses back exactly.
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

    /// Reads a summary previously written by [`ScenarioSummary::write`] out
    /// of `dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary file is missing or unreadable, or a
    /// required line is missing or malformed.
    pub fn read(dir: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(dir.join(SUMMARY_FILE_NAME))?;
        parse_summary(&text).ok_or_else(|| {
            io::Error::other(format!(
                "malformed {SUMMARY_FILE_NAME} in {}",
                dir.display()
            ))
        })
    }
}

/// How many past runs of each scenario [`archive_run`] keeps.
const KEPT_RUNS_PER_SCENARIO: usize = 100;

/// Copies one finished scenario run's artifacts from `dir` into
/// `root\history\<scenario>\<UTC time>-<pass or fail>`, and removes all
/// but the newest [`KEPT_RUNS_PER_SCENARIO`] runs of that scenario. The
/// scenario's own directory under `root` always holds the latest run, which
/// `cargo xtask vm test` reads; the history keeps the earlier ones, so an
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
/// that Core received in `window_us`, each replayed and rendered on one
/// line with its marks, `failing trace` and `untimed`, as
/// `verbatim_replay::failure_inputs` gives them
/// (`docs/crates/verbatim-replay.md`), prefixed with the file's name. A
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
                shown.time_us,
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

fn parse_optional_count(value: &str) -> Option<usize> {
    if value == "unknown" {
        None
    } else {
        value.parse().ok()
    }
}

fn format_optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |ms| ms.to_string())
}

fn parse_optional_u64(value: &str) -> Option<u64> {
    if value == "unknown" {
        None
    } else {
        value.parse().ok()
    }
}

/// The pure parse behind [`ScenarioSummary::read`], split out so it can be
/// unit tested directly against hand-written text without touching a
/// filesystem.
fn parse_summary(text: &str) -> Option<ScenarioSummary> {
    let mut name = None;
    let mut passed = None;
    let mut latency_records = None;
    let mut latency_reached_audio = None;
    let mut saw_latency_records_line = false;
    let mut saw_latency_reached_audio_line = false;
    let mut max_event_to_queue_ms = None;
    let mut max_event_to_audio_ms = None;
    let mut outpost_crash_count: Option<Option<usize>> = None;
    let mut outpost_crashes = Vec::new();
    let mut optional_utterances = Vec::new();

    for line in text.lines() {
        let (key, value) = line.split_once(": ")?;
        match key {
            "name" => name = Some(value.to_owned()),
            "result" => {
                passed = Some(match value {
                    "pass" => true,
                    "fail" => false,
                    _ => return None,
                });
            }
            "latency_records" => {
                latency_records = parse_optional_count(value);
                saw_latency_records_line = true;
            }
            "latency_reached_audio" => {
                latency_reached_audio = parse_optional_count(value);
                saw_latency_reached_audio_line = true;
            }
            // The timing lines are newer than the count lines; a summary
            // written before they existed simply leaves them `None`.
            "max_event_to_queue_ms" => max_event_to_queue_ms = parse_optional_u64(value),
            "max_event_to_audio_ms" => max_event_to_audio_ms = parse_optional_u64(value),
            "outpost_crashes" => outpost_crash_count = Some(parse_optional_count(value)),
            "outpost_crash" => outpost_crashes.push(parse_outpost_crash(value)?),
            "optional_utterance" => optional_utterances.push(value.to_owned()),
            _ => {}
        }
    }

    Some(ScenarioSummary {
        name: name?,
        passed: passed?,
        latency_records: saw_latency_records_line
            .then_some(latency_records)
            .flatten(),
        latency_reached_audio: saw_latency_reached_audio_line
            .then_some(latency_reached_audio)
            .flatten(),
        max_event_to_queue_ms,
        max_event_to_audio_ms,
        // A summary written before crashes were counted has no count line,
        // and its crashes are unknown; a count that disagrees with the
        // crash lines is malformed.
        outpost_crashes: match outpost_crash_count {
            Some(Some(count)) if count == outpost_crashes.len() => Some(outpost_crashes),
            Some(Some(_)) => return None,
            _ => None,
        },
        optional_utterances,
    })
}

/// Reads back one crash as [`OutpostCrash`]'s `Display` writes it.
fn parse_outpost_crash(value: &str) -> Option<OutpostCrash> {
    let (pid, rest) = value.strip_prefix("pid ")?.split_once(", code ")?;
    let (code, dump) = rest.split_once(", dump ")?;
    let exit_code = if code == "unknown" {
        None
    } else {
        let code = u32::from_str_radix(code.strip_prefix("0x")?, 16).ok()?;
        Some(i32::from_ne_bytes(code.to_ne_bytes()))
    };
    Some(OutpostCrash {
        pid: pid.parse().ok()?,
        exit_code,
        dump: (dump != "none").then(|| dump.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-09T10:00:00Z, in milliseconds since the Unix epoch.
    const TEN_O_CLOCK_MS: u64 = 1_791_540_000_000;

    /// A dump of `inputs`, each with the effect count reducing it gives,
    /// from a new state.
    fn dump_of(inputs: Vec<verbatim_model::Input>) -> String {
        let mut state = verbatim_core::SrState::new();
        let recorded: Vec<verbatim_core::RecordedInput> = inputs
            .into_iter()
            .map(|input| verbatim_core::RecordedInput {
                effect_count: verbatim_core::reduce(&mut state, &input).len(),
                input,
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

    fn typed(trace: TraceId, text: &str, at_ms: u64) -> verbatim_model::Input {
        verbatim_model::Input::CharacterTyped {
            trace_id: trace,
            key_seq: verbatim_model::KeySeq::none_yet(),
            text: text.to_owned(),
            at_ms,
        }
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
                // No time of its own: it takes the input's before it.
                verbatim_model::Input::Tick,
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
            format!(
                "{line}: CharacterTyped trace {trace}, at_ms {at_ms}, key_seq 0, text \"{text}\". 0 effects"
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
                "flight-recorder.jsonl: (untimed) 6: Tick. 0 effects".to_owned(),
                format!(
                    "outpost-conhost-7.log: 2026-10-09T09:59:58.200000Z DEBUG verbatim_outpost: a watch is open trace={trace}"
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
    fn scenario_dir_joins_root_and_name() {
        let root = PathBuf::from(r"C:\somewhere\e2e-artifacts");
        assert_eq!(
            scenario_dir(&root, "notepad_and_verbatim_menu"),
            PathBuf::from(r"C:\somewhere\e2e-artifacts\notepad_and_verbatim_menu")
        );
    }

    #[test]
    fn resolve_artifacts_root_honors_the_override() {
        assert_eq!(
            resolve_artifacts_root(Some(r"C:\overridden\artifacts".to_owned())),
            PathBuf::from(r"C:\overridden\artifacts")
        );
    }

    #[test]
    fn resolve_artifacts_root_defaults_under_the_workspace_root() {
        assert_eq!(
            resolve_artifacts_root(None),
            crate::scenario::workspace_root()
                .join("target")
                .join("e2e-artifacts")
        );
    }

    #[test]
    fn summary_round_trips_through_write_and_read() {
        let dir = temp_dir("round-trip");
        let summary =
            ScenarioSummary::new("menu_and_settings_dialog", true, Some(&sample_records()));
        summary.write(&dir).expect("writes the summary");

        let read_back = ScenarioSummary::read(&dir).expect("reads the summary back");
        assert_eq!(read_back, summary);
    }

    #[test]
    fn summary_round_trips_with_no_latency_data() {
        let dir = temp_dir("round-trip-no-latency");
        let summary = ScenarioSummary::new("notepad_and_verbatim_menu", false, None);
        summary.write(&dir).expect("writes the summary");

        let read_back = ScenarioSummary::read(&dir).expect("reads the summary back");
        assert_eq!(read_back, summary);
        assert_eq!(read_back.latency_records, None);
        assert_eq!(read_back.latency_reached_audio, None);
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
        assert_eq!(ScenarioSummary::read(&dir).expect("reads it back"), summary);
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
        assert_eq!(ScenarioSummary::read(&dir).expect("reads it back"), summary);
    }

    #[test]
    fn read_fails_when_the_summary_file_is_missing() {
        let dir = temp_dir("missing-summary");
        fs::create_dir_all(&dir).expect("create empty dir");
        assert!(ScenarioSummary::read(&dir).is_err());
    }

    #[test]
    fn parse_summary_rejects_an_unknown_result_word() {
        let text =
            "name: x\nresult: maybe\nlatency_records: unknown\nlatency_reached_audio: unknown\n";
        assert_eq!(parse_summary(text), None);
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
}
