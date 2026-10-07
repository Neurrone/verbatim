//! The number an end-to-end test harness puts on each key it injects, so
//! that Verbatim can say when it has handled that key.
//!
//! A test that asserts nothing more was said after its last key needs
//! evidence that Verbatim has finished with the key, rather than a period
//! of silence. The harness's agent numbers every key stroke it injects and
//! carries the number in the key event's extra information; the keyboard
//! hook reads it back, and once everything the stroke caused has been
//! handed on, Verbatim reports the number as handled. A stroke is several
//! key events (modifiers down, the key down and up, modifiers up): all of
//! them carry the stroke's number, and the last one is marked, since that
//! is when the stroke is complete.
//!
//! The extra information is a pointer-sized integer. Its top sixteen bits
//! hold a fixed tag, which no other program's injected keys are expected
//! to carry, the next bit marks the stroke's last event, and the remaining
//! bits hold the number.

/// The tag in the top sixteen bits of a numbered key's extra information.
const TAG: u64 = 0x5645 << 48;

/// The bits holding the tag.
const TAG_MASK: u64 = 0xFFFF << 48;

/// The bit marking a stroke's last key event.
const LAST: u64 = 1 << 47;

/// The bits holding the number.
const NUMBER_MASK: u64 = LAST - 1;

/// The largest number a key can carry.
pub const MAX_NUMBER: u64 = NUMBER_MASK;

/// A numbered key event, as [`decode`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberedKey {
    /// The stroke's number.
    pub number: u64,
    /// Whether this is the stroke's last key event.
    pub last: bool,
}

/// The extra information for a key event of stroke `number`, the stroke's
/// last when `last` is set. A number above [`MAX_NUMBER`] keeps only its
/// low bits.
#[must_use]
pub fn encode(number: u64, last: bool) -> u64 {
    TAG | if last { LAST } else { 0 } | (number & NUMBER_MASK)
}

/// The numbered key a key event's extra information names, or `None` when
/// it carries no harness number.
#[must_use]
pub fn decode(extra: u64) -> Option<NumberedKey> {
    (extra & TAG_MASK == TAG).then_some(NumberedKey {
        number: extra & NUMBER_MASK,
        last: extra & LAST != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_and_its_last_mark_round_trip() {
        assert_eq!(
            decode(encode(41, false)),
            Some(NumberedKey {
                number: 41,
                last: false
            })
        );
        assert_eq!(
            decode(encode(MAX_NUMBER, true)),
            Some(NumberedKey {
                number: MAX_NUMBER,
                last: true
            })
        );
    }

    #[test]
    fn extra_information_without_the_tag_is_not_numbered() {
        assert_eq!(decode(0), None);
        assert_eq!(decode(0x5642_544D), None);
        assert_eq!(decode(LAST | 7), None);
    }
}
