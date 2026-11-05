//! Reading a part of a text at a range the calling code proves
//! is in it. A miss there can only be a logic bug: it stops a debug build,
//! so unit tests and the end-to-end suite fail on it, and reads as empty in
//! a release build, which degrades the speech instead of crashing. The
//! crate has no logging, so a debug assertion is the only way to surface
//! it.

use std::fmt::Debug;
use std::slice::SliceIndex;

/// The part of `text` in `range`, which the caller proves lies in `text`
/// on character boundaries; empty, after a failed debug assertion, when it
/// does not.
#[track_caller]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the range is taken by value, as `get` takes it, so a call reads as an index does"
)]
pub(crate) fn text_in<R>(text: &str, range: R) -> &str
where
    R: SliceIndex<str, Output = str> + Debug + Clone,
{
    let Some(part) = text.get(range.clone()) else {
        debug_assert!(
            false,
            "range {range:?} outside a text of {} bytes, or off its character boundaries",
            text.len()
        );
        return "";
    };
    part
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proven_range_reads_its_part() {
        assert_eq!(text_in("état", 0..3), "ét");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(
        expected = "range 1..2 outside a text of 5 bytes, or off its character boundaries"
    )]
    fn a_range_off_a_character_boundary_stops_a_debug_build() {
        text_in("état", 1..2);
    }
}
