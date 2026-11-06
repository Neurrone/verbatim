//! The remote operation the facts send: one that reads an element's name,
//! built with Verbatim's own `verbatim-uia-rops`, so it is one request.

use verbatim_uia_rops::{Builder, Error};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_NamePropertyId};

/// Runs a remote operation reading `element`'s name.
///
/// # Errors
///
/// The operation's error, as an HRESULT.
pub fn name(element: &IUIAutomationElement) -> windows::core::Result<String> {
    let mut builder = Builder::new();
    let imported = builder.import_element(element);
    let name = builder.property(imported, UIA_NamePropertyId.0);
    let name = builder.stringify(name);
    builder.add_to_results(name);
    let outcome = builder.finish().execute().map_err(as_windows)?;
    outcome.get(name).map_err(as_windows)
}

/// Runs a remote operation reading `element`'s `property`, or, with
/// `up` set, the property of its raw-view parent, navigated to inside the
/// operation; the value stringified, as the operation's stringify writes
/// it.
///
/// # Errors
///
/// The operation's error, as an HRESULT.
pub fn property(
    element: &IUIAutomationElement,
    property: i32,
    up: bool,
) -> windows::core::Result<String> {
    let mut builder = Builder::new();
    let mut target = builder.import_element(element);
    if up {
        target = builder.navigate(target, verbatim_uia_rops::NavigationDirection::Parent);
    }
    let value = builder.property(target, property);
    let value = builder.stringify(value);
    builder.add_to_results(value);
    let outcome = builder.finish().execute().map_err(as_windows)?;
    outcome.get(value).map_err(as_windows)
}

fn as_windows(error: Error) -> windows::core::Error {
    match error {
        Error::Unavailable(e) | Error::Import(e) | Error::Execute(e) | Error::Uia(e) => e,
        Error::Failed(failure) => windows::core::Error::from_hresult(failure.extended_error),
        other => windows::core::Error::new(E_FAIL, other.to_string()),
    }
}
