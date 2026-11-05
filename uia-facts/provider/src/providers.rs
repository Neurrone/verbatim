//! The provider program's COM objects: a UIA fragment tree per UIA window,
//! its patterns, and an `IAccessible` for the MSAA window. Every method
//! enters through [`Call::enter`], which logs it and applies the holds.
//!
//! The tree: node 0 is the window's root; node i's parent is (i - 1) / 4,
//! so its children are 4i + 1 to 4i + 4. Node 1 (`First`) is a button with
//! the invoke pattern, node 2 (`Second`) a check box with the toggle
//! pattern, node 3 (`Notes`) a document with the text pattern, node 4 a
//! button with the `LegacyIAccessible` pattern, and every other node a button
//! with the invoke pattern.

#![allow(
    clippy::inline_always,
    clippy::ref_as_ptr,
    clippy::used_underscore_binding,
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    // windows-rs names its constants as Windows does.
    non_upper_case_globals
)]

use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, ITypeInfo, SAFEARRAY,
};
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    IAccessible, IAccessible_Impl, IInvokeProvider, IInvokeProvider_Impl,
    ILegacyIAccessibleProvider, ILegacyIAccessibleProvider_Impl, IRawElementProviderAdviseEvents,
    IRawElementProviderAdviseEvents_Impl, IRawElementProviderFragment,
    IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
    IRawElementProviderSimple_Impl, ITextProvider, ITextProvider_Impl, ITextRangeProvider,
    ITextRangeProvider_Impl, IToggleProvider, IToggleProvider_Impl, LresultFromObject,
    NavigateDirection, NavigateDirection_FirstChild, NavigateDirection_LastChild,
    NavigateDirection_NextSibling, NavigateDirection_Parent, NavigateDirection_PreviousSibling,
    NotificationKind_Other, NotificationProcessing_All, ProviderOptions,
    ProviderOptions_ServerSideProvider, ProviderOptions_UseComThreading,
    StructureChangeType_ChildrenInvalidated, SupportedTextSelection, SupportedTextSelection_Single,
    TextPatternRangeEndpoint, TextPatternRangeEndpoint_Start, TextUnit, ToggleState,
    ToggleState_Off, ToggleState_On, UIA_AutomationFocusChangedEventId, UIA_AutomationIdPropertyId,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_ControlTypePropertyId,
    UIA_DocumentControlTypeId, UIA_EVENT_ID, UIA_HasKeyboardFocusPropertyId,
    UIA_Invoke_InvokedEventId, UIA_InvokePatternId, UIA_IsKeyboardFocusablePropertyId,
    UIA_LegacyIAccessiblePatternId, UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID,
    UIA_PaneControlTypeId, UIA_TEXTATTRIBUTE_ID, UIA_Text_TextChangedEventId, UIA_TextPatternId,
    UIA_TogglePatternId, UiaAppendRuntimeId, UiaGetReservedNotSupportedValue,
    UiaHostProviderFromHwnd, UiaPoint, UiaRaiseAutomationEvent,
    UiaRaiseAutomationPropertyChangedEvent, UiaRaiseNotificationEvent,
    UiaRaiseStructureChangedEvent, UiaRect, UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, WM_GETOBJECT};
use windows::core::{BOOL, BSTR, Error, GUID, IUnknown, Interface, PCWSTR, Result, implement};

use crate::state::{self, Call, Kind};

/// The text of node 3's document.
const TEXT: &str = "The first line of the notes.\nThe second line of the notes.\n";

fn parent(node: usize) -> Option<usize> {
    (node > 0).then(|| (node - 1) / 4)
}

fn children(node: usize) -> std::ops::Range<usize> {
    let nodes = state::state().nodes;
    let first = (4 * node + 1).min(nodes);
    first..(4 * node + 5).min(nodes)
}

fn options() -> ProviderOptions {
    if state::state().com {
        ProviderOptions(ProviderOptions_ServerSideProvider.0 | ProviderOptions_UseComThreading.0)
    } else {
        ProviderOptions_ServerSideProvider
    }
}

/// Node `node` of window `window`'s tree as a new provider object.
pub(crate) fn node(window: usize, node: usize) -> IRawElementProviderSimple {
    let hwnd = state::window(window).hwnd;
    if node == 0 {
        Root { window, hwnd }.into()
    } else {
        Node { window, node }.into()
    }
}

fn fragment(window: usize, index: usize) -> Result<IRawElementProviderFragment> {
    node(window, index).cast()
}

/// Answers a UIA window's `WM_GETOBJECT` for its root object id, and for
/// `OBJID_CLIENT` under `--bridge`, and the MSAA window's for
/// `OBJID_CLIENT`; `None` for every other message.
pub(crate) fn get_object(
    hwnd: HWND,
    window: usize,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    if message != WM_GETOBJECT {
        return None;
    }
    let object = state::object_id(lparam);
    match state::kind(window) {
        Kind::Uia
            if object == UiaRootObjectId || (state::state().bridge && object == OBJID_CLIENT.0) =>
        {
            let _call = Call::enter("GetObject", window, None);
            let provider = node(window, 0);
            // SAFETY: the arguments of this window's own WM_GETOBJECT.
            Some(unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &provider) })
        }
        Kind::Msaa if object == OBJID_CLIENT.0 => {
            let _call = Call::enter("GetObject", window, None);
            let accessible: IAccessible = Accessible { window }.into();
            // SAFETY: the arguments of this window's own WM_GETOBJECT and a
            // live object of the interface named.
            Some(unsafe { LresultFromObject(&IAccessible::IID, wparam, &accessible) })
        }
        _ => None,
    }
}

/// An event the provider raises.
#[derive(Clone, Copy)]
pub(crate) enum Event {
    Focus,
    Name,
    Notification,
    Structure,
    Text,
}

impl Event {
    pub(crate) fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "focus" => Event::Focus,
            "name" => Event::Name,
            "notification" => Event::Notification,
            "structure" => Event::Structure,
            "text" => Event::Text,
            _ => return None,
        })
    }
}

/// Raises `count` events on node `index` of window `window`, `pace` a
/// second (0 for one at once), on this UI thread. Answers with the time the
/// first was raised.
pub(crate) fn raise(
    event: Event,
    window: usize,
    index: usize,
    pace: usize,
    count: usize,
) -> String {
    let provider = node(window, index);
    let first = state::now_us();
    let start = std::time::Instant::now();
    for i in 0..count {
        if pace > 0 {
            let due = std::time::Duration::from_secs_f64(
                f64::from(u32::try_from(i).expect("a count fits"))
                    / f64::from(u32::try_from(pace).expect("a pace fits")),
            );
            while start.elapsed() < due {
                std::hint::spin_loop();
            }
        }
        if let Err(error) = raise_one(event, window, index, &provider) {
            return format!("err raising: {error}");
        }
    }
    format!("ok {first}")
}

fn raise_one(
    event: Event,
    window: usize,
    index: usize,
    provider: &IRawElementProviderSimple,
) -> Result<()> {
    match event {
        Event::Focus => {
            // SAFETY: a live provider of this process.
            unsafe { UiaRaiseAutomationEvent(provider, UIA_AutomationFocusChangedEventId) }
        }
        Event::Text => {
            // SAFETY: as above.
            unsafe { UiaRaiseAutomationEvent(provider, UIA_Text_TextChangedEventId) }
        }
        Event::Name => {
            let name = state::tree(window).names[index].clone();
            let old = VARIANT::from(BSTR::from(""));
            let new = VARIANT::from(BSTR::from(name.as_str()));
            // SAFETY: as above, with two live VARIANTs.
            unsafe {
                UiaRaiseAutomationPropertyChangedEvent(provider, UIA_NamePropertyId, &old, &new)
            }
        }
        Event::Notification => {
            let name = state::tree(window).names[index].clone();
            // SAFETY: as above, with two live strings.
            unsafe {
                UiaRaiseNotificationEvent(
                    provider,
                    NotificationKind_Other,
                    NotificationProcessing_All,
                    &BSTR::from(name.as_str()),
                    &BSTR::from("uia-facts"),
                )
            }
        }
        Event::Structure => {
            let mut id = [
                UiaAppendRuntimeId.cast_signed(),
                i32::try_from(index).expect("a node index fits"),
            ];
            // SAFETY: as above, with a live runtime id of two integers.
            unsafe {
                UiaRaiseStructureChangedEvent(
                    provider,
                    StructureChangeType_ChildrenInvalidated,
                    id.as_mut_ptr(),
                    2,
                )
            }
        }
    }
}

fn runtime_id(index: usize) -> Result<*mut SAFEARRAY> {
    let values = [
        UiaAppendRuntimeId.cast_signed(),
        i32::try_from(index).expect("a node index fits"),
    ];
    // SAFETY: a new vector of two VT_I4 elements, owned by the caller.
    let array = unsafe { SafeArrayCreateVector(VT_I4, 0, 2) };
    if array.is_null() {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    for (position, value) in (0i32..).zip(values.iter()) {
        // SAFETY: a live array of VT_I4 and a pointer to a live i32.
        unsafe { SafeArrayPutElement(array, &raw const position, (&raw const *value).cast()) }?;
    }
    Ok(array)
}

fn property(window: usize, index: usize, id: UIA_PROPERTY_ID) -> VARIANT {
    let tree = state::tree(window);
    match id {
        UIA_NamePropertyId => VARIANT::from(BSTR::from(tree.names[index].as_str())),
        UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(format!("n{index}").as_str())),
        UIA_ControlTypePropertyId => VARIANT::from(match index {
            0 => UIA_PaneControlTypeId.0,
            2 => UIA_CheckBoxControlTypeId.0,
            3 => UIA_DocumentControlTypeId.0,
            _ => UIA_ButtonControlTypeId.0,
        }),
        UIA_IsKeyboardFocusablePropertyId => VARIANT::from(true),
        UIA_HasKeyboardFocusPropertyId => VARIANT::from(tree.focus == index),
        _ => VARIANT::default(),
    }
}

fn navigate(
    window: usize,
    index: usize,
    direction: NavigateDirection,
) -> Result<IRawElementProviderFragment> {
    let target = match direction {
        NavigateDirection_Parent => parent(index),
        NavigateDirection_FirstChild => children(index).next(),
        NavigateDirection_LastChild => children(index).last(),
        NavigateDirection_NextSibling => {
            parent(index).and_then(|p| children(p).find(|c| *c == index + 1))
        }
        NavigateDirection_PreviousSibling => {
            parent(index).and_then(|p| children(p).find(|c| *c + 1 == index))
        }
        _ => None,
    };
    match target {
        Some(target) => fragment(window, target),
        None => Err(Error::empty()),
    }
}

fn pattern(window: usize, index: usize, id: UIA_PATTERN_ID) -> Result<IUnknown> {
    Ok(match (index, id) {
        (2, UIA_TogglePatternId) => IToggleProvider::from(Toggle { window, index }).into(),
        (3, UIA_TextPatternId) => ITextProvider::from(Text { window, index }).into(),
        (4, UIA_LegacyIAccessiblePatternId) => {
            ILegacyIAccessibleProvider::from(Legacy { window, index }).into()
        }
        (i, UIA_InvokePatternId) if i != 0 && !(2..=4).contains(&i) => {
            IInvokeProvider::from(Invoke { window, index }).into()
        }
        _ => return Err(Error::empty()),
    })
}

/// A window's root node: also the fragment root, told of registrations.
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IRawElementProviderAdviseEvents,
    Agile = false
)]
struct Root {
    window: usize,
    hwnd: isize,
}

/// Every node but the root.
#[implement(IRawElementProviderSimple, IRawElementProviderFragment, Agile = false)]
struct Node {
    window: usize,
    node: usize,
}

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        let _call = Call::enter("ProviderOptions", self.window, Some(0));
        Ok(options())
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        let _call = Call::enter("GetPatternProvider", self.window, Some(0));
        pattern(self.window, 0, id)
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let _call = Call::enter("GetPropertyValue", self.window, Some(0));
        Ok(property(self.window, 0, id))
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let _call = Call::enter("HostRawElementProvider", self.window, Some(0));
        // SAFETY: this provider's own window.
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as *mut _)) }
    }
}

impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let _call = Call::enter("Navigate", self.window, Some(0));
        navigate(self.window, 0, direction)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetRuntimeId", self.window, Some(0));
        // The window's element takes its runtime id from its handle.
        Ok(std::ptr::null_mut())
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let _call = Call::enter("BoundingRectangle", self.window, Some(0));
        Ok(UiaRect::default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetEmbeddedFragmentRoots", self.window, Some(0));
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        let _call = Call::enter("SetFocus", self.window, Some(0));
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        let _call = Call::enter("FragmentRoot", self.window, Some(0));
        node(self.window, 0).cast()
    }
}

impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, _x: f64, _y: f64) -> Result<IRawElementProviderFragment> {
        let _call = Call::enter("ElementProviderFromPoint", self.window, Some(0));
        fragment(self.window, 0)
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let _call = Call::enter("GetFocus", self.window, Some(0));
        let focus = state::tree(self.window).focus;
        if focus == 0 {
            Err(Error::empty())
        } else {
            fragment(self.window, focus)
        }
    }
}

impl IRawElementProviderAdviseEvents_Impl for Root_Impl {
    fn AdviseEventAdded(&self, event: UIA_EVENT_ID, _properties: *const SAFEARRAY) -> Result<()> {
        state::log_advise("AdviseEventAdded", self.window, event.0);
        Ok(())
    }
    fn AdviseEventRemoved(&self, event: UIA_EVENT_ID, _properties: *const SAFEARRAY) -> Result<()> {
        state::log_advise("AdviseEventRemoved", self.window, event.0);
        Ok(())
    }
}

impl IRawElementProviderSimple_Impl for Node_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        let _call = Call::enter("ProviderOptions", self.window, Some(self.node));
        Ok(options())
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        let _call = Call::enter("GetPatternProvider", self.window, Some(self.node));
        pattern(self.window, self.node, id)
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let _call = Call::enter("GetPropertyValue", self.window, Some(self.node));
        Ok(property(self.window, self.node, id))
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let _call = Call::enter("HostRawElementProvider", self.window, Some(self.node));
        Err(Error::empty())
    }
}

impl IRawElementProviderFragment_Impl for Node_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let _call = Call::enter("Navigate", self.window, Some(self.node));
        navigate(self.window, self.node, direction)
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetRuntimeId", self.window, Some(self.node));
        runtime_id(self.node)
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let _call = Call::enter("BoundingRectangle", self.window, Some(self.node));
        Ok(UiaRect::default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetEmbeddedFragmentRoots", self.window, Some(self.node));
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        let _call = Call::enter("SetFocus", self.window, Some(self.node));
        state::set_focus(self.window, self.node);
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        let _call = Call::enter("FragmentRoot", self.window, Some(self.node));
        node(self.window, 0).cast()
    }
}

#[implement(IInvokeProvider, Agile = false)]
struct Invoke {
    window: usize,
    index: usize,
}

impl IInvokeProvider_Impl for Invoke_Impl {
    fn Invoke(&self) -> Result<()> {
        let _call = Call::enter("Invoke", self.window, Some(self.index));
        let provider = node(self.window, self.index);
        // SAFETY: a live provider of this process.
        unsafe { UiaRaiseAutomationEvent(&provider, UIA_Invoke_InvokedEventId) }
    }
}

#[implement(IToggleProvider, Agile = false)]
struct Toggle {
    window: usize,
    index: usize,
}

static TOGGLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl IToggleProvider_Impl for Toggle_Impl {
    fn Toggle(&self) -> Result<()> {
        let _call = Call::enter("Toggle", self.window, Some(self.index));
        TOGGLED.fetch_xor(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn ToggleState(&self) -> Result<ToggleState> {
        let _call = Call::enter("ToggleState", self.window, Some(self.index));
        Ok(if TOGGLED.load(std::sync::atomic::Ordering::SeqCst) {
            ToggleState_On
        } else {
            ToggleState_Off
        })
    }
}

#[implement(ILegacyIAccessibleProvider, Agile = false)]
struct Legacy {
    window: usize,
    index: usize,
}

impl Legacy_Impl {
    fn call(&self, name: &'static str) -> Call {
        Call::enter(name, self.window, Some(self.index))
    }
}

impl ILegacyIAccessibleProvider_Impl for Legacy_Impl {
    fn Select(&self, _flags: i32) -> Result<()> {
        let _call = self.call("LegacySelect");
        Ok(())
    }
    fn DoDefaultAction(&self) -> Result<()> {
        let _call = self.call("LegacyDoDefaultAction");
        Ok(())
    }
    fn SetValue(&self, _value: &PCWSTR) -> Result<()> {
        let _call = self.call("LegacySetValue");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn GetIAccessible(&self) -> Result<IAccessible> {
        let _call = self.call("LegacyGetIAccessible");
        Err(Error::empty())
    }
    fn ChildId(&self) -> Result<i32> {
        let _call = self.call("LegacyChildId");
        Ok(0)
    }
    fn Name(&self) -> Result<BSTR> {
        let _call = self.call("LegacyName");
        Ok(BSTR::from(
            state::tree(self.window).names[self.index].as_str(),
        ))
    }
    fn Value(&self) -> Result<BSTR> {
        let _call = self.call("LegacyValue");
        Ok(BSTR::new())
    }
    fn Description(&self) -> Result<BSTR> {
        let _call = self.call("LegacyDescription");
        Ok(BSTR::new())
    }
    fn Role(&self) -> Result<u32> {
        let _call = self.call("LegacyRole");
        // ROLE_SYSTEM_PUSHBUTTON
        Ok(0x2b)
    }
    fn State(&self) -> Result<u32> {
        let _call = self.call("LegacyState");
        Ok(0)
    }
    fn Help(&self) -> Result<BSTR> {
        let _call = self.call("LegacyHelp");
        Ok(BSTR::new())
    }
    fn KeyboardShortcut(&self) -> Result<BSTR> {
        let _call = self.call("LegacyKeyboardShortcut");
        Ok(BSTR::new())
    }
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let _call = self.call("LegacyGetSelection");
        Ok(std::ptr::null_mut())
    }
    fn DefaultAction(&self) -> Result<BSTR> {
        let _call = self.call("LegacyDefaultAction");
        Ok(BSTR::from("Press"))
    }
}

#[implement(ITextProvider, Agile = false)]
struct Text {
    window: usize,
    index: usize,
}

impl Text_Impl {
    fn range(&self, start: usize, end: usize) -> ITextRangeProvider {
        Range {
            window: self.window,
            index: self.index,
            span: std::cell::Cell::new((start, end)),
        }
        .into()
    }
}

fn text_length() -> usize {
    TEXT.encode_utf16().count()
}

impl ITextProvider_Impl for Text_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetSelection", self.window, Some(self.index));
        Ok(std::ptr::null_mut())
    }
    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        let _call = Call::enter("GetVisibleRanges", self.window, Some(self.index));
        Ok(std::ptr::null_mut())
    }
    fn RangeFromChild(
        &self,
        _child: windows::core::Ref<IRawElementProviderSimple>,
    ) -> Result<ITextRangeProvider> {
        let _call = Call::enter("RangeFromChild", self.window, Some(self.index));
        Err(Error::from_hresult(E_INVALIDARG))
    }
    fn RangeFromPoint(&self, _point: &UiaPoint) -> Result<ITextRangeProvider> {
        let _call = Call::enter("RangeFromPoint", self.window, Some(self.index));
        Ok(self.range(0, 0))
    }
    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        let _call = Call::enter("DocumentRange", self.window, Some(self.index));
        Ok(self.range(0, text_length()))
    }
    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        let _call = Call::enter("SupportedTextSelection", self.window, Some(self.index));
        Ok(SupportedTextSelection_Single)
    }
}

/// A range of the document's text, in UTF-16 units. Moving by any unit
/// moves by the whole document: the facts read text, they do not navigate
/// it.
#[implement(ITextRangeProvider, Agile = false)]
struct Range {
    window: usize,
    index: usize,
    span: std::cell::Cell<(usize, usize)>,
}

impl Range_Impl {
    fn call(&self, name: &'static str) -> Call {
        Call::enter(name, self.window, Some(self.index))
    }
    fn copy(&self) -> ITextRangeProvider {
        Range {
            window: self.window,
            index: self.index,
            span: std::cell::Cell::new(self.span.get()),
        }
        .into()
    }
}

impl ITextRangeProvider_Impl for Range_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> {
        let _call = self.call("RangeClone");
        Ok(self.copy())
    }
    fn Compare(&self, _range: windows::core::Ref<ITextRangeProvider>) -> Result<BOOL> {
        let _call = self.call("RangeCompare");
        Ok(false.into())
    }
    fn CompareEndpoints(
        &self,
        _endpoint: TextPatternRangeEndpoint,
        _target: windows::core::Ref<ITextRangeProvider>,
        _target_endpoint: TextPatternRangeEndpoint,
    ) -> Result<i32> {
        let _call = self.call("RangeCompareEndpoints");
        Ok(0)
    }
    fn ExpandToEnclosingUnit(&self, _unit: TextUnit) -> Result<()> {
        let _call = self.call("ExpandToEnclosingUnit");
        self.span.set((0, text_length()));
        Ok(())
    }
    fn FindAttribute(
        &self,
        _id: UIA_TEXTATTRIBUTE_ID,
        _value: &VARIANT,
        _backward: BOOL,
    ) -> Result<ITextRangeProvider> {
        let _call = self.call("FindAttribute");
        Err(Error::empty())
    }
    fn FindText(
        &self,
        _text: &BSTR,
        _backward: BOOL,
        _ignore_case: BOOL,
    ) -> Result<ITextRangeProvider> {
        let _call = self.call("FindText");
        Err(Error::empty())
    }
    fn GetAttributeValue(&self, _id: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        let _call = self.call("GetAttributeValue");
        // SAFETY: no precondition.
        let unsupported = unsafe { UiaGetReservedNotSupportedValue() }?;
        Ok(VARIANT::from(unsupported))
    }
    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        let _call = self.call("GetBoundingRectangles");
        Ok(std::ptr::null_mut())
    }
    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> {
        let _call = self.call("GetEnclosingElement");
        Ok(node(self.window, self.index))
    }
    fn GetText(&self, max: i32) -> Result<BSTR> {
        let _call = self.call("GetText");
        let (start, end) = self.span.get();
        let units: Vec<u16> = TEXT.encode_utf16().collect();
        let end = usize::try_from(max).map_or(end, |max| end.min(start + max));
        Ok(BSTR::from_wide(&units[start..end]))
    }
    fn Move(&self, _unit: TextUnit, _count: i32) -> Result<i32> {
        let _call = self.call("Move");
        Ok(0)
    }
    fn MoveEndpointByUnit(
        &self,
        _endpoint: TextPatternRangeEndpoint,
        _unit: TextUnit,
        _count: i32,
    ) -> Result<i32> {
        let _call = self.call("MoveEndpointByUnit");
        Ok(0)
    }
    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        _target: windows::core::Ref<ITextRangeProvider>,
        _target_endpoint: TextPatternRangeEndpoint,
    ) -> Result<()> {
        let _call = self.call("MoveEndpointByRange");
        let (start, end) = self.span.get();
        self.span
            .set(if endpoint == TextPatternRangeEndpoint_Start {
                (end, end)
            } else {
                (start, start)
            });
        Ok(())
    }
    fn Select(&self) -> Result<()> {
        let _call = self.call("RangeSelect");
        Ok(())
    }
    fn AddToSelection(&self) -> Result<()> {
        let _call = self.call("RangeAddToSelection");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        let _call = self.call("RangeRemoveFromSelection");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn ScrollIntoView(&self, _top: BOOL) -> Result<()> {
        let _call = self.call("ScrollIntoView");
        Ok(())
    }
    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {
        let _call = self.call("RangeGetChildren");
        Ok(std::ptr::null_mut())
    }
}

/// The MSAA window's own `IAccessible`: a client object with no children.
#[implement(IAccessible, Agile = false)]
struct Accessible {
    window: usize,
}

impl Accessible_Impl {
    fn call(&self, name: &'static str) -> Call {
        Call::enter(name, self.window, None)
    }
}

impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _index: u32, _lcid: u32) -> Result<ITypeInfo> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _names: *const PCWSTR,
        _count: u32,
        _lcid: u32,
        _ids: *mut i32,
    ) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn Invoke(
        &self,
        _member: i32,
        _riid: *const GUID,
        _lcid: u32,
        _flags: DISPATCH_FLAGS,
        _params: *const DISPPARAMS,
        _result: *mut VARIANT,
        _exception: *mut EXCEPINFO,
        _argument_error: *mut u32,
    ) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
}

impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> Result<IDispatch> {
        let _call = self.call("accParent");
        Err(Error::empty())
    }
    fn accChildCount(&self) -> Result<i32> {
        let _call = self.call("accChildCount");
        Ok(0)
    }
    fn get_accChild(&self, _child: &VARIANT) -> Result<IDispatch> {
        let _call = self.call("accChild");
        Err(Error::from_hresult(E_INVALIDARG))
    }
    fn get_accName(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accName");
        Ok(BSTR::from(format!("msaa window {}", self.window).as_str()))
    }
    fn get_accValue(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accValue");
        Ok(BSTR::new())
    }
    fn get_accDescription(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accDescription");
        Ok(BSTR::new())
    }
    fn get_accRole(&self, _child: &VARIANT) -> Result<VARIANT> {
        let _call = self.call("accRole");
        // ROLE_SYSTEM_CLIENT
        Ok(VARIANT::from(0xa_i32))
    }
    fn get_accState(&self, _child: &VARIANT) -> Result<VARIANT> {
        let _call = self.call("accState");
        Ok(VARIANT::from(0_i32))
    }
    fn get_accHelp(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accHelp");
        Ok(BSTR::new())
    }
    fn get_accHelpTopic(&self, _file: *mut BSTR, _child: &VARIANT) -> Result<i32> {
        let _call = self.call("accHelpTopic");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn get_accKeyboardShortcut(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accKeyboardShortcut");
        Ok(BSTR::new())
    }
    fn accFocus(&self) -> Result<VARIANT> {
        let _call = self.call("accFocus");
        Ok(VARIANT::default())
    }
    fn accSelection(&self) -> Result<VARIANT> {
        let _call = self.call("accSelection");
        Ok(VARIANT::default())
    }
    fn get_accDefaultAction(&self, _child: &VARIANT) -> Result<BSTR> {
        let _call = self.call("accDefaultAction");
        Ok(BSTR::new())
    }
    fn accSelect(&self, _flags: i32, _child: &VARIANT) -> Result<()> {
        let _call = self.call("accSelect");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn accLocation(
        &self,
        _left: *mut i32,
        _top: *mut i32,
        _width: *mut i32,
        _height: *mut i32,
        _child: &VARIANT,
    ) -> Result<()> {
        let _call = self.call("accLocation");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn accNavigate(&self, _direction: i32, _start: &VARIANT) -> Result<VARIANT> {
        let _call = self.call("accNavigate");
        Ok(VARIANT::default())
    }
    fn accHitTest(&self, _x: i32, _y: i32) -> Result<VARIANT> {
        let _call = self.call("accHitTest");
        Ok(VARIANT::default())
    }
    fn accDoDefaultAction(&self, _child: &VARIANT) -> Result<()> {
        let _call = self.call("accDoDefaultAction");
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn put_accName(&self, _child: &VARIANT, _name: &BSTR) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn put_accValue(&self, _child: &VARIANT, _value: &BSTR) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
}
