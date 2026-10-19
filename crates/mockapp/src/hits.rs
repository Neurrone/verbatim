//! Provider-side hit counters: how many times each provider method was
//! called, for the tests that pin each operation's cost exactly
//! (`docs/performance.md`, the operation ledger).
//!
//! One counter per provider method, shared by every node's provider object,
//! plus one for `WM_GETOBJECT`, which the host window answers. A test reads
//! and resets them with two window messages sent to the host window, so the
//! read is synchronous with the window thread that runs every provider call:
//! once a client's call has returned, every hit it caused is counted.
//!
//! - [`WM_HITS_READ`]: `wParam` is the method's index in [`Method::ALL`],
//!   and the result is its count.
//! - [`WM_HITS_RESET`]: zeroes every counter; the result is 0.
//!
//! The tests compile this file into their own shared module, for
//! [`Method`] and the message numbers, so the two sides cannot disagree.
//!
//! Counting is also where every provider call starts, so the `slow`
//! command's delay is applied here: each counted call is answered that
//! much later ([`set_delay`]); and so is the `hold` command's hold: the
//! next counted call announces itself and waits until `release`
//! ([`hold_next`], [`release`]).
//!
//! The registrations UIA reports to the fragment root
//! (`IRawElementProviderAdviseEvents`) are counted here too, per event,
//! and read with [`WM_ADVISED_READ`]: how many registrations for the
//! event are live, added less removed.
//!
//! - [`WM_ADVISED_READ`]: `wParam` is the event's id, and the result is
//!   its live registrations.
//!
//! The `WM_GETOBJECT`s the host window takes are also counted by their
//! object id, and for `UiaRootObjectId` by `wParam` ([`GetObjectKind`]),
//! so a test can tell UIA's connection messages, `UiaRootObjectId` with a
//! nonzero `wParam`, from MSAA's, and read with [`WM_GET_OBJECTS_READ`]; [`WM_HITS_RESET`] zeroes
//! them too.
//!
//! - [`WM_GET_OBJECTS_READ`]: `wParam` is the kind's index in
//!   [`GetObjectKind::ALL`], and the result is its count.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Reads one counter: `wParam` is the method's index in [`Method::ALL`].
pub(crate) const WM_HITS_READ: u32 = WM_APP + 2;

/// Zeroes every counter.
pub(crate) const WM_HITS_RESET: u32 = WM_APP + 3;

/// Reads how many registrations for one event are live: `wParam` is the
/// event's id.
pub(crate) const WM_ADVISED_READ: u32 = WM_APP + 4;

/// Reads how many `WM_GETOBJECT`s of one kind the host window took:
/// `wParam` is the kind's index in [`GetObjectKind::ALL`].
pub(crate) const WM_GET_OBJECTS_READ: u32 = WM_APP + 7;

/// A `WM_GETOBJECT` the host window took, by its object id (`lParam`), and
/// for `UiaRootObjectId` by whether its `wParam` is zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GetObjectKind {
    /// `UiaRootObjectId` with a nonzero `wParam`, the message a UIA
    /// connection begins with.
    UiaRootNonzero,
    /// `UiaRootObjectId` with a zero `wParam`.
    UiaRootZero,
    /// `OBJID_CLIENT`, MSAA's request for the client area.
    Client,
    /// `OBJID_WINDOW`, MSAA's request for the window object.
    Window,
    /// `OBJID_QUERYCLASSNAMEIDX`, which MSAA sends to choose its standard
    /// proxy for a window that does not answer its request.
    QueryClassNameIdx,
    /// Any other object id.
    Other,
}

/// `UiaRootObjectId`, the object id UIA asks a window's provider by.
const UIA_ROOT_OBJECT_ID: i32 = -25;

/// `OBJID_CLIENT`.
const OBJID_CLIENT: i32 = -4;

/// `OBJID_WINDOW`.
const OBJID_WINDOW: i32 = 0;

/// `OBJID_QUERYCLASSNAMEIDX`.
const OBJID_QUERYCLASSNAMEIDX: i32 = -12;

impl GetObjectKind {
    /// Every kind, in counter order: a kind's index here is the `wParam`
    /// that reads it.
    pub(crate) const ALL: [GetObjectKind; 6] = [
        GetObjectKind::UiaRootNonzero,
        GetObjectKind::UiaRootZero,
        GetObjectKind::Client,
        GetObjectKind::Window,
        GetObjectKind::QueryClassNameIdx,
        GetObjectKind::Other,
    ];

    /// The kind of a `WM_GETOBJECT` with these parameters; the object id is
    /// 32 bits, zero-extended into `lParam`.
    #[allow(
        dead_code,
        reason = "mockapp counts by it; the tests compile this file too"
    )]
    pub(crate) fn of(wparam: usize, lparam: isize) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the object id is the low 32 bits of lParam"
        )]
        let object = lparam as i32;
        match object {
            UIA_ROOT_OBJECT_ID if wparam != 0 => GetObjectKind::UiaRootNonzero,
            UIA_ROOT_OBJECT_ID => GetObjectKind::UiaRootZero,
            OBJID_CLIENT => GetObjectKind::Client,
            OBJID_WINDOW => GetObjectKind::Window,
            OBJID_QUERYCLASSNAMEIDX => GetObjectKind::QueryClassNameIdx,
            _ => GetObjectKind::Other,
        }
    }

    /// The kind's name: its object id, and for `UiaRootObjectId` its
    /// `wParam`.
    #[allow(
        dead_code,
        reason = "the tests read it, compiling this file into their own module"
    )]
    pub(crate) fn name(self) -> &'static str {
        match self {
            GetObjectKind::UiaRootNonzero => "UiaRootObjectId, nonzero wParam",
            GetObjectKind::UiaRootZero => "UiaRootObjectId, zero wParam",
            GetObjectKind::Client => "OBJID_CLIENT",
            GetObjectKind::Window => "OBJID_WINDOW",
            GetObjectKind::QueryClassNameIdx => "OBJID_QUERYCLASSNAMEIDX",
            GetObjectKind::Other => "another object id",
        }
    }
}

/// One counter per [`GetObjectKind`], in [`GetObjectKind::ALL`] order.
static GET_OBJECTS: [AtomicU32; GetObjectKind::ALL.len()] =
    [const { AtomicU32::new(0) }; GetObjectKind::ALL.len()];

/// Counts one `WM_GETOBJECT` with these parameters, as a hit of
/// [`Method::GetObject`] and by its kind.
#[allow(
    dead_code,
    reason = "mockapp counts with it; the tests compile this file too"
)]
pub(crate) fn hit_get_object(wparam: usize, lparam: isize) {
    GET_OBJECTS[GetObjectKind::of(wparam, lparam) as usize].fetch_add(1, Ordering::Relaxed);
    hit(Method::GetObject);
}

/// The count for the kind at `index` in [`GetObjectKind::ALL`], or 0 for
/// an index past the end.
#[allow(
    dead_code,
    reason = "mockapp answers with it; the tests compile this file too"
)]
pub(crate) fn read_get_objects(index: usize) -> u32 {
    GET_OBJECTS
        .get(index)
        .map_or(0, |counter| counter.load(Ordering::Relaxed))
}

/// A provider method mockapp counts calls to, named as its interface names
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    /// `WM_GETOBJECT`, answered by the host window.
    GetObject,
    /// `IRawElementProviderSimple::ProviderOptions`.
    ProviderOptions,
    /// `IRawElementProviderSimple::GetPatternProvider`.
    GetPatternProvider,
    /// `IRawElementProviderSimple::GetPropertyValue`.
    GetPropertyValue,
    /// `IRawElementProviderSimple::HostRawElementProvider`.
    HostRawElementProvider,
    /// `IRawElementProviderFragment::Navigate`.
    Navigate,
    /// `IRawElementProviderFragment::GetRuntimeId`.
    GetRuntimeId,
    /// `IRawElementProviderFragment::BoundingRectangle`.
    BoundingRectangle,
    /// `IRawElementProviderFragment::GetEmbeddedFragmentRoots`.
    GetEmbeddedFragmentRoots,
    /// `IRawElementProviderFragment::SetFocus`.
    SetFocus,
    /// `IRawElementProviderFragment::FragmentRoot`.
    FragmentRoot,
    /// `IRawElementProviderFragmentRoot::ElementProviderFromPoint`.
    ElementProviderFromPoint,
    /// `IRawElementProviderFragmentRoot::GetFocus`.
    GetFocus,
    /// `IToggleProvider::Toggle`.
    Toggle,
    /// `IToggleProvider::ToggleState`.
    ToggleState,
    /// `IExpandCollapseProvider::Expand`.
    Expand,
    /// `IExpandCollapseProvider::Collapse`.
    Collapse,
    /// `IExpandCollapseProvider::ExpandCollapseState`.
    ExpandCollapseState,
    /// `IValueProvider::SetValue`.
    SetValue,
    /// `IValueProvider::Value`.
    Value,
    /// `IValueProvider::IsReadOnly`.
    IsReadOnly,
    /// `ISelectionItemProvider::Select`.
    Select,
    /// `ISelectionItemProvider::AddToSelection`.
    AddToSelection,
    /// `ISelectionItemProvider::RemoveFromSelection`.
    RemoveFromSelection,
    /// `ISelectionItemProvider::IsSelected`.
    IsSelected,
    /// `ISelectionItemProvider::SelectionContainer`.
    SelectionContainer,
    /// `ISelectionProvider::GetSelection`.
    GetSelection,
    /// `ISelectionProvider::CanSelectMultiple`.
    CanSelectMultiple,
    /// `ISelectionProvider::IsSelectionRequired`.
    IsSelectionRequired,
    /// `ISelectionProvider2::FirstSelectedItem`.
    FirstSelectedItem,
    /// `ISelectionProvider2::LastSelectedItem`.
    LastSelectedItem,
    /// `ISelectionProvider2::CurrentSelectedItem`.
    CurrentSelectedItem,
    /// `ISelectionProvider2::ItemCount`.
    ItemCount,
    /// `IAccessible::accParent`.
    AccParent,
    /// `IAccessible::accChildCount`.
    AccChildCount,
    /// `IAccessible::get_accChild`.
    AccChild,
    /// `IAccessible::get_accName`.
    AccName,
    /// `IAccessible::get_accValue`.
    AccValue,
    /// `IAccessible::get_accDescription`.
    AccDescription,
    /// `IAccessible::get_accRole`.
    AccRole,
    /// `IAccessible::get_accState`.
    AccState,
    /// `IAccessible::get_accHelp`.
    AccHelp,
    /// `IAccessible::get_accHelpTopic`.
    AccHelpTopic,
    /// `IAccessible::get_accKeyboardShortcut`.
    AccKeyboardShortcut,
    /// `IAccessible::accFocus`.
    AccFocus,
    /// `IAccessible::accSelection`.
    AccSelection,
    /// `IAccessible::get_accDefaultAction`.
    AccDefaultAction,
    /// `IAccessible::accSelect`.
    AccSelect,
    /// `IAccessible::accLocation`.
    AccLocation,
    /// `IAccessible::accNavigate`.
    AccNavigate,
    /// `IAccessible::accHitTest`.
    AccHitTest,
    /// `IAccessible::accDoDefaultAction`.
    AccDoDefaultAction,
    /// `IAccessible::put_accName`.
    PutAccName,
    /// `IAccessible::put_accValue`.
    PutAccValue,
    /// `IDispatch::GetTypeInfoCount`.
    GetTypeInfoCount,
    /// `IDispatch::GetTypeInfo`.
    GetTypeInfo,
    /// `IDispatch::GetIDsOfNames`.
    GetIdsOfNames,
    /// `IDispatch::Invoke`.
    DispatchInvoke,
    /// `ITextProvider::GetSelection`.
    TextGetSelection,
    /// `ITextProvider::GetVisibleRanges`.
    TextGetVisibleRanges,
    /// `ITextProvider::DocumentRange`.
    TextDocumentRange,
    /// `ITextProvider2::GetCaretRange`.
    TextGetCaretRange,
    /// `ITextRangeProvider::Clone`.
    RangeClone,
    /// `ITextRangeProvider::Compare`.
    RangeCompare,
    /// `ITextRangeProvider::CompareEndpoints`.
    RangeCompareEndpoints,
    /// `ITextRangeProvider::ExpandToEnclosingUnit`.
    RangeExpandToEnclosingUnit,
    /// `ITextRangeProvider::FindText`.
    RangeFindText,
    /// `ITextRangeProvider::GetAttributeValue`.
    RangeGetAttributeValue,
    /// `ITextRangeProvider::GetBoundingRectangles`.
    RangeGetBoundingRectangles,
    /// `ITextRangeProvider::GetText`.
    RangeGetText,
    /// `ITextRangeProvider::Move`.
    RangeMove,
    /// `ITextRangeProvider::MoveEndpointByUnit`.
    RangeMoveEndpointByUnit,
    /// `ITextRangeProvider::MoveEndpointByRange`.
    RangeMoveEndpointByRange,
    /// `ITextRangeProvider::Select`.
    RangeSelect,
}

/// How many methods are counted.
const COUNT: usize = Method::ALL.len();

/// One counter per method, in [`Method::ALL`] order.
static HITS: [AtomicU32; COUNT] = [const { AtomicU32::new(0) }; COUNT];

impl Method {
    /// Every counted method, in counter order: a method's index here is the
    /// `wParam` that reads it.
    pub(crate) const ALL: [Method; 74] = [
        Method::GetObject,
        Method::ProviderOptions,
        Method::GetPatternProvider,
        Method::GetPropertyValue,
        Method::HostRawElementProvider,
        Method::Navigate,
        Method::GetRuntimeId,
        Method::BoundingRectangle,
        Method::GetEmbeddedFragmentRoots,
        Method::SetFocus,
        Method::FragmentRoot,
        Method::ElementProviderFromPoint,
        Method::GetFocus,
        Method::Toggle,
        Method::ToggleState,
        Method::Expand,
        Method::Collapse,
        Method::ExpandCollapseState,
        Method::SetValue,
        Method::Value,
        Method::IsReadOnly,
        Method::Select,
        Method::AddToSelection,
        Method::RemoveFromSelection,
        Method::IsSelected,
        Method::SelectionContainer,
        Method::GetSelection,
        Method::CanSelectMultiple,
        Method::IsSelectionRequired,
        Method::FirstSelectedItem,
        Method::LastSelectedItem,
        Method::CurrentSelectedItem,
        Method::ItemCount,
        Method::AccParent,
        Method::AccChildCount,
        Method::AccChild,
        Method::AccName,
        Method::AccValue,
        Method::AccDescription,
        Method::AccRole,
        Method::AccState,
        Method::AccHelp,
        Method::AccHelpTopic,
        Method::AccKeyboardShortcut,
        Method::AccFocus,
        Method::AccSelection,
        Method::AccDefaultAction,
        Method::AccSelect,
        Method::AccLocation,
        Method::AccNavigate,
        Method::AccHitTest,
        Method::AccDoDefaultAction,
        Method::PutAccName,
        Method::PutAccValue,
        Method::GetTypeInfoCount,
        Method::GetTypeInfo,
        Method::GetIdsOfNames,
        Method::DispatchInvoke,
        Method::TextGetSelection,
        Method::TextGetVisibleRanges,
        Method::TextDocumentRange,
        Method::TextGetCaretRange,
        Method::RangeClone,
        Method::RangeCompare,
        Method::RangeCompareEndpoints,
        Method::RangeExpandToEnclosingUnit,
        Method::RangeFindText,
        Method::RangeGetAttributeValue,
        Method::RangeGetBoundingRectangles,
        Method::RangeGetText,
        Method::RangeMove,
        Method::RangeMoveEndpointByUnit,
        Method::RangeMoveEndpointByRange,
        Method::RangeSelect,
    ];

    /// The method's name, as its interface names it.
    #[allow(
        dead_code,
        reason = "the tests read it, compiling this file into their own module"
    )]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Method::GetObject => "WM_GETOBJECT",
            Method::ProviderOptions => "ProviderOptions",
            Method::GetPatternProvider => "GetPatternProvider",
            Method::GetPropertyValue => "GetPropertyValue",
            Method::HostRawElementProvider => "HostRawElementProvider",
            Method::Navigate => "Navigate",
            Method::GetRuntimeId => "GetRuntimeId",
            Method::BoundingRectangle => "BoundingRectangle",
            Method::GetEmbeddedFragmentRoots => "GetEmbeddedFragmentRoots",
            Method::SetFocus => "SetFocus",
            Method::FragmentRoot => "FragmentRoot",
            Method::ElementProviderFromPoint => "ElementProviderFromPoint",
            Method::GetFocus => "GetFocus",
            Method::Toggle => "Toggle",
            Method::ToggleState => "ToggleState",
            Method::Expand => "Expand",
            Method::Collapse => "Collapse",
            Method::ExpandCollapseState => "ExpandCollapseState",
            Method::SetValue => "SetValue",
            Method::Value => "Value",
            Method::IsReadOnly => "IsReadOnly",
            Method::Select => "Select",
            Method::AddToSelection => "AddToSelection",
            Method::RemoveFromSelection => "RemoveFromSelection",
            Method::IsSelected => "IsSelected",
            Method::SelectionContainer => "SelectionContainer",
            Method::GetSelection => "GetSelection",
            Method::CanSelectMultiple => "CanSelectMultiple",
            Method::IsSelectionRequired => "IsSelectionRequired",
            Method::FirstSelectedItem => "FirstSelectedItem",
            Method::LastSelectedItem => "LastSelectedItem",
            Method::CurrentSelectedItem => "CurrentSelectedItem",
            Method::ItemCount => "ItemCount",
            Method::AccParent => "accParent",
            Method::AccChildCount => "accChildCount",
            Method::AccChild => "get_accChild",
            Method::AccName => "get_accName",
            Method::AccValue => "get_accValue",
            Method::AccDescription => "get_accDescription",
            Method::AccRole => "get_accRole",
            Method::AccState => "get_accState",
            Method::AccHelp => "get_accHelp",
            Method::AccHelpTopic => "get_accHelpTopic",
            Method::AccKeyboardShortcut => "get_accKeyboardShortcut",
            Method::AccFocus => "accFocus",
            Method::AccSelection => "accSelection",
            Method::AccDefaultAction => "get_accDefaultAction",
            Method::AccSelect => "accSelect",
            Method::AccLocation => "accLocation",
            Method::AccNavigate => "accNavigate",
            Method::AccHitTest => "accHitTest",
            Method::AccDoDefaultAction => "accDoDefaultAction",
            Method::PutAccName => "put_accName",
            Method::PutAccValue => "put_accValue",
            Method::GetTypeInfoCount => "GetTypeInfoCount",
            Method::GetTypeInfo => "GetTypeInfo",
            Method::GetIdsOfNames => "GetIDsOfNames",
            Method::DispatchInvoke => "IDispatch::Invoke",
            Method::TextGetSelection => "ITextProvider::GetSelection",
            Method::TextGetVisibleRanges => "GetVisibleRanges",
            Method::TextDocumentRange => "DocumentRange",
            Method::TextGetCaretRange => "GetCaretRange",
            Method::RangeClone => "Clone",
            Method::RangeCompare => "Compare",
            Method::RangeCompareEndpoints => "CompareEndpoints",
            Method::RangeExpandToEnclosingUnit => "ExpandToEnclosingUnit",
            Method::RangeFindText => "FindText",
            Method::RangeGetAttributeValue => "GetAttributeValue",
            Method::RangeGetBoundingRectangles => "GetBoundingRectangles",
            Method::RangeGetText => "GetText",
            Method::RangeMove => "Move",
            Method::RangeMoveEndpointByUnit => "MoveEndpointByUnit",
            Method::RangeMoveEndpointByRange => "MoveEndpointByRange",
            Method::RangeSelect => "ITextRangeProvider::Select",
        }
    }

    /// The method's counter index, its position in [`Method::ALL`].
    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

// Every method is listed in `Method::ALL` at its own index, once.
const _: () = {
    let mut index = 0;
    while index < COUNT {
        assert!(Method::ALL[index] as usize == index);
        index += 1;
    }
};

/// How long each provider call waits before it is answered, in
/// milliseconds: the `slow` command's delay, 0 for none.
static DELAY_MS: AtomicU64 = AtomicU64::new(0);

/// How long each pattern request waits more: the `slow-patterns`
/// command's delay, 0 for none.
static PATTERN_DELAY_MS: AtomicU64 = AtomicU64::new(0);

/// Counts one call to `method`, then waits the `slow` command's delay, and
/// for a pattern request the `slow-patterns` command's as well, so
/// the call is answered that much later, as by an application busy
/// building a window; or, when the `hold` command asked for it, says
/// `held <method>` on stdout and waits until `release`, so a test knows a
/// client's call is in progress and decides when it ends.
pub(crate) fn hit(method: Method) {
    HITS[method.index()].fetch_add(1, Ordering::Relaxed);
    if HOLD.swap(false, Ordering::Relaxed) {
        // Printed as the window thread's acknowledgements are, where
        // `ready` went.
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "held {method:?}");
        let _ = stdout.flush();
        drop(stdout);
        let released = RELEASED
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _released = RELEASED
            .1
            .wait_while(released, |released| !*released)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    let mut delay = DELAY_MS.load(Ordering::Relaxed);
    if matches!(method, Method::GetPatternProvider) {
        delay += PATTERN_DELAY_MS.load(Ordering::Relaxed);
    }
    if delay > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
}

/// Whether the next counted call is held ([`hold_next`]).
static HOLD: AtomicBool = AtomicBool::new(false);

/// Whether a held call may go on, and the condition it waits on.
static RELEASED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Holds the next provider call, whichever it is, until [`release`]: the
/// `hold` command.
pub(crate) fn hold_next() {
    *RELEASED
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    HOLD.store(true, Ordering::Relaxed);
}

/// Lets a held call go on: the `release` command, which the stdin thread
/// applies itself, since the window thread is the one held.
pub(crate) fn release() {
    *RELEASED
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    RELEASED.1.notify_all();
}

/// The live registrations per event, by event id.
static ADVISED: Mutex<BTreeMap<i32, i64>> = Mutex::new(BTreeMap::new());

/// Counts a registration UIA reported added (`added`) or removed for event
/// `event`.
pub(crate) fn advise(event: i32, added: bool) {
    let mut advised = ADVISED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *advised.entry(event).or_default() += if added { 1 } else { -1 };
}

/// The live registrations for event `event`.
pub(crate) fn advised(event: i32) -> i64 {
    ADVISED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&event)
        .copied()
        .unwrap_or(0)
}

/// Makes every provider call from now on wait `ms` milliseconds before it
/// is answered; 0 answers at once again.
pub(crate) fn set_delay(ms: u64) {
    DELAY_MS.store(ms, Ordering::Relaxed);
}

/// Makes every pattern request from now on wait `ms` milliseconds more
/// before it is answered; 0 adds nothing again.
pub(crate) fn set_pattern_delay(ms: u64) {
    PATTERN_DELAY_MS.store(ms, Ordering::Relaxed);
}

/// The count for the method at `index` in [`Method::ALL`], or 0 for an
/// index past the end.
pub(crate) fn read(index: usize) -> u32 {
    HITS.get(index)
        .map_or(0, |counter| counter.load(Ordering::Relaxed))
}

/// Zeroes every counter, the `WM_GETOBJECT` kinds' too.
pub(crate) fn reset() {
    for counter in HITS.iter().chain(&GET_OBJECTS) {
        counter.store(0, Ordering::Relaxed);
    }
}
