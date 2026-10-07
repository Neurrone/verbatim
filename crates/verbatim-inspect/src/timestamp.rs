//! Human-readable local timestamps for control-plane milliseconds.
//!
//! The protocol carries milliseconds since the Unix epoch; humans read
//! `2026-07-14T10:42:32.158`. Formatting converts to the machine's local
//! time (DST-correct via `SystemTimeToTzSpecificLocalTime`) and omits any
//! zone suffix by design — this is a local dev tool showing local moments.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime, TIME_ZONE_INFORMATION,
};

/// 100-nanosecond intervals between the Windows epoch (1601) and the Unix
/// epoch (1970).
const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

/// Formats milliseconds since the Unix epoch as local wall-clock time,
/// `2026-07-14T10:42:32.158`. Falls back to the raw value with an `ms`
/// suffix if conversion fails (a wildly out-of-range value).
#[must_use]
pub fn local(unix_ms: u64) -> String {
    in_zone(unix_ms, None)
}

/// Formats milliseconds since the Unix epoch as wall-clock time in `zone`,
/// or in the machine's current time zone when `None`.
fn in_zone(unix_ms: u64, zone: Option<&TIME_ZONE_INFORMATION>) -> String {
    match to_zone_systemtime(unix_ms, zone) {
        Some(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        ),
        None => format!("{unix_ms} ms"),
    }
}

fn to_zone_systemtime(unix_ms: u64, zone: Option<&TIME_ZONE_INFORMATION>) -> Option<SYSTEMTIME> {
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
    // The SYSTEMTIME is converted with the zone information for that moment, so
    // timestamps around DST transitions render correctly.
    // SAFETY: valid pointers, the zone's living through the call; None
    // selects the current time zone.
    unsafe {
        SystemTimeToTzSpecificLocalTime(
            zone.map(std::ptr::from_ref),
            &raw const utc,
            &raw mut local,
        )
        .ok()?;
    }
    Some(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-07-14 02:40:40.888 UTC.
    const SUMMER: u64 = 1_783_996_840_888;

    /// 2026-01-15 12:00:00.123 UTC.
    const WINTER: u64 = 1_768_478_400_123;

    /// A zone `bias_minutes` west of UTC with no daylight saving time.
    fn fixed_zone(bias_minutes: i32) -> TIME_ZONE_INFORMATION {
        TIME_ZONE_INFORMATION {
            Bias: bias_minutes,
            ..TIME_ZONE_INFORMATION::default()
        }
    }

    /// US Eastern time: five hours west of UTC, and four from 2:00 on the
    /// second Sunday in March to 2:00 on the first Sunday in November.
    fn eastern() -> TIME_ZONE_INFORMATION {
        let rule = |month, week| SYSTEMTIME {
            wMonth: month,
            wDayOfWeek: 0,
            wDay: week,
            wHour: 2,
            ..SYSTEMTIME::default()
        };
        TIME_ZONE_INFORMATION {
            Bias: 300,
            StandardDate: rule(11, 1),
            DaylightDate: rule(3, 2),
            DaylightBias: -60,
            ..TIME_ZONE_INFORMATION::default()
        }
    }

    #[test]
    fn formats_the_wall_clock_time_in_the_zone() {
        assert_eq!(
            in_zone(SUMMER, Some(&fixed_zone(0))),
            "2026-07-14T02:40:40.888"
        );
        assert_eq!(
            in_zone(SUMMER, Some(&fixed_zone(-480))),
            "2026-07-14T10:40:40.888"
        );
        // Daylight saving time applies to the moment, not to now.
        assert_eq!(in_zone(SUMMER, Some(&eastern())), "2026-07-13T22:40:40.888");
        assert_eq!(in_zone(WINTER, Some(&eastern())), "2026-01-15T07:00:00.123");
    }

    #[test]
    fn an_out_of_range_moment_falls_back_to_the_raw_value() {
        assert_eq!(local(u64::MAX), format!("{} ms", u64::MAX));
    }
}
