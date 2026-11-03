//! The local clock as a screen reader's time command says it, for
//! [`Request::LocalClock`](crate::protocol::Request::LocalClock): a test
//! reads it independently of the screen reader before it presses the
//! command, so the time and date it expects are fixed beforehand.
//!
//! The time is formatted without seconds and the date in the long format,
//! each in the user's own locale formats, as Verbatim formats them
//! (`verbatim-app`'s `datetime`). A read close to the next minute first
//! waits for the minute to turn, on a waitable timer set for that minute:
//! the clock's own evidence, so the screen reader's read, made at once
//! after, falls in the same minute.

use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, SYSTEMTIME, WAIT_OBJECT_0};
use windows::Win32::Globalization::{
    DATE_LONGDATE, ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};
use windows::Win32::System::Threading::{
    CreateWaitableTimerW, SetWaitableTimer, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// A minute in the system clock's 100-nanosecond units. Every time zone's
/// offset from UTC is a whole number of minutes, so a local minute turns
/// when a UTC minute does.
const MINUTE_TICKS: u64 = 600_000_000;

/// How long past the minute's turn the timer may be late before the read
/// fails: a bound on a hang.
const TIMER_GRACE: Duration = Duration::from_secs(5);

/// The local time, without seconds, and the local date, in the long
/// format, each in the user's locale formats; when the next minute is due
/// within `turn_within`, read once it has turned.
///
/// # Errors
///
/// Returns an error if the timer cannot be made, set or waited on, or the
/// time or date cannot be formatted.
pub(crate) fn local_clock(turn_within: Duration) -> io::Result<(String, String)> {
    // SAFETY: no arguments; returns the system time.
    let now = unsafe { GetSystemTimeAsFileTime() };
    let ticks = (u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime);
    let next_minute = (ticks / MINUTE_TICKS + 1) * MINUTE_TICKS;
    let remaining = Duration::from_nanos((next_minute - ticks) * 100);
    if remaining <= turn_within {
        wait_until(next_minute, remaining + TIMER_GRACE)?;
    }
    // SAFETY: no arguments; returns the local time.
    let at = unsafe { GetLocalTime() };
    Ok((format_time(&at)?, format_date(&at)?))
}

/// Waits, on a waitable timer, until the system clock reaches `ticks`, for
/// at most `bound`.
fn wait_until(ticks: u64, bound: Duration) -> io::Result<()> {
    // SAFETY: an unnamed, auto-reset timer with default security.
    let timer =
        unsafe { CreateWaitableTimerW(None, false, PCWSTR::null()) }.map_err(io::Error::other)?;
    // SAFETY: a handle just made, owned here alone, closed when dropped.
    let timer = unsafe { OwnedHandle::from_raw_handle(timer.0) };
    let handle = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&timer));
    // A positive due time is absolute, in UTC system time.
    let due = i64::try_from(ticks).map_err(io::Error::other)?;
    // SAFETY: the timer is open; the due time outlives the call; no
    // completion routine.
    unsafe { SetWaitableTimer(handle, &raw const due, 0, None, None, false) }
        .map_err(io::Error::other)?;
    let bound_ms = u32::try_from(bound.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the timer is open.
    if unsafe { WaitForSingleObject(handle, bound_ms) } == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the minute did not turn within {bound:?}"
        )))
    }
}

/// `at` formatted without seconds in the user's time format.
fn format_time(at: &SYSTEMTIME) -> io::Result<String> {
    // SAFETY: no buffer, so the call only reports the length it needs; the
    // time outlives the call; a null locale is the user's own.
    let length = unsafe {
        GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(at),
            PCWSTR::null(),
            None,
        )
    };
    let mut buffer = vec![0u16; buffer_length(length, "time")?];
    // SAFETY: a live buffer of the reported length.
    let written = unsafe {
        GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(at),
            PCWSTR::null(),
            Some(&mut buffer),
        )
    };
    decode(&buffer, written, "time")
}

/// `at` in the user's long date format.
fn format_date(at: &SYSTEMTIME) -> io::Result<String> {
    let flags = ENUM_DATE_FORMATS_FLAGS(DATE_LONGDATE.0);
    // SAFETY: as in `format_time`; the trailing null is the reserved
    // calendar parameter.
    let length = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            flags,
            Some(at),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
        )
    };
    let mut buffer = vec![0u16; buffer_length(length, "date")?];
    // SAFETY: as in `format_time`.
    let written = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            flags,
            Some(at),
            PCWSTR::null(),
            Some(&mut buffer),
            PCWSTR::null(),
        )
    };
    decode(&buffer, written, "date")
}

/// The buffer length a sizing call reported, or an error naming `what`.
fn buffer_length(length: i32, what: &str) -> io::Result<usize> {
    usize::try_from(length)
        .ok()
        .filter(|&length| length > 0)
        .ok_or_else(|| io::Error::other(format!("the {what} could not be formatted")))
}

/// A formatting call's output, `written` counting the terminating null.
fn decode(buffer: &[u16], written: i32, what: &str) -> io::Result<String> {
    let written = buffer_length(written, what)?;
    buffer
        .get(..written - 1)
        .map(String::from_utf16_lossy)
        .ok_or_else(|| io::Error::other(format!("the {what} overran its buffer")))
}
