//! Shapes: every cached read of a UIA element declared together with the
//! cache request that fills it (`docs/design/uia-shapes.md`).
//!
//! A shape is a plain struct whose fields are what one purpose reads of an
//! element: properties, each read with a named kind, and groups, which are
//! shapes themselves. One declaration ([`uia_shape!`], private to this
//! crate) gives both the shape's request, its [`Shape::PROPERTIES`] and
//! [`Shape::PATTERNS`], and its read, so the two cannot disagree. An
//! element filled by a shape's request is a [`Cached<S>`], which only this
//! crate makes, and its only cached read is [`Cached::read`]: so a property
//! the request did not hold is never read, and UIA is never made to raise
//! and report an error for one, which cost a quarter of a processor in a
//! terminal flood (`docs/design/wake-ups.md`, item 12).
//!
//! A `Cached<S>` is neither `Send` nor `Sync`, as the element is not; an
//! [`Agile<S>`] carries it to another thread of this process with its shape,
//! and [`Live`] is an element for calls into the application, with no
//! cached read at all.

use std::marker::PhantomData;

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationCondition, IUIAutomationElement, TreeScope,
    UIA_PATTERN_ID, UIA_PROPERTY_ID,
};
use windows::core::AgileReference;

use crate::com::{variant_bool, variant_f64, variant_i32, variant_optional_bool, variant_string};
use crate::element::ElementExt;

mod shapes;
#[cfg(test)]
mod tests;

pub use shapes::{Kept, ListenerEvent, PropertyEvent, RoleCore, States, TextEvent, ValueSet};

mod sealed {
    /// Seals [`super::Shape`], [`super::Ungated`] and [`super::Covers`]: only
    /// this module's declarations implement them.
    pub trait Sealed {}
}

/// A shape: what one purpose reads of an element, and the cache request
/// that fills it. Sealed: shapes are declared in this crate alone.
pub trait Shape: sealed::Sealed + Sized + 'static {
    /// The run-time choices of what the request holds: `()` for a shape
    /// with none.
    type Gates: Copy + Send + Sync + 'static;
    /// Every property of every field and group, without repeats.
    const PROPERTIES: &'static [UIA_PROPERTY_ID];
    /// The properties every request of this shape holds, whatever its
    /// gates.
    const UNGATED: &'static [UIA_PROPERTY_ID];
    /// The patterns the request holds: those read, and those only
    /// prefetched.
    const PATTERNS: &'static [UIA_PATTERN_ID];
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

/// Where a shape's read takes its cached values from: the element, or in
/// tests a recorder of what was read.
pub(crate) trait CachedReads {
    /// A cached property's raw value, `None` when it was not cached.
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;
    /// As [`value`](Self::value), with UIA's default for the property read
    /// as not supported.
    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT>;
}

impl CachedReads for IUIAutomationElement {
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        ElementExt::cached_value(self, property)
    }

    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        ElementExt::cached_value_ignoring_default(self, property)
    }
}

/// The read kinds a shape's properties are read with, one method each,
/// with the handling of UIA's defaults the crate's cached readers have
/// always had. No other crate can make one.
pub struct Reader<'a> {
    source: &'a dyn CachedReads,
}

impl Reader<'_> {
    /// `false` when not cached or not a boolean.
    pub(crate) fn bool(&self, property: UIA_PROPERTY_ID) -> bool {
        self.source
            .value(property)
            .is_some_and(|value| variant_bool(&value))
    }

    /// `None` when not supported, or UIA supplies only its default.
    pub(crate) fn optional_bool(&self, property: UIA_PROPERTY_ID) -> Option<bool> {
        variant_optional_bool(&self.source.value_ignoring_default(property)?)
    }

    /// `None` when not cached or not an integer.
    pub(crate) fn i32(&self, property: UIA_PROPERTY_ID) -> Option<i32> {
        variant_i32(&self.source.value(property)?)
    }

    /// `None` when not supported, or UIA supplies only its default.
    pub(crate) fn f64(&self, property: UIA_PROPERTY_ID) -> Option<f64> {
        variant_f64(&self.source.value_ignoring_default(property)?)
    }

    /// `None` when not cached, not text, or empty.
    pub(crate) fn string(&self, property: UIA_PROPERTY_ID) -> Option<String> {
        variant_string(&self.source.value(property)?)
    }

    /// 0 for an element that is not itself a window.
    pub(crate) fn window_handle(&self, property: UIA_PROPERTY_ID) -> isize {
        self.i32(property).unwrap_or(0) as isize
    }

    /// The process id, which UIA stores as a signed integer.
    pub(crate) fn process_id(&self, property: UIA_PROPERTY_ID) -> Option<u32> {
        self.i32(property).map(i32::cast_unsigned)
    }
}

/// An element whose cache was filled by `S`'s request. Made only by this
/// crate: by a fill from a [`Request<S>`], by a registration's part for
/// `S` ([`crate::Part::shaped`]), by [`Agile::resolve`], and by
/// [`Cached::narrow`].
pub struct Cached<S: Shape> {
    live: Live,
    gates: S::Gates,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Cached<S> {
    /// `element`, filled by `S`'s request for `gates`.
    pub(crate) fn filled(element: IUIAutomationElement, gates: S::Gates) -> Self {
        Self {
            live: Live { element },
            gates,
            shape: PhantomData,
        }
    }

    /// The shape, read from the cache. Local.
    #[must_use]
    pub fn read(&self) -> S {
        S::read_from(
            &Reader {
                source: &self.live.element,
            },
            &self.gates,
        )
    }

    /// The element, for calls into the application.
    #[must_use]
    pub fn live(&self) -> &Live {
        &self.live
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
            shape: PhantomData,
        })
    }

    /// The same element read as `T`, whose request `S`'s holds.
    #[must_use]
    pub fn narrow<T: Ungated>(&self) -> Cached<T>
    where
        S: Covers<T>,
    {
        Cached::filled(self.live.element.clone(), ())
    }
}

impl<S: Shape> Clone for Cached<S> {
    fn clone(&self) -> Self {
        Self {
            live: self.live.clone(),
            gates: self.gates,
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
/// with the shape and gates its element was filled with. `Send` and `Sync`.
pub struct Agile<S: Shape> {
    element: AgileReference<IUIAutomationElement>,
    gates: S::Gates,
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
        Ok(Cached::filled(self.element.resolve()?, self.gates))
    }
}

impl<S: Shape> Clone for Agile<S> {
    fn clone(&self) -> Self {
        Self {
            element: self.element.clone(),
            gates: self.gates,
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
}

/// `S`'s cache request, for the gates it was built with
/// ([`crate::Uia::request_fixed`]).
pub struct Request<S: Shape> {
    cache: IUIAutomationCacheRequest,
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
            gates,
            shape: PhantomData,
        })
    }
}

impl<S: Shape> Request<S> {
    /// The cache request, and the gates the elements it fills carry.
    pub(crate) fn parts(&self) -> (&IUIAutomationCacheRequest, S::Gates) {
        (&self.cache, self.gates)
    }
}

impl<S: Shape> Clone for Request<S> {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            gates: self.gates,
            shape: PhantomData,
        }
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
}

/// Declares a shape: its struct, and its [`Shape`] implementation, whose
/// request and read both come from the one field list. `gates: none`
/// declares a shape with no gates, which is [`Ungated`]: each group is read
/// with `&()` as its gates, so a group with gates does not compile here.
/// `covers` implements [`Covers`] for each shape named, with a
/// compile-time assertion that this shape's request holds the named
/// shape's every property and pattern.
macro_rules! uia_shape {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            gates: none;
            groups { $( $(#[$gmeta:meta])* $group_field:ident: $group:ident ),* $(,)? }
            properties {
                $( $(#[$pmeta:meta])* $field:ident: $property:ident => $kind:ident ),* $(,)?
            }
            prefetch { $( $pattern:ident ),* $(,)? }
            covers { $( $covered:ident ),* $(,)? }
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $( $(#[$gmeta])* pub $group_field: $group, )*
            $( $(#[$pmeta])* pub $field: $crate::shape::kind_type!($kind), )*
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
                &[$( $pattern ),*],
            );

            fn push_request(_gates: &(), out: &mut Vec<UIA_PROPERTY_ID>) {
                out.extend_from_slice(Self::PROPERTIES);
            }

            fn read_from(reader: &Reader<'_>, _gates: &()) -> Self {
                Self {
                    $( $group_field: <$group as Shape>::read_from(reader, &()), )*
                    $( $field: reader.$kind($property), )*
                }
            }
        }

        impl Ungated for $name {}

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

pub(crate) use {kind_type, pattern_set, property_set, uia_shape};
