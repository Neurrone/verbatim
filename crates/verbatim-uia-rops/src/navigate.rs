//! One object-navigation step (roadmap M3, `docs/crates/verbatim-outpost.md`):
//! the neighbor of an element in the raw view, with Verbatim's cached
//! property set, and the element's nearest window, which the outpost needs
//! to correct the neighbor's backend. A remote program takes the step and
//! finds the window in one round trip; the classic implementation behind
//! the same signature finds the window (`NormalizeElementBuildCache`) and
//! takes the step with a tree walker built with the cache, one call each.
//! [`navigation_step`] is the one function callers use.
//!
//! A program's walk ends at the process's top-level window, whose parent
//! and siblings it reports as none, where a tree walker goes on to the
//! desktop and other applications' windows; so a caller takes a step from
//! a top-level window classically.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_NativeWindowHandlePropertyId, UIA_PROPERTY_ID,
};

use verbatim_uia::map::cached_native_window_handle;
use verbatim_uia::{Uia, WalkerExt};

use crate::builder::{Builder, kind};
use crate::caret::gone_or_timed_out;
use crate::error::Error;
use crate::focus::{Path, RemoteCache};
use crate::instruction::TypeTest;
use crate::opcode::NavigationDirection;

/// What [`navigation_step`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct StepQuery<'a> {
    /// The element to step from, with the base cache filled (its native
    /// window handle is read from it).
    pub element: &'a IUIAutomationElement,
    /// Which neighbor: parent, next or previous sibling, or first child.
    pub direction: NavigationDirection,
    /// The properties cached on the neighbor:
    /// [`verbatim_uia::cached_properties`] for the snapshot code.
    pub properties: &'a [UIA_PROPERTY_ID],
}

/// The answer to a [`StepQuery`].
#[derive(Debug)]
pub struct Step {
    /// The neighbor, with the query's properties cached; `None` at a tree
    /// edge.
    pub neighbor: Option<IUIAutomationElement>,
    /// The native window handle of the element, or of its nearest raw-view
    /// ancestor that has one, as `verbatim_uia::nearest_window_handle`
    /// finds it; `None` when none has one.
    pub window: Option<isize>,
}

/// The signature both implementations share.
pub type NavigationStepFn = fn(&Uia, &StepQuery<'_>) -> Result<Step, Error>;

/// One navigation step, the one function call sites use: the remote
/// program when `remote` is true, falling back to the classic calls for
/// this call when it fails (except for an element whose provider has gone
/// or did not answer in time, which they would meet too), and the classic
/// calls alone when `remote` is false.
///
/// # Errors
///
/// The classic implementation's [`Error`] when it ran and failed, or the
/// program's when the provider has gone or did not answer in time.
pub fn navigation_step(
    uia: &Uia,
    query: &StepQuery<'_>,
    remote: bool,
) -> Result<(Step, Path), Error> {
    if !remote {
        return navigation_step_classic(uia, query).map(|step| (step, Path::Classic));
    }
    match navigation_step_remote(uia, query) {
        Ok(step) => Ok((step, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) => Err(error),
        Err(error) => navigation_step_classic(uia, query).map(|step| (step, Path::Fallback(error))),
    }
}

/// The step in one round trip: the element's own window from its cache,
/// else each raw-view ancestor's `NativeWindowHandle` until one has one,
/// then `Navigate` in the query's direction and the neighbor's cache filled
/// inside the provider.
///
/// `uia` is unused; it is in the signature so that this and
/// [`navigation_step_classic`] are interchangeable ([`NavigationStepFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program.
pub fn navigation_step_remote(_uia: &Uia, query: &StepQuery<'_>) -> Result<Step, Error> {
    let mut b = Builder::new();
    let element = b.import_element(query.element);
    let own = cached_native_window_handle(query.element);
    let window = b.new_int(i32::try_from(own).unwrap_or(0));
    let window = b.add_to_results(window);
    if own == 0 {
        let none = b.int(0);
        let current = b.navigate(element, NavigationDirection::Parent);
        b.while_(
            |b| {
                let unset = b.equal(window, none);
                let null = b.is_null(current);
                let more = b.not(null);
                b.and(unset, more)
            },
            |b| {
                let handle = b.property(current, UIA_NativeWindowHandlePropertyId.0);
                let is_int = b.is(TypeTest::Int, handle);
                b.if_(is_int, |b| b.set(window, handle.assume::<kind::Int>()));
                let parent = b.navigate(current, NavigationDirection::Parent);
                b.set(current, parent);
            },
        );
    }
    let cache = RemoteCache::new(&mut b, query.properties);
    let neighbor = b.navigate(element, query.direction);
    let neighbor = b.add_to_results(neighbor);
    let found = b.is_null(neighbor);
    let found = b.not(found);
    b.if_(found, |b| cache.populate(b, neighbor));
    let outcome = b.finish().execute()?;
    let window = outcome.get(window)?;
    Ok(Step {
        neighbor: outcome.get(neighbor)?,
        window: (window != 0).then_some(window as isize),
    })
}

/// The step the classic way, the fallback and the reference: the nearest
/// window (`verbatim_uia::nearest_window_handle`, one call), then one raw
/// view tree walker step built with the cache (one call). A step that
/// fails because the element is gone is an error; any other failure reads
/// as no neighbor, as `Uia::navigate` reads it.
///
/// # Errors
///
/// [`Error::Uia`] when the cache request or walker cannot be made, or the
/// element is gone.
pub fn navigation_step_classic(uia: &Uia, query: &StepQuery<'_>) -> Result<Step, Error> {
    let window = verbatim_uia::nearest_window_handle(query.element);
    let cache = uia.cache_request(query.properties)?;
    let walker = uia.raw_view_walker()?;
    let neighbor = match query.direction {
        NavigationDirection::Parent => walker.parent(query.element, &cache),
        NavigationDirection::NextSibling => walker.next_sibling(query.element, &cache),
        NavigationDirection::PreviousSibling => walker.previous_sibling(query.element, &cache),
        NavigationDirection::FirstChild => walker.first_child(query.element, &cache),
        NavigationDirection::LastChild => {
            return Err(Error::Uia(windows::core::Error::from(
                windows::Win32::Foundation::E_INVALIDARG,
            )));
        }
    };
    let neighbor = match neighbor {
        Ok(neighbor) => Some(neighbor),
        Err(error) if verbatim_uia::element_is_gone(&error) => return Err(Error::Uia(error)),
        Err(_) => None,
    };
    Ok(Step { neighbor, window })
}
