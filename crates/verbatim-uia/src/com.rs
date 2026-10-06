//! Small COM helpers shared across the UIA client modules: multithreaded
//! apartment initialization and extraction of scalars from the `VARIANT`s and
//! `SAFEARRAY`s that UIA returns.

use windows::Win32::Foundation::{
    CO_E_OBJNOTCONNECTED, RPC_E_DISCONNECTED, RPC_E_SERVER_DIED, RPC_E_SERVER_DIED_DNE,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, FADF_BSTR, FADF_DISPATCH, FADF_RECORD,
    FADF_UNKNOWN, FADF_VARIANT, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetElemsize,
    SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_R8, VariantToStringAlloc};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_E_ELEMENTNOTAVAILABLE};
use windows::core::HRESULT;

/// The RPC server is unavailable, as an `HRESULT`: what a call answers once
/// the provider's process has exited.
const RPC_S_SERVER_UNAVAILABLE: HRESULT = HRESULT(0x8007_06BA_u32.cast_signed());

/// The remote procedure call failed, as an `HRESULT`.
const RPC_S_CALL_FAILED: HRESULT = HRESULT(0x8007_06BE_u32.cast_signed());

/// The remote procedure call failed and did not execute, as an `HRESULT`.
const RPC_S_CALL_FAILED_DNE: HRESULT = HRESULT(0x8007_06BF_u32.cast_signed());

/// Whether a failed call on an element says the element itself is gone: UIA
/// reports it no longer available, or the provider's process or proxy has
/// disconnected or become unreachable. Any other failure, a timeout from a busy provider above
/// all, says nothing about whether the element is still alive.
#[must_use]
pub fn element_is_gone(error: &windows::core::Error) -> bool {
    [
        HRESULT(UIA_E_ELEMENTNOTAVAILABLE.cast_signed()),
        RPC_E_DISCONNECTED,
        CO_E_OBJNOTCONNECTED,
        RPC_E_SERVER_DIED,
        RPC_E_SERVER_DIED_DNE,
        RPC_S_SERVER_UNAVAILABLE,
        RPC_S_CALL_FAILED,
        RPC_S_CALL_FAILED_DNE,
    ]
    .contains(&error.code())
}

/// Runs the body of the UIA event handler `handler`, catching a panic, which
/// would otherwise abort the whole process at the COM boundary: a panic is
/// logged and the event dropped, as the outpost's worker drops an entry
/// whose handling panicked.
pub(crate) fn guarded(handler: &str, body: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        tracing::error!(
            handler,
            "a UIA event callback panicked; the event is dropped"
        );
    }
}

/// Joins this thread to the process multithreaded apartment (architecture
/// section 4: UIA client threads live in the MTA).
///
/// Each successful call adds one to this thread's COM initialization count,
/// and none is ever undone: `CoIncrementMTAUsage` keeps the MTA alive for
/// the life of the process (see the client's first-time setup), so an
/// unbalanced initialization costs nothing, and calling this again on the
/// same thread is harmless.
///
/// # Errors
///
/// Returns the COM error if apartment initialization fails, including
/// `RPC_E_CHANGED_MODE` on a thread already in a single-threaded apartment,
/// where a UIA client would be bound to that apartment and need its message
/// pump.
pub fn init_mta() -> windows::core::Result<()> {
    // SAFETY: CoInitializeEx with no reserved pointer is always sound; the
    // returned HRESULT is inspected rather than assumed successful.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()
}

/// Reads a `VARIANT` as text, returning `None` for empty or absent values
/// so downstream `Option<String>` fields stay faithful. A number or boolean
/// is converted to its text, as `VariantToStringAlloc` converts it.
#[must_use]
pub fn variant_string(value: &VARIANT) -> Option<String> {
    // SAFETY: `value` is a valid VARIANT, as every `&VARIANT` the `windows`
    // crate hands out is; the call only reads it.
    let raw = unsafe { VariantToStringAlloc(value) }.ok()?;
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is the non-null, null-terminated string just allocated.
    let text = unsafe { raw.to_string() }.ok();
    // SAFETY: `raw` was allocated with the COM task allocator for this
    // caller, which frees it once, here, after its last read.
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    text.filter(|s| !s.is_empty())
}

/// Reads a `VARIANT` integer property (control type, process id, enum
/// states), `None` when it does not convert to one.
#[must_use]
pub fn variant_i32(value: &VARIANT) -> Option<i32> {
    i32::try_from(value).ok()
}

/// Reads a `VARIANT` boolean property, `None` when the value is not a
/// boolean, as UIA's "not supported" sentinel is not.
#[must_use]
pub fn variant_optional_bool(value: &VARIANT) -> Option<bool> {
    (value.vt() == VT_BOOL).then(|| bool::try_from(value).unwrap_or(false))
}

/// Reads a `VARIANT` floating-point property, `None` when the value is not a
/// number.
#[must_use]
pub fn variant_f64(value: &VARIANT) -> Option<f64> {
    (value.vt() == VT_R8)
        .then(|| f64::try_from(value).ok())
        .flatten()
}

/// Reads a `VARIANT` boolean property, defaulting to `false` when the value is
/// absent or not a boolean (an unsupported property reads as "not set").
#[must_use]
pub fn variant_bool(value: &VARIANT) -> bool {
    bool::try_from(value).unwrap_or(false)
}

/// `element`'s runtime id, empty when the read fails or the array is not
/// one of `i32`. The array UIA returns is owned here and destroyed once, so
/// no caller handles it.
#[must_use]
pub fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    // SAFETY: `element` is a live interface; the call returns a SAFEARRAY
    // the caller owns.
    unsafe { element.GetRuntimeId() }
        // SAFETY: the array was just returned to this caller, which hands
        // its ownership to the helper.
        .map(|array| unsafe { take_safearray::<i32>(array) })
        .unwrap_or_default()
}

/// Copies a `SAFEARRAY` of `f64` (a text range's bounding rectangles) into
/// a `Vec`, destroying the array afterward. Returns an empty vector for a
/// null array or one that is not one-dimensional with 8-byte plain
/// elements.
///
/// # Safety
///
/// `array` must be null or a valid `SAFEARRAY` owned by the caller; this
/// function takes ownership and destroys it.
pub(crate) unsafe fn take_f64_safearray(array: *mut SAFEARRAY) -> Vec<f64> {
    // SAFETY: the caller's guarantee.
    unsafe { take_safearray(array) }
}

/// The element kinds of a `SAFEARRAY` whose elements own something (a
/// string, an interface, a nested `VARIANT`, or a record), which copying an
/// element out would duplicate rather than read.
const OWNING_ELEMENTS: u16 =
    FADF_BSTR.0 | FADF_UNKNOWN.0 | FADF_DISPATCH.0 | FADF_VARIANT.0 | FADF_RECORD.0;

/// Copies a one-dimensional `SAFEARRAY` of `T` into a `Vec`, destroying the
/// array afterward. The array comes from another process through UIA, so
/// its shape is checked rather than trusted: an array that is not
/// one-dimensional, whose elements are not exactly the size of `T`, or
/// whose elements own anything, reads as empty.
///
/// # Safety
///
/// `array` must be null or a valid `SAFEARRAY` owned by the caller; this
/// function takes ownership and destroys it. `T` must be a plain value type
/// for which every bit pattern is valid.
unsafe fn take_safearray<T: Copy + Default>(array: *mut SAFEARRAY) -> Vec<T> {
    if array.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: `array` is a valid SAFEARRAY (the caller's guarantee).
    let dimensions = unsafe { SafeArrayGetDim(array) };
    // SAFETY: as above.
    let element_size = unsafe { SafeArrayGetElemsize(array) };
    // SAFETY: as above; the descriptor's feature flags are a plain field.
    let features = unsafe { (*array).fFeatures.0 };
    let plain = features & OWNING_ELEMENTS == 0;
    if dimensions == 1 && plain && usize::try_from(element_size) == Ok(size_of::<T>()) {
        // SAFETY: as above, and the array has one dimension.
        let lower = unsafe { SafeArrayGetLBound(array, 1) };
        // SAFETY: as above.
        let upper = unsafe { SafeArrayGetUBound(array, 1) };
        if let (Ok(lower), Ok(upper)) = (lower, upper) {
            for index in lower..=upper {
                let mut element = T::default();
                // SAFETY: the index is within the bounds just read, and
                // `element` is a `T`, exactly the element size checked
                // above, so the copy fills it and nothing more; the
                // elements own nothing, so the copy duplicates no
                // resource.
                let read = unsafe {
                    SafeArrayGetElement(array, &raw const index, (&raw mut element).cast())
                };
                if read.is_ok() {
                    out.push(element);
                }
            }
        }
    }
    // SAFETY: the caller handed over ownership; the array is destroyed
    // exactly once, here, after its last read.
    let _ = unsafe { SafeArrayDestroy(array) };
    out
}
