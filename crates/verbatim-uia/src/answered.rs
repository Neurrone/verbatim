//! The proxy rule (`docs/design/listener-without-connections.md`, section
//! 5): an outpost's lookup by handle, or of the focused element, in a
//! window whose verdict says it has a provider is not answered when the
//! element it returns is served only by `UIAutomationCore.dll`'s own proxies,
//! as a lookup of an application busy past the connection timeout returns
//! it. Such an element is not read or kept; the next event or query that
//! needs the window looks it up again, so a refusal is never retried here.
//!
//! The test reads the provider description cached with the lookup (UIA
//! property 30107, no extra call): answered when any provider it names
//! comes from a module other than `UIAutomationCore.dll`, under whatever
//! role (main, override, nested), so File Explorer's child windows whose
//! main provider is the MSAA proxy under an override from `DUI70.dll` are
//! answered. An empty or missing description counts as answered: a lookup
//! is refused only on positive evidence.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, UIA_ProviderDescriptionPropertyId,
};

use crate::shape::{Cached, ProviderOrigin};

/// How many refusals in a row of one window's lookups are logged at debug
/// before the next lookup is logged at info with the count, so a window
/// that never answers shows in the logs.
const REFUSALS_BEFORE_INFO: u32 = 3;

/// Each window's refusals in a row, by handle; 0 counts the focused
/// element's lookups, which name no window.
static REFUSALS: LazyLock<Mutex<HashMap<isize, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `description`, a provider description as UIA reports it, names
/// providers and every one of them comes from `UIAutomationCore.dll`.
pub(crate) fn served_only_by_uia_proxies(description: &str) -> bool {
    let mut modules = description
        .split('(')
        .filter_map(|part| {
            part.strip_prefix("unmanaged:")
                .or_else(|| part.strip_prefix("managed:"))
        })
        .map(|rest| rest.split(')').next().unwrap_or(rest).trim())
        .peekable();
    modules.peek().is_some()
        && modules.all(|module| module.eq_ignore_ascii_case("UIAutomationCore.dll"))
}

/// A copy of `cache` that also caches the provider description, for
/// [`checked`]. Local.
pub(crate) fn with_description(
    cache: &IUIAutomationCacheRequest,
) -> windows::core::Result<IUIAutomationCacheRequest> {
    // SAFETY: `cache` is a live cache request; the copy is the caller's.
    let copy = unsafe { cache.Clone() }?;
    // SAFETY: a live cache request; a plain property id.
    unsafe { copy.AddProperty(UIA_ProviderDescriptionPropertyId) }?;
    Ok(copy)
}

/// `element`, returned by a lookup of `window` (0 for the focused element)
/// with a cache from [`with_description`], or an error when the proxy rule
/// refuses it. Local: the description is read from the cache.
pub(crate) fn checked(
    window: isize,
    element: IUIAutomationElement,
) -> windows::core::Result<IUIAutomationElement> {
    let description = Cached::<ProviderOrigin>::filled(element.clone(), ())
        .read()
        .description
        .unwrap_or_default();
    let mut refusals = REFUSALS.lock().unwrap_or_else(PoisonError::into_inner);
    if !served_only_by_uia_proxies(&description) {
        refusals.remove(&window);
        return Ok(element);
    }
    let count = refusals.entry(window).or_insert(0);
    *count += 1;
    if *count > REFUSALS_BEFORE_INFO {
        tracing::info!(
            window,
            refusals = *count,
            "a lookup is again served only by UIA's proxies"
        );
    } else {
        tracing::debug!(
            window,
            refusals = *count,
            "a lookup served only by UIA's proxies is not answered"
        );
    }
    Err(windows::core::Error::new(
        E_FAIL,
        "the element is served only by UIA's own proxies",
    ))
}

#[cfg(test)]
mod tests {
    use super::served_only_by_uia_proxies;

    /// Recorded descriptions, one of each kind, with the answer the rule
    /// gives (`target/e2e-evidence/listener-no-connections/
    /// proxy-rule-descriptions.txt`, from `e39` and `e40` to `e55`).
    const RECORDED: [(&str, bool); 9] = [
        // File Explorer's CtrlNotifySink, a DUI70 override over the MSAA proxy.
        (
            "[pid:13732,providerId:0x37C002C Override:Nested [pid:9820,providerId:0x37C002C Override(parent link):Unidentified Provider (unmanaged:DUI70.dll)]; Main:Nested [pid:9820,providerId:0x37C002C Annotation(parent link):Microsoft: Annotation Proxy (unmanaged:UIAutomationCore.DLL); Main:Microsoft: MSAA Proxy (unmanaged:UIAutomationCore.DLL)]; Hwnd(parent link):Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // File Explorer's NamespaceTreeControl.
        (
            "[pid:13732,providerId:0x42B064A Override(parent link):Nested [pid:9820,providerId:0x42B064A Override(parent link):Unidentified Provider (unmanaged:DUI70.dll)]; Main:Nested [pid:9820,providerId:0x42B064A Annotation(parent link):Microsoft: Annotation Proxy (unmanaged:UIAutomationCore.DLL); Main:Microsoft: MSAA Proxy (unmanaged:UIAutomationCore.DLL)]; Hwnd:Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // File Explorer's SHELLDLL_DefView.
        (
            "[pid:13732,providerId:0xEE0844 Override(parent link):Nested [pid:9820,providerId:0xEE0844 Override(parent link):Unidentified Provider (unmanaged:DUI70.dll)]; Main:Microsoft: Non-Control Proxy (unmanaged:uiautomationcore.dll); Hwnd:Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // File Explorer's ScrollBar.
        (
            "[pid:13732,providerId:0x1DE0298 Override(parent link):Nested [pid:9820,providerId:0x1DE0298 Override(parent link):Unidentified Provider (unmanaged:DUI70.dll)]; Main:Nested [pid:9820,providerId:0x1DE0298 Annotation(parent link):Microsoft: Annotation Proxy (unmanaged:UIAutomationCore.DLL); Main:Microsoft: MSAA Proxy (unmanaged:UIAutomationCore.DLL)]; Hwnd:Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // mockapp's own provider.
        (
            "[pid:17176,providerId:0x3E606C6 Main:Nested [pid:6700,providerId:0x3E606C6 Main(parent link):Unidentified Provider (unmanaged:mockapp.exe)]; Nonclient:Microsoft: Non-Client Proxy (unmanaged:uiautomationcore.dll); Hwnd(parent link):Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // conhost's own provider.
        (
            "[pid:6332,providerId:0x2F10852 Main:Nested [pid:14460,providerId:0x2F10852 Main(parent link):Microsoft Console Host Window (unmanaged:conhost.exe)]; Nonclient:Microsoft: Non-Client Proxy (unmanaged:uiautomationcore.dll); Hwnd(parent link):Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // Edge's own provider.
        (
            "[pid:13732,providerId:0x20C0648 Main:Nested [pid:10628,providerId:0x20C0648 Main(parent link):Unidentified Provider (unmanaged:msedge.dll)]; Hwnd(parent link):Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            false,
        ),
        // UIA's proxy, returned in a collision.
        (
            "[pid:380,providerId:0x3DE06C6 Main:Nested [pid:17712,providerId:0x3DE06C6 Annotation(parent link):Microsoft: Annotation Proxy (unmanaged:uiautomationcore.dll); Main:Microsoft: MSAA Proxy (unmanaged:uiautomationcore.dll)]; Nonclient:Microsoft: Non-Client Proxy (unmanaged:uiautomationcore.dll); Hwnd(parent link):Microsoft: HWND Proxy (unmanaged:uiautomationcore.dll)]",
            true,
        ),
        // The empty description.
        ("", false),
    ];

    #[test]
    fn recorded_descriptions_are_judged_as_recorded() {
        let judged: Vec<bool> = RECORDED
            .iter()
            .map(|(description, _)| served_only_by_uia_proxies(description))
            .collect();
        let expected: Vec<bool> = RECORDED.iter().map(|&(_, refused)| refused).collect();
        assert_eq!(judged, expected);
    }
}
