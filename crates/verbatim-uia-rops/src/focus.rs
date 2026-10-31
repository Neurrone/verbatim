//! The focus ancestry: a focused element's ancestors with their cached
//! properties, read in one round trip by a remote program, or hop by hop
//! by the classic walk behind the same signature, and [`focus_ancestry`],
//! the one function callers use, which picks between them.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_ComboBoxControlTypeId, UIA_E_ELEMENTNOTAVAILABLE, UIA_E_TIMEOUT,
    UIA_HasKeyboardFocusPropertyId, UIA_IsValuePatternAvailablePropertyId,
    UIA_LegacyIAccessibleStatePropertyId, UIA_ListControlTypeId, UIA_MenuItemControlTypeId,
    UIA_NativeWindowHandlePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_RuntimeIdPropertyId,
    UIA_Selection2FirstSelectedItemPropertyId, UIA_SelectionSelectionPropertyId,
    UIA_TabControlTypeId,
};

use verbatim_uia::{
    Absent, Ancestor, Cached, Direction, ElementExt, Fill, FocusInput, LEFT_OUT_WHEN_UNSUPPORTED,
    Shape, Uia,
};

use verbatim_model::EarlierElement;

use crate::builder::{Builder, Reg, kind};
use crate::caret::gone_or_timed_out;
use crate::error::Error;
use crate::instruction::TypeTest;
use crate::opcode::{Comparison, NavigationDirection};
use crate::operation::Value;

/// What [`focus_ancestry`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct FocusQuery<'a> {
    /// The focused element, with its cache filled: the focus event's
    /// sender, or the focused element read for it, read as what the query
    /// reads of it, which every shape it is read as covers.
    pub element: &'a Cached<FocusInput>,
    /// Runtime ids of elements the caller already knows (the previous
    /// focus's ancestors): the walk stops at the first ancestor that has
    /// one.
    pub known: &'a [Vec<i32>],
    /// The element the caller already holds under the focused element's
    /// runtime id, when it holds one: whether it still has the keyboard
    /// focus is read live too ([`Ancestry::previous_focused`]). An
    /// application can give a dead element's runtime id to a new one, and
    /// an element the caller holds that no longer has the focus, or cannot
    /// be read, is not the focused element whatever its id says.
    pub previous: Option<&'a IUIAutomationElement>,
    /// The most ancestors to return.
    pub depth_limit: u32,
    /// How each ancestor's cache is filled: as an [`Ancestor`], with the
    /// gates its request was built with, which leave out the element's
    /// place, as every read of many elements does (`verbatim_uia::Place`).
    pub fill: &'a Fill<Ancestor>,
    /// How the element's own cache ([`FocusQuery::refresh`]) and its
    /// selected child's are filled: as [`FocusQuery::fill`] fills an
    /// ancestor's, and, since each is read alone, with its place.
    pub element_fill: &'a Fill<Ancestor>,
    /// Whether the classic walk's time is up, checked before each of its
    /// calls after the first: it then makes no more, reporting what the
    /// calls skipped would have read as unknown and the ancestry as cut
    /// short ([`Ancestry::out_of_time`]). The remote program is one call
    /// and does not check it.
    pub out_of_time: Option<&'a dyn Fn() -> bool>,
    /// Whether the element must have the keyboard focus, read live, for
    /// anything else to be read ([`FocusAncestry::NotFocused`]), as NVDA's
    /// `shouldAllowUIAFocusEvent` judges a focus event's sender. False
    /// reads the ancestry whatever the element's focus.
    pub require_focus: bool,
    /// Whether the element's own cache is filled afresh by
    /// [`FocusQuery::fill`] in the same round trip
    /// ([`Ancestry::element`]): its value, states, and details as they are
    /// when the focus is read, which the focus listener reports a focus
    /// with, as NVDA reads those afresh when it handles a focus event.
    pub refresh: bool,
    /// The element's nearest window, its own or its nearest ancestor's,
    /// when the caller has already found it, as the focus listener has for
    /// NVDA's native check: the classic walk then makes no call to find it
    /// ([`Ancestry::window`]), and starts from it when it judges the
    /// process boundary. `None` leaves the walk to find it. The remote
    /// program finds it inside its one round trip and does not read this.
    pub nearest_window: Option<isize>,
    /// Told the name of each of the classic walk's calls as it begins,
    /// such as `"a step to the parent"`, so the caller can time them; a
    /// call ends where the next begins, or where the walk returns. The
    /// selected child's read, one call or a few, is told as one. `None`
    /// tells nobody. The remote program is one call and does not tell it.
    pub before_call: Option<&'a dyn Fn(&'static str)>,
    /// Whether the element has the `Value` pattern, when the caller's
    /// cache of it already says, as a node's does: the classic walk then
    /// asks nothing of a combo box to tell whether its first selected item
    /// is its value. `None` has the walk take it from the element read
    /// afresh, or ask it live without one. The remote program asks it
    /// inside the provider and does not read this.
    pub value_available: Option<bool>,
    /// Whether a window is read as a plain window, with no call: a ghost's
    /// or a hung application's, by NVDA's rule
    /// (`NVDAObjects/window/__init__.py` 85 to 97). The classic walk asks
    /// it of the window of each parent a step reaches, other than the
    /// nearest window, and stops there without keeping the parent
    /// ([`Ancestry::plain_window`]), as NVDA makes the one step and then
    /// reads that window as a plain window and goes on by the window
    /// hierarchy. Local calls only, never made into the application.
    /// `None` reads no window as plain. The remote program cannot ask it
    /// between its steps and does not read this: a caller with such a
    /// window above the focus runs the classic walk instead.
    pub plain: Option<&'a dyn Fn(isize) -> bool>,
    /// Told what the classic walk reads as it reads it ([`Progress`]), so
    /// a caller can share a walk's progress with a thread that publishes
    /// it should one of the walk's calls not return in time. `None` tells
    /// nobody. The remote program is one call and does not tell it.
    pub progress: Option<&'a dyn Fn(Progress<'_>)>,
}

/// What the classic walk has just read ([`FocusQuery::progress`]), told
/// once each, in the order the walk reads them.
#[derive(Clone, Copy)]
pub enum Progress<'a> {
    /// Whether [`FocusQuery::previous`] has the keyboard focus, read live
    /// ([`Ancestry::previous_focused`]).
    PreviousFocused(EarlierElement),
    /// The element with its cache filled afresh ([`Ancestry::element`]).
    Element(&'a Cached<Ancestor>),
    /// The first selected child ([`Ancestry::selected_child`]).
    SelectedChild(&'a Cached<Ancestor>),
    /// The next ancestor kept, nearest first ([`Ancestry::ancestors`]).
    Ancestor(&'a Cached<Ancestor>),
}

/// The answer to a [`FocusQuery`].
#[derive(Debug)]
pub enum FocusAncestry {
    /// The element no longer has the keyboard focus, read live, and the
    /// query required it: the focus event is stale and nothing else was
    /// read.
    NotFocused,
    /// The element has the focus.
    Focused(Ancestry),
}

/// A focused element's ancestors, selected child, and window.
#[derive(Debug)]
pub struct Ancestry {
    /// The raw-view ancestors, nearest first, each filled by the query's
    /// fill. The walk ends at the process's top-level window, or at the
    /// window below the process boundary; the desktop root is never
    /// included.
    pub ancestors: Vec<Cached<Ancestor>>,
    /// Which of the query's known runtime ids the last ancestor has, when
    /// the walk stopped at a known ancestor.
    pub met_known: Option<usize>,
    /// Whether the walk stopped at the depth limit with ancestors left.
    pub depth_limited: bool,
    /// Whether the classic walk stopped when the query's time was up, with
    /// ancestors left. Never set by the remote program.
    pub out_of_time: bool,
    /// How the walk's step to a parent failed below the top, which ends
    /// the walk with the ancestors unknown, as an out-of-time walk does
    /// ([`Self::ancestors_unknown`]); `None` when no step failed. Only
    /// reaching the top ends a walk as known, as NVDA reads the window
    /// hierarchy instead when its parent read fails
    /// (`NVDAObjects/UIA/__init__.py` 2142 to 2145). For the classic walk
    /// the top is the desktop root, a top-level window's parent, and any
    /// step below it that fails or finds no parent sets this. Inside the
    /// remote program a top-level window's parent is null, so its walk
    /// ends at any null parent, and sets this, as
    /// [`FailedHop::NoParent`], unless the last element read is the own
    /// element (runtime id `[42, handle]`) of a top-level window or of a
    /// window below the process boundary, or reports a top-level window's
    /// handle: any other window handle reported below it, as a fragment
    /// element can report one it does not host, does not make the walk
    /// complete.
    pub failed_hop: Option<FailedHop>,
    /// The window a step reached that reads as a plain window
    /// ([`FocusQuery::plain`]): the walk stopped there, the parent in it
    /// not kept, and the chain goes on from that window as a plain window.
    /// Set by the classic walk only.
    pub plain_window: Option<isize>,
    /// For a list or tab control, the first selected child, filled by the
    /// query's element fill: what `verbatim_uia::Uia::selected_node` reports; for a
    /// combo box without the `Value` pattern, its first selected item, its
    /// value (`verbatim_uia::map::value_from_selection`). Optional: `None`
    /// too when its read failed, as on an item gone, or, in the classic
    /// walk, timed out or ran out of the query's time, the rest of the
    /// ancestry read without it.
    pub selected_child: Option<Cached<Ancestor>>,
    /// The native window handle of the element, or of its nearest raw-view
    /// ancestor that has one, as `verbatim_uia::nearest_window_handle`
    /// finds it (NVDA's `getNearestWindowHandle`), looking past where the
    /// ancestor walk stopped when it has to. `None` when nothing up to the
    /// top has one, or the classic walk ran out of time first.
    pub window: Option<isize>,
    /// Whether [`FocusQuery::previous`] has the keyboard focus, read live:
    /// focused, not focused, or gone when its read failed other than by
    /// time (in a remote program, when the element's import failed the
    /// run; one whose read answers no boolean there counts as not
    /// focused); `None` when the query gave no previous element.
    pub previous_focused: Option<EarlierElement>,
    /// With [`FocusQuery::refresh`], the element itself with its cache
    /// filled afresh; `None` without it.
    pub element: Option<Cached<Ancestor>>,
    /// For a menu item, by its cached control type, its legacy MSAA state
    /// bits (`LegacyIAccessibleState`), read by the remote program in its
    /// one round trip, where a menu item that no pattern makes checkable
    /// may say it is checked (`verbatim_uia::map::wants_legacy_checked_state`):
    /// `Some(None)` when the element does not support the property. `None`
    /// when it was not read: for any other element, and always from the
    /// classic walk, which leaves the read to its caller, one live call
    /// made only when the caller needs it.
    pub legacy_state: Option<Option<i32>>,
}

impl Ancestry {
    /// Whether the walk did not finish, so the ancestors past those read
    /// are unknown: it ran out of time ([`Self::out_of_time`]) or a step
    /// to a parent failed ([`Self::failed_hop`]).
    #[must_use]
    pub fn ancestors_unknown(&self) -> bool {
        self.out_of_time || self.failed_hop.is_some()
    }
}

/// How a walk's step to a parent failed ([`Ancestry::failed_hop`]).
pub use verbatim_uia::FailedHop;

/// The signature both implementations share, so a caller can hold either.
pub type FocusAncestryFn = fn(&Uia, &FocusQuery<'_>) -> Result<FocusAncestry, Error>;

/// Which implementation answered a [`focus_ancestry`] call.
#[derive(Debug)]
pub enum Path {
    /// The remote program, in one cross-process round trip.
    Remote,
    /// The classic walk, as the caller asked.
    Classic,
    /// The classic walk, because the remote program failed with this
    /// error. [`Error::Import`] means the element is served by a
    /// client-side proxy, which will not change for its window.
    Fallback(Error),
}

impl Path {
    /// The path's name for a log line: `remote`, `classic`, or `fallback`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Classic => "classic",
            Self::Fallback(_) => "fallback",
        }
    }
}

/// The focus ancestry, the one function call sites use: the remote program
/// when `remote` is true, falling back to the classic walk for this call
/// when remote operations are unavailable or the program fails, and the
/// classic walk alone when `remote` is false. A run that fails because an
/// element is gone, when the query holds a previous element, is run once
/// more without it, and the previous element answered as gone when that
/// run succeeds. Says which path answered, and
/// for a fallback, why, so the caller can stop trying the remote program
/// for a window whose import failed.
///
/// A run that fails because the provider did not answer within UIA's
/// transaction timeout, or because its element is gone, is not a program
/// failure: the classic walk would fail the same way, after waiting on a
/// stalled application a second time, so the run's error is returned.
///
/// NVDA makes each call site choose instead, asking `remote.isSupported()`
/// before building a program; here the choice and the fallback live in one
/// place, and the caller decides only whether to try.
///
/// # Errors
///
/// The classic walk's [`Error`], when it ran and failed; the remote
/// program's, when it timed out or its element is gone. Any other failed
/// remote program is never returned, only reported in [`Path::Fallback`].
pub fn focus_ancestry(
    uia: &Uia,
    query: &FocusQuery<'_>,
    remote: bool,
) -> Result<(FocusAncestry, Path), Error> {
    if !remote {
        return focus_ancestry_classic(uia, query).map(|answer| (answer, Path::Classic));
    }
    let error = match focus_ancestry_remote(uia, query) {
        Ok(answer) => return Ok((answer, Path::Remote)),
        Err(error) => error,
    };
    // UIA fails a whole run, before its first instruction, when an element
    // it imports is gone (verified against mockapp: `ExecutionFailure`,
    // `UIA_E_ELEMENTNOTAVAILABLE`, no failing instruction), so the program
    // cannot catch the held element's read itself. Run again without it: if
    // that succeeds, the held element was the one gone, and has no focus.
    if query.previous.is_some() && is_element_gone(&error) {
        let without = FocusQuery {
            previous: None,
            ..*query
        };
        if let Ok(answer) = focus_ancestry_remote(uia, &without) {
            let answer = match answer {
                FocusAncestry::Focused(ancestry) => FocusAncestry::Focused(Ancestry {
                    previous_focused: Some(EarlierElement::Gone),
                    ..ancestry
                }),
                FocusAncestry::NotFocused => FocusAncestry::NotFocused,
            };
            return Ok((answer, Path::Remote));
        }
    }
    if gone_or_timed_out(&error) {
        return Err(error);
    }
    focus_ancestry_classic(uia, query).map(|answer| (answer, Path::Fallback(error)))
}

/// Whether a classic call failed because the provider did not answer
/// within UIA's transaction timeout.
fn timed_out(error: &windows::core::Error) -> bool {
    error.code().0.cast_unsigned() == UIA_E_TIMEOUT
}

/// Whether a run failed because an element it imported is gone.
fn is_element_gone(error: &Error) -> bool {
    error
        .hresult()
        .is_some_and(|code| code.0.cast_unsigned() == UIA_E_ELEMENTNOTAVAILABLE)
}

/// The string the remote program makes of a runtime id (its `Stringify`
/// instruction, verified on Windows 11 26200): the integers in decimal,
/// separated by commas, in square brackets, such as `[42,14681214,4,5]`.
/// The known runtime ids go into the program as these keys.
#[must_use]
pub fn runtime_id_key(runtime_id: &[i32]) -> String {
    let parts: Vec<String> = runtime_id.iter().map(i32::to_string).collect();
    format!("[{}]", parts.join(","))
}

/// Whether the focused element's selected child is read with it, by its
/// cached control type: a list's or a tab control's, reported with it, as
/// `wants_selected_child` decides from the mapped role, and a combo box's,
/// its value when it has no `Value` pattern
/// (`verbatim_uia::map::value_from_selection`), which is asked of the
/// element where the selection is read. Every shape the query's element is
/// read as holds the control type, so it is never read live.
fn wants_selected_child(element: &FocusInput) -> bool {
    element.control_type.is_some_and(|control_type| {
        control_type == UIA_ListControlTypeId.0
            || control_type == UIA_TabControlTypeId.0
            || control_type == UIA_ComboBoxControlTypeId.0
    })
}

/// Whether the classic walk reads the focused element's selected child
/// ([`wants_selected_child`]): a combo box's only without the `Value`
/// pattern, whose value is otherwise its own, as the caller's cache says
/// when it holds it (`cached`, [`FocusQuery::value_available`]), as the
/// element read afresh, `fresh`, says when there is one, and as `live`
/// answers otherwise, one call, where the remote program asks it inside
/// the provider.
fn reads_selected_child(
    element: &FocusInput,
    (live, fresh, cached): (&verbatim_uia::Live, Option<&Cached<Ancestor>>, Option<bool>),
    before_call: &dyn Fn(&'static str),
) -> windows::core::Result<bool> {
    if element.control_type != Some(UIA_ComboBoxControlTypeId.0) {
        return Ok(wants_selected_child(element));
    }
    if let Some(value_available) = cached {
        return Ok(!value_available);
    }
    if let Some(fresh) = fresh {
        return Ok(!fresh.read().node.states.value_available);
    }
    before_call("the combo box's Value pattern");
    Ok(!live
        .element()
        .current_bool(UIA_IsValuePatternAvailablePropertyId)?)
}

/// A read of the selected child as optional: one that timed out or found
/// its container or item gone answers `None`, as nothing read; any other
/// error, a local one such as a cache request that could not be made, is
/// returned.
fn optional<T>(read: windows::core::Result<Option<T>>) -> Result<Option<T>, Error> {
    match read {
        Ok(found) => Ok(found),
        Err(error) if verbatim_uia::timed_out(&error) || verbatim_uia::element_is_gone(&error) => {
            Ok(None)
        }
        Err(error) => Err(Error::Uia(error)),
    }
}

/// Whether the focused element's legacy MSAA state is read with it, by its
/// cached control type: a menu item, the one kind of element whose checked
/// state may be only there (`verbatim_uia::map::wants_legacy_checked_state`).
fn wants_legacy_state(element: &FocusInput) -> bool {
    element.control_type == Some(UIA_MenuItemControlTypeId.0)
}

/// A native window handle, `None` for 0, an element that is not a window.
fn own_window(hwnd: isize) -> Option<isize> {
    Some(hwnd).filter(|&hwnd| hwnd != 0)
}

/// Whether the classic walk stops at an element below the process
/// boundary: one whose window, its own `hwnd` or, for the nearest window's
/// own element held without its handle, `nearest`, has a parent window or
/// visible owner that another process serves. Local calls only.
fn below_the_boundary(hwnd: isize, runtime_id: &[i32], nearest: Option<isize>) -> bool {
    let window = if hwnd != 0 {
        hwnd
    } else {
        nearest
            .filter(|&nearest| is_window_element(runtime_id, nearest))
            .unwrap_or(0)
    };
    window != 0 && verbatim_uia::parent_across_process_boundary(window).is_some()
}

/// Whether a step up reached a window, `hwnd`, that another process than
/// the nearest window's serves. Local calls only.
fn stepped_across(hwnd: isize, nearest: Option<isize>) -> bool {
    nearest.is_some_and(|nearest| {
        hwnd != 0
            && hwnd != nearest
            && verbatim_uia::provider_process(hwnd) != verbatim_uia::provider_process(nearest)
    })
}

/// Whether the element whose runtime id is `runtime_id` is window `hwnd`'s
/// own element: UI Automation gives a window's element the runtime id
/// `[42, hwnd]`, the handle's low 32 bits.
fn is_window_element(runtime_id: &[i32], hwnd: isize) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a runtime id holds a window handle's low 32 bits"
    )]
    let handle = hwnd as i32;
    runtime_id == [42, handle]
}

/// Whether the remote program's walk, ending at a null parent at the
/// element whose runtime id is `runtime_id` and whose cached
/// `NativeWindowHandle` is `hwnd`, reached the top: that element's window
/// is a top-level window or one below the process boundary, whose parent
/// the program cannot reach, the rule the classic walk stops by
/// ([`below_the_boundary`]). Its window is the one its runtime id names,
/// `[42, handle]` ([`is_window_element`]), since a read inside the provider
/// can leave a window's handle out (`docs/uia-bugs.md`, "Inside a remote
/// operation, conhost's window has no handle or title"), or the one whose
/// handle it reports. A handle the element reports of a window that no
/// longer exists is neither, so a fragment element reporting one does not
/// end the walk at the top. Local calls only.
fn ends_at_the_top(runtime_id: &[i32], hwnd: isize) -> bool {
    let own = match runtime_id {
        // A runtime id holds the handle's low 32 bits, which a window
        // handle's value fits in, sign-extended.
        &[42, handle] if handle != 0 => handle as isize,
        _ => 0,
    };
    let at_the_top = |window: isize| {
        verbatim_uia::is_top_level_window(window)
            || verbatim_uia::parent_across_process_boundary(window).is_some()
    };
    (own != 0 && at_the_top(own)) || (hwnd != 0 && hwnd != own && at_the_top(hwnd))
}

/// The focus ancestry in one cross-process round trip: a program that
/// reads the element's `HasKeyboardFocus` live and stops if it is false,
/// then reads the previous element's (`FocusQuery::previous`), then, with
/// [`FocusQuery::refresh`], fills the element's own cache afresh, then,
/// for a menu item, reads its legacy MSAA state
/// ([`Ancestry::legacy_state`]), then reads a list's or tab control's
/// selected child, or a combo box's when it has no `Value` pattern, then walks
/// raw-view parents, filling each one's cache inside the provider, until
/// the top-level window, a known ancestor, or the depth limit, and finds
/// the element's nearest window, walking on past where it stopped if no
/// ancestor read so far has a window handle. A walk that ends at a null
/// parent anywhere but at an element whose window, named by its runtime id
/// or by the handle it reports, is a top-level window or one below the
/// process boundary, ended below the top, and is reported with the
/// ancestors unknown ([`Ancestry::failed_hop`]).
///
/// `uia` is unused; it is in the signature so that this and
/// [`focus_ancestry_classic`] are interchangeable ([`FocusAncestryFn`]).
///
/// # Errors
///
/// Any [`Error`] from running the program; [`Error::Import`] means the
/// element is served by a client-side proxy, and the classic walk is the
/// answer for its window.
#[expect(
    clippy::too_many_lines,
    reason = "one program, read top to bottom as it runs"
)]
pub fn focus_ancestry_remote(_uia: &Uia, query: &FocusQuery<'_>) -> Result<FocusAncestry, Error> {
    let input = query.element.read();
    let mut b = Builder::new();
    let element = b.import_element(query.element.live().element());

    let focused = b.property(element, UIA_HasKeyboardFocusPropertyId.0);
    let focused = b.add_to_results(focused.assume::<kind::Bool>());
    if query.require_focus {
        let not_focused = b.not(focused);
        b.if_(not_focused, Builder::halt);
    }

    // The element held under the same runtime id: its own live focus. A
    // held element that is gone fails the whole run before it starts
    // ([`focus_ancestry`] runs it again without it).
    let previous_focused = b.new_bool(false);
    let previous_focused = b.add_to_results(previous_focused);
    if let Some(previous) = query.previous {
        let previous = b.import_element(previous);
        let has = b.property(previous, UIA_HasKeyboardFocusPropertyId.0);
        let is_bool = b.is(TypeTest::Bool, has);
        b.if_(is_bool, |b| {
            b.set(previous_focused, has.assume::<kind::Bool>());
        });
    }

    let cache = RemoteCache::new(&mut b, query.fill);
    // The element's own requests, with its place, made only when the
    // element or its selected child is read, so no other program grows.
    let reads_alone = query.refresh || wants_selected_child(&input);
    let element_cache = reads_alone.then(|| RemoteCache::new(&mut b, query.element_fill));

    // The element's own cache, filled afresh: its register is the element
    // read back, with its absent record.
    let element_absent = b.new_int(0);
    let element_absent = b.add_to_results(element_absent);
    if let (true, Some(element_cache)) = (query.refresh, &element_cache) {
        let absent = element_cache.populate(&mut b, element);
        b.set(element_absent, absent);
        b.add_to_results(element);
    }

    // A menu item's legacy MSAA state, ignoring its default, so an element
    // without the LegacyIAccessible pattern answers "not supported". Its
    // registers are made only for a menu item, so no other focus's program
    // grows.
    let legacy = wants_legacy_state(&input).then(|| {
        let legacy_state = b.new_int(0);
        let legacy_state = b.add_to_results(legacy_state);
        let supported = b.new_bool(false);
        let supported = b.add_to_results(supported);
        let property = b.int(UIA_LegacyIAccessibleStatePropertyId.0);
        let ignore_default = b.bool(true);
        let state = b.get_property_value(element, property, ignore_default);
        let is_int = b.is(TypeTest::Int, state);
        b.if_(is_int, |b| {
            b.set(legacy_state, state.assume::<kind::Int>());
            let yes = b.bool(true);
            b.set(supported, yes);
        });
        (legacy_state, supported)
    });

    let selected = b.new_null_element();
    let selected = b.add_to_results(selected);
    let selected_absent = b.new_int(0);
    let selected_absent = b.add_to_results(selected_absent);
    if let (true, Some(cache)) = (wants_selected_child(&input), &element_cache) {
        let read_selected = |b: &mut Builder| {
            // `SelectionPattern2`'s first selected item, ignoring its default,
            // so a provider without the pattern answers "not supported".
            let property = b.int(UIA_Selection2FirstSelectedItemPropertyId.0);
            let ignore_default = b.bool(true);
            let first = b.get_property_value(element, property, ignore_default);
            let unsupported = b.is(TypeTest::NotSupported, first);
            b.if_else(
                unsupported,
                |b| {
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
                            let absent = cache.populate(b, first);
                            b.set(selected_absent, absent);
                            b.set(selected, first);
                        });
                    });
                },
                |b| {
                    let is_element = b.is(TypeTest::Element, first);
                    b.if_(is_element, |b| {
                        let first = first.assume::<kind::Element>();
                        let null = b.is_null(first);
                        let some = b.not(null);
                        b.if_(some, |b| {
                            let absent = cache.populate(b, first);
                            b.set(selected_absent, absent);
                            b.set(selected, first);
                        });
                    });
                },
            );
        };
        // The selected item is optional: a read of it that fails, as on an
        // item gone, leaves it out, and the focus and its ancestors are
        // read without it (`docs/design/focus-pipeline-rework.md`, the
        // audit's difference 4). Its registers are set only once it is
        // filled.
        let is_combo_box = input.control_type == Some(UIA_ComboBoxControlTypeId.0);
        b.try_catch(
            |b| {
                if is_combo_box {
                    // A combo box's selected item is its value only without
                    // the `Value` pattern, which the program asks for itself.
                    let available = b
                        .property(element, UIA_IsValuePatternAvailablePropertyId.0)
                        .assume::<kind::Bool>();
                    let without = b.not(available);
                    b.if_(without, read_selected);
                } else {
                    read_selected(b);
                }
            },
            |_, _| {},
        );
    }

    let known = b.new_string_map();
    for (index, runtime_id) in query.known.iter().enumerate() {
        let key = b.string(&runtime_id_key(runtime_id));
        let index = b.int(i32::try_from(index).unwrap_or(i32::MAX));
        b.map_insert(known, key, index);
    }

    // The nearest window: the element's own, from its cache, or the first
    // ancestor's that has one.
    let window = b.new_int(
        own_window(input.hwnd)
            .and_then(|hwnd| i32::try_from(hwnd).ok())
            .unwrap_or(0),
    );
    let window = b.add_to_results(window);
    let no_window = b.int(0);
    let read_window = |b: &mut Builder, from: Reg<kind::Element>| {
        let unset = b.equal(window, no_window);
        b.if_(unset, |b| {
            let handle = b.property(from, UIA_NativeWindowHandlePropertyId.0);
            let is_int = b.is(TypeTest::Int, handle);
            b.if_(is_int, |b| b.set(window, handle.assume::<kind::Int>()));
        });
    };

    let ancestors = b.new_array();
    let ancestors = b.add_to_results(ancestors);
    // Each ancestor's absent record, in the same order.
    let absents = b.new_array();
    let absents = b.add_to_results(absents);
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
            let absent = cache.populate(b, current);
            b.array_append(ancestors, current);
            b.array_append(absents, absent);
            b.add_assign(count, one);
            read_window(b, current);
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
    // A walk that stopped short with no window yet goes on looking, from
    // where it stopped, without returning what it passes.
    b.while_(
        |b| {
            let unset = b.equal(window, no_window);
            let null = b.is_null(current);
            let more = b.not(null);
            b.and(unset, more)
        },
        |b| {
            read_window(b, current);
            let parent = b.navigate(current, NavigationDirection::Parent);
            b.set(current, parent);
        },
    );

    let outcome = b.finish().execute()?;
    if query.require_focus && !outcome.get(focused)? {
        return Ok(FocusAncestry::NotFocused);
    }
    let absents = outcome.get(absents)?;
    let ancestors: Vec<Cached<Ancestor>> = outcome
        .get(ancestors)?
        .into_iter()
        .zip(absents)
        .filter_map(|(value, absent)| match (value, absent) {
            (Value::Element(element), Value::Int(absent)) => {
                Some(query.fill.claim(element, absent_record(absent)))
            }
            _ => None,
        })
        .collect();
    let window = outcome.get(window)?;
    let met_known = usize::try_from(outcome.get(met_known)?).ok();
    let depth_limited = outcome.get(depth_limited)?;
    // A walk that stopped at neither a known ancestor nor the depth limit
    // ended at a null parent. Inside a program a top-level window's parent
    // is null, as is the parent of a window below the process boundary
    // (verified, `docs/crates/verbatim-uia-rops.md`), and only a walk
    // whose last element is such a window's own element, or reports a
    // top-level window's handle, is complete ([`ends_at_the_top`]). Any
    // other ended at a missing parent below the window, even one whose
    // elements reported a window handle, as a fragment element can report
    // a handle it does not host: its ancestors are unknown, as the classic
    // walk reports a missing parent below the root, and as NVDA reads the
    // window hierarchy when its parent read fails
    // (`NVDAObjects/UIA/__init__.py` 2142 to 2145).
    let failed_hop = (met_known.is_none()
        && !depth_limited
        && !ancestors.last().map_or_else(
            || ends_at_the_top(&query.element.runtime_id(), query.element.read().hwnd),
            |top| ends_at_the_top(&top.runtime_id(), top.read().node.role.hwnd),
        ))
    .then_some(FailedHop::NoParent);
    let claimed = |element: Option<IUIAutomationElement>, absent: i32| {
        element.map(|element| query.element_fill.claim(element, absent_record(absent)))
    };
    Ok(FocusAncestry::Focused(Ancestry {
        ancestors,
        met_known,
        depth_limited,
        out_of_time: false,
        failed_hop,
        plain_window: None,
        selected_child: claimed(outcome.get(selected)?, outcome.get(selected_absent)?),
        window: (window != 0).then_some(window as isize),
        previous_focused: match query.previous {
            Some(_) => Some(if outcome.get(previous_focused)? {
                EarlierElement::Focused
            } else {
                EarlierElement::NotFocused
            }),
            None => None,
        },
        element: if query.refresh {
            claimed(outcome.get(element)?, outcome.get(element_absent)?)
        } else {
            None
        },
        legacy_state: match legacy {
            Some((state, supported)) => {
                Some(outcome.get(supported)?.then_some(outcome.get(state)?))
            }
            None => None,
        },
    }))
}

/// The absent record a program returned for an element, as its bits.
pub(crate) fn absent_record(bits: i32) -> Absent {
    Absent::from_bits(bits.cast_unsigned())
}

/// The remote program's cache requests, from a [`Fill`]'s properties and
/// patterns: one per combination of the
/// [`LEFT_OUT_WHEN_UNSUPPORTED`] properties an element supports, since
/// `PopulateCache` replaces an element's cache rather than adding to it
/// (verified). A remotely filled cache stores a property's default where a
/// locally built cache stores UIA's "not supported" value, so the program
/// leaves such a property out of an element's cache when the element does
/// not support it, and returns which it left out as the element's
/// [`Absent`] record, which its read answers as "not supported"
/// (`docs/design/uia-shapes.md`, section 7, decision 3).
pub(crate) struct RemoteCache {
    /// The checked properties, as the program's constants, each with its
    /// bit in an [`Absent`] record.
    checked: Vec<(Reg<kind::Int>, u32)>,
    /// The request for each combination, indexed by a bit mask of the
    /// checked properties supported, bit `i` for `checked[i]`.
    requests: Vec<Reg<kind::CacheRequest>>,
}

impl RemoteCache {
    pub(crate) fn new<S: Shape>(b: &mut Builder, fill: &Fill<S>) -> Self {
        Self::with(b, fill.properties(), fill.patterns())
    }

    /// The requests for exactly `properties` and `patterns`.
    fn with(b: &mut Builder, properties: &[UIA_PROPERTY_ID], patterns: &[UIA_PATTERN_ID]) -> Self {
        let checked: Vec<(UIA_PROPERTY_ID, u32)> = LEFT_OUT_WHEN_UNSUPPORTED
            .iter()
            .zip(0u32..)
            .filter(|(property, _)| properties.contains(property))
            .map(|(&property, bit)| (property, bit))
            .collect();
        let requests = (0..1usize << checked.len())
            .map(|supported| {
                let request = b.new_cache_request();
                for property in properties {
                    let position = checked.iter().position(|(checked, _)| checked == property);
                    if position.is_none_or(|index| supported & (1 << index) != 0) {
                        b.cache_request_add_property(request, property.0);
                    }
                }
                for pattern in patterns {
                    b.cache_request_add_pattern(request, pattern.0);
                }
                request
            })
            .collect();
        let checked = checked
            .iter()
            .map(|&(property, bit)| (b.int(property.0), bit))
            .collect();
        Self { checked, requests }
    }

    /// Emits the instructions that fill `element`'s cache, and answers the
    /// register that then holds its [`Absent`] record's bits.
    pub(crate) fn populate(&self, b: &mut Builder, element: Reg<kind::Element>) -> Reg<kind::Int> {
        let absent = b.new_int(0);
        if self.checked.is_empty() {
            b.populate_cache(element, self.requests[0]);
            return absent;
        }
        let supported = b.new_int(0);
        let ignore_default = b.bool(true);
        for (index, &(property, bit)) in self.checked.iter().enumerate() {
            let value = b.get_property_value(element, property, ignore_default);
            let unsupported = b.is(TypeTest::NotSupported, value);
            b.if_else(
                unsupported,
                |b| {
                    let flag = b.int(1 << bit);
                    b.add_assign(absent, flag);
                },
                |b| {
                    let flag = b.int(1 << index);
                    b.add_assign(supported, flag);
                },
            );
        }
        for (mask, &request) in self.requests.iter().enumerate() {
            let mask = b.int(i32::try_from(mask).unwrap_or(i32::MAX));
            let chosen = b.equal(supported, mask);
            b.if_(chosen, |b| b.populate_cache(element, request));
        }
        absent
    }
}

/// Tells `before_call`, if there is one, that the call `call` begins
/// ([`FocusQuery::before_call`]).
fn tell(before_call: Option<&dyn Fn(&'static str)>, call: &'static str) {
    if let Some(before_call) = before_call {
        before_call(call);
    }
}

/// The focus ancestry the classic way, the focus listener's read and the
/// outposts' fallback: a live `HasKeyboardFocus` read with
/// [`FocusQuery::require_focus`], the previous element's, the element's
/// own cache filled afresh with [`FocusQuery::refresh`]
/// (`BuildUpdatedCache`), the selected child through the `Selection`
/// pattern ([`Uia::selected`], as `Uia::selected_node` reads it), then one
/// `GetParentElementBuildCache` round trip per ancestor over the raw view,
/// the walk `Uia::ancestors` makes, every ancestor filled by the query's
/// fill's request and the element and its selected child by its element
/// fill's. Stops at a known ancestor, at the
/// depth limit, below the desktop root, below the process boundary
/// (`docs/design/listener-without-connections.md`, terms), at a step into a
/// window that reads as a plain window ([`FocusQuery::plain`]), and when the
/// query's time is up, which is asked before every call but the first
/// focus read, so that at most the call under way runs past it.
///
/// The boundary is judged with local calls only, twice. Before each step,
/// an element whose window has a parent window, or a visible owner for a
/// top-level window, that another process serves
/// ([`verbatim_uia::parent_across_process_boundary`]) is the last
/// ancestor; its window is its own, or, for the nearest window's own
/// element held without its handle (runtime id `[42, window]`), that
/// nearest window, so that such an element never steps into the other
/// process first. After each step, a parent with a window of its own other
/// than the nearest window so far, served by another process, as an owner
/// shown between the check and the step makes it, is not kept, and the
/// walk is complete.
///
/// The nearest window is [`FocusQuery::nearest_window`] when the caller
/// gave it; else it comes from the walked ancestors' caches, or, when the
/// walk stopped before reaching one, from one more call
/// ([`verbatim_uia::nearest_window_handle`]).
///
/// # Errors
///
/// [`Error::Uia`] when the focus read, the cache request, or the tree
/// walker fails, or when the previous element's focus read or a hop finds
/// the provider did not answer within UIA's transaction timeout; a hop
/// that finds no parent, or fails otherwise, ends the walk with the
/// ancestors unknown ([`Ancestry::failed_hop`]). A selected child whose
/// read timed out or found an element gone ([`Uia::selected`]) is left
/// out, and the walk goes on.
#[expect(
    clippy::too_many_lines,
    reason = "one walk, read top to bottom as it runs, as the program is"
)]
pub fn focus_ancestry_classic(uia: &Uia, query: &FocusQuery<'_>) -> Result<FocusAncestry, Error> {
    let input = query.element.read();
    let live = query.element.live();
    // The query's time is asked before every call into the application,
    // so at most the call under way runs past it; what the calls skipped
    // would have read is unknown, and the walk then reads no ancestor.
    let out_of_time = || query.out_of_time.is_some_and(|out_of_time| out_of_time());
    let before_call = |call| tell(query.before_call, call);
    if query.require_focus && {
        before_call("the focus's keyboard focus");
        !live.element().has_keyboard_focus()?
    } {
        return Ok(FocusAncestry::NotFocused);
    }
    // A failed read is an element that is gone, but a provider that did
    // not answer in time has said nothing about it.
    let previous_focused = match query.previous.filter(|_| !out_of_time()).map(|previous| {
        before_call("the previous focus's keyboard focus");
        previous.has_keyboard_focus()
    }) {
        None => None,
        Some(Ok(true)) => Some(EarlierElement::Focused),
        Some(Ok(false)) => Some(EarlierElement::NotFocused),
        Some(Err(error)) if timed_out(&error) => return Err(Error::Uia(error)),
        Some(Err(_)) => Some(EarlierElement::Gone),
    };
    let progress = |read: Progress<'_>| {
        if let Some(progress) = query.progress {
            progress(read);
        }
    };
    if let Some(focused) = previous_focused {
        progress(Progress::PreviousFocused(focused));
    }
    let request = query.fill.request();
    let element_request = query.element_fill.request();
    let element = if query.refresh && !out_of_time() {
        before_call("the focus's fresh cache");
        Some(live.refresh(element_request)?)
    } else {
        None
    };
    if let Some(element) = &element {
        progress(Progress::Element(element));
    }
    // The selected child is optional: one whose read timed out, or whose
    // container or item is gone, is left out, and the focus and its
    // ancestors are read without it (`docs/design/focus-pipeline-rework.md`,
    // the audit's difference 4), as NVDA's walk reads no selection.
    let selected_child = match reads_selected_child(
        &input,
        (live, element.as_ref(), query.value_available),
        &before_call,
    ) {
        Ok(true) if !out_of_time() => {
            before_call("the selected child");
            optional(uia.selected_within(live, element_request, &out_of_time))?
        }
        Ok(_) => None,
        Err(error) => optional(Err(error))?,
    };
    if let Some(child) = &selected_child {
        progress(Progress::SelectedChild(child));
    }
    // The desktop root is served in this process, so reading it is local.
    before_call("the desktop root and the tree walker");
    let root = verbatim_uia::runtime_id(&uia.root_element()?);
    let walker = uia.raw_view_walker()?;
    let mut ancestry = Ancestry {
        ancestors: Vec::new(),
        met_known: None,
        depth_limited: false,
        out_of_time: false,
        failed_hop: None,
        plain_window: None,
        selected_child,
        window: own_window(input.hwnd).or(query.nearest_window),
        previous_focused,
        element,
        legacy_state: None,
    };
    let mut current = live.clone();
    let mut current_window = input.hwnd;
    let mut current_id = query.element.runtime_id();
    let mut complete = false;
    loop {
        if out_of_time() {
            ancestry.out_of_time = true;
            break;
        }
        // The walk stops below the process boundary: an element whose
        // window is the child of another process's window, as the Settings
        // page's content is of `ApplicationFrameHost`'s frame, or a
        // top-level window owned by another process's, as a dialog can be,
        // is the last ancestor, and the outpost of its application
        // continues the chain from that window through MSAA
        // (`docs/design/listener-without-connections.md`, section 3, step
        // 11, and section 4.2). A local check, made for an element with a
        // window of its own and for the nearest window's own element held
        // without its handle.
        if below_the_boundary(current_window, &current_id, ancestry.window) {
            complete = true;
            break;
        }
        before_call("a step to the parent");
        let parent = match current.walk(&walker, Direction::Parent, request) {
            Ok(parent) => parent,
            // A provider that did not answer in time has not said there is
            // no parent: the walk failed, and what it read is not the
            // ancestry.
            Err(error) if timed_out(&error) => return Err(Error::Uia(error)),
            // Any other failed step, or one that finds no parent, from an
            // element below the root leaves the ancestors unknown: only the
            // root ends a known walk.
            Err(error) => {
                ancestry.failed_hop = Some(FailedHop::of(&error));
                break;
            }
        };
        let runtime_id = parent.runtime_id();
        if runtime_id == root {
            complete = true;
            break;
        }
        if ancestry.ancestors.len() >= usize::try_from(query.depth_limit).unwrap_or(usize::MAX) {
            ancestry.depth_limited = true;
            break;
        }
        current_window = parent.read().node.role.hwnd;
        // The step reached another process's window after all, as when an
        // owner is shown between the check above and the step: the parent
        // is that process's, and the walk is complete below it.
        if stepped_across(current_window, ancestry.window) {
            complete = true;
            break;
        }
        // The step reached a window other than the nearest one that reads
        // as a plain window, a ghost's or a hung application's: NVDA makes
        // this one step, reads that window as a plain window and goes on by
        // the window hierarchy (`correctAPIForRelation`,
        // `NVDAObjects/window/__init__.py` 85 to 97), so the parent is not
        // kept and nothing more is asked of the application. Local calls.
        if current_window != 0
            && ancestry
                .window
                .is_some_and(|nearest| nearest != current_window)
            && query.plain.is_some_and(|plain| plain(current_window))
        {
            ancestry.plain_window = Some(current_window);
            complete = true;
            break;
        }
        if ancestry.window.is_none() {
            ancestry.window = own_window(current_window);
        }
        current = parent.live().clone();
        progress(Progress::Ancestor(&parent));
        ancestry.ancestors.push(parent);
        let known = query.known.iter().position(|known| *known == runtime_id);
        current_id = runtime_id;
        if let Some(index) = known {
            ancestry.met_known = Some(index);
            break;
        }
    }
    if ancestry.window.is_none() && !complete && !ancestry.ancestors_unknown() && !out_of_time() {
        before_call("the nearest window");
        ancestry.window = verbatim_uia::nearest_window_handle(live.element());
    }
    Ok(FocusAncestry::Focused(ancestry))
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::E_FAIL;

    use super::FailedHop;

    /// A step that failed with a code keeps it, and one that answered a
    /// null element, the error carrying no failure code, found no parent.
    #[test]
    fn a_failed_step_is_told_by_its_code() {
        assert_eq!(
            FailedHop::of(&windows::core::Error::from(E_FAIL)),
            FailedHop::Error(E_FAIL)
        );
        assert_eq!(
            FailedHop::of(&windows::core::Error::empty()),
            FailedHop::NoParent
        );
    }
}
