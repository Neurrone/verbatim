//! An element found again by its path: from an element the caller has, one
//! raw-view level down at a time, each level's child the one whose runtime
//! id is the path's next. The outpost finds its own copy of a focus's
//! element this way, from the nearest ancestor with a window of its own,
//! by the runtime ids of the chain the focus listener read from the
//! event's own element (`docs/design/focus-pipeline.md`, section 12.5):
//! a UIA element cannot be handed from the process that received the event
//! to another. A remote program walks the whole path in one round trip;
//! the classic implementation behind the same signature takes one call per
//! child it looks at. [`element_by_path`] is the one function callers use.

use std::time::Instant;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_PROPERTY_ID, UIA_RuntimeIdPropertyId,
};

use verbatim_uia::{ElementExt as _, Uia, WalkerExt};

use crate::builder::Builder;
use crate::caret::gone_or_timed_out;
use crate::error::Error;
use crate::focus::{Path, RemoteCache, runtime_id_key};
use crate::opcode::NavigationDirection;

/// What [`element_by_path`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct PathQuery<'a> {
    /// Where the path starts: an element the caller has, such as a window's
    /// element from its handle.
    pub root: &'a IUIAutomationElement,
    /// The runtime ids of the elements below `root`, outermost first, the
    /// last the element wanted. Empty for `root` itself.
    pub path: &'a [Vec<i32>],
    /// The properties cached on the element found:
    /// [`verbatim_uia::cached_properties`] for the snapshot code.
    pub properties: &'a [UIA_PROPERTY_ID],
    /// When the classic walk gives up between its calls, answering that
    /// nothing was found. The remote program is one call and does not
    /// check it.
    pub deadline: Option<Instant>,
}

/// The signature both implementations share: the element at the end of the
/// path, with the query's properties cached, or `None` when a level has no
/// child with the path's runtime id.
pub type ElementByPathFn = fn(&Uia, &PathQuery<'_>) -> Result<Option<IUIAutomationElement>, Error>;

/// The element at the end of a path, the one function call sites use: the
/// remote program when `remote` is true, falling back to the classic walk
/// for this call when it fails (except for an element whose provider has
/// gone or did not answer in time, which the walk would meet too), and the
/// classic walk alone when `remote` is false. Says which path answered.
///
/// # Errors
///
/// The classic walk's [`Error`] when it ran and failed, or the program's
/// when the provider has gone or did not answer in time.
pub fn element_by_path(
    uia: &Uia,
    query: &PathQuery<'_>,
    remote: bool,
) -> Result<(Option<IUIAutomationElement>, Path), Error> {
    if !remote {
        return element_by_path_classic(uia, query).map(|found| (found, Path::Classic));
    }
    match element_by_path_remote(uia, query) {
        Ok(found) => Ok((found, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => {
            element_by_path_classic(uia, query).map(|found| (found, Path::Fallback(error)))
        }
    }
}

/// The element at the end of a path in one round trip: for each runtime id
/// of the path, the current element's raw-view children are looked at in
/// order until one has it, and the walk goes down to that child; a level
/// with no such child ends the walk with nothing found. The element found
/// has its cache filled inside the provider. A level with more children
/// than one run can look at stops the run at the platform's instruction
/// limit, and [`element_by_path`] answers that call classically.
///
/// `uia` is unused; it is in the signature so that this and
/// [`element_by_path_classic`] are interchangeable ([`ElementByPathFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program; [`Error::Import`] means the root
/// is served by a client-side proxy.
pub fn element_by_path_remote(
    _uia: &Uia,
    query: &PathQuery<'_>,
) -> Result<Option<IUIAutomationElement>, Error> {
    let mut b = Builder::new();
    let root = b.import_element(query.root);
    let cache = RemoteCache::new(&mut b, query.properties);
    let found = b.new_null_element();
    let found = b.add_to_results(found);
    let current = b.new_null_element();
    b.set(current, root);
    let lost = b.new_bool(false);
    let yes = b.bool(true);
    for runtime_id in query.path {
        let target = b.string(&runtime_id_key(runtime_id));
        let on_path = b.not(lost);
        b.if_(on_path, |b| {
            let child = b.navigate(current, NavigationDirection::FirstChild);
            b.while_(
                |b| {
                    let null = b.is_null(child);
                    b.not(null)
                },
                |b| {
                    let key = b.property(child, UIA_RuntimeIdPropertyId.0);
                    let key = b.stringify(key);
                    let matched = b.equal(key, target);
                    b.if_(matched, Builder::break_loop);
                    let next = b.navigate(child, NavigationDirection::NextSibling);
                    b.set(child, next);
                },
            );
            let none = b.is_null(child);
            b.if_else(none, |b| b.set(lost, yes), |b| b.set(current, child));
        });
    }
    let on_path = b.not(lost);
    b.if_(on_path, |b| {
        cache.populate(b, current);
        b.set(found, current);
    });
    let outcome = b.finish().execute()?;
    outcome.get(found)
}

/// The element at the end of a path the classic way, the fallback and the
/// reference: for each runtime id of the path, the current element's first
/// raw-view child and then its next siblings, each one call
/// (`GetFirstChildElementBuildCache` and `GetNextSiblingElementBuildCache`),
/// until one has it. Stops with nothing found at a level with no such
/// child, and at the query's deadline, checked between calls.
///
/// # Errors
///
/// [`Error::Uia`] when the cache request or the tree walker cannot be made,
/// or a step fails for any reason other than reaching no element.
pub fn element_by_path_classic(
    uia: &Uia,
    query: &PathQuery<'_>,
) -> Result<Option<IUIAutomationElement>, Error> {
    let cache = uia.cache_request(query.properties)?;
    let walker = uia.raw_view_walker()?;
    let out_of_time = || {
        query
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    };
    // A step that reaches no element comes back as an error with no
    // failure code (`WalkerExt`).
    let step = |found: windows::core::Result<IUIAutomationElement>| match found {
        Ok(element) => Ok(Some(element)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(Error::Uia(error)),
    };
    let mut current = query.root.build_updated_cache(&cache)?;
    for runtime_id in query.path {
        let mut child = step(walker.first_child(&current, &cache))?;
        loop {
            if out_of_time() {
                return Ok(None);
            }
            let Some(element) = child else {
                return Ok(None);
            };
            if verbatim_uia::runtime_id(&element) == *runtime_id {
                current = element;
                break;
            }
            child = step(walker.next_sibling(&element, &cache))?;
        }
    }
    Ok(Some(current))
}
