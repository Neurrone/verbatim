//! A selection in an element the focus controls: NVDA's base
//! `event_selection` (`source/NVDAObjects/__init__.py`) reads the focus's
//! `controllerFor` live for each selection event and speaks the selected
//! object when it is inside one of the controlled objects, as a search
//! box's suggestions are spoken while the focus stays in the box. A remote
//! program reads the relation and searches each controlled element's
//! subtree for the selected element in one round trip; the classic
//! implementation behind the same signature reads the relation and
//! searches with `FindFirstBuildCache`, one call each.
//! [`controlled_selection`] is the one function callers use.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_ControllerForPropertyId, UIA_PROPERTY_ID, UIA_RuntimeIdPropertyId,
};

use verbatim_uia::Uia;

use crate::builder::{Builder, kind};
use crate::caret::gone_or_timed_out;
use crate::error::Error;
use crate::focus::{Path, RemoteCache, runtime_id_key};
use crate::instruction::TypeTest;
use crate::opcode::{Comparison, NavigationDirection};

/// What [`controlled_selection`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct ControlledQuery<'a> {
    /// The focus, whose `ControllerFor` relation is read live.
    pub focused: &'a IUIAutomationElement,
    /// The runtime id of the element a selection event named.
    pub selected: &'a [i32],
    /// The properties cached on the selected element when it is found:
    /// [`verbatim_uia::cached_properties`] for the snapshot code.
    pub properties: &'a [UIA_PROPERTY_ID],
}

/// The signature both implementations share: the selected element, with
/// the query's properties cached, when it is a descendant of an element
/// the focus controls; `None` when the focus controls nothing or the
/// element is in none of what it controls.
pub type ControlledSelectionFn =
    fn(&Uia, &ControlledQuery<'_>) -> Result<Option<IUIAutomationElement>, Error>;

/// The controlled selection, the one function call sites use: the remote
/// program when `remote` is true, falling back to the classic calls for
/// this call when it fails (except for an element whose provider has gone
/// or did not answer in time, which they would meet too), and the classic
/// calls alone when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic implementation's [`Error`] when it ran and failed, or the
/// program's when the provider has gone or did not answer in time.
pub fn controlled_selection(
    uia: &Uia,
    query: &ControlledQuery<'_>,
    remote: bool,
) -> Result<(Option<IUIAutomationElement>, Path), Error> {
    if !remote {
        return controlled_selection_classic(uia, query).map(|found| (found, Path::Classic));
    }
    match controlled_selection_remote(uia, query) {
        Ok(found) => Ok((found, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => {
            controlled_selection_classic(uia, query).map(|found| (found, Path::Fallback(error)))
        }
    }
}

/// The controlled selection in one round trip: the focus's
/// `ControllerFor` read, then each controlled element's descendants walked
/// in the raw view, depth first, until one has the selected element's
/// runtime id, whose cache is then filled inside the provider. The
/// controlled element itself is not its own descendant. A subtree too
/// large for one run stops it at the platform's instruction limit, and
/// [`controlled_selection`] answers that call classically.
///
/// `uia` is unused; it is in the signature so that this and
/// [`controlled_selection_classic`] are interchangeable
/// ([`ControlledSelectionFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn controlled_selection_remote(
    _uia: &Uia,
    query: &ControlledQuery<'_>,
) -> Result<Option<IUIAutomationElement>, Error> {
    if query.selected.is_empty() {
        return Ok(None);
    }
    let mut b = Builder::new();
    let focused = b.import_element(query.focused);
    let cache = RemoteCache::new(&mut b, query.properties);
    let target = b.string(&runtime_id_key(query.selected));
    let found = b.new_null_element();
    let found = b.add_to_results(found);
    let relation = b.property(focused, UIA_ControllerForPropertyId.0);
    let is_array = b.is(TypeTest::Array, relation);
    b.if_(is_array, |b| {
        let controlled = relation.assume::<kind::Array>();
        let size = b.array_size(controlled);
        let index = b.new_uint(0);
        let one = b.uint(1);
        let zero = b.int(0);
        let step = b.int(1);
        b.while_(
            |b| {
                let more = b.compare(index, size, Comparison::LessThan);
                let none = b.is_null(found);
                b.and(more, none)
            },
            |b| {
                let root = b.array_get_at(controlled, index).assume::<kind::Element>();
                b.add_assign(index, one);
                // How far below the controlled element the walk is: back
                // at 0, its subtree is done.
                let depth = b.new_int(1);
                let current = b.navigate(root, NavigationDirection::FirstChild);
                b.while_(
                    |b| {
                        let null = b.is_null(current);
                        b.not(null)
                    },
                    |b| {
                        let runtime_id = b.property(current, UIA_RuntimeIdPropertyId.0);
                        let key = b.stringify(runtime_id);
                        let matched = b.equal(key, target);
                        b.if_(matched, |b| {
                            cache.populate(b, current);
                            b.set(found, current);
                            b.break_loop();
                        });
                        let child = b.navigate(current, NavigationDirection::FirstChild);
                        let has_child = b.is_null(child);
                        let has_child = b.not(has_child);
                        b.if_(has_child, |b| {
                            b.set(current, child);
                            b.add_assign(depth, step);
                            b.continue_loop();
                        });
                        // No child: the next sibling of this element or of
                        // the nearest ancestor below the controlled element
                        // that has one.
                        let next = b.navigate(current, NavigationDirection::NextSibling);
                        b.while_(
                            |b| {
                                let null = b.is_null(next);
                                let inside = b.compare(depth, zero, Comparison::GreaterThan);
                                b.and(null, inside)
                            },
                            |b| {
                                let parent = b.navigate(current, NavigationDirection::Parent);
                                b.set(current, parent);
                                b.subtract_assign(depth, step);
                                let above = b.compare(depth, zero, Comparison::GreaterThan);
                                b.if_(above, |b| {
                                    let sibling =
                                        b.navigate(current, NavigationDirection::NextSibling);
                                    b.set(next, sibling);
                                });
                            },
                        );
                        b.set(current, next);
                    },
                );
            },
        );
    });
    let outcome = b.finish().execute()?;
    outcome.get(found)
}

/// The controlled selection the classic way, the fallback and the
/// reference: the focus's `ControllerFor` read, then
/// [`Uia::controlled_descendant`]'s search under each controlled element,
/// one call each.
///
/// # Errors
///
/// [`Error::Uia`] when the cache request cannot be made, the relation
/// cannot be read, or a search fails.
pub fn controlled_selection_classic(
    uia: &Uia,
    query: &ControlledQuery<'_>,
) -> Result<Option<IUIAutomationElement>, Error> {
    let cache = uia.cache_request(query.properties)?;
    Ok(uia.controlled_descendant(query.focused, query.selected, &cache)?)
}
