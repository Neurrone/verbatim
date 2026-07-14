//! Small COM helpers shared across the UIA client modules: multithreaded
//! apartment initialization and extraction of scalars from the `VARIANT`s and
//! `SAFEARRAY`s that UIA returns.

use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::System::Variant::{
    VARIANT, VariantToBooleanWithDefault, VariantToInt32, VariantToStringAlloc,
};

/// `RPC_E_CHANGED_MODE`: this thread already joined the other apartment kind.
/// Harmless for us — an outpost thread that is already in some apartment can
/// still use the UIA client library — so it is treated as success.
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106_u32.cast_signed();

/// Joins this thread to the process multithreaded apartment (architecture
/// section 4: UIA client threads live in the MTA). Idempotent per thread.
///
/// # Errors
///
/// Returns the COM error if apartment initialization fails for a reason other
/// than the thread already being in a different apartment.
pub fn init_mta() -> windows::core::Result<()> {
    // SAFETY: CoInitializeEx with no reserved pointer is always sound; the
    // returned HRESULT is inspected rather than assumed successful.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() || hr.0 == RPC_E_CHANGED_MODE {
        Ok(())
    } else {
        hr.ok()
    }
}

/// Reads a `VARIANT` string property, returning `None` for empty or absent
/// values so downstream `Option<String>` fields stay faithful.
///
/// # Safety
///
/// `value` must be a valid `VARIANT` (as produced by a UIA cached-property
/// getter). Ownership of `value` stays with the caller.
pub unsafe fn variant_string(value: &VARIANT) -> Option<String> {
    // SAFETY: the caller guarantees `value` is a valid VARIANT; the allocated
    // PWSTR is freed with CoTaskMemFree before returning, per its contract.
    unsafe {
        let raw = VariantToStringAlloc(value).ok()?;
        if raw.is_null() {
            return None;
        }
        let text = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0.cast()));
        text.filter(|s| !s.is_empty())
    }
}

/// Reads a `VARIANT` integer property (control type, process id, enum states).
///
/// # Safety
///
/// `value` must be a valid `VARIANT`.
pub unsafe fn variant_i32(value: &VARIANT) -> Option<i32> {
    // SAFETY: the caller guarantees `value` is a valid VARIANT.
    unsafe { VariantToInt32(value).ok() }
}

/// Reads a `VARIANT` boolean property, defaulting to `false` when the value is
/// absent or not a boolean (an unsupported property reads as "not set").
///
/// # Safety
///
/// `value` must be a valid `VARIANT`.
pub unsafe fn variant_bool(value: &VARIANT) -> bool {
    // SAFETY: the caller guarantees `value` is a valid VARIANT.
    unsafe { VariantToBooleanWithDefault(value, false).as_bool() }
}

/// Copies a UIA runtime-id `SAFEARRAY` of `i32` into a `Vec`, destroying the
/// array afterward. Returns an empty vector for a null or malformed array.
///
/// # Safety
///
/// `array` must be a `SAFEARRAY` pointer owned by the caller (as returned by
/// `IUIAutomationElement::GetRuntimeId`); this function takes ownership and
/// destroys it.
pub unsafe fn take_i32_safearray(array: *mut SAFEARRAY) -> Vec<i32> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: `array` is a valid, caller-owned SAFEARRAY of i32. Bounds come
    // from the array itself; each element is read by index and the array is
    // destroyed exactly once before returning.
    unsafe {
        let mut out = Vec::new();
        if let (Ok(lower), Ok(upper)) = (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1))
        {
            for index in lower..=upper {
                let mut element: i32 = 0;
                if SafeArrayGetElement(array, &raw const index, (&raw mut element).cast()).is_ok() {
                    out.push(element);
                }
            }
        }
        let _ = SafeArrayDestroy(array);
        out
    }
}
