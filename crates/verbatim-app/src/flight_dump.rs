//! Flight-recorder dumps to disk (architecture section 9, milestone M2):
//! writes the reducer's flight recorder to a `dumps` folder next to the
//! executable, on demand from the control plane or automatically just
//! before the process dies of a panic.
//!
//! The recorder itself lives behind `Arc<Mutex<ReducerRecorder>>`, shared
//! between the reducer thread (which records each input as it processes it)
//! and both dump triggers here. Locking is brief in every case: recording
//! is a bounded ring push, and dumping only clones the retained entries
//! before releasing the lock and doing file I/O.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use verbatim_core::{ReducerRecorder, dump};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::FileTimeToSystemTime;

/// 100-nanosecond intervals between the Windows epoch (1601) and the Unix
/// epoch (1970); the same constant and `FILETIME` conversion
/// `verbatim-inspect`'s `timestamp` module uses for display, reused here to
/// name a dump instead — without that module's further conversion to local
/// time, since a dump's own timestamp is UTC.
const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

/// Current UTC time, rendered two ways: an ISO-8601-ish string for the
/// dump's header (colons, a `Z` suffix) and a filesystem-safe variant for
/// the dump's file name (hyphens in place of colons, since Windows paths
/// cannot contain colons after the drive letter).
struct UtcNow {
    header: String,
    filename_safe: String,
}

fn utc_now() -> UtcNow {
    let unix_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    let Some(systemtime) = to_utc_systemtime(unix_ms) else {
        // Practically unreachable (a wildly out-of-range clock), but a
        // dump must never fail to name itself: fall back to the raw
        // millisecond count instead of a calendar date.
        return UtcNow {
            header: format!("{unix_ms}-ms-since-unix-epoch"),
            filename_safe: unix_ms.to_string(),
        };
    };
    UtcNow {
        header: format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            systemtime.wYear,
            systemtime.wMonth,
            systemtime.wDay,
            systemtime.wHour,
            systemtime.wMinute,
            systemtime.wSecond,
            systemtime.wMilliseconds
        ),
        filename_safe: format!(
            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-{:03}Z",
            systemtime.wYear,
            systemtime.wMonth,
            systemtime.wDay,
            systemtime.wHour,
            systemtime.wMinute,
            systemtime.wSecond,
            systemtime.wMilliseconds
        ),
    }
}

/// Converts Unix milliseconds to a UTC `SYSTEMTIME` via `FILETIME`, the
/// same conversion `verbatim-inspect`'s `timestamp` module performs before
/// its further step to local time. `None` for a value so large the
/// intermediate `FILETIME` tick count overflows, or that the Win32 call
/// otherwise rejects.
fn to_utc_systemtime(unix_ms: u64) -> Option<SYSTEMTIME> {
    let ticks = UNIX_EPOCH_AS_FILETIME.checked_add(unix_ms.checked_mul(10_000)?)?;
    let filetime = FILETIME {
        #[expect(clippy::cast_possible_truncation, reason = "low half by construction")]
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut systemtime = SYSTEMTIME::default();
    // SAFETY: `filetime` is a valid, initialized FILETIME; `systemtime` is
    // a valid out-pointer.
    unsafe {
        FileTimeToSystemTime(&raw const filetime, &raw mut systemtime).ok()?;
    }
    Some(systemtime)
}

/// Writes the flight recorder's current contents to `dumps_dir` (created if
/// missing) as `flight-<UTC timestamp>.jsonl`, and returns the path
/// written. Never panics: a poisoned lock (the reducer thread panicking
/// while holding it, the very case the panic hook exists for) is tolerated
/// by recovering the possibly-torn data rather than propagating the
/// poisoning.
///
/// # Errors
///
/// Returns an error if the `dumps` folder cannot be created or the file
/// cannot be written.
pub fn dump_now(recorder: &Arc<Mutex<ReducerRecorder>>, dumps_dir: &Path) -> io::Result<PathBuf> {
    let inputs: Vec<_> = {
        let recorder = recorder.lock().unwrap_or_else(PoisonError::into_inner);
        recorder.snapshot().cloned().collect()
    };

    fs::create_dir_all(dumps_dir)?;
    let now = utc_now();
    let path = dumps_dir.join(format!("flight-{}.jsonl", now.filename_safe));
    let mut file = fs::File::create(&path)?;
    dump::write_dump(&mut file, env!("CARGO_PKG_VERSION"), &now.header, &inputs)?;
    Ok(path)
}

/// Installs a panic hook that writes a flight-recorder dump before the
/// process dies, chaining to the previously installed hook (so the default
/// panic message, or anything set up earlier, still runs). The hook itself
/// must never panic: every fallible step is wrapped so a failure is logged
/// and swallowed rather than turning a panic-time handler into a second,
/// masking panic.
pub fn install_panic_hook(recorder: Arc<Mutex<ReducerRecorder>>, dumps_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dump_now(&recorder, &dumps_dir)
        })) {
            Ok(Ok(path)) => {
                tracing::error!(path = %path.display(), "flight-recorder dump written before panic");
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "flight-recorder dump failed while handling a panic");
            }
            Err(_) => {
                tracing::error!("flight-recorder dump panicked while handling a panic; swallowed");
            }
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use verbatim_model::Input;

    /// A fresh temp directory per test invocation, so parallel test threads
    /// (and repeated runs) never collide on the same `dumps` folder.
    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("verbatim-{label}-{nanos}-{count}"))
    }

    #[test]
    fn dump_now_writes_a_readable_dump_and_returns_its_path() {
        let dir = unique_temp_dir("flight-dump-test");
        let recorder = Arc::new(Mutex::new(ReducerRecorder::new(8)));
        recorder.lock().expect("lock").record_input(Input::Tick, 0);

        let path = dump_now(&recorder, &dir).expect("dump succeeds");
        assert!(path.exists());
        assert_eq!(path.parent(), Some(dir.as_path()));

        let file = fs::File::open(&path).expect("opens the dump");
        let mut reader = io::BufReader::new(file);
        let contents = dump::read_dump(&mut reader).expect("dump parses");
        assert!(!contents.truncated);
        assert_eq!(contents.inputs.len(), 1);
        assert_eq!(contents.inputs[0].input, Input::Tick);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dump_now_recovers_from_a_poisoned_recorder_lock() {
        let dir = unique_temp_dir("flight-dump-poison-test");
        let recorder = Arc::new(Mutex::new(ReducerRecorder::new(8)));

        // Poison the lock the way a panicking reducer thread would.
        let poisoning_recorder = Arc::clone(&recorder);
        let _ = std::thread::spawn(move || {
            let _guard = poisoning_recorder.lock().expect("lock");
            panic!("simulated reducer-thread panic while holding the recorder lock");
        })
        .join();

        let path = dump_now(&recorder, &dir).expect("dump succeeds despite the poisoned lock");
        assert!(path.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
