//! A UIA client object on a thread of the multithreaded apartment, as
//! Microsoft advises and Verbatim's own client threads are, and the calls
//! the facts make through it. Each fact names its one API path; these are
//! thin wrappers that keep the unsafe calls in one place.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, CoalesceEventsOptions_Disabled, CoalesceEventsOptions_Enabled,
    ConnectionRecoveryBehaviorOptions_Disabled, ConnectionRecoveryBehaviorOptions_Enabled,
    IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationCondition, IUIAutomationElement,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_NamePropertyId,
};
use windows::core::{AgileReference, BSTR, HRESULT, Interface, Result};

/// `UIA_E_TIMEOUT`, a call that waited its timeout.
pub const UIA_E_TIMEOUT: HRESULT = HRESULT(0x8013_1505_u32.cast_signed());

/// Joins this thread to the multithreaded apartment.
///
/// # Panics
///
/// When the thread is already in another apartment.
pub fn mta() {
    // SAFETY: called before any COM use on this thread, or again in the
    // same apartment, which only adds a reference.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .expect("this thread joins the MTA");
}

/// A client object's two `IUIAutomation6` settings that can change what UIA
/// does: connection recovery and event coalescing. Both are off unless the
/// program is run with `--recovery` or `--coalescing` ([`crate::args`]);
/// [`client`] makes every client object with them, and a second client
/// process started through [`crate::role::Role`] is given the same flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Connection recovery on (`--recovery`).
    pub recovery: bool,
    /// Event coalescing on (`--coalescing`).
    pub coalescing: bool,
}

impl Settings {
    /// The settings as a header line names them.
    #[must_use]
    pub fn describe(self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        format!(
            "connection recovery {}, event coalescing {}",
            on(self.recovery),
            on(self.coalescing)
        )
    }

    /// The settings as command-line flags.
    #[must_use]
    pub fn flags(self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.recovery {
            flags.push("--recovery");
        }
        if self.coalescing {
            flags.push("--coalescing");
        }
        flags
    }
}

/// The settings [`client`] uses, as `recovery << 1 | coalescing`.
static SETTINGS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Sets the settings every client object this process makes uses; the
/// command line's flags set them at start.
pub fn use_settings(settings: Settings) {
    let bits = (u8::from(settings.recovery) << 1) | u8::from(settings.coalescing);
    SETTINGS.store(bits, std::sync::atomic::Ordering::SeqCst);
}

/// The settings in use.
#[must_use]
pub fn settings() -> Settings {
    let bits = SETTINGS.load(std::sync::atomic::Ordering::SeqCst);
    Settings {
        recovery: bits & 2 != 0,
        coalescing: bits & 1 != 0,
    }
}

/// A new `CUIAutomation8` client object with the settings in use.
///
/// # Panics
///
/// When it cannot be made or UIA refuses its settings.
#[must_use]
pub fn client() -> IUIAutomation6 {
    // SAFETY: the class and interface are UIA's own, made in process.
    let client: IUIAutomation6 =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .expect("a CUIAutomation8 client object");
    let settings = settings();
    set_recovery(&client, settings.recovery);
    set_coalescing(&client, settings.coalescing);
    client
}

/// A client object of the older `CUIAutomation` coclass, the only client
/// object made otherwise than by [`client`]: a fact compares it with
/// `CUIAutomation8`. It has no `IUIAutomation6`, so neither setting
/// applies to it, and no `IUIAutomation2`, so its timeouts cannot be set.
///
/// # Panics
///
/// When it cannot be made.
#[must_use]
pub fn older_client() -> windows::Win32::UI::Accessibility::IUIAutomation {
    // SAFETY: the class and interface are UIA's own, made in process.
    unsafe {
        CoCreateInstance(
            &windows::Win32::UI::Accessibility::CUIAutomation,
            None,
            CLSCTX_INPROC_SERVER,
        )
    }
    .expect("a CUIAutomation client object")
}

/// Sets the process's connection and transaction timeouts through `client`.
///
/// # Panics
///
/// When UIA refuses them.
pub fn set_timeouts(client: &IUIAutomation6, connection_ms: u32, transaction_ms: u32) {
    // SAFETY: a live client object.
    unsafe { client.SetConnectionTimeout(connection_ms) }.expect("the connection timeout");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(transaction_ms) }.expect("the transaction timeout");
}

/// The process's connection and transaction timeouts, read through
/// `client`.
///
/// # Panics
///
/// When UIA refuses the read.
#[must_use]
pub fn timeouts(client: &IUIAutomation6) -> (u32, u32) {
    // SAFETY: a live client object.
    let connection = unsafe { client.ConnectionTimeout() }.expect("the connection timeout");
    // SAFETY: as above.
    let transaction = unsafe { client.TransactionTimeout() }.expect("the transaction timeout");
    (connection, transaction)
}

/// Turns the client object's connection recovery on or off.
fn set_recovery(client: &IUIAutomation6, on: bool) {
    let option = if on {
        ConnectionRecoveryBehaviorOptions_Enabled
    } else {
        ConnectionRecoveryBehaviorOptions_Disabled
    };
    // SAFETY: a live client object.
    unsafe { client.SetConnectionRecoveryBehavior(option) }.expect("connection recovery");
}

/// Turns the client object's event coalescing on or off.
fn set_coalescing(client: &IUIAutomation6, on: bool) {
    let option = if on {
        CoalesceEventsOptions_Enabled
    } else {
        CoalesceEventsOptions_Disabled
    };
    // SAFETY: a live client object.
    unsafe { client.SetCoalesceEvents(option) }.expect("event coalescing");
}

/// `ElementFromHandle` of window `hwnd`: a new connection when the client
/// process has none to that window.
///
/// # Errors
///
/// UIA's error.
pub fn element(client: &IUIAutomation6, hwnd: isize) -> Result<IUIAutomationElement> {
    traced("ElementFromHandle", || {
        // SAFETY: a live client object; any window handle is accepted.
        unsafe { client.ElementFromHandle(HWND(hwnd as *mut _)) }
    })
}

/// `ElementFromHandleBuildCache` of window `hwnd` with `request`.
///
/// # Errors
///
/// UIA's error.
pub fn element_cached(
    client: &IUIAutomation6,
    hwnd: isize,
    request: &IUIAutomationCacheRequest,
) -> Result<IUIAutomationElement> {
    traced("ElementFromHandleBuildCache", || {
        // SAFETY: a live client object and cache request.
        unsafe { client.ElementFromHandleBuildCache(HWND(hwnd as *mut _), request) }
    })
}

/// The condition that an element's automation id is `id`; the provider
/// program names node n `n<n>`.
///
/// # Errors
///
/// UIA's error.
pub fn automation_id(client: &IUIAutomation6, id: &str) -> Result<IUIAutomationCondition> {
    // SAFETY: a live client object and a VARIANT holding a string.
    unsafe {
        client.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
    }
}

/// The condition that an element's name is `name`.
///
/// # Errors
///
/// UIA's error.
pub fn name_is(client: &IUIAutomation6, name: &str) -> Result<IUIAutomationCondition> {
    // SAFETY: as above.
    unsafe { client.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name))) }
}

/// Node `node` of the provider window whose element is `root`, found by
/// its automation id.
///
/// # Errors
///
/// UIA's error.
pub fn node(
    client: &IUIAutomation6,
    root: &IUIAutomationElement,
    node: usize,
) -> Result<IUIAutomationElement> {
    let condition = automation_id(client, &format!("n{node}"))?;
    traced(&format!("FindFirst of node {node}"), || {
        // SAFETY: live element and condition.
        unsafe { root.FindFirst(TreeScope_Descendants, &condition) }
    })
}

/// The element's current name.
///
/// # Errors
///
/// UIA's error.
pub fn name(element: &IUIAutomationElement) -> Result<String> {
    traced("CurrentName", || {
        // SAFETY: a live element.
        unsafe { element.CurrentName() }.map(|n| n.to_string())
    })
}

/// A cache request for `properties`.
///
/// # Errors
///
/// UIA's error.
pub fn cache_request(
    client: &IUIAutomation6,
    properties: &[windows::Win32::UI::Accessibility::UIA_PROPERTY_ID],
) -> Result<IUIAutomationCacheRequest> {
    // SAFETY: a live client object.
    let request = unsafe { client.CreateCacheRequest() }?;
    for property in properties {
        // SAFETY: a live cache request.
        unsafe { request.AddProperty(*property) }?;
    }
    Ok(request)
}

/// Runs `call`, timing it.
pub fn timed<T>(call: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = call();
    let took = start.elapsed();
    crate::note::evidence(&format!("a timed call took {:.1} ms", ms(took)));
    (value, took)
}

/// A duration in milliseconds.
#[must_use]
pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// A call's outcome as text: `ok`, or its HRESULT, by name where the facts
/// name it.
#[must_use]
pub fn outcome<T>(result: &Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => hresult(error.code()),
    }
}

/// An HRESULT by name where the facts name it, otherwise in hexadecimal.
#[must_use]
pub fn hresult(code: HRESULT) -> String {
    match code.0.cast_unsigned() {
        0x8013_1505 => "UIA_E_TIMEOUT".to_owned(),
        0x8004_0201 => "UIA_E_ELEMENTNOTAVAILABLE".to_owned(),
        0x8001_011E => "RPC_E_NO_CONTEXT".to_owned(),
        0x8007_0057 => "E_INVALIDARG".to_owned(),
        0x8000_4005 => "E_FAIL".to_owned(),
        0 => "S_OK".to_owned(),
        other => format!("0x{other:08X}"),
    }
}

/// An element that can be handed to another thread.
pub type Shared = AgileReference<IUIAutomationElement>;

/// `element`, to hand to another thread.
///
/// # Panics
///
/// Never for a UIA element, which is free-threaded.
#[must_use]
pub fn share(element: &IUIAutomationElement) -> Shared {
    AgileReference::new(element).expect("an agile reference to an element")
}

/// `shared`, resolved on this thread.
///
/// # Panics
///
/// Never for a UIA element, which is free-threaded.
#[must_use]
pub fn resolve(shared: &Shared) -> IUIAutomationElement {
    shared
        .resolve()
        .expect("an element resolves on any MTA thread")
}

/// Runs `work` on a new thread of the multithreaded apartment.
pub fn spawn<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> JoinHandle<T> {
    std::thread::spawn(move || {
        mta();
        work()
    })
}

/// A call's outcome with its start and end on [`crate::now_us`]'s clock.
#[derive(Clone, Debug)]
pub struct Timed {
    /// [`outcome`]'s text.
    pub outcome: String,
    /// When the call started.
    pub start: u64,
    /// When it returned.
    pub end: u64,
}

impl Timed {
    /// Runs `call` and times it.
    pub fn of<T>(call: impl FnOnce() -> Result<T>) -> Self {
        let start = crate::now_us();
        let result = call();
        let end = crate::now_us();
        let timed = Self {
            outcome: outcome(&result),
            start,
            end,
        };
        crate::note::evidence(&format!(
            "a timed call: {} after {:.1} ms",
            timed.outcome,
            timed.ms()
        ));
        timed
    }

    /// How long it took, in milliseconds.
    #[must_use]
    pub fn ms(&self) -> f64 {
        crate::stats::us_to_ms(self.end.saturating_sub(self.start))
    }
}

/// An element's cached provider description (UIA property 30107).
///
/// # Errors
///
/// UIA's error, or one when the value is not a string.
pub fn description(element: &IUIAutomationElement) -> Result<String> {
    // SAFETY: a live element.
    let value = unsafe {
        element.GetCachedPropertyValue(
            windows::Win32::UI::Accessibility::UIA_ProviderDescriptionPropertyId,
        )
    }?;
    BSTR::try_from(&value).map(|b| b.to_string())
}

/// Whether a provider description names providers and every one comes
/// from `UIAutomationCore.dll`: the answer of UIA's own proxies alone, by
/// Verbatim's own proxy rule.
pub use verbatim_uia::served_only_by_uia_proxies;

/// What an element fetched with its provider description cached is
/// served by: `own` when the description names a provider outside UIA's
/// own proxies, `proxy` when UIA's proxies alone serve it (by Verbatim's
/// proxy rule), or the error.
#[must_use]
pub fn served_by(result: &Result<IUIAutomationElement>) -> String {
    match result {
        Err(error) => hresult(error.code()),
        Ok(element) => match description(element) {
            Ok(text) if served_only_by_uia_proxies(&text) => "proxy".to_owned(),
            Ok(_) => "own".to_owned(),
            Err(error) => format!("no description ({})", hresult(error.code())),
        },
    }
}

/// `ElementFromHandleBuildCache` of window `hwnd` with its provider
/// description cached, classified by [`served_by`].
///
/// # Panics
///
/// When UIA cannot make a cache request.
#[must_use]
pub fn lookup(client: &IUIAutomation6, hwnd: isize) -> String {
    let request = cache_request(
        client,
        &[windows::Win32::UI::Accessibility::UIA_ProviderDescriptionPropertyId],
    )
    .expect("a cache request");
    served_by(&element_cached(client, hwnd, &request))
}

/// `UiaHasServerSideProvider` of window `hwnd`: whether UIA judges that
/// it has a provider of its own.
#[must_use]
pub fn has_server_side_provider(hwnd: isize) -> bool {
    let start = Instant::now();
    // SAFETY: any window handle is accepted.
    let answer = unsafe {
        windows::Win32::UI::Accessibility::UiaHasServerSideProvider(HWND(hwnd as *mut _))
    }
    .as_bool();
    crate::note::trace(&format!(
        "uia UiaHasServerSideProvider: {answer} in {:.1} ms",
        ms(start.elapsed())
    ));
    answer
}

/// Casts a client object to an older interface.
///
/// # Panics
///
/// Never for UIA's own interfaces, which `CUIAutomation8` implements.
#[must_use]
pub fn as_interface<T: Interface>(client: &IUIAutomation6) -> T {
    client
        .cast()
        .expect("CUIAutomation8 implements every IUIAutomation interface")
}

/// Waits `length`: a deliberate pause, used only where the fact is about a
/// timed stall or a pause, as each README states. A wait of a second or
/// more is named first by `why` ([`crate::note::waiting`]), so that a long
/// run does not look hung.
pub fn pause(length: Duration, why: &str) {
    if length >= Duration::from_secs(1) {
        crate::note::waiting(&format!("{:.1} s, {why}", length.as_secs_f64()));
    }
    std::thread::sleep(length);
}

/// Runs UIA call `what` under `--verbose`'s trace: its time and result.
fn traced<T>(what: &str, call: impl FnOnce() -> Result<T>) -> Result<T> {
    let start = Instant::now();
    let result = call();
    crate::note::trace(&format!(
        "uia {what}: {} in {:.1} ms",
        outcome(&result),
        ms(start.elapsed())
    ));
    result
}
