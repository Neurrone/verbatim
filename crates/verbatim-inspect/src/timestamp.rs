//! Human-readable local timestamps for control-plane milliseconds.
//!
//! The protocol carries milliseconds since the Unix epoch; humans read
//! `2026-07-14T10:42:32.158`. Formatting converts to the machine's local
//! time (DST-correct via `SystemTimeToTzSpecificLocalTime`) and omits any
//! zone suffix by design — this is a local dev tool showing local moments.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

/// 100-nanosecond intervals between the Windows epoch (1601) and the Unix
/// epoch (1970).
const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

/// Formats milliseconds since the Unix epoch as local wall-clock time,
/// `2026-07-14T10:42:32.158`. Falls back to the raw value with an `ms`
/// suffix if conversion fails (a wildly out-of-range value).
#[must_use]
pub fn local(unix_ms: u64) -> String {
    match to_local_systemtime(unix_ms) {
        Some(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        ),
        None => format!("{unix_ms} ms"),
    }
}

fn to_local_systemtime(unix_ms: u64) -> Option<SYSTEMTIME> {
    let ticks = UNIX_EPOCH_AS_FILETIME.checked_add(unix_ms.checked_mul(10_000)?)?;
    let filetime = FILETIME {
        #[expect(clippy::cast_possible_truncation, reason = "low half by construction")]
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    // SAFETY: valid pointers to a FILETIME and out SYSTEMTIME.
    unsafe {
        FileTimeToSystemTime(&raw const filetime, &raw mut utc).ok()?;
    }
    let mut local = SYSTEMTIME::default();
    // SAFETIME is converted with the zone information for that moment, so
    // timestamps around DST transitions render correctly.
    // SAFETY: valid pointers; None selects the current time zone.
    unsafe {
        SystemTimeToTzSpecificLocalTime(None, &raw const utc, &raw mut local).ok()?;
    }
    Some(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_shape_and_falls_back() {
        // A fixed moment in 2026; the exact clock fields depend on the
        // machine's time zone, so assert the shape, not the digits.
        let formatted = local(1_783_996_840_888);
        assert_eq!(formatted.len(), "2026-07-14T10:42:32.158".len());
        assert_eq!(&formatted[4..5], "-");
        assert_eq!(&formatted[10..11], "T");
        assert_eq!(&formatted[19..20], ".");
        assert!(formatted.starts_with("2026-"));

        // Out-of-range falls back to the raw value.
        assert_eq!(local(u64::MAX), format!("{} ms", u64::MAX));
    }
}
