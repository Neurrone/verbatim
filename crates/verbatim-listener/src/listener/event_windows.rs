//! The window facts one `WinEvent` is handled with ([`EventWindows`]):
//! made as the event's handling starts and dropped as it ends, so that
//! the foreground's watch, the tell and the progress path each see the one
//! answer read for the event, and none is read twice, as NVDA's
//! `WinEvent` callback reads a window's class once (`winEventCallback`,
//! `nvda/source/IAccessibleHandler/internalWinEventHandler.py`). Every
//! read of Windows' state the event's handling makes goes through it, so
//! the event path's call-count test sees each one
//! (`a_flood_events_window_facts_are_read_once`; the ownership scan,
//! `xtask/src/ownership.rs`, holds the event path to it).

use std::cell::RefCell;
use std::collections::BTreeMap;

use verbatim_win32::window::Readability;
use verbatim_win32::window_facts::{ReadOnce, SystemReads, WindowReads};

/// What the listener reads of Windows' own state as it handles a
/// `WinEvent`, beside a window's facts: whether a window exists, whether
/// it is visible, whether an existing window is hung, and the process of
/// a thread.
pub(super) trait EventReads: WindowReads {
    /// Whether `window` is a window that exists (`IsWindow`).
    fn exists(&self, window: isize) -> bool;
    /// Whether `window` is visible (`IsWindowVisible`).
    fn visible(&self, window: isize) -> bool;
    /// Whether `window`, a window found to exist, is ghosted or hung
    /// ([`verbatim_win32::window::hang_state`]).
    fn hang_state(&self, window: isize) -> Readability;
    /// The process `thread` belongs to, `None` when it cannot be read.
    fn process_of_thread(&self, thread: u32) -> Option<u32>;
    /// Whether this process can open the input desktop now
    /// ([`verbatim_win32::window::input_desktop_opens`]).
    fn input_desktop_opens(&self) -> bool;
}

impl EventReads for SystemReads {
    fn exists(&self, window: isize) -> bool {
        verbatim_win32::window::window_exists(window)
    }

    fn visible(&self, window: isize) -> bool {
        verbatim_win32::window::window_is_visible(window)
    }

    fn hang_state(&self, window: isize) -> Readability {
        verbatim_win32::window::hang_state(window)
    }

    fn process_of_thread(&self, thread: u32) -> Option<u32> {
        super::process_of_thread(thread)
    }

    fn input_desktop_opens(&self) -> bool {
        verbatim_win32::window::input_desktop_opens()
    }
}

/// The facts of the windows one `WinEvent` names, each read at most once
/// while it is handled: their own facts ([`ReadOnce`]), the window's class
/// given as the hook's filter read it, whether each exists, and the
/// process of each thread, which cannot change while the thread lives.
/// The foreground window, the active window and whether one window is
/// inside another are read each time they are asked, as [`ReadOnce`]
/// reads them, and so is whether a window is hung. Nothing in it outlives
/// the event.
pub(super) struct EventWindows<'r, R> {
    facts: ReadOnce<'r, R>,
    reads: &'r R,
    readability: &'r dyn Fn(isize) -> Readability,
    existing: RefCell<BTreeMap<isize, bool>>,
    processes: RefCell<BTreeMap<u32, Option<u32>>>,
}

impl<'r, R: EventReads> EventWindows<'r, R> {
    /// Nothing read yet for the event but the class of its window, `hook`,
    /// when the hook's filter read it; facts read from `reads`, and the
    /// progress path's readability judged by `readability`, the
    /// listener's one judge of it.
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
            existing: RefCell::new(BTreeMap::new()),
            processes: RefCell::new(BTreeMap::new()),
        }
    }

    /// The windows' own facts.
    pub(super) fn facts(&self) -> &ReadOnce<'r, R> {
        &self.facts
    }

    /// The reads with nothing kept, for what is read each time it is
    /// asked: the foreground's part of a tell's facts, read under the
    /// tells' lock.
    pub(super) fn live(&self) -> &'r R {
        self.reads
    }

    /// Whether `window` exists, read once for the event.
    pub(super) fn exists(&self, window: isize) -> bool {
        if let Some(&exists) = self.existing.borrow().get(&window) {
            return exists;
        }
        let exists = self.reads.exists(window);
        self.existing.borrow_mut().insert(window, exists);
        exists
    }

    /// Whether `window` can be read, as the tell judges it: gone unless it
    /// exists ([`exists`](Self::exists), read once for the event), then
    /// whether it is ghosted or hung, read now.
    pub(super) fn readability(&self, window: isize) -> Readability {
        if window == 0 || !self.exists(window) {
            return Readability::Gone;
        }
        self.reads.hang_state(window)
    }

    /// Whether `window` can be read, judged now by the listener's one
    /// judge, as the progress path judges it after the tell is sent.
    pub(super) fn readability_now(&self, window: isize) -> Readability {
        (self.readability)(window)
    }

    /// Whether this process can open the input desktop, read now.
    pub(super) fn input_desktop_opens(&self) -> bool {
        self.reads.input_desktop_opens()
    }

    /// Whether `window` is visible, read now.
    pub(super) fn visible(&self, window: isize) -> bool {
        self.reads.visible(window)
    }

    /// The process `thread` belongs to, read once for the event.
    pub(super) fn process_of_thread(&self, thread: u32) -> Option<u32> {
        if let Some(&process) = self.processes.borrow().get(&thread) {
            return process;
        }
        let process = self.reads.process_of_thread(thread);
        self.processes.borrow_mut().insert(thread, process);
        process
    }
}
