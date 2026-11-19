//! The window facts one `WinEvent` is handled with ([`EventWindows`]):
//! made as the event's handling starts and dropped as it ends, so that
//! the foreground's watch, the tell and the progress path each see the one
//! answer read for the event, and none is read twice, as NVDA's
//! `WinEvent` callback reads a window's class once (`winEventCallback`,
//! `nvda/source/IAccessibleHandler/internalWinEventHandler.py`).

use std::cell::RefCell;
use std::collections::BTreeMap;

use verbatim_win32::window::Readability;
use verbatim_win32::window_facts::{ReadOnce, SystemReads, WindowReads};

/// What the listener reads of Windows' own state as it handles a
/// `WinEvent`: a window's facts, and whether a window exists.
pub(super) trait EventReads: WindowReads {
    /// Whether `window` is a window that exists (`IsWindow`).
    fn exists(&self, window: isize) -> bool;
}

impl EventReads for SystemReads {
    fn exists(&self, window: isize) -> bool {
        verbatim_win32::window::window_exists(window)
    }
}

/// The facts of the windows one `WinEvent` names, each read at most once
/// while it is handled: their own facts ([`ReadOnce`]), the window's class
/// given as the hook's filter read it, and each window's readability, by
/// the listener's one judge of it. The foreground window, the active
/// window and whether one window is inside another are read each time
/// they are asked, as [`ReadOnce`] reads them. Nothing in it outlives the
/// event.
pub(super) struct EventWindows<'r, R> {
    facts: ReadOnce<'r, R>,
    reads: &'r R,
    readability: &'r dyn Fn(isize) -> Readability,
    readable: RefCell<BTreeMap<isize, Readability>>,
}

impl<'r, R: EventReads> EventWindows<'r, R> {
    /// Nothing read yet for the event but the class of its window, `hook`,
    /// when the hook's filter read it; facts read from `reads`, and
    /// readability judged by `readability`.
    pub(super) fn new(
        reads: &'r R,
        readability: &'r dyn Fn(isize) -> Readability,
        hook: Option<(isize, &str)>,
    ) -> Self {
        let facts = ReadOnce::new(reads);
        Self {
            facts: match hook {
                Some((window, class)) => facts.with_class(window, class.to_owned()),
                None => facts,
            },
            reads,
            readability,
            readable: RefCell::new(BTreeMap::new()),
        }
    }

    /// The windows' own facts.
    pub(super) fn facts(&self) -> &ReadOnce<'r, R> {
        &self.facts
    }

    /// Whether `window` exists, read now.
    pub(super) fn exists(&self, window: isize) -> bool {
        self.reads.exists(window)
    }

    /// Whether `window` can be read, judged once for the event.
    pub(super) fn readability(&self, window: isize) -> Readability {
        if let Some(&answer) = self.readable.borrow().get(&window) {
            return answer;
        }
        let answer = (self.readability)(window);
        self.readable.borrow_mut().insert(window, answer);
        answer
    }
}
