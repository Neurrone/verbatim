//! Safe wrappers over the UIA client interfaces: an element's cached and
//! live reads, the tree walker's steps, and the pattern methods the crate
//! calls, each method holding one documented `unsafe` call.
//!
//! The `windows` crate marks every COM method `unsafe` because its bindings
//! are generated, not because these calls have preconditions the caller
//! must uphold. An interface value such as `IUIAutomationElement` is a
//! counted reference to a live COM object, which the `windows` crate keeps
//! alive for as long as the value exists, so calling one of its methods
//! with plain values or other such interface values cannot be unsound: a
//! dead provider, an unsupported property, or a missing cache entry comes
//! back as a failed `HRESULT`, which these wrappers return as an error or
//! as `None`. What remains `unsafe` in the crate is genuinely low-level:
//! `VARIANT` and `SAFEARRAY` handling in `com.rs`, the event handler
//! registrations, the provider probe's window messages, and apartment
//! setup.
//!
//! Every method that reaches the application's process counts one call
//! on this thread ([`crate::calls`]); the cached reads are local and count
//! nothing.

use windows::Win32::Foundation::RECT;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationElementArray, IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationSelectionPattern, IUIAutomationTextPattern, IUIAutomationTextRange,
    IUIAutomationTextRangeArray, IUIAutomationTogglePattern, IUIAutomationTreeWalker, TreeScope,
    UIA_CONTROLTYPE_ID, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_TEXTATTRIBUTE_ID,
};
use windows::core::Interface;

use verbatim_model::CallKind;

use crate::calls::count;
use crate::com::{variant_bool, variant_f64, variant_i32, variant_optional_bool, variant_string};

/// Safe reads and calls on a UIA element ([`IUIAutomationElement`]).
///
/// The `cached_*` methods read the element's cache, filled when the element
/// was fetched or rebuilt with a cache request, and never leave this
/// process; a property that was not cached, or that the element does not
/// support, reads as `None` (or `false`). The other methods are
/// cross-process calls to the application's provider, each counted once.
pub trait ElementExt {
    /// A cached property's raw value, `None` when it was not cached.
    fn cached_value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;

    /// A cached property's raw value, with UIA's default for the property
    /// read as "not supported" (`GetCachedPropertyValueEx` ignoring
    /// defaults), `None` when it was not cached.
    fn cached_value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;

    /// A cached integer property, `None` when it was not cached or is not
    /// an integer (UIA's "not supported" value is not).
    fn cached_i32(&self, property: UIA_PROPERTY_ID) -> Option<i32> {
        let value = self.cached_value(property)?;
        // SAFETY: `value` is a VARIANT UIA returned, owned here.
        unsafe { variant_i32(&value) }
    }

    /// A cached integer property, `None` when it was not cached, the element
    /// does not support it, or UIA only supplies the property's default.
    fn cached_i32_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<i32> {
        let value = self.cached_value_ignoring_default(property)?;
        // SAFETY: `value` is a VARIANT UIA returned, owned here.
        unsafe { variant_i32(&value) }
    }

    /// A cached boolean property, `false` when it was not cached or is not a
    /// boolean.
    fn cached_bool(&self, property: UIA_PROPERTY_ID) -> bool {
        self.cached_value(property)
            // SAFETY: `value` is a VARIANT UIA returned, owned here.
            .is_some_and(|value| unsafe { variant_bool(&value) })
    }

    /// A cached boolean property, `None` when the element does not support
    /// it or UIA only supplies the property's default.
    fn cached_optional_bool(&self, property: UIA_PROPERTY_ID) -> Option<bool> {
        let value = self.cached_value_ignoring_default(property)?;
        // SAFETY: `value` is a VARIANT UIA returned, owned here.
        unsafe { variant_optional_bool(&value) }
    }

    /// A cached floating-point property, `None` when the element does not
    /// support it or UIA only supplies the property's default.
    fn cached_f64(&self, property: UIA_PROPERTY_ID) -> Option<f64> {
        let value = self.cached_value_ignoring_default(property)?;
        // SAFETY: `value` is a VARIANT UIA returned, owned here.
        unsafe { variant_f64(&value) }
    }

    /// A cached string property, `None` when it was not cached, not a
    /// string, or empty.
    fn cached_string(&self, property: UIA_PROPERTY_ID) -> Option<String> {
        let value = self.cached_value(property)?;
        // SAFETY: `value` is a VARIANT UIA returned, owned here.
        unsafe { variant_string(&value) }
    }

    /// The cached bounding rectangle, through UIA's typed accessor.
    fn cached_bounding_rectangle(&self) -> Option<RECT>;

    /// The cached control type.
    fn cached_control_type(&self) -> Option<UIA_CONTROLTYPE_ID>;

    /// The cached UIA framework, such as `WinForm` or `XAML`.
    fn cached_framework_id(&self) -> Option<String>;

    /// The control type, read live. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn current_control_type(&self) -> windows::core::Result<UIA_CONTROLTYPE_ID>;

    /// Whether the element has the keyboard focus, read live rather than
    /// from the cache. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn has_keyboard_focus(&self) -> windows::core::Result<bool>;

    /// A copy of the element with its cache refilled by `cache`, which also
    /// proves the element still answers. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the element is gone or the provider fails.
    fn build_updated_cache(
        &self,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;

    /// The element's control pattern `pattern` as the interface `T`, fetched
    /// live. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the element does not support the pattern or the
    /// provider fails.
    fn current_pattern<T: Interface>(&self, pattern: UIA_PATTERN_ID) -> windows::core::Result<T>;

    /// The elements the element names in its `ControllerFor` relation, read
    /// live. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn controller_for(&self) -> windows::core::Result<Vec<IUIAutomationElement>>;

    /// The first element within `scope` of this one that matches
    /// `condition`, `Ok(None)` when none does. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the search fails for a reason other than finding
    /// nothing.
    fn find_first(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
    ) -> windows::core::Result<Option<IUIAutomationElement>>;

    /// [`find_first`](Self::find_first), the match built with `cache`.
    /// Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the search fails for a reason other than finding
    /// nothing.
    fn find_first_build_cache(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>>;
}

/// No match is a null element, which `windows` reports as an error carrying
/// no failure code.
fn found(
    result: windows::core::Result<IUIAutomationElement>,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    match result {
        Ok(element) => Ok(Some(element)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(error),
    }
}

impl ElementExt for IUIAutomationElement {
    fn cached_value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        // SAFETY: `self` is a live element (see the module comment); a
        // cached read takes a plain property id and stays in this process.
        unsafe { self.GetCachedPropertyValue(property) }.ok()
    }

    fn cached_value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        // SAFETY: as in `cached_value`.
        unsafe { self.GetCachedPropertyValueEx(property, true) }.ok()
    }

    fn cached_bounding_rectangle(&self) -> Option<RECT> {
        // SAFETY: as in `cached_value`; the typed accessor reads the cached
        // `BoundingRectangle` property.
        unsafe { self.CachedBoundingRectangle() }.ok()
    }

    fn cached_control_type(&self) -> Option<UIA_CONTROLTYPE_ID> {
        // SAFETY: as in `cached_value`.
        unsafe { self.CachedControlType() }.ok()
    }

    fn cached_framework_id(&self) -> Option<String> {
        // SAFETY: as in `cached_value`; the BSTR is owned by the result.
        unsafe { self.CachedFrameworkId() }
            .ok()
            .map(|framework| framework.to_string())
    }

    fn current_control_type(&self) -> windows::core::Result<UIA_CONTROLTYPE_ID> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live element (see the module comment).
        unsafe { self.CurrentControlType() }
    }

    fn has_keyboard_focus(&self) -> windows::core::Result<bool> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live element (see the module comment).
        unsafe { self.CurrentHasKeyboardFocus() }.map(windows::core::BOOL::as_bool)
    }

    fn build_updated_cache(
        &self,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: `self` and `cache` are live COM objects (see the module
        // comment); a gone element fails the call.
        unsafe { self.BuildUpdatedCache(cache) }
    }

    fn current_pattern<T: Interface>(&self, pattern: UIA_PATTERN_ID) -> windows::core::Result<T> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live element; the pattern object is queried
        // for `T`'s own interface id, so the returned value is a `T`.
        unsafe { self.GetCurrentPatternAs::<T>(pattern) }
    }

    fn controller_for(&self) -> windows::core::Result<Vec<IUIAutomationElement>> {
        count(CallKind::Uia);
        // SAFETY: `self` is a live element (see the module comment).
        let array = unsafe { self.CurrentControllerFor() }?;
        Ok(elements_of(&array))
    }

    fn find_first(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        count(CallKind::Uia);
        // SAFETY: `self` and `condition` are live COM objects (see the
        // module comment); `scope` is a plain value.
        found(unsafe { self.FindFirst(scope, condition) })
    }

    fn find_first_build_cache(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        count(CallKind::Uia);
        // SAFETY: as in `find_first`.
        found(unsafe { self.FindFirstBuildCache(scope, condition, cache) })
    }
}

/// The elements of a UIA element array, in order. Reading the array is
/// local; an element that cannot be read is left out.
#[must_use]
pub fn elements_of(array: &IUIAutomationElementArray) -> Vec<IUIAutomationElement> {
    // SAFETY: `array` is a live element array (see the module comment).
    let length = unsafe { array.Length() }.unwrap_or(0);
    (0..length)
        // SAFETY: `index` is within the array's length.
        .filter_map(|index| unsafe { array.GetElement(index) }.ok())
        .collect()
}

/// Safe steps of a UIA tree walker ([`IUIAutomationTreeWalker`]), each one
/// cross-process call that returns the element reached, built with the
/// given cache. A step that reaches nothing (a root's parent, a last
/// child's next sibling) comes back as an error carrying no failure code,
/// as `windows` reports a null element.
pub trait WalkerExt {
    /// `element`'s parent.
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error for no parent.
    fn parent(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;

    /// `element`'s next sibling.
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error for no next sibling.
    fn next_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;

    /// `element`'s previous sibling.
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error for no previous sibling.
    fn previous_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;

    /// `element`'s first child.
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error for no child.
    fn first_child(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;

    /// `element` itself if the walker's condition matches it, else its
    /// nearest ancestor that matches (`NormalizeElementBuildCache`).
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error when nothing matches.
    fn normalize(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement>;
}

impl WalkerExt for IUIAutomationTreeWalker {
    fn parent(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: the walker, `element`, and `cache` are live COM objects
        // (see the module comment).
        unsafe { self.GetParentElementBuildCache(element, cache) }
    }

    fn next_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: as in `parent`.
        unsafe { self.GetNextSiblingElementBuildCache(element, cache) }
    }

    fn previous_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: as in `parent`.
        unsafe { self.GetPreviousSiblingElementBuildCache(element, cache) }
    }

    fn first_child(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: as in `parent`.
        unsafe { self.GetFirstChildElementBuildCache(element, cache) }
    }

    fn normalize(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia);
        // SAFETY: as in `parent`.
        unsafe { self.NormalizeElementBuildCache(element, cache) }
    }
}

/// The current selection of a `Selection` pattern, read live.
/// Cross-process.
pub(crate) fn current_selection(
    pattern: &IUIAutomationSelectionPattern,
) -> windows::core::Result<Vec<IUIAutomationElement>> {
    count(CallKind::Uia);
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    let array = unsafe { pattern.GetCurrentSelection() }?;
    Ok(elements_of(&array))
}

/// Invokes an `Invoke` pattern. Cross-process.
pub(crate) fn invoke(pattern: &IUIAutomationInvokePattern) -> windows::core::Result<()> {
    count(CallKind::Uia);
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Invoke() }
}

/// Toggles a `Toggle` pattern. Cross-process.
pub(crate) fn toggle(pattern: &IUIAutomationTogglePattern) -> windows::core::Result<()> {
    count(CallKind::Uia);
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Toggle() }
}

/// Selects a `SelectionItem` pattern's element. Cross-process.
pub(crate) fn select(pattern: &IUIAutomationSelectionItemPattern) -> windows::core::Result<()> {
    count(CallKind::Uia);
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Select() }
}

/// A `Text` pattern's visible ranges. Cross-process.
pub(crate) fn visible_ranges(
    pattern: &IUIAutomationTextPattern,
) -> windows::core::Result<Vec<IUIAutomationTextRange>> {
    count(CallKind::Uia);
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    let array = unsafe { pattern.GetVisibleRanges() }?;
    Ok(ranges_of(&array))
}

/// The ranges of a text range array, in order. Local.
fn ranges_of(array: &IUIAutomationTextRangeArray) -> Vec<IUIAutomationTextRange> {
    // SAFETY: `array` is a live range array (see the module comment).
    let length = unsafe { array.Length() }.unwrap_or(0);
    (0..length)
        // SAFETY: `index` is within the array's length.
        .filter_map(|index| unsafe { array.GetElement(index) }.ok())
        .collect()
}

/// A text range's value for the text attribute `attribute`. Cross-process.
pub(crate) fn attribute_value(
    range: &IUIAutomationTextRange,
    attribute: UIA_TEXTATTRIBUTE_ID,
) -> windows::core::Result<VARIANT> {
    count(CallKind::Uia);
    // SAFETY: `range` is a live text range (see the module comment).
    unsafe { range.GetAttributeValue(attribute) }
}
