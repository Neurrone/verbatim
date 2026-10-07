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
//! much later ([`set_delay`]).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Reads one counter: `wParam` is the method's index in [`Method::ALL`].
pub(crate) const WM_HITS_READ: u32 = WM_APP + 2;

/// Zeroes every counter.
pub(crate) const WM_HITS_RESET: u32 = WM_APP + 3;

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

/// Counts one call to `method`, then waits the `slow` command's delay, so
/// the call is answered that much later, as by an application busy
/// building a window.
pub(crate) fn hit(method: Method) {
    HITS[method.index()].fetch_add(1, Ordering::Relaxed);
    let delay = DELAY_MS.load(Ordering::Relaxed);
    if delay > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
}

/// Makes every provider call from now on wait `ms` milliseconds before it
/// is answered; 0 answers at once again.
pub(crate) fn set_delay(ms: u64) {
    DELAY_MS.store(ms, Ordering::Relaxed);
}

/// The count for the method at `index` in [`Method::ALL`], or 0 for an
/// index past the end.
pub(crate) fn read(index: usize) -> u32 {
    HITS.get(index)
        .map_or(0, |counter| counter.load(Ordering::Relaxed))
}

/// Zeroes every counter.
pub(crate) fn reset() {
    for counter in &HITS {
        counter.store(0, Ordering::Relaxed);
    }
}
