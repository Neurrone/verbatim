//! The nearest window of an element inside a program: its own native
//! window, else each raw-view ancestor's, as
//! `verbatim_uia::nearest_window_handle` finds it, which a step through
//! the tree reports so the outpost can correct the neighbor's backend.

use windows::Win32::UI::Accessibility::UIA_NativeWindowHandlePropertyId;

use crate::builder::{Builder, Reg, kind};
use crate::instruction::TypeTest;
use crate::opcode::NavigationDirection;

/// Emits the nearest window of `element`, whose own window is `own`, 0
/// for none: `own`, else each raw-view ancestor's `NativeWindowHandle`
/// until one has one; the register among the results, 0 for none found.
pub(crate) fn emit_nearest_window(
    b: &mut Builder,
    element: Reg<kind::Element>,
    own: isize,
) -> Reg<kind::Int> {
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
    window
}
