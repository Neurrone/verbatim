//! A window's handle, never 0 ([`Window`]): the logic crates parse a
//! handle that may be 0 for none into `Option<Window>` where it enters
//! them, so no rule judges a 0 as none.

use std::num::NonZeroIsize;

/// A window's handle, never 0. The protocol and the system's calls carry
/// a handle as an `isize`, 0 for none; [`Window::new`] parses one, and
/// [`Window::get`] gives it back for a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Window(NonZeroIsize);

impl Window {
    /// The window of `handle`; `None` for 0, no window.
    #[must_use]
    pub const fn new(handle: isize) -> Option<Self> {
        match NonZeroIsize::new(handle) {
            Some(handle) => Some(Self(handle)),
            None => None,
        }
    }

    /// The handle, for a call or the protocol.
    #[must_use]
    pub const fn get(self) -> isize {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_of_0_is_no_window_and_any_other_comes_back_unchanged() {
        assert_eq!(Window::new(0), None);
        assert_eq!(Window::new(-7).map(Window::get), Some(-7));
    }
}
