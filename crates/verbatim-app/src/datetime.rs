//! Localized current time and date strings for the Verbatim+F12 command.
//!
//! NVDA's approach, adopted wholesale: the OS formats the values per the
//! user's regional preferences — `GetTimeFormatEx` and `GetDateFormatEx`
//! with the user default locale — so no Fluent message is involved; the
//! user's own Windows settings are the localization. The time is spoken
//! without seconds and the date in the long format, matching NVDA's flags.
//!
//! This lives in `verbatim-app` because the composition root owns command
//! routing and nothing else needs these two functions; a shared crate
//! would gain a public API for one command's formatting.

use windows::Win32::Globalization::{
    DATE_LONGDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS,
};
use windows::core::PCWSTR;

/// The current local time, formatted per the user's locale without seconds.
/// `None` when the OS call fails; callers log rather than speak that.
pub(crate) fn local_time() -> Option<String> {
    // Sizing call first (a null buffer asks for the required length), then
    // the formatting call. A null locale name is LOCALE_NAME_USER_DEFAULT;
    // a null format string selects the user's configured time format; a
    // null SYSTEMTIME formats the current local time.
    // SAFETY: both calls pass either no buffer or a live, correctly sized
    // one; the API writes at most the returned length.
    unsafe {
        let length = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, None, PCWSTR::null(), None);
        let mut buffer = vec![0u16; usize::try_from(length).ok().filter(|&len| len > 0)?];
        let written = GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            None,
            PCWSTR::null(),
            Some(&mut buffer),
        );
        string_from(&buffer, written)
    }
}

/// The current local date in the user's long date format. `None` when the
/// OS call fails.
pub(crate) fn local_date() -> Option<String> {
    // Same two-call shape as `local_time`; the trailing null is the
    // reserved calendar parameter.
    // SAFETY: as in `local_time` — no buffer, then a live, correctly sized
    // one.
    unsafe {
        let length = GetDateFormatEx(
            PCWSTR::null(),
            DATE_LONGDATE,
            None,
            PCWSTR::null(),
            None,
            PCWSTR::null(),
        );
        let mut buffer = vec![0u16; usize::try_from(length).ok().filter(|&len| len > 0)?];
        let written = GetDateFormatEx(
            PCWSTR::null(),
            DATE_LONGDATE,
            None,
            PCWSTR::null(),
            Some(&mut buffer),
            PCWSTR::null(),
        );
        string_from(&buffer, written)
    }
}

/// Decodes a formatting call's output: `written` counts UTF-16 units
/// including the terminating null, and zero means the call failed.
fn string_from(buffer: &[u16], written: i32) -> Option<String> {
    let written = usize::try_from(written).ok().filter(|&len| len > 0)?;
    Some(String::from_utf16_lossy(buffer.get(..written - 1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_formats_to_a_non_empty_string() {
        let time = local_time().expect("the OS formats the current time");
        assert!(!time.trim().is_empty());
    }

    #[test]
    fn the_date_formats_to_a_non_empty_string() {
        let date = local_date().expect("the OS formats the current date");
        assert!(!date.trim().is_empty());
    }

    #[test]
    fn decoding_strips_the_terminating_null() {
        // "9:41" plus the null terminator, five units written.
        let buffer: Vec<u16> = "9:41\0".encode_utf16().collect();
        assert_eq!(string_from(&buffer, 5).as_deref(), Some("9:41"));
        assert_eq!(string_from(&buffer, 0), None, "zero means the call failed");
        assert_eq!(string_from(&buffer, -1), None);
    }
}
