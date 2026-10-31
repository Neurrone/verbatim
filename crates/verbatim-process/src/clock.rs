//! The clock every Verbatim process stamps the times it compares across
//! processes with: a key's press in the hook, a read's start in an
//! outpost, a reply's arrival in Core (`docs/design/terminal-model.md`,
//! "Keys"). It is the performance counter, which is monotonic and the
//! same in every process on Windows 10 and later, plus one offset that
//! makes it read as microseconds since the Unix epoch. The offset is taken
//! once, by the first Verbatim process to need it, and handed to every
//! process it launches in [`OFFSET_VARIABLE`], which they inherit with the
//! rest of its environment: so every process of one Verbatim run reads
//! the same clock, however the system's own clock is set meanwhile, and
//! its times still name the moment they were taken closely enough for
//! logs and for the end-to-end harness to match them with its own.

use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// What time it is on the monotonic clock: [`Instant::now`] unless a test
/// sets its own, for the lifetime of a kept verdict and the time budget of
/// a classic walk.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What time it is on the run's shared clock, in microseconds
/// ([`now_us`]), for the stamp an observation is judged at: the shared
/// clock unless a test sets its own.
pub type ObservationClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The environment variable a launching process hands its offset on in.
pub const OFFSET_VARIABLE: &str = "VERBATIM_CLOCK_OFFSET_US";

/// The offset from the performance counter's microseconds to the Unix
/// epoch's.
static OFFSET: OnceLock<u64> = OnceLock::new();

/// The performance counter in microseconds since the system started.
fn counter_us() -> u64 {
    let mut count = 0;
    let mut frequency = 0;
    // SAFETY: both are valid out-pointers for the call; on Windows XP and
    // later neither call fails.
    unsafe {
        let _ = QueryPerformanceCounter(&raw mut count);
    }
    // SAFETY: as above.
    unsafe {
        let _ = QueryPerformanceFrequency(&raw mut frequency);
    }
    let (count, frequency) = (
        u128::try_from(count).unwrap_or(0),
        u128::try_from(frequency).unwrap_or(1).max(1),
    );
    u64::try_from(count * 1_000_000 / frequency).unwrap_or(u64::MAX)
}

/// This run's offset: the one handed on by the process that launched this
/// one, or else taken now from the system's clock.
#[must_use]
pub fn offset() -> u64 {
    *OFFSET.get_or_init(|| {
        std::env::var(OFFSET_VARIABLE)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| {
                let wall = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| {
                        u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
                    });
                wall.saturating_sub(counter_us())
            })
    })
}

/// Now, in microseconds since the Unix epoch on this run's clock.
#[must_use]
pub fn now_us() -> u64 {
    offset().saturating_add(counter_us())
}

/// Hands this run's offset on to every process launched from now on.
///
/// # Safety
///
/// It sets an environment variable, so no other thread may be reading or
/// writing the environment: call it first thing in `main`, before any
/// thread starts.
pub unsafe fn hand_on() {
    let offset = offset();
    // SAFETY: the caller guarantees no other thread touches the
    // environment.
    unsafe { std::env::set_var(OFFSET_VARIABLE, offset.to_string()) };
}
