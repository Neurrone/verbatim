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
//! rather than scraping the subprocess's own stdout — so the "one line per
//! scenario" run summary `docs/roadmap.md`'s M3 Track B item asks for is
//! built from a small, deliberately-written file instead of fragile text
//! parsing.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use verbatim_control::protocol::LatencyRecord;

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
    /// How many latency timelines were on record when the scenario finished,
    /// when the fetch itself succeeded — see
    /// [`crate::scenario::Scenario::latency_snapshot`], called best-effort so
    /// a broken control connection after a failed scenario cannot also break
    /// summary writing.
    pub latency_records: Option<usize>,
    /// How many of those timelines had reached audio.
    pub latency_reached_audio: Option<usize>,
    /// The worst event-to-queue latency (milliseconds) across the timelines,
    /// or `None` when there were no timelines or the fetch failed. This is
    /// the deterministic pipeline latency — event observed to speech
    /// queued — independent of the synthesizer, so it is the M3 capture-synth
    /// budget's measured quantity.
    pub max_event_to_queue_ms: Option<u64>,
    /// The worst event-to-audio latency (milliseconds) across the timelines
    /// that reached audio, or `None` when none did. Under the capture synth's
    /// instant sink this is close to the pipeline latency; under a real synth
    /// it includes synthesis and is the looser end-to-end smoke number.
    pub max_event_to_audio_ms: Option<u64>,
}

impl ScenarioSummary {
    /// Builds a summary from a scenario's outcome and, when the fetch
    /// succeeded, its latency records.
    #[must_use]
    pub fn new(name: &str, passed: bool, latency: Option<&[LatencyRecord]>) -> Self {
        Self {
            name: name.to_owned(),
            passed,
            latency_records: latency.map(<[LatencyRecord]>::len),
            latency_reached_audio: latency.map(|records| {
                records
                    .iter()
                    .filter(|record| record.audio_started_at_ms.is_some())
                    .count()
            }),
            max_event_to_queue_ms: latency.and_then(|records| {
                records
                    .iter()
                    .filter_map(|record| {
                        record
                            .speech_queued_at_ms
                            .map(|queued| queued.saturating_sub(record.event_observed_at_ms))
                    })
                    .max()
            }),
            max_event_to_audio_ms: latency.and_then(|records| {
                records
                    .iter()
                    .filter_map(|record| {
                        record
                            .audio_started_at_ms
                            .map(|audio| audio.saturating_sub(record.event_observed_at_ms))
                    })
                    .max()
            }),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn sample_records() -> Vec<LatencyRecord> {
        vec![
            latency_record(Some(10)),
            latency_record(Some(20)),
            latency_record(None),
        ]
    }

    fn latency_record(audio_started_at_ms: Option<u64>) -> LatencyRecord {
        LatencyRecord {
            trace_id: verbatim_model::TraceId::mint(),
            event_observed_at_ms: 0,
            speech_queued_at_ms: Some(5),
            audio_started_at_ms,
            stages: Vec::new(),
        }
    }
}
