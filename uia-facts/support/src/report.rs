//! A fact binary's output (`uia-facts/readme.md`, "Output" and
//! "Verdicts"): a header naming the machine, one line per claim, and an
//! exit code of 0 when every judged claim passes, 1 when any differs, and 2
//! when none differs but some are inconclusive.

use std::fmt::Debug;
use std::path::Path;
use std::process::Command;

use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::core::{HSTRING, w};

use crate::stats::{as_f64, number, wilson};

/// A claim's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// It matches the record.
    Pass,
    /// It does not.
    Differ,
    /// Neither, with the reason.
    Inconclusive(String),
    /// An open question's answer, not judged: the first run's goes into
    /// the README as the record.
    New,
    /// A figure printed for trends, not judged.
    Trend,
}

impl Verdict {
    fn word(&self) -> String {
        match self {
            Verdict::Pass => "pass".to_owned(),
            Verdict::Differ => "differ".to_owned(),
            Verdict::Inconclusive(reason) => format!("inconclusive ({reason})"),
            Verdict::New => "new".to_owned(),
            Verdict::Trend => "recorded for trends".to_owned(),
        }
    }
}

/// The claims of one run.
pub struct Report {
    verdicts: Vec<Verdict>,
}

impl Report {
    /// Prints the header for fact `fact` and starts the report.
    #[must_use]
    pub fn start(fact: &str) -> Self {
        println!("fact: {fact}");
        for (name, value) in machine() {
            println!("{name}: {value}");
        }
        Self {
            verdicts: Vec::new(),
        }
    }

    /// Starts case `name`: prints what it sets up and does, in plain
    /// words, and forgets the evidence of the case before.
    pub fn case(&mut self, name: &str, what: &str) {
        crate::note::clear();
        println!("case {name}: {what}");
    }

    /// Prints one claim's line, then, when its verdict is differ or
    /// inconclusive, the case's evidence ([`crate::note`]).
    pub fn claim(&mut self, claim: &str, observed: &str, recorded: &str, verdict: Verdict) {
        Self::line(claim, observed, recorded, &verdict);
        if matches!(verdict, Verdict::Differ | Verdict::Inconclusive(_)) {
            crate::note::print_evidence();
        }
        self.verdicts.push(verdict);
    }

    fn line(claim: &str, observed: &str, recorded: &str, verdict: &Verdict) {
        let recorded = if recorded.is_empty() {
            String::new()
        } else {
            format!("; recorded {recorded}")
        };
        println!("{claim}: observed {observed}{recorded}: {}", verdict.word());
    }

    /// Prints a claim that a missing precondition left unjudged, with the
    /// reason and no evidence, since nothing ran.
    pub fn unmet(&mut self, claim: &str, reason: &str) {
        let verdict = Verdict::Inconclusive(reason.to_owned());
        Self::line(claim, "nothing", "", &verdict);
        self.verdicts.push(verdict);
    }

    /// Ends the run with the exit code its verdicts give.
    pub fn finish(self) -> ! {
        let code = if self.verdicts.contains(&Verdict::Differ) {
            1
        } else if self
            .verdicts
            .iter()
            .any(|v| matches!(v, Verdict::Inconclusive(_)))
        {
            2
        } else {
            0
        };
        println!("exit: {code}");
        crate::exit_now(code)
    }
}

/// A deterministic outcome's verdict: pass when every run equals
/// `recorded`, differ when every run agrees on another value, and
/// inconclusive when the runs disagree.
#[must_use]
pub fn deterministic<T: PartialEq + Debug>(runs: &[T], recorded: &T) -> Verdict {
    match runs.first() {
        None => Verdict::Inconclusive("no run".to_owned()),
        Some(first) if runs.iter().any(|r| r != first) => {
            Verdict::Inconclusive(format!("the runs disagree: {runs:?}"))
        }
        Some(first) if first == recorded => Verdict::Pass,
        Some(_) => Verdict::Differ,
    }
}

/// Runs' values as text: the value and "in n of n" when every run agrees,
/// otherwise each run's.
#[must_use]
pub fn runs<T: PartialEq + Debug>(runs: &[T]) -> String {
    match runs.first() {
        Some(first) if runs.iter().all(|r| r == first) => {
            format!("{first:?} in {} of {}", runs.len(), runs.len())
        }
        _ => format!("{runs:?}"),
    }
}

/// A relation's verdict, one `bool` a run: pass when it held in every run,
/// differ when it held in none, inconclusive otherwise.
#[must_use]
pub fn relation(runs: &[bool]) -> Verdict {
    let held = runs.iter().filter(|r| **r).count();
    if runs.is_empty() {
        Verdict::Inconclusive("no run".to_owned())
    } else if held == runs.len() {
        Verdict::Pass
    } else if held == 0 {
        Verdict::Differ
    } else {
        Verdict::Inconclusive(format!("it held in {held} of {} runs", runs.len()))
    }
}

/// The band a wait on the timeouts `timeouts_ms` in turn lies in: from
/// their sum to the sum of 1.15 times each plus 50 ms
/// (`uia-facts/readme.md`, "Verdicts").
#[must_use]
pub fn band(timeouts_ms: &[f64]) -> (f64, f64) {
    let low = timeouts_ms.iter().sum();
    let high = timeouts_ms.iter().map(|t| t * 1.15 + 50.0).sum();
    (low, high)
}

/// A band's text, "low to high ms".
#[must_use]
pub fn band_text(band: (f64, f64)) -> String {
    format!("band {} to {} ms", number(band.0, 0), number(band.1, 0))
}

/// A banded figure's verdict: pass when every trial lies inside, differ
/// when every trial lies outside, inconclusive otherwise.
#[must_use]
pub fn in_band(trials_ms: &[f64], band: (f64, f64)) -> Verdict {
    let inside = trials_ms
        .iter()
        .filter(|v| **v >= band.0 && **v <= band.1)
        .count();
    if trials_ms.is_empty() {
        Verdict::Inconclusive("no trial".to_owned())
    } else if inside == trials_ms.len() {
        Verdict::Pass
    } else if inside == 0 {
        Verdict::Differ
    } else {
        Verdict::Inconclusive("some trials lie outside the band".to_owned())
    }
}

/// A rate's verdict: `k` of `n` observed against a recorded `kr` of `nr`.
/// With no occurrence, differ when the recorded rate would almost surely
/// have shown one in `n` trials, and inconclusive otherwise; with any, pass
/// when the recorded rate lies inside the observed 95 percent Wilson
/// interval, differ when the two intervals do not overlap.
#[must_use]
pub fn rate(k: usize, n: usize, kr: usize, nr: usize) -> Verdict {
    let p = as_f64(kr) / as_f64(nr);
    if k == 0 {
        let unseen = (1.0 - p).powf(as_f64(n));
        return if unseen < 0.05 {
            Verdict::Differ
        } else {
            Verdict::Inconclusive(format!("{n} trials could have missed it"))
        };
    }
    let observed = wilson(k, n);
    let recorded = wilson(kr, nr);
    if p >= observed.0 && p <= observed.1 {
        Verdict::Pass
    } else if observed.1 < recorded.0 || recorded.1 < observed.0 {
        Verdict::Differ
    } else {
        Verdict::Inconclusive("the intervals overlap".to_owned())
    }
}

/// The machine's description for the header: date, Windows build with its
/// revision, `UIAutomationCore.dll`'s file version, the processor and the
/// commit.
///
/// # Panics
///
/// When this executable's path cannot be read.
#[must_use]
pub fn machine() -> Vec<(&'static str, String)> {
    let build = registry(
        r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuild",
    );
    let revision =
        registry(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "UBR").and_then(|hex| {
            u32::from_str_radix(hex.trim_start_matches("0x"), 16)
                .ok()
                .map(|r| r.to_string())
        });
    let windows = match (build, revision) {
        (Some(build), Some(revision)) => format!("10.0.{build}.{revision}"),
        _ => "unread".to_owned(),
    };
    let processor = registry(
        r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .unwrap_or_else(|| "unread".to_owned());
    let exe = std::env::current_exe().expect("this executable's path");
    let commit = Command::new("git")
        .arg("-C")
        .arg(exe.parent().expect("its folder"))
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unread".to_owned(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        );
    vec![
        ("date", today()),
        ("windows", windows),
        ("uiautomationcore", uiautomationcore_version()),
        ("processor", processor),
        ("client", crate::uia::settings().describe()),
        ("commit", commit),
    ]
}

fn registry(key: &str, value: &str) -> Option<String> {
    let output = Command::new("reg")
        .args(["query", key, "/v", value])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with(value))?;
    let mut parts = line.split_whitespace();
    parts.next()?;
    parts.next()?;
    Some(parts.collect::<Vec<_>>().join(" "))
}

fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn uiautomationcore_version() -> String {
    let mut buffer = [0u16; 260];
    // SAFETY: a live buffer.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    let directory = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
    let path = HSTRING::from(
        Path::new(&directory)
            .join("UIAutomationCore.dll")
            .as_os_str(),
    );
    // SAFETY: a live, terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return "unread".to_owned();
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: a live buffer of `size` bytes.
    if unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.is_err() {
        return "unread".to_owned();
    }
    let mut info: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut info_length = 0u32;
    // SAFETY: the version data read above; the root block names its
    // VS_FIXEDFILEINFO, which points into `data`.
    let found = unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\"),
            &raw mut info,
            &raw mut info_length,
        )
    };
    if !found.as_bool() || info.is_null() {
        return "unread".to_owned();
    }
    // SAFETY: VerQueryValueW pointed `info` at a VS_FIXEDFILEINFO inside
    // `data`, which lives to the end of this function.
    let fixed = unsafe { &*info.cast::<VS_FIXEDFILEINFO>() };
    format!(
        "{}.{}.{}.{}",
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xffff,
        fixed.dwFileVersionLS >> 16,
        fixed.dwFileVersionLS & 0xffff
    )
}
