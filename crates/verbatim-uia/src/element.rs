//! Safe wrappers over the UIA client interfaces: an element's live reads,
//! the tree walker's steps, and the pattern methods the crate calls, each
//! method holding one documented `unsafe` call. An element's cached reads
//! are not here: the shape module makes every one (`crate::Shape`).
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
//! on this thread ([`crate::calls`]).

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationElementArray, IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationSelectionPattern, IUIAutomationTextPattern, IUIAutomationTextRange,
    IUIAutomationTextRangeArray, IUIAutomationTogglePattern, IUIAutomationTreeWalker, TreeScope,
    UIA_CONTROLTYPE_ID, UIA_GridItemContainingGridPropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID,
    UIA_TEXTATTRIBUTE_ID,
};
use windows::core::Interface;

use verbatim_model::{CallKind, TableId};

use crate::calls::count;
use crate::com::{runtime_id, variant_element, variant_i32};

/// Safe live reads and calls on a UIA element ([`IUIAutomationElement`]),
/// each a cross-process call to the application's provider, counted once.
/// The element's cache is read through a shape alone ([`crate::Cached`]).
pub trait ElementExt {
    /// The grid a grid's item is in (`GridItem`'s `ContainingGrid`), read
    /// live, `Ok(None)` when it names none. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn current_containing_grid(&self) -> windows::core::Result<Option<IUIAutomationElement>>;

    /// The control type, read live. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn current_control_type(&self) -> windows::core::Result<UIA_CONTROLTYPE_ID>;

    /// An integer property read live, `Ok(None)` when the element does not
    /// support it, UIA only supplies its default, or it is not an integer.
    /// Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn current_i32_ignoring_default(
        &self,
        property: UIA_PROPERTY_ID,
    ) -> windows::core::Result<Option<i32>>;

    /// A boolean property read live, `false` when the answer is not a
    /// boolean, such as a pattern's availability. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the read.
    fn current_bool(&self, property: UIA_PROPERTY_ID) -> windows::core::Result<bool>;

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

/// The string a remote program makes of a runtime id (its `Stringify`
/// instruction, verified on Windows 11 26200): the integers in decimal,
/// separated by commas, in square brackets, such as `[42,14681214,4,5]`.
#[must_use]
pub fn runtime_id_key(runtime_id: &[i32]) -> String {
    let parts: Vec<String> = runtime_id.iter().map(i32::to_string).collect();
    format!("[{}]", parts.join(","))
}

/// The identity of the table whose runtime id's key is `key`
/// ([`runtime_id_key`]), so that a remote program's and a classic read's
/// answers for one grid are equal, and `DefaultHasher`'s fixed keys make
/// the focus listener's and an outpost's equal too.
#[must_use]
pub fn table_id(key: &str) -> TableId {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    TableId(hasher.finish())
}

/// The table of the grid's item `element`, read the classic way: its
/// `ContainingGrid`, one call into the application, and that element's
/// runtime id, which UIA keeps with it. `None` when it names no grid, or
/// the read fails, by UIA's timeout or any other error, which speaks the
/// cell as one whose table is not known.
#[must_use]
pub fn containing_grid(element: &IUIAutomationElement) -> Option<TableId> {
    match element.current_containing_grid() {
        Ok(grid) => grid
            .map(|grid| runtime_id(&grid))
            .filter(|runtime_id| !runtime_id.is_empty())
            .map(|runtime_id| table_id(&runtime_id_key(&runtime_id))),
        Err(error) => {
            tracing::debug!(%error, "a cell's containing grid could not be read");
            None
        }
    }
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
    fn current_control_type(&self) -> windows::core::Result<UIA_CONTROLTYPE_ID> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment).
        unsafe { self.CurrentControlType() }
    }

    fn current_i32_ignoring_default(
        &self,
        property: UIA_PROPERTY_ID,
    ) -> windows::core::Result<Option<i32>> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment); the
        // property id is a plain value.
        let value = unsafe { self.GetCurrentPropertyValueEx(property, true) }?;
        Ok(variant_i32(&value))
    }

    fn current_containing_grid(&self) -> windows::core::Result<Option<IUIAutomationElement>> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment); the
        // property id is a plain value.
        let value = unsafe { self.GetCurrentPropertyValue(UIA_GridItemContainingGridPropertyId) }?;
        Ok(variant_element(&value))
    }

    fn current_bool(&self, property: UIA_PROPERTY_ID) -> windows::core::Result<bool> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment); the
        // property id is a plain value.
        let value = unsafe { self.GetCurrentPropertyValue(property) }?;
        Ok(bool::try_from(&value).unwrap_or(false))
    }

    fn has_keyboard_focus(&self) -> windows::core::Result<bool> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment).
        unsafe { self.CurrentHasKeyboardFocus() }.map(windows::core::BOOL::as_bool)
    }

    fn build_updated_cache(
        &self,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia)?;
        // SAFETY: `self` and `cache` are live COM objects (see the module
        // comment); a gone element fails the call.
        unsafe { self.BuildUpdatedCache(cache) }
    }

    fn current_pattern<T: Interface>(&self, pattern: UIA_PATTERN_ID) -> windows::core::Result<T> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element; the pattern object is queried
        // for `T`'s own interface id, so the returned value is a `T`.
        unsafe { self.GetCurrentPatternAs::<T>(pattern) }
    }

    fn controller_for(&self) -> windows::core::Result<Vec<IUIAutomationElement>> {
        count(CallKind::Uia)?;
        // SAFETY: `self` is a live element (see the module comment).
        let array = unsafe { self.CurrentControllerFor() }?;
        Ok(elements_of(&array))
    }

    fn find_first(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
    ) -> windows::core::Result<Option<IUIAutomationElement>> {
        count(CallKind::Uia)?;
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
        count(CallKind::Uia)?;
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
        // SAFETY: `array` is live; an index outside it just fails with
        // `E_INVALIDARG`, and these are all within its length.
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
        count(CallKind::Uia)?;
        // SAFETY: the walker, `element`, and `cache` are live COM objects
        // (see the module comment).
        unsafe { self.GetParentElementBuildCache(element, cache) }
    }

    fn next_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia)?;
        // SAFETY: as in `parent`.
        unsafe { self.GetNextSiblingElementBuildCache(element, cache) }
    }

    fn previous_sibling(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia)?;
        // SAFETY: as in `parent`.
        unsafe { self.GetPreviousSiblingElementBuildCache(element, cache) }
    }

    fn first_child(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia)?;
        // SAFETY: as in `parent`.
        unsafe { self.GetFirstChildElementBuildCache(element, cache) }
    }

    fn normalize(
        &self,
        element: &IUIAutomationElement,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<IUIAutomationElement> {
        count(CallKind::Uia)?;
        // SAFETY: as in `parent`.
        unsafe { self.NormalizeElementBuildCache(element, cache) }
    }
}

/// The current selection of a `Selection` pattern, read live.
/// Cross-process.
pub(crate) fn current_selection(
    pattern: &IUIAutomationSelectionPattern,
) -> windows::core::Result<Vec<IUIAutomationElement>> {
    count(CallKind::Uia)?;
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    let array = unsafe { pattern.GetCurrentSelection() }?;
    Ok(elements_of(&array))
}

/// Invokes an `Invoke` pattern. Cross-process.
pub(crate) fn invoke(pattern: &IUIAutomationInvokePattern) -> windows::core::Result<()> {
    count(CallKind::Uia)?;
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Invoke() }
}

/// Toggles a `Toggle` pattern. Cross-process.
pub(crate) fn toggle(pattern: &IUIAutomationTogglePattern) -> windows::core::Result<()> {
    count(CallKind::Uia)?;
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Toggle() }
}

/// Selects a `SelectionItem` pattern's element. Cross-process.
pub(crate) fn select(pattern: &IUIAutomationSelectionItemPattern) -> windows::core::Result<()> {
    count(CallKind::Uia)?;
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    unsafe { pattern.Select() }
}

/// A `Text` pattern's visible ranges. Cross-process.
pub(crate) fn visible_ranges(
    pattern: &IUIAutomationTextPattern,
) -> windows::core::Result<Vec<IUIAutomationTextRange>> {
    count(CallKind::Uia)?;
    // SAFETY: `pattern` is a live pattern object (see the module comment).
    let array = unsafe { pattern.GetVisibleRanges() }?;
    Ok(ranges_of(&array))
}

/// The ranges of a text range array, in order. Local.
fn ranges_of(array: &IUIAutomationTextRangeArray) -> Vec<IUIAutomationTextRange> {
    // SAFETY: `array` is a live range array (see the module comment).
    let length = unsafe { array.Length() }.unwrap_or(0);
    (0..length)
        // SAFETY: `array` is live; an index outside it just fails with
        // `E_INVALIDARG`, and these are all within its length.
        .filter_map(|index| unsafe { array.GetElement(index) }.ok())
        .collect()
}

/// A text range's value for the text attribute `attribute`. Cross-process.
pub(crate) fn attribute_value(
    range: &IUIAutomationTextRange,
    attribute: UIA_TEXTATTRIBUTE_ID,
) -> windows::core::Result<VARIANT> {
    count(CallKind::Uia)?;
    // SAFETY: `range` is a live text range (see the module comment).
    unsafe { range.GetAttributeValue(attribute) }
}
