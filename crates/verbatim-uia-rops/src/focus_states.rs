//! A focus's states, read once its event registration is in, with the
//! children of the dialogs the focus enters, from which their text is
//! gathered (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in
//! this build"). NVDA's `event_gainFocus` registers for the focus's events
//! before it speaks it (`NVDAObjects/UIA/__init__.py` 1623 to 1626), and
//! gathers a dialog's text from its children inside the same focus event,
//! so both are read after the registration. A remote program reads the
//! focus's cache and each dialog's children in one round trip; the classic
//! implementation behind the same signature reads the focus and each
//! dialog's children in one call each. [`focus_states`] is the one function
//! callers use.

use verbatim_uia::{Cached, DialogText, Fill, Live, Node, Uia};

use crate::builder::Builder;
use crate::caret::gone_or_timed_out;
use crate::error::Error;
use crate::focus::{Path, RemoteCache, absent_record};
use crate::opcode::{Comparison, NavigationDirection};
use crate::operation::Value;

/// The most children one run of the program reads, across every dialog,
/// which keeps its worst case under the platform's limit on the
/// instructions one run may execute
/// (`crates/mockapp/tests/instruction_limit.rs`); a dialog whose children
/// do not fit is answered unread, for the caller to read classically.
pub const DIALOG_CHILDREN_PER_RUN: i32 = 60;

/// What [`focus_states`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct StatesQuery<'a> {
    /// The focus, whose cache is filled anew.
    pub focus: &'a Live,
    /// How the focus's cache is filled: as a [`Node`].
    pub fill: &'a Fill<Node>,
    /// The dialogs the focus enters whose text is to be gathered, in the
    /// caller's order.
    pub dialogs: &'a [Live],
    /// How each dialog child's cache is filled: as a [`DialogText`].
    pub children: &'a Fill<DialogText>,
}

/// What both implementations answer.
#[derive(Debug)]
pub struct States {
    /// The focus with its cache filled now; `None` when the classic read
    /// failed.
    pub focus: Option<Cached<Node>>,
    /// Each dialog's children, in the query's order, each filled as a
    /// [`DialogText`]: `None` for a dialog whose children the program did
    /// not read, because they did not fit in its run or their read failed,
    /// which the caller reads classically.
    pub children: Vec<Option<Vec<Cached<DialogText>>>>,
}

/// The signature both implementations share ([`States`]).
pub type FocusStatesFn = fn(&Uia, &StatesQuery<'_>) -> Result<States, Error>;

/// The focus's states with its dialogs' children, the one function call
/// sites use: the remote program when `remote` is true, falling back to the
/// classic calls for this call when it fails (except at UIA's transaction
/// timeout, which they would meet too), and the classic calls alone when
/// `remote` is false. Says which path answered.
///
/// # Errors
///
/// The program's [`Error`] when the provider did not answer in time.
pub fn focus_states(
    uia: &Uia,
    query: &StatesQuery<'_>,
    remote: bool,
) -> Result<(States, Path), Error> {
    if !remote {
        return focus_states_classic(uia, query).map(|states| (states, Path::Classic));
    }
    match focus_states_remote(uia, query) {
        Ok(states) => Ok((states, Path::Remote)),
        Err(error) if gone_or_timed_out(&error) && !is_gone(&error) => Err(error),
        Err(error) => {
            focus_states_classic(uia, query).map(|states| (states, Path::Fallback(error)))
        }
    }
}

/// Whether a failure says an element is gone.
fn is_gone(error: &Error) -> bool {
    use windows::Win32::UI::Accessibility::UIA_E_ELEMENTNOTAVAILABLE;
    error
        .hresult()
        .is_some_and(|code| code.0.cast_unsigned() == UIA_E_ELEMENTNOTAVAILABLE)
}

/// The focus's states and its dialogs' children in one round trip: the
/// focus's cache filled inside the provider, then each dialog's children in
/// the raw view, each filled inside the provider, up to
/// [`DIALOG_CHILDREN_PER_RUN`] in all. A dialog whose children fail to be
/// read, or do not all fit, is answered unread.
///
/// `uia` is unused; it is in the signature so that this and
/// [`focus_states_classic`] are interchangeable ([`FocusStatesFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program: a focus gone fails the run.
pub fn focus_states_remote(_uia: &Uia, query: &StatesQuery<'_>) -> Result<States, Error> {
    let mut b = Builder::new();
    let focus = b.import_element(query.focus.element());
    let dialogs: Vec<_> = query
        .dialogs
        .iter()
        .map(|dialog| b.import_element(dialog.element()))
        .collect();
    let node_cache = RemoteCache::new(&mut b, query.fill);
    let child_cache = RemoteCache::new(&mut b, query.children);
    let focus_absent = node_cache.populate(&mut b, focus);
    let focus_absent = b.add_to_results(focus_absent);
    let focus_out = b.new_null_element();
    let focus_out = b.add_to_results(focus_out);
    b.set(focus_out, focus);
    // Each child read, in order, with its dialog's place and its absent
    // record; and each dialog whose children were all read.
    let found = b.new_array();
    let found = b.add_to_results(found);
    let parents = b.new_array();
    let parents = b.add_to_results(parents);
    let absents = b.new_array();
    let absents = b.add_to_results(absents);
    let complete = b.new_array();
    let complete = b.add_to_results(complete);
    let read = b.new_int(0);
    let limit = b.int(DIALOG_CHILDREN_PER_RUN);
    let one = b.int(1);
    for (place, dialog) in dialogs.into_iter().enumerate() {
        let place = b.int(i32::try_from(place).unwrap_or(i32::MAX));
        b.try_catch(
            |b| {
                let child = b.navigate(dialog, NavigationDirection::FirstChild);
                b.while_(
                    |b| {
                        let none = b.is_null(child);
                        let some = b.not(none);
                        let room = b.compare(read, limit, Comparison::LessThan);
                        b.and(some, room)
                    },
                    |b| {
                        let absent = child_cache.populate(b, child);
                        b.array_append(found, child);
                        b.array_append(parents, place);
                        b.array_append(absents, absent);
                        b.add_assign(read, one);
                        let next = b.navigate(child, NavigationDirection::NextSibling);
                        b.set(child, next);
                    },
                );
                let none = b.is_null(child);
                b.if_(none, |b| b.array_append(complete, place));
            },
            // Its children read before the failure stay in the answer but
            // are not taken: the dialog is not complete.
            |_, _| {},
        );
    }
    let outcome = b.finish().execute()?;
    let ints = |values: Vec<Value>| -> Vec<i32> {
        values
            .into_iter()
            .map(|value| match value {
                Value::Int(value) => value,
                _ => -1,
            })
            .collect()
    };
    let focus_absent = absent_record(outcome.get(focus_absent)?);
    let focus = outcome
        .get(focus_out)?
        .map(|focus| query.fill.claim(focus, focus_absent));
    let found = outcome.get(found)?;
    let parents = ints(outcome.get(parents)?);
    let absents = ints(outcome.get(absents)?);
    let complete = ints(outcome.get(complete)?);
    let mut children: Vec<Option<Vec<Cached<DialogText>>>> = (0..query.dialogs.len())
        .map(|place| {
            complete
                .contains(&i32::try_from(place).unwrap_or(i32::MAX))
                .then(Vec::new)
        })
        .collect();
    for ((value, parent), absent) in found.into_iter().zip(parents).zip(absents) {
        if let (Value::Element(element), Some(Some(list))) = (
            value,
            usize::try_from(parent)
                .ok()
                .and_then(|parent| children.get_mut(parent)),
        ) {
            list.push(query.children.claim(element, absent_record(absent)));
        }
    }
    Ok(States { focus, children })
}

/// The focus's states and its dialogs' children the classic way, the
/// fallback and the reference: the focus's cache built anew, one call, and
/// each dialog's children, one call each (`verbatim_uia::Uia::children`);
/// a dialog whose children cannot be read has none, as the gathering's own
/// read answers it.
///
/// # Errors
///
/// None of its own: each read's failure is answered in [`States`].
pub fn focus_states_classic(uia: &Uia, query: &StatesQuery<'_>) -> Result<States, Error> {
    let focus = query.focus.refresh(query.fill.request()).ok();
    let children = query
        .dialogs
        .iter()
        .map(|dialog| {
            Some(
                uia.children(dialog, query.children.request())
                    .unwrap_or_default(),
            )
        })
        .collect();
    Ok(States { focus, children })
}
