//! Small helpers for the MSAA client: building child-id `VARIANT`s and
//! tidying the text that `IAccessible` reads answer.

use std::mem::ManuallyDrop;

use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};

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

/// A name, description, or other text an `IAccessible` read answered, with
/// the empty string mapped to `None` so absent names and values stay
/// faithful.
#[must_use]
pub fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty())
}

/// [`non_empty`] for a name or value, which NVDA also treats as absent
/// when it is only whitespace.
#[must_use]
pub fn visible_text(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
}
