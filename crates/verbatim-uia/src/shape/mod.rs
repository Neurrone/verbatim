//! Shapes: every cached read of a UIA element declared together with the
//! cache request that fills it (`docs/design/uia-shapes.md`).
//!
//! A shape is a plain struct whose fields are what one purpose reads of an
//! element: properties, each read with a named kind, groups, which are
//! shapes themselves, and cached patterns. One declaration
//! ([`uia_shape!`], private to this crate) gives both the shape's request,
//! its [`Shape::PROPERTIES`] and [`Shape::PATTERNS`], and its read, so the
//! two cannot disagree. An element filled by a shape's request is a
//! [`Cached<S>`], which only this crate makes, and its only cached read is
//! [`Cached::read`]: so a property the request did not hold is never read,
//! and UIA is never made to raise and report an error for one, which cost
//! a quarter of a processor in a terminal flood
//! (`docs/design/wake-ups.md`, item 12).
//!
//! A shape may have gates: run-time choices, from the active theme, of
//! which of its fields are fetched. A gated property's gate is a `bool`
//! and a gated group's an `Option` of the group's own gates; the field
//! reads as an `Option`, `None` when it was not fetched. A [`Request<S>`]
//! keeps the gates it was built with and every element it fills carries
//! them, so request and read cannot disagree.
//!
//! A `Cached<S>` is neither `Send` nor `Sync`, as the element is not; an
//! [`Agile<S>`] carries it to another thread of this process with its shape,
//! and [`Live`] is an element for calls into the application, with no
//! cached read at all. A remote program in `verbatim-uia-rops` fills caches
//! through a [`Fill<S>`].

use std::cell::OnceCell;
use std::marker::PhantomData;

use verbatim_model::Rect;
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationTreeWalker, TreeScope, UIA_IsDataValidForFormPropertyId, UIA_PATTERN_ID,
    UIA_PROPERTY_ID,
};
use windows::core::{AgileReference, IUnknown, Interface};

use crate::com::{
    is_not_supported, variant_bool, variant_element, variant_element_count, variant_f64,
    variant_i32, variant_optional_bool, variant_string,
};
use crate::element::{ElementExt, WalkerExt};

mod shapes;
#[cfg(test)]
mod tests;

pub use shapes::{
    Ancestor, AncestorGates, Cell, Description, DialogText, FocusInput, HelpTextRefresh, Kept,
    ListenerEvent, ListenerSelection, Node, NodeGates, Place, Position, ProcessOrigin,
    PropertyEvent, ProviderOrigin, RoleCore, SelectedItem, SelectionContainer, ShellButton,
    Shortcut, States, SuggestionsRefresh, TextAreaPattern, TextEvent, ValueSet, WindowLookup,
};

mod sealed {
    /// Seals [`super::Shape`], [`super::Ungated`] and [`super::Covers`]: only
    /// this module's declarations implement them.
    pub trait Sealed {}
}

/// A shape: what one purpose reads of an element, and the cache request
/// that fills it. Sealed: shapes are declared in this crate alone.
pub trait Shape: sealed::Sealed + Sized + 'static {
    /// The run-time choices of what the request holds: a `bool` per gated
    /// property, each group's gates, as an `Option` for a gated group
    /// (`None`: not fetched); `()` for a shape with none.
    type Gates: Copy + Send + Sync + std::fmt::Debug + PartialEq + 'static;
    /// Every property of every field and group, without repeats.
    const PROPERTIES: &'static [UIA_PROPERTY_ID];
    /// The properties every request of this shape holds, whatever its
    /// gates.
    const UNGATED: &'static [UIA_PROPERTY_ID];
    /// The patterns the request holds: those read, and those only
    /// prefetched.
    const PATTERNS: &'static [UIA_PATTERN_ID];
    /// The patterns the request holds that no read uses.
    #[doc(hidden)]
    const PREFETCHED: &'static [UIA_PATTERN_ID];
    /// Every gate on: every field fetched.
    fn all_on() -> Self::Gates;
    /// Every combination of the shape's gates, for the recording test.
    #[cfg(test)]
    fn combinations() -> Vec<Self::Gates>;
    /// Pushes the properties the request for `gates` holds onto `out`,
    /// possibly with repeats.
    #[doc(hidden)]
    fn push_request(gates: &Self::Gates, out: &mut Vec<UIA_PROPERTY_ID>);
    /// Reads the shape from `reader`, an element filled by the request
    /// for `gates`. The only code that reads a cache.
    #[doc(hidden)]
    fn read_from(reader: &Reader<'_>, gates: &Self::Gates) -> Self;
}

/// A shape with no gate at any depth: what an event registration and a
/// covered shape must be.
pub trait Ungated: Shape<Gates = ()> {}

/// `Self`'s request holds every property and pattern of `T`'s, ungated, so
/// an element filled by it can be read as `T` ([`Cached::narrow`]).
pub trait Covers<T: Ungated>: Shape {}

/// The properties a remote program leaves out of an element's cache when
/// the element does not support them, each recorded in the element's
/// [`Absent`] record: `IsDataValidForForm`, whose default of false, which
/// a remotely filled cache stores where a locally built one stores UIA's
/// "not supported" value, would read as an invalid entry. (`ValueIsReadOnly`
/// and `RangeValueValue` are read only where their patterns are available
/// instead.)
pub const LEFT_OUT_WHEN_UNSUPPORTED: &[UIA_PROPERTY_ID] = &[UIA_IsDataValidForFormPropertyId];

/// Which of the [`LEFT_OUT_WHEN_UNSUPPORTED`] properties a remote program
/// left out of an element's cache, as the program reports it: bit `i` for
/// the property at index `i`. A read of a property the record holds
/// answers "not supported" without asking UIA, as UIA would answer for an
/// element without it. Empty for an element filled locally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Absent(u32);

impl Absent {
    /// No property left out.
    pub const NONE: Self = Self(0);

    /// The record a remote program returned, bit `i` for the property at
    /// index `i` of [`LEFT_OUT_WHEN_UNSUPPORTED`].
    #[must_use]
    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether `property` was left out.
    fn holds(self, property: UIA_PROPERTY_ID) -> bool {
        LEFT_OUT_WHEN_UNSUPPORTED
            .iter()
            .position(|&left_out| left_out == property)
            .is_some_and(|bit| self.0 & (1 << bit) != 0)
    }
}

/// Where a shape's read takes its cached values from: the element, or in
/// tests a recorder of what was read.
pub(crate) trait CachedReads {
    /// A cached property's raw value, `None` when it was not cached.
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;
    /// As [`value`](Self::value), with UIA's default for the property read
    /// as not supported.
    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;
    /// A cached rectangle property, through UIA's typed accessor.
    fn rect(&self, property: UIA_PROPERTY_ID) -> Option<RECT>;
    /// A cached pattern object, `None` when the cache holds none.
    fn pattern(&self, pattern: UIA_PATTERN_ID) -> Option<IUnknown>;
}

/// A cached read's result, counting it as a failed read when UIA answered
/// with a failure ([`crate::calls`]). A pattern the element does not
/// support answers a null pointer, which `windows` reports as an error
/// carrying no failure code: that read succeeded, and is not counted.
fn cached<T>(result: windows::core::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            if error.code().is_err() {
                crate::calls::count_failed_read();
            }
            None
        }
    }
}

/// The only cached reads of an element in the workspace: every one goes
/// through a shape's generated read (`cargo xtask ci` rejects a cached read
/// anywhere else). Each is local: the cache was filled when the element was
/// fetched or refreshed.
impl CachedReads for IUIAutomationElement {
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        // SAFETY: `self` is a live element (`crate::element`'s module
        // comment); a cached read takes a plain property id and stays in
        // this process.
        cached(unsafe { self.GetCachedPropertyValue(property) })
    }

    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        // SAFETY: as in `value`; UIA's default reads as "not supported".
        cached(unsafe { self.GetCachedPropertyValueEx(property, true) })
    }

    /// The bounding rectangle is UIA's one rectangle property.
    fn rect(&self, _property: UIA_PROPERTY_ID) -> Option<RECT> {
        // SAFETY: as in `value`; the typed accessor reads the cached
        // `BoundingRectangle` property.
        cached(unsafe { self.CachedBoundingRectangle() })
    }

    fn pattern(&self, pattern: UIA_PATTERN_ID) -> Option<IUnknown> {
        // SAFETY: `self` is a live element; a cached read stays in this
        // process, and the pattern object is queried for `IUnknown`.
        cached(unsafe { self.GetCachedPatternAs::<IUnknown>(pattern) })
    }
}

/// A property whose value is an element, such as `SelectionPattern2`'s
/// first selected item, read ignoring UIA's default.
#[derive(Clone, Debug, PartialEq)]
pub enum ElementValue {
    /// The element does not support the property.
    NotSupported,
    /// The element the property names.
    Element(IUIAutomationElement),
}

/// The read kinds a shape's properties are read with, one method each,
/// with the handling of UIA's defaults the crate's cached readers have
/// always had. No other crate can make one.
pub struct Reader<'a> {
    source: &'a dyn CachedReads,
    absent: Absent,
}

impl Reader<'_> {
    /// A property's raw value, `None` when it was not cached or the absent
    /// record holds it.
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        if self.absent.holds(property) {
            return None;
        }
        self.source.value(property)
    }

    /// As [`value`](Self::value), ignoring UIA's default.
    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        if self.absent.holds(property) {
            return None;
        }
        self.source.value_ignoring_default(property)
    }

    /// `false` when not cached or not a boolean.
    pub(crate) fn bool(&self, property: UIA_PROPERTY_ID) -> bool {
        self.value(property)
            .is_some_and(|value| variant_bool(&value))
    }

    /// `None` when not supported, or UIA supplies only its default.
    pub(crate) fn optional_bool(&self, property: UIA_PROPERTY_ID) -> Option<bool> {
        variant_optional_bool(&self.value_ignoring_default(property)?)
    }

    /// `None` when not cached or not an integer.
    pub(crate) fn i32(&self, property: UIA_PROPERTY_ID) -> Option<i32> {
        variant_i32(&self.value(property)?)
    }

    /// `None` when not supported, or UIA supplies only its default.
    pub(crate) fn f64(&self, property: UIA_PROPERTY_ID) -> Option<f64> {
        variant_f64(&self.value_ignoring_default(property)?)
    }

    /// `None` when not cached, not text, or empty.
    pub(crate) fn string(&self, property: UIA_PROPERTY_ID) -> Option<String> {
        variant_string(&self.value(property)?)
    }

    /// 0 for an element that is not itself a window.
    pub(crate) fn window_handle(&self, property: UIA_PROPERTY_ID) -> isize {
        self.i32(property).unwrap_or(0) as isize
    }

    /// The process id, which UIA stores as a signed integer.
    pub(crate) fn process_id(&self, property: UIA_PROPERTY_ID) -> Option<u32> {
        self.i32(property).map(i32::cast_unsigned)
    }

    /// A one-based property (`PositionInSet`, `SizeOfSet`, `Level`), `None`
    /// when UIA reports its "not supported" default of zero or less: zero
    /// was observed live on an hwnd-hosted root element, whose host
    /// provider answers 0 rather than leaving the value empty.
    pub(crate) fn one_based(&self, property: UIA_PROPERTY_ID) -> Option<u32> {
        self.i32(property)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0)
    }

    /// A rectangle, `None` when UIA reports its "not supported" default of
    /// an all-zero rectangle or the read fails: a genuine zero-area
    /// rectangle at the origin is not one Verbatim needs to tell from an
    /// unreported one. The rectangle comes from a provider in another
    /// process, so one whose size does not fit is `None` too, rather than
    /// overflowing.
    pub(crate) fn rect(&self, property: UIA_PROPERTY_ID) -> Option<Rect> {
        if self.absent.holds(property) {
            return None;
        }
        let rect = self.source.rect(property)?;
        if rect.left == 0 && rect.top == 0 && rect.right == 0 && rect.bottom == 0 {
            return None;
        }
        Some(Rect {
            left: rect.left,
            top: rect.top,
            width: rect.right.checked_sub(rect.left)?,
            height: rect.bottom.checked_sub(rect.top)?,
        })
    }

    /// An element-valued property read ignoring UIA's default: `None` when
    /// not cached or not an element.
    pub(crate) fn element(&self, property: UIA_PROPERTY_ID) -> Option<ElementValue> {
        let value = self.value_ignoring_default(property)?;
        if is_not_supported(&value) {
            Some(ElementValue::NotSupported)
        } else {
            variant_element(&value).map(ElementValue::Element)
        }
    }

    /// Whether an element-array property names any element: `false` when
    /// not cached, not an element array, or empty.
    pub(crate) fn names_any(&self, property: UIA_PROPERTY_ID) -> bool {
        self.value(property)
            .is_some_and(|value| variant_element_count(&value) > 0)
    }

    /// A cached pattern object as the interface `T`, `None` when the cache
    /// holds none: the element does not support the pattern.
    pub(crate) fn pattern<T: Interface>(&self, pattern: UIA_PATTERN_ID) -> Option<T> {
        self.source
            .pattern(pattern)
            .and_then(|unknown| unknown.cast().ok())
    }
}

/// An element whose cache was filled by `S`'s request. Made only by this
/// crate: by a fill from a [`Request<S>`], by a registration's part for
/// `S` ([`crate::Part::shaped`]), by [`Agile::resolve`], by
/// [`Cached::narrow`], and by [`Fill::claim`] for a remote program's
/// result.
pub struct Cached<S: Shape> {
    live: Live,
    gates: S::Gates,
    absent: Absent,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Cached<S> {
    /// `element`, filled locally by `S`'s request for `gates`.
    pub(crate) fn filled(element: IUIAutomationElement, gates: S::Gates) -> Self {
        Self::claimed(element, gates, Absent::NONE)
    }

    /// `element`, filled by `S`'s request for `gates`, with the properties
    /// `absent` records left out.
    fn claimed(element: IUIAutomationElement, gates: S::Gates, absent: Absent) -> Self {
        Self {
            live: Live { element },
            gates,
            absent,
            shape: PhantomData,
        }
    }

    /// The shape, read from the cache. Local.
    #[must_use]
    pub fn read(&self) -> S {
        S::read_from(
            &Reader {
                source: &self.live.element,
                absent: self.absent,
            },
            &self.gates,
        )
    }

    /// What the cache holds for `property` as UIA answers a cached read
    /// of it, with UIA's default for the property read as "not supported"
    /// when `ignore_default` is set: for the tests that observe what UIA
    /// stores for a property its provider answered in some way, such as
    /// failing, where a shape's read kinds would hide the difference.
    /// Verbatim's own code reads only through the shape. Local.
    ///
    /// # Errors
    ///
    /// UIA's error when the cached read fails, as for a property the
    /// cache does not hold.
    pub fn observe_cached(
        &self,
        property: UIA_PROPERTY_ID,
        ignore_default: bool,
    ) -> windows::core::Result<VARIANT> {
        // SAFETY: as in `CachedReads::value`.
        unsafe {
            self.live
                .element
                .GetCachedPropertyValueEx(property, ignore_default)
        }
    }

    /// The element, for calls into the application.
    #[must_use]
    pub fn live(&self) -> &Live {
        &self.live
    }

    /// The gates the element's request was built with.
    #[must_use]
    pub fn gates(&self) -> S::Gates {
        self.gates
    }

    /// The process the element belongs to, from its cache, when its
    /// request was made with [`Request::with_process`]; `None` when the
    /// cache does not hold it. Local.
    #[must_use]
    pub fn cached_process(&self) -> Option<u32> {
        Cached::<ProcessOrigin>::filled(self.live.element.clone(), ())
            .read()
            .process_id
    }

    /// The element's runtime id, which UIA keeps with every element
    /// whatever its cache holds. Local.
    #[must_use]
    pub fn runtime_id(&self) -> Vec<i32> {
        crate::com::runtime_id(&self.live.element)
    }

    /// The element, with its shape, for another thread of this process.
    ///
    /// # Errors
    ///
    /// The COM error if the agile reference cannot be made.
    pub fn agile(&self) -> windows::core::Result<Agile<S>> {
        Ok(Agile {
            element: AgileReference::new(&self.live.element)?,
            gates: self.gates,
            absent: self.absent,
            shape: PhantomData,
        })
    }

    /// The same element read as `T`, whose request `S`'s holds.
    #[must_use]
    pub fn narrow<T: Ungated>(&self) -> Cached<T>
    where
        S: Covers<T>,
    {
        Cached::claimed(self.live.element.clone(), (), self.absent)
    }
}

impl<S: Shape> Clone for Cached<S> {
    fn clone(&self) -> Self {
        Self {
            live: self.live.clone(),
            gates: self.gates,
            absent: self.absent,
            shape: PhantomData,
        }
    }
}

impl<S: Shape> std::fmt::Debug for Cached<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cached")
            .field("shape", &std::any::type_name::<S>())
            .finish_non_exhaustive()
    }
}

/// A [`Cached<S>`] for another thread of this process: an agile reference,
/// with the shape, gates and absent record its element was filled with.
/// `Send` and `Sync`.
pub struct Agile<S: Shape> {
    element: AgileReference<IUIAutomationElement>,
    gates: S::Gates,
    absent: Absent,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Agile<S> {
    /// The element on this thread, with its shape. Local: within the
    /// process, the element and its cache are the same object.
    ///
    /// # Errors
    ///
    /// The COM error if the reference cannot be resolved.
    pub fn resolve(&self) -> windows::core::Result<Cached<S>> {
        Ok(Cached::claimed(
            self.element.resolve()?,
            self.gates,
            self.absent,
        ))
    }

    /// The untyped agile reference, for a registration's scope
    /// ([`crate::Scope::Elements`]), which reads nothing of the element's
    /// cache. Local.
    #[must_use]
    pub fn reference(&self) -> AgileReference<IUIAutomationElement> {
        self.element.clone()
    }

    /// Whether `other` is this same agile reference, as a clone of it is:
    /// an element kept again gets another. Local.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        self.element == other.element
    }
}

impl<S: Shape> Clone for Agile<S> {
    fn clone(&self) -> Self {
        Self {
            element: self.element.clone(),
            gates: self.gates,
            absent: self.absent,
            shape: PhantomData,
        }
    }
}

impl<S: Shape> std::fmt::Debug for Agile<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agile")
            .field("shape", &std::any::type_name::<S>())
            .finish_non_exhaustive()
    }
}

/// Which neighbour a tree walker step reaches ([`Live::walk`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The parent.
    Parent,
    /// The next sibling.
    NextSibling,
    /// The previous sibling.
    PreviousSibling,
    /// The first child.
    FirstChild,
}

/// An element for calls into the application: patterns, text, live reads,
/// imports into remote operations, and filling its cache into a shape. It
/// has no cached read.
#[derive(Clone, Debug)]
pub struct Live {
    element: IUIAutomationElement,
}

impl Live {
    /// `element`, for calls into its application.
    #[must_use]
    pub fn new(element: IUIAutomationElement) -> Self {
        Self { element }
    }

    /// The raw element.
    #[must_use]
    pub fn element(&self) -> &IUIAutomationElement {
        &self.element
    }

    /// The element's runtime id, which UIA keeps with every element. Local.
    #[must_use]
    pub fn runtime_id(&self) -> Vec<i32> {
        crate::com::runtime_id(&self.element)
    }

    /// The element with its cache filled by `request`. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the call.
    pub fn refresh<S: Shape>(&self, request: &Request<S>) -> windows::core::Result<Cached<S>> {
        let fresh = self.element.build_updated_cache(&request.cache)?;
        Ok(Cached::filled(fresh, request.gates))
    }

    /// The first element under this one, within `scope`, that matches
    /// `condition`, with its cache filled by `request`; `None` for no
    /// match. Cross-process.
    ///
    /// # Errors
    ///
    /// The COM error if the provider fails the call.
    pub fn find_first<S: Shape>(
        &self,
        scope: TreeScope,
        condition: &IUIAutomationCondition,
        request: &Request<S>,
    ) -> windows::core::Result<Option<Cached<S>>> {
        Ok(self
            .element
            .find_first_build_cache(scope, condition, &request.cache)?
            .map(|found| Cached::filled(found, request.gates)))
    }

    /// The neighbour `direction` names in `walker`'s view, with its cache
    /// filled by `request`: one cross-process call.
    ///
    /// # Errors
    ///
    /// The COM error, or, for no such neighbour, the error carrying no
    /// failure code that `windows` reports for a null element.
    pub fn walk<S: Shape>(
        &self,
        walker: &IUIAutomationTreeWalker,
        direction: Direction,
        request: &Request<S>,
    ) -> windows::core::Result<Cached<S>> {
        let cache = &request.cache;
        let found = match direction {
            Direction::Parent => walker.parent(&self.element, cache),
            Direction::NextSibling => walker.next_sibling(&self.element, cache),
            Direction::PreviousSibling => walker.previous_sibling(&self.element, cache),
            Direction::FirstChild => walker.first_child(&self.element, cache),
        }?;
        Ok(Cached::filled(found, request.gates))
    }

    /// The neighbour `direction` names in `walker`'s view, with no cache:
    /// for a walk that compares the runtime ids of the elements it passes,
    /// which UIA keeps with every element, and reads nothing else of them.
    /// One cross-process call that asks for no property, so a step over an
    /// element whose properties are costly to answer costs no more than any
    /// other: a top-level window's title bar, for one, has UIA ask the
    /// window's thread for its location (`WM_GETTITLEBARINFOEX`), which that
    /// thread can take seconds to answer.
    ///
    /// # Errors
    ///
    /// The COM error, or, for no such neighbour, the error carrying no
    /// failure code that `windows` reports for a null element.
    pub fn step(
        &self,
        walker: &IUIAutomationTreeWalker,
        direction: Direction,
    ) -> windows::core::Result<Self> {
        crate::calls::count(verbatim_model::CallKind::Uia);
        let element = &self.element;
        let found = match direction {
            // SAFETY: the walker and the element are live COM objects.
            Direction::Parent => unsafe { walker.GetParentElement(element) },
            // SAFETY: as above.
            Direction::NextSibling => unsafe { walker.GetNextSiblingElement(element) },
            // SAFETY: as above.
            Direction::PreviousSibling => unsafe { walker.GetPreviousSiblingElement(element) },
            // SAFETY: as above.
            Direction::FirstChild => unsafe { walker.GetFirstChildElement(element) },
        }?;
        Ok(Self::new(found))
    }

    /// The element itself if `walker`'s condition matches it, else its
    /// nearest ancestor that matches, with its cache filled by `request`
    /// (`NormalizeElementBuildCache`): one cross-process call.
    ///
    /// # Errors
    ///
    /// The COM error, or the null-element error when nothing matches.
    pub fn normalize<S: Shape>(
        &self,
        walker: &IUIAutomationTreeWalker,
        request: &Request<S>,
    ) -> windows::core::Result<Cached<S>> {
        let found = walker.normalize(&self.element, &request.cache)?;
        Ok(Cached::filled(found, request.gates))
    }
}

/// `S`'s cache request, for the gates it was built with
/// ([`crate::Uia::request`], [`crate::Uia::request_fixed`]).
pub struct Request<S: Shape> {
    cache: IUIAutomationCacheRequest,
    /// `cache` with the provider description added, for the proxy rule's
    /// lookups, built at the first such lookup and kept
    /// ([`described`](Self::described)).
    described: OnceCell<IUIAutomationCacheRequest>,
    gates: S::Gates,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Request<S> {
    /// The request for `gates`, built on `client`.
    pub(crate) fn build(
        client: &windows::Win32::UI::Accessibility::IUIAutomation,
        gates: S::Gates,
    ) -> windows::core::Result<Self> {
        let cache = crate::cache::cache_request(client, &request_properties::<S>(&gates))?;
        for &pattern in S::PATTERNS {
            // SAFETY: `cache` is a live cache request; a pattern id is a
            // plain value.
            unsafe { cache.AddPattern(pattern) }?;
        }
        Ok(Self {
            cache,
            described: OnceCell::new(),
            gates,
            shape: PhantomData,
        })
    }

    /// The gates the request was built with.
    #[must_use]
    pub fn gates(&self) -> S::Gates {
        self.gates
    }

    /// The cache request, and the gates the elements it fills carry.
    pub(crate) fn parts(&self) -> (&IUIAutomationCacheRequest, S::Gates) {
        (&self.cache, self.gates)
    }

    /// A copy of this request that also caches the element's process
    /// ([`Cached::cached_process`]), for a read whose element must be
    /// checked against the process it was asked of before anything else is
    /// read of it. Local.
    ///
    /// # Errors
    ///
    /// The COM error if the cache request cannot be copied or added to.
    pub fn with_process(&self) -> windows::core::Result<Self> {
        // SAFETY: `cache` is a live cache request; the copy is this one's.
        let cache = unsafe { self.cache.Clone() }?;
        // SAFETY: a live cache request; a plain property id.
        unsafe { cache.AddProperty(windows::Win32::UI::Accessibility::UIA_ProcessIdPropertyId) }?;
        Ok(Self {
            cache,
            described: OnceCell::new(),
            gates: self.gates,
            shape: PhantomData,
        })
    }

    /// The cache request with the provider description added, for a lookup
    /// the proxy rule judges (`crate::answered`): built from it once, at
    /// the first such lookup, and kept with it. Local.
    pub(crate) fn described(&self) -> windows::core::Result<&IUIAutomationCacheRequest> {
        if let Some(described) = self.described.get() {
            return Ok(described);
        }
        let described = crate::answered::with_description(&self.cache)?;
        Ok(self.described.get_or_init(|| described))
    }
}

impl<S: Shape> Clone for Request<S> {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            described: self.described.clone(),
            gates: self.gates,
            shape: PhantomData,
        }
    }
}

/// What a remote program needs to fill caches as `S`'s request does, and
/// the only way outside this crate to wrap the elements it returns as
/// [`Cached<S>`]: `verbatim-uia-rops` builds its programs' cache requests
/// from [`properties`](Self::properties) and
/// [`patterns`](Self::patterns), takes its classic fallback with
/// [`request`](Self::request), and wraps what a program returns with
/// [`claim`](Self::claim). That a program fills its caches from this list
/// and nothing else is that crate's reviewed code, not something the
/// compiler proves (`docs/design/uia-shapes.md`, section 3.5, items 8 and
/// 9). Neither `Clone` nor `Copy`.
pub struct Fill<S: Shape> {
    request: Request<S>,
    properties: Vec<UIA_PROPERTY_ID>,
}

impl<S: Shape> Fill<S> {
    /// The token for `request`'s fills. Local.
    #[doc(hidden)]
    #[must_use]
    pub fn new(request: &Request<S>) -> Self {
        Self {
            request: request.clone(),
            properties: request_properties::<S>(&request.gates),
        }
    }

    /// The properties the program's cache requests hold.
    #[must_use]
    pub fn properties(&self) -> &[UIA_PROPERTY_ID] {
        &self.properties
    }

    /// The patterns the program's cache requests hold.
    #[must_use]
    pub fn patterns(&self) -> &'static [UIA_PATTERN_ID] {
        S::PATTERNS
    }

    /// The request a classic fallback fills with.
    #[must_use]
    pub fn request(&self) -> &Request<S> {
        &self.request
    }

    /// `element`, which a remote program returned with its cache filled
    /// from [`properties`](Self::properties) and with the properties
    /// `absent` records left out, as `S`.
    #[doc(hidden)]
    #[must_use]
    pub fn claim(&self, element: IUIAutomationElement, absent: Absent) -> Cached<S> {
        Cached::claimed(element, self.request.gates, absent)
    }
}

/// The properties `S`'s request for `gates` holds, without repeats, in the
/// order they were first pushed.
pub(crate) fn request_properties<S: Shape>(gates: &S::Gates) -> Vec<UIA_PROPERTY_ID> {
    let mut pushed = Vec::new();
    S::push_request(gates, &mut pushed);
    let mut properties: Vec<UIA_PROPERTY_ID> = Vec::with_capacity(pushed.len());
    for property in pushed {
        if !properties.contains(&property) {
            properties.push(property);
        }
    }
    properties
}

/// Wraps an element UIA delivered with an event of a part for `S`, whose
/// cache `S`'s request filled.
pub(crate) fn delivered<S: Ungated>(element: &IUIAutomationElement) -> Cached<S> {
    Cached::filled(element.clone(), ())
}

/// One of `options`, chosen by `index` taken as a mixed-radix number, whose
/// lowest digit is consumed: how the macro's `combinations` lists every
/// combination of a shape's gates.
#[cfg(test)]
pub(crate) fn pick<T: Copy>(options: &[T], index: &mut usize) -> T {
    let chosen = options[*index % options.len()];
    *index /= options.len();
    chosen
}

/// Sets of ids, computed at compile time: the distinct members of several
/// lists, in first-seen order, and whether one set holds another. Two
/// passes, as an array's length must be a constant: one counts, the other
/// fills.
macro_rules! id_sets {
    ($ty:ident, $count:ident, $seen:ident, $union:ident, $holds:ident) => {
        /// How many distinct ids `lists` hold.
        pub(crate) const fn $count(lists: &[&[$ty]]) -> usize {
            let mut count = 0;
            let mut list = 0;
            while list < lists.len() {
                let mut index = 0;
                while index < lists[list].len() {
                    if !$seen(lists, list, index) {
                        count += 1;
                    }
                    index += 1;
                }
                list += 1;
            }
            count
        }

        /// Whether the id at `lists[list][index]` came earlier.
        const fn $seen(lists: &[&[$ty]], list: usize, index: usize) -> bool {
            let id = lists[list][index].0;
            let mut earlier = 0;
            while earlier <= list {
                let end = if earlier == list {
                    index
                } else {
                    lists[earlier].len()
                };
                let mut at = 0;
                while at < end {
                    if lists[earlier][at].0 == id {
                        return true;
                    }
                    at += 1;
                }
                earlier += 1;
            }
            false
        }

        /// The `N` distinct ids of `lists`, in first-seen order.
        pub(crate) const fn $union<const N: usize>(lists: &[&[$ty]]) -> [$ty; N] {
            let mut out = [$ty(0); N];
            let mut filled = 0;
            let mut list = 0;
            while list < lists.len() {
                let mut index = 0;
                while index < lists[list].len() {
                    if !$seen(lists, list, index) {
                        out[filled] = lists[list][index];
                        filled += 1;
                    }
                    index += 1;
                }
                list += 1;
            }
            assert!(filled == N);
            out
        }

        /// Whether `set` holds every id of `members`.
        pub(crate) const fn $holds(set: &[$ty], members: &[$ty]) -> bool {
            let mut member = 0;
            while member < members.len() {
                let mut found = false;
                let mut at = 0;
                while at < set.len() {
                    if set[at].0 == members[member].0 {
                        found = true;
                    }
                    at += 1;
                }
                if !found {
                    return false;
                }
                member += 1;
            }
            true
        }
    };
}

id_sets!(
    UIA_PROPERTY_ID,
    count_properties,
    property_seen,
    property_union,
    holds_properties
);
id_sets!(
    UIA_PATTERN_ID,
    count_patterns,
    pattern_seen,
    pattern_union,
    holds_patterns
);

/// The distinct properties of several lists, as a `&'static` slice computed
/// at compile time.
macro_rules! property_set {
    ($($list:expr),* $(,)?) => {{
        const LISTS: &[&[UIA_PROPERTY_ID]] = &[$($list),*];
        const COUNT: usize = $crate::shape::count_properties(LISTS);
        const SET: [UIA_PROPERTY_ID; COUNT] = $crate::shape::property_union::<COUNT>(LISTS);
        &SET
    }};
}

/// [`property_set!`] for patterns.
macro_rules! pattern_set {
    ($($list:expr),* $(,)?) => {{
        const LISTS: &[&[UIA_PATTERN_ID]] = &[$($list),*];
        const COUNT: usize = $crate::shape::count_patterns(LISTS);
        const SET: [UIA_PATTERN_ID; COUNT] = $crate::shape::pattern_union::<COUNT>(LISTS);
        &SET
    }};
}

/// The Rust type of a read kind.
macro_rules! kind_type {
    (bool) => { bool };
    (optional_bool) => { Option<bool> };
    (i32) => { Option<i32> };
    (f64) => { Option<f64> };
    (string) => { Option<String> };
    (window_handle) => { isize };
    (process_id) => { Option<u32> };
    (one_based) => { Option<u32> };
    (rect) => { Option<verbatim_model::Rect> };
    (element) => { Option<$crate::shape::ElementValue> };
    (names_any) => { bool };
}

/// The `Covers` implementations of a shape, each with a compile-time
/// assertion that the shape's ungated request holds the covered shape's
/// every property and pattern.
macro_rules! covers {
    ($name:ident: $( $covered:ident ),*) => {
        $(
            impl Covers<$covered> for $name {}
            const _: () = assert!(
                $crate::shape::holds_properties(
                    <$name as Shape>::UNGATED,
                    <$covered as Shape>::PROPERTIES,
                ) && $crate::shape::holds_patterns(
                    <$name as Shape>::PATTERNS,
                    <$covered as Shape>::PATTERNS,
                ),
                concat!(stringify!($name), " does not hold all of ", stringify!($covered)),
            );
        )*
    };
}

/// Declares a shape: its struct, and its [`Shape`] implementation, whose
/// request and read both come from the one field list.
///
/// `gates: none` declares a shape with no gates, which is [`Ungated`]:
/// each group is read with `&()` as its gates, so a group with gates does
/// not compile there. It may read cached `patterns`, each as an `Option`
/// of its interface, and `prefetch` patterns no read uses.
///
/// `gates: Name` declares a shape with gates, generating the struct `Name`
/// of them: a `bool` for each of its `gated_properties`, an `Option` of
/// the group's own gates for each of its `gated_groups`, and the gates of
/// each of its ungated `groups` as they are. Each gated field reads as an
/// `Option`, `None` when its gate was off, without touching the cache. A
/// gated field is declared in its own section, since a `macro_rules!`
/// macro cannot tell a `#[gated]` marker from the field's documentation.
///
/// `covers` implements [`Covers`] for each shape named, with a
/// compile-time assertion that this shape's ungated request holds the
/// named shape's every property and pattern.
macro_rules! uia_shape {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            gates: none;
            groups { $( $(#[$gmeta:meta])* $group_field:ident: $group:ident ),* $(,)? }
            properties {
                $( $(#[$pmeta:meta])* $field:ident: $property:ident => $kind:ident ),* $(,)?
            }
            $( patterns {
                $( $(#[$patmeta:meta])* $pattern_field:ident: $pattern_id:ident => $pattern_type:ident ),* $(,)?
            } )?
            prefetch { $( $pattern:ident ),* $(,)? }
            covers { $( $covered:ident ),* $(,)? }
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $( $(#[$gmeta])* pub $group_field: $group, )*
            $( $(#[$pmeta])* pub $field: $crate::shape::kind_type!($kind), )*
            $( $( $(#[$patmeta])* pub $pattern_field: Option<$pattern_type>, )* )?
        }

        impl $crate::shape::sealed::Sealed for $name {}

        impl Shape for $name {
            type Gates = ();
            const PROPERTIES: &'static [UIA_PROPERTY_ID] = $crate::shape::property_set!(
                $( <$group as Shape>::PROPERTIES, )*
                &[$( $property ),*],
            );
            const UNGATED: &'static [UIA_PROPERTY_ID] = Self::PROPERTIES;
            const PATTERNS: &'static [UIA_PATTERN_ID] = $crate::shape::pattern_set!(
                $( <$group as Shape>::PATTERNS, )*
                &[$( $( $pattern_id ),* )?],
                &[$( $pattern ),*],
            );
            const PREFETCHED: &'static [UIA_PATTERN_ID] = $crate::shape::pattern_set!(
                $( <$group as Shape>::PREFETCHED, )*
                &[$( $pattern ),*],
            );

            fn all_on() {}

            #[cfg(test)]
            fn combinations() -> Vec<()> {
                vec![()]
            }

            fn push_request(_gates: &(), out: &mut Vec<UIA_PROPERTY_ID>) {
                out.extend_from_slice(Self::PROPERTIES);
            }

            fn read_from(reader: &Reader<'_>, _gates: &()) -> Self {
                Self {
                    $( $group_field: <$group as Shape>::read_from(reader, &()), )*
                    $( $field: reader.$kind($property), )*
                    $( $( $pattern_field: reader.pattern::<$pattern_type>($pattern_id), )* )?
                }
            }
        }

        impl Ungated for $name {}

        $crate::shape::covers!($name: $( $covered ),*);
    };
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            gates: $gates:ident;
            groups { $( $(#[$gmeta:meta])* $group_field:ident: $group:ident ),* $(,)? }
            gated_groups {
                $( $(#[$ggmeta:meta])* $gated_group_field:ident: $gated_group:ident ),* $(,)?
            }
            properties {
                $( $(#[$pmeta:meta])* $field:ident: $property:ident => $kind:ident ),* $(,)?
            }
            gated_properties {
                $( $(#[$gpmeta:meta])* $gated_field:ident: $gated_property:ident => $gated_kind:ident ),* $(,)?
            }
            covers { $( $covered:ident ),* $(,)? }
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $( $(#[$gmeta])* pub $group_field: $group, )*
            $( $(#[$ggmeta])* pub $gated_group_field: Option<$gated_group>, )*
            $( $(#[$pmeta])* pub $field: $crate::shape::kind_type!($kind), )*
            $( $(#[$gpmeta])* pub $gated_field: Option<$crate::shape::kind_type!($gated_kind)>, )*
        }

        #[doc = concat!("The gates of [`", stringify!($name), "`]: which of its fields a request fetches.")]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $gates {
            $(
                #[doc = concat!("The gates of its `", stringify!($group_field), "` group.")]
                pub $group_field: <$group as Shape>::Gates,
            )*
            $(
                #[doc = concat!("Whether its `", stringify!($gated_group_field), "` group is fetched, with that group's gates; `None` when it is not.")]
                pub $gated_group_field: Option<<$gated_group as Shape>::Gates>,
            )*
            $(
                #[doc = concat!("Whether its `", stringify!($gated_field), "` property is fetched.")]
                pub $gated_field: bool,
            )*
        }

        impl $crate::shape::sealed::Sealed for $name {}

        impl Shape for $name {
            type Gates = $gates;
            const PROPERTIES: &'static [UIA_PROPERTY_ID] = $crate::shape::property_set!(
                $( <$group as Shape>::PROPERTIES, )*
                $( <$gated_group as Shape>::PROPERTIES, )*
                &[$( $property ),*],
                &[$( $gated_property ),*],
            );
            const UNGATED: &'static [UIA_PROPERTY_ID] = $crate::shape::property_set!(
                $( <$group as Shape>::UNGATED, )*
                &[$( $property ),*],
            );
            const PATTERNS: &'static [UIA_PATTERN_ID] = $crate::shape::pattern_set!(
                $( <$group as Shape>::PATTERNS, )*
            );
            const PREFETCHED: &'static [UIA_PATTERN_ID] = $crate::shape::pattern_set!(
                $( <$group as Shape>::PREFETCHED, )*
            );

            fn all_on() -> $gates {
                $gates {
                    $( $group_field: <$group as Shape>::all_on(), )*
                    $( $gated_group_field: Some(<$gated_group as Shape>::all_on()), )*
                    $( $gated_field: true, )*
                }
            }

            #[cfg(test)]
            fn combinations() -> Vec<$gates> {
                $( let $group_field = <$group as Shape>::combinations(); )*
                $(
                    let $gated_group_field: Vec<_> = std::iter::once(None)
                        .chain(<$gated_group as Shape>::combinations().into_iter().map(Some))
                        .collect();
                )*
                $( let $gated_field = [false, true]; )*
                let total = 1usize
                    $( * $group_field.len() )*
                    $( * $gated_group_field.len() )*
                    $( * $gated_field.len() )*;
                (0..total)
                    .map(|mut index| $gates {
                        $( $group_field: $crate::shape::pick(&$group_field, &mut index), )*
                        $( $gated_group_field: $crate::shape::pick(&$gated_group_field, &mut index), )*
                        $( $gated_field: $crate::shape::pick(&$gated_field, &mut index), )*
                    })
                    .collect()
            }

            fn push_request(gates: &$gates, out: &mut Vec<UIA_PROPERTY_ID>) {
                $( <$group as Shape>::push_request(&gates.$group_field, out); )*
                $(
                    if let Some(inner) = &gates.$gated_group_field {
                        <$gated_group as Shape>::push_request(inner, out);
                    }
                )*
                $( out.push($property); )*
                $(
                    if gates.$gated_field {
                        out.push($gated_property);
                    }
                )*
            }

            fn read_from(reader: &Reader<'_>, gates: &$gates) -> Self {
                Self {
                    $( $group_field: <$group as Shape>::read_from(reader, &gates.$group_field), )*
                    $(
                        $gated_group_field: gates
                            .$gated_group_field
                            .as_ref()
                            .map(|inner| <$gated_group as Shape>::read_from(reader, inner)),
                    )*
                    $( $field: reader.$kind($property), )*
                    $( $gated_field: gates.$gated_field.then(|| reader.$gated_kind($gated_property)), )*
                }
            }
        }

        $(
            // A gated group's patterns would be gated patterns, which a
            // request does not have.
            const _: () = assert!(
                <$gated_group as Shape>::PATTERNS.is_empty(),
                concat!(stringify!($gated_group), " has patterns and cannot be gated"),
            );
        )*

        $crate::shape::covers!($name: $( $covered ),*);
    };
}

pub(crate) use {covers, kind_type, pattern_set, property_set, uia_shape};
