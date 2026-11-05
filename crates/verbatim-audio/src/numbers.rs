//! Conversions between the crate's numbers that `From` does not offer:
//! counts as `u64`, counts as floats, and floats as integers.

/// A count of frames or samples as a `u64`, which holds every `usize` on
/// the targets Verbatim builds for; saturating at `u64::MAX` otherwise.
pub(crate) fn count_u64(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// `value` as the nearest `f64`, as an `as` conversion rounds it: both
/// halves are exact in an `f64`, so their sum is rounded once.
pub(crate) fn u64_to_f64(value: u64) -> f64 {
    let high = u32::try_from(value >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & 0xFFFF_FFFF).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

/// `value` rounded toward zero, saturating: NaN becomes 0, and a value past
/// either end of `i128`, which holds every integer the crate converts to,
/// becomes that end. The crate's one conversion from a float to an
/// integer; the narrower ones below saturate from it in turn.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "`as` from a float to an integer saturates, as this helper states"
)]
fn float_to_i128(value: f64) -> i128 {
    value as i128
}

/// `value` as a `T`, or the end of `T`'s range on its side of zero.
fn saturate<T: TryFrom<i128>>(value: i128, min: T, max: T) -> T {
    T::try_from(value).unwrap_or(if value < 0 { min } else { max })
}

/// `value` rounded toward zero to a `u64`, saturating as an `as`
/// conversion does: below 0 and NaN become 0.
pub(crate) fn float_to_u64(value: f64) -> u64 {
    saturate(float_to_i128(value), 0, u64::MAX)
}

/// `value` rounded toward zero to a `u32`, saturating as an `as`
/// conversion does: below 0 and NaN become 0.
pub(crate) fn float_to_u32(value: f64) -> u32 {
    saturate(float_to_i128(value), 0, u32::MAX)
}

/// `value` rounded toward zero to an `i16`, saturating as an `as`
/// conversion does: NaN becomes 0.
pub(crate) fn float_to_i16(value: f64) -> i16 {
    saturate(float_to_i128(value), i16::MIN, i16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "the cast is the reference")]
    fn u64_to_f64_rounds_as_a_cast_does() {
        for value in [
            0,
            1,
            u64::from(u32::MAX),
            (1 << 53) + 1,
            (1 << 53) + 3,
            u64::MAX - 1_000,
            u64::MAX,
        ] {
            assert_eq!(
                u64_to_f64(value).to_bits(),
                (value as f64).to_bits(),
                "{value}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the cast is the reference"
    )]
    fn float_conversions_saturate_as_a_cast_does() {
        for value in [
            f64::NAN,
            -1e300,
            -40_000.7,
            -0.9,
            0.0,
            0.9,
            40_000.7,
            5e9,
            1e300,
        ] {
            assert_eq!(float_to_u64(value), value as u64, "{value}");
            assert_eq!(float_to_u32(value), value as u32, "{value}");
            assert_eq!(float_to_i16(value), value as i16, "{value}");
        }
    }
}
