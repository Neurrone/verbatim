//! The focus ancestry: a focused element's ancestors with their cached
//! properties, read in one round trip by a remote program, or hop by hop
//! by the classic walk behind the same signature.

use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement, UIA_HasKeyboardFocusPropertyId,
    UIA_IsDataValidForFormPropertyId, UIA_ListControlTypeId, UIA_PROPERTY_ID,
    UIA_RuntimeIdPropertyId, UIA_SelectionSelectionPropertyId, UIA_TabControlTypeId,
};

use verbatim_uia::Uia;

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::instruction::TypeTest;
use crate::opcode::{Comparison, NavigationDirection};
use crate::operation::Value;

/// What [`focus_ancestry_remote`] and [`focus_ancestry_classic`] are asked.
#[derive(Clone, Copy)]
pub struct FocusQuery<'a> {
    /// The focused element, normally the focus event's sender, which
    /// arrives with its cache already filled.
    pub element: &'a IUIAutomationElement,
    /// Runtime ids of elements the caller already knows (the previous
    /// focus's ancestors): the walk stops at the first ancestor that has
    /// one.
    pub known: &'a [Vec<i32>],
    /// The most ancestors to return.
    pub depth_limit: u32,
    /// The properties cached on every returned element:
    /// [`verbatim_uia::CACHED_PROPERTIES`] for the snapshot code.
    pub properties: &'a [UIA_PROPERTY_ID],
}

/// The answer to a [`FocusQuery`].
#[derive(Debug)]
pub enum FocusAncestry {
    /// The element no longer has the keyboard focus, read live: the focus
    /// event is stale and nothing else was read.
    NotFocused,
    /// The element has the focus.
    Focused(Ancestry),
}

/// A focused element's ancestors and selected child.
#[derive(Debug)]
pub struct Ancestry {
    /// The raw-view ancestors, nearest first, each with the query's
    /// properties cached. The walk ends at the process's top-level window;
    /// the desktop root is never included.
    pub ancestors: Vec<IUIAutomationElement>,
    /// Which of the query's known runtime ids the last ancestor has, when
    /// the walk stopped at a known ancestor.
    pub met_known: Option<usize>,
    /// Whether the walk stopped at the depth limit with ancestors left.
    pub depth_limited: bool,
    /// For a list or tab control, the first selected child, with the
    /// query's properties cached: what `verbatim_uia::Uia::selected_child`
    /// reports.
    pub selected_child: Option<IUIAutomationElement>,
}

/// The signature both implementations share, so a caller can hold either.
pub type FocusAncestryFn = fn(&Uia, &FocusQuery<'_>) -> Result<FocusAncestry, Error>;

/// The string the remote program makes of a runtime id (its `Stringify`
/// instruction, verified on Windows 11 26200): the integers in decimal,
/// separated by commas, in square brackets, such as `[42,14681214,4,5]`.
/// The known runtime ids go into the program as these keys.
#[must_use]
pub fn runtime_id_key(runtime_id: &[i32]) -> String {
    let parts: Vec<String> = runtime_id.iter().map(i32::to_string).collect();
    format!("[{}]", parts.join(","))
}

/// Whether the focused element's selected child is reported with it, by
/// its cached control type (read live if it is not cached), as
/// `wants_selected_child` decides from the mapped role.
fn wants_selected_child(element: &IUIAutomationElement) -> bool {
    // SAFETY: `element` is a live element; both reads fail safely.
    let control_type =
        unsafe { element.CachedControlType() }.or_else(|_| unsafe { element.CurrentControlType() });
    control_type.is_ok_and(|control_type| {
        control_type == UIA_ListControlTypeId || control_type == UIA_TabControlTypeId
    })
}

/// The focus ancestry in one cross-process round trip: a program that
/// reads the element's `HasKeyboardFocus` live and stops if it is false,
/// then reads a list's or tab control's selected child, then walks
/// raw-view parents, filling each one's cache inside the provider, until
/// the top-level window, a known ancestor, or the depth limit.
///
/// `uia` is unused; it is in the signature so that this and
/// [`focus_ancestry_classic`] are interchangeable ([`FocusAncestryFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program; [`Error::Import`] means the
/// element is served by a client-side proxy, and the classic walk is the
/// answer for its window.
pub fn focus_ancestry_remote(_uia: &Uia, query: &FocusQuery<'_>) -> Result<FocusAncestry, Error> {
    let mut b = Builder::new();
    let element = b.import_element(query.element);

    let focused = b.property(element, UIA_HasKeyboardFocusPropertyId.0);
    let focused = b.add_to_results(focused.assume::<kind::Bool>());
    let not_focused = b.not(focused);
    b.if_(not_focused, Builder::halt);

    let cache = RemoteCache::new(&mut b, query.properties);

    let selected = b.new_null_element();
    let selected = b.add_to_results(selected);
    if wants_selected_child(query.element) {
        // The Selection pattern has no method instructions; its
        // `Selection` property is the same array of elements.
        let selection = b.property(element, UIA_SelectionSelectionPropertyId.0);
        let is_array = b.is(TypeTest::Array, selection);
        b.if_(is_array, |b| {
            let selection = selection.assume::<kind::Array>();
            let size = b.array_size(selection);
            let zero = b.uint(0);
            let any = b.compare(size, zero, Comparison::GreaterThan);
            b.if_(any, |b| {
                let first = b.array_get_at(selection, zero).assume::<kind::Element>();
                cache.populate(b, first);
                b.set(selected, first);
            });
        });
    }

    let known = b.new_string_map();
    for (index, runtime_id) in query.known.iter().enumerate() {
        let key = b.string(&runtime_id_key(runtime_id));
        let index = b.int(i32::try_from(index).unwrap_or(i32::MAX));
        b.map_insert(known, key, index);
    }

    let ancestors = b.new_array();
    let ancestors = b.add_to_results(ancestors);
    let met_known = b.new_int(-1);
    let met_known = b.add_to_results(met_known);
    let depth_limited = b.new_bool(false);
    let depth_limited = b.add_to_results(depth_limited);
    let count = b.new_int(0);
    let limit = b.int(i32::try_from(query.depth_limit).unwrap_or(i32::MAX));
    let one = b.int(1);
    let current = b.navigate(element, NavigationDirection::Parent);
    b.while_(
        |b| {
            let null = b.is_null(current);
            b.not(null)
        },
        |b| {
            let full = b.compare(count, limit, Comparison::GreaterThanOrEqual);
            b.if_(full, |b| {
                let yes = b.bool(true);
                b.set(depth_limited, yes);
                b.break_loop();
            });
            cache.populate(b, current);
            b.array_append(ancestors, current);
            b.add_assign(count, one);
            let runtime_id = b.property(current, UIA_RuntimeIdPropertyId.0);
            let key = b.stringify(runtime_id);
            let is_known = b.map_has_key(known, key);
            b.if_(is_known, |b| {
                let index = b.map_lookup(known, key).assume::<kind::Int>();
                b.set(met_known, index);
                b.break_loop();
            });
            let parent = b.navigate(current, NavigationDirection::Parent);
            b.set(current, parent);
        },
    );

    let outcome = b.finish().execute()?;
    if !outcome.get(focused)? {
        return Ok(FocusAncestry::NotFocused);
    }
    let ancestors = outcome
        .get(ancestors)?
        .into_iter()
        .filter_map(|value| match value {
            Value::Element(element) => Some(element),
            _ => None,
        })
        .collect();
    Ok(FocusAncestry::Focused(Ancestry {
        ancestors,
        met_known: usize::try_from(outcome.get(met_known)?).ok(),
        depth_limited: outcome.get(depth_limited)?,
        selected_child: outcome.get(selected)?,
    }))
}

/// The properties a snapshot reads ignoring defaults that no pattern's
/// availability gates: `IsDataValidForForm`, whose default of false would
/// otherwise read as an invalid entry. A remotely filled cache stores a
/// property's default where a locally built cache stores UIA's "not
/// supported" value, so the remote program leaves these out of an
/// element's cache when the element does not support them, which the
/// snapshot reads as it reads "not supported". (`ValueIsReadOnly` and
/// `RangeValueValue` are gated on their patterns instead.)
pub const LEFT_OUT_WHEN_UNSUPPORTED: &[UIA_PROPERTY_ID] = &[UIA_IsDataValidForFormPropertyId];

/// The remote program's cache requests: one per combination of the
/// [`LEFT_OUT_WHEN_UNSUPPORTED`] properties an element supports, since
/// `PopulateCache` replaces an element's cache rather than adding to it
/// (verified).
struct RemoteCache {
    /// The checked properties, as the program's constants.
    checked: Vec<Reg<kind::Int>>,
    /// The request for each combination, indexed by a bit mask of the
    /// checked properties supported.
    requests: Vec<Reg<kind::CacheRequest>>,
}

impl RemoteCache {
    fn new(b: &mut Builder, properties: &[UIA_PROPERTY_ID]) -> Self {
        let checked: Vec<UIA_PROPERTY_ID> = LEFT_OUT_WHEN_UNSUPPORTED
            .iter()
            .copied()
            .filter(|property| properties.contains(property))
            .collect();
        let requests = (0..1usize << checked.len())
            .map(|supported| {
                let request = b.new_cache_request();
                for property in properties {
                    let position = checked.iter().position(|checked| checked == property);
                    if position.is_none_or(|bit| supported & (1 << bit) != 0) {
                        b.cache_request_add_property(request, property.0);
                    }
                }
                request
            })
            .collect();
        let checked = checked.iter().map(|property| b.int(property.0)).collect();
        Self { checked, requests }
    }

    /// Emits the instructions that fill `element`'s cache.
    fn populate(&self, b: &mut Builder, element: Reg<kind::Element>) {
        if self.checked.is_empty() {
            b.populate_cache(element, self.requests[0]);
            return;
        }
        let supported = b.new_int(0);
        let ignore_default = b.bool(true);
        for (bit, &property) in self.checked.iter().enumerate() {
            let value = b.get_property_value(element, property, ignore_default);
            let unsupported = b.is(TypeTest::NotSupported, value);
            let is_supported = b.not(unsupported);
            b.if_(is_supported, |b| {
                let flag = b.int(1 << bit);
                b.add_assign(supported, flag);
            });
        }
        for (mask, &request) in self.requests.iter().enumerate() {
            let mask = b.int(i32::try_from(mask).unwrap_or(i32::MAX));
            let chosen = b.equal(supported, mask);
            b.if_(chosen, |b| b.populate_cache(element, request));
        }
    }
}

/// The focus ancestry the classic way, the fallback and the reference:
/// a live `HasKeyboardFocus` read, the selected child through the
/// `Selection` pattern ([`verbatim_uia::selected_element`], as
/// `Uia::selected_child` reads it), then one
/// `GetParentElementBuildCache` round trip per ancestor over the raw view,
/// the walk `Uia::ancestor_chain` makes. Stops where the remote program
/// stops: at a known ancestor, at the depth limit, or below the desktop
/// root.
///
/// # Errors
///
/// [`Error::Uia`] when the focus read, the cache request, or the tree
/// walker fails; a hop that finds no parent ends the walk.
pub fn focus_ancestry_classic(uia: &Uia, query: &FocusQuery<'_>) -> Result<FocusAncestry, Error> {
    // SAFETY: `query.element` is a live element; a live property read.
    if !unsafe { query.element.CurrentHasKeyboardFocus() }?.as_bool() {
        return Ok(FocusAncestry::NotFocused);
    }
    let client = uia.client();
    let cache = cache_request(client, query.properties)?;
    let selected_child = if wants_selected_child(query.element) {
        // SAFETY: `query.element` is live.
        unsafe { verbatim_uia::selected_element(query.element, &cache) }
    } else {
        None
    };
    // SAFETY: `client` is a live IUIAutomation; the root element is the
    // desktop, served in this process.
    let root = verbatim_uia::runtime_id(&unsafe { client.GetRootElement() }?);
    // SAFETY: as above.
    let walker = unsafe { client.RawViewWalker() }?;
    let mut ancestry = Ancestry {
        ancestors: Vec::new(),
        met_known: None,
        depth_limited: false,
        selected_child,
    };
    let mut current = query.element.clone();
    // SAFETY: `current` is the caller's live element or a parent just
    // built with `cache`; a failed hop is the root.
    while let Ok(parent) = unsafe { walker.GetParentElementBuildCache(&current, &cache) } {
        let runtime_id = verbatim_uia::runtime_id(&parent);
        if runtime_id == root {
            break;
        }
        if ancestry.ancestors.len() >= usize::try_from(query.depth_limit).unwrap_or(usize::MAX) {
            ancestry.depth_limited = true;
            break;
        }
        ancestry.ancestors.push(parent.clone());
        if let Some(index) = query.known.iter().position(|known| *known == runtime_id) {
            ancestry.met_known = Some(index);
            break;
        }
        current = parent;
    }
    Ok(FocusAncestry::Focused(ancestry))
}

/// A cache request for `properties`, built locally.
fn cache_request(
    client: &IUIAutomation,
    properties: &[UIA_PROPERTY_ID],
) -> windows::core::Result<IUIAutomationCacheRequest> {
    // SAFETY: `client` is a live IUIAutomation; adding a property takes a
    // plain id.
    unsafe {
        let request = client.CreateCacheRequest()?;
        for &property in properties {
            request.AddProperty(property)?;
        }
        Ok(request)
    }
}
