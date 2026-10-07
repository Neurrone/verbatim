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

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    DATE_LONGDATE, ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx, GetTimeFormatEx,
    LOCALE_NOUSEROVERRIDE, TIME_FORMAT_FLAGS, TIME_NOSECONDS,
};
use windows::core::{HSTRING, PCWSTR};

/// The current local time, formatted per the user's locale without seconds.
/// `None` when the OS call fails; callers log rather than speak that.
pub(crate) fn local_time() -> Option<String> {
    format_time(None, None)
}

/// The current local date in the user's long date format. `None` when the
/// OS call fails.
pub(crate) fn local_date() -> Option<String> {
    format_date(None, None)
}

/// The locale name to pass for `locale`, and the flag to add to the
/// formatting flags. `None` is the user's own locale with their regional
/// overrides, passed as a null name (`LOCALE_NAME_USER_DEFAULT`); a named
/// locale is formatted with its own formats, ignoring any overrides, so it
/// formats the same on every machine.
fn locale_name_and_flag(locale: Option<&str>) -> (Option<HSTRING>, u32) {
    locale.map_or((None, 0), |name| {
        (Some(HSTRING::from(name)), LOCALE_NOUSEROVERRIDE)
    })
}

/// `at`, or the current local time when `None`, formatted without seconds
/// in the time format of `locale` (the user's own when `None`).
fn format_time(locale: Option<&str>, at: Option<&SYSTEMTIME>) -> Option<String> {
    let (name, extra) = locale_name_and_flag(locale);
    let name = name
        .as_ref()
        .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
    let flags = TIME_FORMAT_FLAGS(TIME_NOSECONDS.0 | extra);
    let at = at.map(std::ptr::from_ref);
    // Sizing call first (a null buffer asks for the required length), then
    // the formatting call. A null format string selects the locale's time
    // format; a null SYSTEMTIME formats the current local time.
    // SAFETY: no buffer, so the call only reports the length it needs; the
    // name and the time outlive the call.
    let length = unsafe { GetTimeFormatEx(name, flags, at, PCWSTR::null(), None) };
    let mut buffer = vec![0u16; usize::try_from(length).ok().filter(|&len| len > 0)?];
    // SAFETY: a live buffer of the reported length; the API writes at most
    // its length.
    let written = unsafe { GetTimeFormatEx(name, flags, at, PCWSTR::null(), Some(&mut buffer)) };
    string_from(&buffer, written)
}

/// `on`, or the current local date when `None`, in the long date format of
/// `locale` (the user's own when `None`).
fn format_date(locale: Option<&str>, on: Option<&SYSTEMTIME>) -> Option<String> {
    let (name, extra) = locale_name_and_flag(locale);
    let name = name
        .as_ref()
        .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
    let flags = ENUM_DATE_FORMATS_FLAGS(DATE_LONGDATE.0 | extra);
    let on = on.map(std::ptr::from_ref);
    // Same two-call shape as `format_time`; the trailing null is the
    // reserved calendar parameter.
    // SAFETY: as in `format_time`: no buffer, so only the length.
    let length = unsafe { GetDateFormatEx(name, flags, on, PCWSTR::null(), None, PCWSTR::null()) };
    let mut buffer = vec![0u16; usize::try_from(length).ok().filter(|&len| len > 0)?];
    // SAFETY: as in `format_time`: a live buffer of the reported length.
    let written = unsafe {
        GetDateFormatEx(
            name,
            flags,
            on,
            PCWSTR::null(),
            Some(&mut buffer),
            PCWSTR::null(),
        )
    };
    string_from(&buffer, written)
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

    /// 2026-10-07 21:41:59.123, a Wednesday.
    const EVENING: SYSTEMTIME = SYSTEMTIME {
        wYear: 2026,
        wMonth: 10,
        wDayOfWeek: 3,
        wDay: 7,
        wHour: 21,
        wMinute: 41,
        wSecond: 59,
        wMilliseconds: 123,
    };

    #[test]
    fn the_time_is_formatted_without_seconds_in_the_locales_format() {
        assert_eq!(
            format_time(Some("en-US"), Some(&EVENING)).as_deref(),
            Some("9:41 PM")
        );
        assert_eq!(
            format_time(Some("de-DE"), Some(&EVENING)).as_deref(),
            Some("21:41")
        );
    }

    #[test]
    fn the_date_is_formatted_in_the_locales_long_format() {
        assert_eq!(
            format_date(Some("en-US"), Some(&EVENING)).as_deref(),
            Some("Wednesday, October 7, 2026")
        );
        assert_eq!(
            format_date(Some("de-DE"), Some(&EVENING)).as_deref(),
            Some("Mittwoch, 7. Oktober 2026")
        );
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
