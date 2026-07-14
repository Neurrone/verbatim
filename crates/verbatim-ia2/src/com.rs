//! Small COM helpers for the MSAA client: building child-id `VARIANT`s and
//! reading scalars out of the `VARIANT`s that `IAccessible` returns.

use std::mem::ManuallyDrop;

use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4, VariantToInt32,
};
use windows::core::BSTR;

/// The `CHILDID_SELF` child identifier, addressing the object itself.
pub const CHILDID_SELF: i32 = 0;

/// Builds a `VT_I4` `VARIANT` wrapping an MSAA child id, for the `varChild`
/// argument of the `IAccessible` accessors. `VARIANT` carries no owned
/// resources for an integer, so it needs no explicit clearing.
#[must_use]
pub fn child_variant(child_id: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: child_id },
            }),
        },
    }
}

/// Reads a `VARIANT` as an `i32` (MSAA roles and state masks arrive this way).
///
/// # Safety
///
/// `value` must be a valid `VARIANT`.
#[must_use]
pub unsafe fn variant_i32(value: &VARIANT) -> Option<i32> {
    // SAFETY: forwarded to the caller's contract.
    unsafe { VariantToInt32(value).ok() }
}

/// Converts an `IAccessible` `BSTR` result to an owned `String`, mapping the
/// empty string to `None` so absent names and values stay faithful.
#[must_use]
pub fn bstr_to_option(text: &BSTR) -> Option<String> {
    let text = text.to_string();
    if text.is_empty() { None } else { Some(text) }
}
