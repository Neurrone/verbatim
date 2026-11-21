//! The window facts a reader's tell carries, which Core's acceptance
//! reads (design `books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects"),
//! and the facts of a node's window, gathered with calls to
//! Windows only and none into an application, for both readers: the
//! listener's tells and the outpost's tells of its followed nodes' changes.
//! The gathering is pure over [`WindowReads`], so every case is a test
//! here; [`SystemReads`] makes the reads, and [`ReadOnce`] makes each of
//! a window's own reads at most once while one event is handled.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use verbatim_model::request::NodeWindow;
use verbatim_model::tell::{AcceptanceFacts, Relation, WindowFacet};
use verbatim_model::{Pid, WindowHandle};

use crate::window::window_class_name;

/// The reads of Windows' own state the window facts are gathered with,
/// every one a call to Windows and none into an application. A handle of
/// 0 is no window.
pub trait WindowReads {
    /// The window's class name, empty for no window.
    fn class(&self, window: isize) -> String;
    /// The thread and process that own the window by
    /// `GetWindowThreadProcessId`, zeros for no window.
    fn owner(&self, window: isize) -> (u32, u32);
    /// The process that serves a console window, its console host, `None`
    /// when it cannot be read.
    fn console_host(&self, window: isize) -> Option<u32>;
    /// `GetAncestor` with `GA_PARENT`.
    fn parent(&self, window: isize) -> isize;
    /// `GetAncestor` with `GA_ROOT`.
    fn root(&self, window: isize) -> isize;
    /// `GetAncestor` with `GA_ROOTOWNER`.
    fn root_owner(&self, window: isize) -> isize;
    /// Whether `child` is a child window of `parent` (`IsChild`).
    fn is_child(&self, parent: isize, child: isize) -> bool;
    /// Whether the window has the topmost extended style.
    fn topmost(&self, window: isize) -> bool;
    /// The desktop window.
    fn desktop(&self) -> isize;
    /// The foreground window, 0 for none.
    fn foreground(&self) -> isize;
    /// The active window of the foreground thread, 0 for none.
    fn active_window(&self) -> isize;
    /// Whether the window is Verbatim's hidden frame.
    fn hidden_frame(&self, window: isize) -> bool;
}

/// The class of a console host's window.
const CONSOLE_CLASS: &str = "ConsoleWindowClass";

/// The class of a popup menu's window.
const MENU_CLASS: &str = "#32768";

/// Whether `window` is a popup menu Verbatim's hidden frame shows: a
/// popup menu has no owner, so its owner chain does not reach the frame,
/// but the frame is in front while its menu is open, and the menu's window
/// belongs to the thread that owns the frame.
fn frame_menu(reads: &impl WindowReads, window: isize) -> bool {
    if reads.class(window) != MENU_CLASS {
        return false;
    }
    let front = reads.foreground();
    front != 0 && reads.hidden_frame(front) && reads.owner(front).0 == reads.owner(window).0
}

/// The part of a tell window's facts that depends on the window alone,
/// gathered as the event arrives, before a hold could let the window go.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one read, each a yes or no, as the tell carries them"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPart {
    /// The window's handle and its root owner's, which the foreground's
    /// part relates to the foreground.
    handles: (isize, isize),
    window: WindowFacet,
    parent_class: String,
    root: WindowFacet,
    root_owner: WindowFacet,
    window_topmost: bool,
    root_topmost: bool,
    desktop: bool,
    hidden_frame: bool,
    hidden_frame_owned: bool,
}

/// `window` as the model names a window.
fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

/// `window` as acceptance reads it: a console window's process is its
/// console host, `host` when the event's own thread named it, and its
/// thread the event's; any other window's, its owner's.
fn facet(reads: &impl WindowReads, window: isize, host: Option<(u32, u32)>) -> WindowFacet {
    let class = reads.class(window);
    let (thread, owner) = reads.owner(window);
    let (thread, process) = if class == CONSOLE_CLASS {
        match host {
            Some((thread, process)) => (thread, process),
            None => (thread, reads.console_host(window).unwrap_or(0)),
        }
    } else {
        (thread, owner)
    };
    WindowFacet {
        handle: handle(window),
        class,
        process: Pid(process),
        thread,
    }
}

/// The facts of `window` that a node read in it carries, by which a
/// request on the node names it (`verbatim_model::request::NodeWindow`):
/// read now, with calls to Windows only.
#[must_use]
pub fn node_window(reads: &impl WindowReads, window: isize) -> NodeWindow {
    let (thread, process) = reads.owner(window);
    NodeWindow {
        handle: handle(window),
        top_level: handle(reads.root(window)),
        root_owner: handle(reads.root_owner(window)),
        process: Pid(process),
        thread,
        desktop: window == reads.desktop(),
    }
}

impl WindowPart {
    /// The facts of `window` that depend on it alone, read now. `host` is
    /// the thread that raised the event and its process, which for a
    /// console window name its console host.
    #[must_use]
    pub fn read(reads: &impl WindowReads, window: isize, host: Option<(u32, u32)>) -> Self {
        let root = reads.root(window);
        let root_owner = reads.root_owner(window);
        let hidden_frame = reads.hidden_frame(window);
        let host_of = |handle: isize| (handle == window).then_some(host).flatten();
        Self {
            handles: (window, root_owner),
            window: facet(reads, window, host),
            parent_class: match reads.parent(window) {
                0 => String::new(),
                parent => reads.class(parent),
            },
            root: facet(reads, root, host_of(root)),
            root_owner: facet(reads, root_owner, host_of(root_owner)),
            window_topmost: reads.topmost(window),
            root_topmost: reads.topmost(root),
            desktop: window == reads.desktop(),
            hidden_frame,
            hidden_frame_owned: !hidden_frame
                && ((root_owner != 0 && reads.hidden_frame(root_owner))
                    || frame_menu(reads, window)),
        }
    }

    /// The whole facts, with the foreground's part read now, as the tell
    /// is placed in the listener's order of tells.
    #[must_use]
    pub fn with_foreground(self, reads: &impl WindowReads) -> AcceptanceFacts {
        let (window, root_owner) = self.handles;
        let foreground = reads.foreground();
        let foreground_root_owner = if foreground == 0 {
            0
        } else {
            reads.root_owner(foreground)
        };
        let active = reads.active_window();
        let relation = |parent: isize, child: isize| {
            if parent == 0 || child == 0 {
                Relation::Outside
            } else if parent == child {
                Relation::Equal
            } else if reads.is_child(parent, child) {
                Relation::Under
            } else {
                Relation::Outside
            }
        };
        AcceptanceFacts {
            window_to_foreground: relation(foreground, window),
            root_owner_to_foreground: relation(foreground, root_owner),
            root_owner_is_foregrounds: foreground != 0 && root_owner == foreground_root_owner,
            window_to_active: relation(active, window),
            foreground: handle(foreground),
            foreground_root_owner: handle(foreground_root_owner),
            active_window: handle(active),
            window: self.window,
            parent_class: self.parent_class,
            root: self.root,
            root_owner: self.root_owner,
            window_topmost: self.window_topmost,
            root_topmost: self.root_topmost,
            desktop: self.desktop,
            hidden_frame: self.hidden_frame,
            hidden_frame_owned: self.hidden_frame_owned,
        }
    }
}

/// [`WindowReads`] over `R` that reads each fact of a window at most once:
/// for the handling of one event, so that every rule that asks a window's
/// class, owner, ancestors, style or hidden-frame mark sees the one answer
/// read for the event, as NVDA's `WinEvent` callback reads a window's class
/// once (`winEventCallback`,
/// `nvda/source/IAccessibleHandler/internalWinEventHandler.py`). The facts of
/// the moment, the foreground window, the active window and whether one
/// window is inside another, are read each time they are asked, as is a
/// console window's host, since a
/// rule that asks them judges the moment it is applied. It is made for one
/// event and dropped with it: nothing about a window is kept from one
/// event to the next.
pub struct ReadOnce<'r, R> {
    reads: &'r R,
    desktop: Cell<Option<isize>>,
    windows: RefCell<BTreeMap<isize, Known>>,
}

/// What has been read of one window while an event is handled.
#[derive(Default)]
struct Known {
    class: Option<String>,
    owner: Option<(u32, u32)>,
    parent: Option<isize>,
    root: Option<isize>,
    root_owner: Option<isize>,
    topmost: Option<bool>,
    hidden_frame: Option<bool>,
}

impl<'r, R: WindowReads> ReadOnce<'r, R> {
    /// Nothing read yet, each fact to be read from `reads`.
    #[must_use]
    pub fn new(reads: &'r R) -> Self {
        Self {
            reads,
            desktop: Cell::new(None),
            windows: RefCell::new(BTreeMap::new()),
        }
    }

    /// `window`'s class already read for this event, `class`, as the hook's
    /// filter read it before the event was delivered.
    #[must_use]
    pub fn with_class(self, window: isize, class: String) -> Self {
        self.windows.borrow_mut().entry(window).or_default().class = Some(class);
        self
    }

    /// The fact `field` of `window`, read with `read` the first time it is
    /// asked.
    fn known<T: Clone>(
        &self,
        window: isize,
        field: fn(&mut Known) -> &mut Option<T>,
        read: impl FnOnce(&R) -> T,
    ) -> T {
        if let Some(value) = self
            .windows
            .borrow_mut()
            .get_mut(&window)
            .and_then(|known| field(known).clone())
        {
            return value;
        }
        let value = read(self.reads);
        *field(self.windows.borrow_mut().entry(window).or_default()) = Some(value.clone());
        value
    }
}

impl<R: WindowReads> WindowReads for ReadOnce<'_, R> {
    fn class(&self, window: isize) -> String {
        self.known(
            window,
            |known| &mut known.class,
            |reads| reads.class(window),
        )
    }

    fn owner(&self, window: isize) -> (u32, u32) {
        self.known(
            window,
            |known| &mut known.owner,
            |reads| reads.owner(window),
        )
    }

    fn console_host(&self, window: isize) -> Option<u32> {
        // Read each time it is asked: only for a console window whose event
        // named no host, which the listener's never is.
        self.reads.console_host(window)
    }

    fn parent(&self, window: isize) -> isize {
        self.known(
            window,
            |known| &mut known.parent,
            |reads| reads.parent(window),
        )
    }

    fn root(&self, window: isize) -> isize {
        self.known(window, |known| &mut known.root, |reads| reads.root(window))
    }

    fn root_owner(&self, window: isize) -> isize {
        self.known(
            window,
            |known| &mut known.root_owner,
            |reads| reads.root_owner(window),
        )
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        self.reads.is_child(parent, child)
    }

    fn topmost(&self, window: isize) -> bool {
        self.known(
            window,
            |known| &mut known.topmost,
            |reads| reads.topmost(window),
        )
    }

    fn desktop(&self) -> isize {
        if let Some(desktop) = self.desktop.get() {
            return desktop;
        }
        let desktop = self.reads.desktop();
        self.desktop.set(Some(desktop));
        desktop
    }

    fn foreground(&self) -> isize {
        self.reads.foreground()
    }

    fn active_window(&self) -> isize {
        self.reads.active_window()
    }

    fn hidden_frame(&self, window: isize) -> bool {
        self.known(
            window,
            |known| &mut known.hidden_frame,
            |reads| reads.hidden_frame(window),
        )
    }
}

/// Windows' own state, read with calls to Windows only.
pub struct SystemReads;

impl WindowReads for SystemReads {
    fn class(&self, window: isize) -> String {
        window_class_name(window)
    }

    fn owner(&self, window: isize) -> (u32, u32) {
        crate::window::window_owner(window)
    }

    fn console_host(&self, window: isize) -> Option<u32> {
        crate::provider::provider_process(window)
    }

    fn parent(&self, window: isize) -> isize {
        crate::window::parent(window)
    }

    fn root(&self, window: isize) -> isize {
        crate::window::top_level_of(window)
    }

    fn root_owner(&self, window: isize) -> isize {
        crate::window::root_owner_of(window)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        crate::window::is_child(parent, child)
    }

    fn topmost(&self, window: isize) -> bool {
        crate::window::is_topmost(window)
    }

    fn desktop(&self) -> isize {
        crate::window::desktop_window()
    }

    fn foreground(&self) -> isize {
        crate::window::foreground_window_handle()
    }

    fn active_window(&self) -> isize {
        crate::window::active_window()
    }

    fn hidden_frame(&self, window: isize) -> bool {
        crate::window::window_is_hidden_frame(window)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const DESKTOP: isize = 0x10;

    /// A made-up desktop: each window's class, owner, parent, owner and
    /// style.
    #[derive(Default)]
    struct Desk {
        windows: BTreeMap<isize, Made>,
        foreground: isize,
        active: isize,
        frame: isize,
    }

    #[derive(Clone)]
    struct Made {
        class: &'static str,
        thread: u32,
        process: u32,
        parent: isize,
        owner: isize,
        topmost: bool,
    }

    impl Desk {
        fn add(&mut self, window: isize, class: &'static str, parent: isize, owner: isize) {
            self.windows.insert(
                window,
                Made {
                    class,
                    thread: 100,
                    process: 200,
                    parent,
                    owner,
                    topmost: false,
                },
            );
        }

        fn made(&self, window: isize) -> Option<&Made> {
            self.windows.get(&window)
        }
    }

    impl WindowReads for Desk {
        fn class(&self, window: isize) -> String {
            self.made(window)
                .map_or_else(String::new, |made| made.class.to_owned())
        }
        fn owner(&self, window: isize) -> (u32, u32) {
            self.made(window)
                .map_or((0, 0), |made| (made.thread, made.process))
        }
        fn console_host(&self, _window: isize) -> Option<u32> {
            Some(300)
        }
        fn parent(&self, window: isize) -> isize {
            self.made(window).map_or(0, |made| made.parent)
        }
        fn root(&self, window: isize) -> isize {
            let mut at = window;
            while let Some(made) = self.made(at)
                && made.parent != 0
                && made.parent != DESKTOP
            {
                at = made.parent;
            }
            at
        }
        fn root_owner(&self, window: isize) -> isize {
            let mut at = self.root(window);
            while let Some(made) = self.made(at)
                && made.owner != 0
            {
                at = self.root(made.owner);
            }
            at
        }
        fn is_child(&self, parent: isize, child: isize) -> bool {
            let mut at = child;
            while let Some(made) = self.made(at)
                && made.parent != 0
                && made.parent != DESKTOP
            {
                if made.parent == parent {
                    return true;
                }
                at = made.parent;
            }
            false
        }
        fn topmost(&self, window: isize) -> bool {
            self.made(window).is_some_and(|made| made.topmost)
        }
        fn desktop(&self) -> isize {
            DESKTOP
        }
        fn foreground(&self) -> isize {
            self.foreground
        }
        fn active_window(&self) -> isize {
            self.active
        }
        fn hidden_frame(&self, window: isize) -> bool {
            window != 0 && window == self.frame
        }
    }

    #[test]
    fn a_child_of_the_foreground_window_is_under_it_and_shares_its_root_owner() {
        let mut desk = Desk::default();
        desk.add(1, "Notepad", DESKTOP, 0);
        desk.add(2, "Edit", 1, 0);
        desk.foreground = 1;
        desk.active = 1;
        desk.windows.get_mut(&1).unwrap().topmost = true;
        let facts = WindowPart::read(&desk, 2, None).with_foreground(&desk);
        assert_eq!(facts.window.class, "Edit");
        assert_eq!(facts.parent_class, "Notepad");
        assert_eq!(facts.root.handle, WindowHandle(1));
        assert_eq!(facts.root_owner.handle, WindowHandle(1));
        assert_eq!(facts.window_to_foreground, Relation::Under);
        assert_eq!(facts.root_owner_to_foreground, Relation::Equal);
        assert!(facts.root_owner_is_foregrounds);
        assert_eq!(facts.window_to_active, Relation::Under);
        assert!(!facts.window_topmost);
        assert!(facts.root_topmost);
        assert!(!facts.desktop);
        assert!(!facts.hidden_frame);
        assert!(!facts.hidden_frame_owned);
    }

    #[test]
    fn a_window_outside_the_foreground_and_none_in_front_relate_to_nothing() {
        let mut desk = Desk::default();
        desk.add(1, "Notepad", DESKTOP, 0);
        desk.add(3, "Other", DESKTOP, 0);
        desk.foreground = 3;
        let facts = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert_eq!(facts.window_to_foreground, Relation::Outside);
        assert_eq!(facts.root_owner_to_foreground, Relation::Outside);
        assert!(!facts.root_owner_is_foregrounds);
        assert_eq!(facts.window_to_active, Relation::Outside);
        desk.foreground = 0;
        let facts = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert_eq!(facts.foreground, WindowHandle(0));
        assert!(!facts.root_owner_is_foregrounds);
    }

    #[test]
    fn a_window_owned_by_the_hidden_frame_carries_the_mark_and_the_frame_its_own() {
        let mut desk = Desk::default();
        desk.add(1, "VerbatimFrame", DESKTOP, 0);
        desk.add(5, "#32768", DESKTOP, 1);
        desk.frame = 1;
        let menu = WindowPart::read(&desk, 5, None).with_foreground(&desk);
        assert!(menu.hidden_frame_owned);
        assert!(!menu.hidden_frame);
        assert_eq!(menu.root_owner.handle, WindowHandle(1));
        let frame = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert!(frame.hidden_frame);
        assert!(!frame.hidden_frame_owned);
    }

    /// Verbatim's popup menu has no owner, as Windows makes a popup menu's
    /// window, but its thread owns the frame, which is in front: it
    /// carries the mark. Another thread's popup menu does not, nor does
    /// the frame's menu once the frame is not in front.
    #[test]
    fn the_hidden_frames_popup_menu_with_no_owner_carries_the_mark() {
        let mut desk = Desk::default();
        desk.add(1, "wxWindowNR", DESKTOP, 0);
        desk.add(5, MENU_CLASS, DESKTOP, 0);
        desk.frame = 1;
        desk.foreground = 1;
        let menu = WindowPart::read(&desk, 5, None).with_foreground(&desk);
        assert!(menu.hidden_frame_owned);
        assert_eq!(menu.root_owner.handle, WindowHandle(5));
        desk.windows.get_mut(&5).unwrap().thread = 101;
        let other = WindowPart::read(&desk, 5, None).with_foreground(&desk);
        assert!(!other.hidden_frame_owned);
        desk.windows.get_mut(&5).unwrap().thread = 100;
        desk.add(2, "Notepad", DESKTOP, 0);
        desk.foreground = 2;
        let behind = WindowPart::read(&desk, 5, None).with_foreground(&desk);
        assert!(!behind.hidden_frame_owned);
    }

    #[test]
    fn a_console_windows_process_is_its_host_named_by_the_events_thread() {
        let mut desk = Desk::default();
        desk.add(9, CONSOLE_CLASS, DESKTOP, 0);
        let told = WindowPart::read(&desk, 9, Some((77, 400))).with_foreground(&desk);
        assert_eq!((told.window.thread, told.window.process), (77, Pid(400)));
        assert_eq!(told.root.process, Pid(400));
        let unnamed = WindowPart::read(&desk, 9, None).with_foreground(&desk);
        assert_eq!(unnamed.window.process, Pid(300));
    }

    /// A node's window carries its top-level window, its root owner, its
    /// owner's thread and process, and whether it is the desktop window.
    #[test]
    fn a_nodes_window_names_its_top_level_window_owner_and_the_desktop() {
        let mut desk = Desk::default();
        desk.add(DESKTOP, "#32769", 0, 0);
        desk.add(1, "Notepad", DESKTOP, 0);
        desk.add(2, "Edit", 1, 0);
        desk.add(5, "#32770", DESKTOP, 1);
        assert_eq!(
            node_window(&desk, 2),
            NodeWindow {
                handle: WindowHandle(2),
                top_level: WindowHandle(1),
                root_owner: WindowHandle(1),
                process: Pid(200),
                thread: 100,
                desktop: false,
            }
        );
        assert_eq!(node_window(&desk, 5).root_owner, WindowHandle(1));
        assert!(node_window(&desk, DESKTOP).desktop);
    }

    /// [`WindowReads`] over a [`Desk`] that counts each read by its name.
    struct Counting<'d> {
        desk: &'d Desk,
        counts: RefCell<BTreeMap<&'static str, u32>>,
    }

    impl Counting<'_> {
        fn count<T>(&self, name: &'static str, value: T) -> T {
            *self.counts.borrow_mut().entry(name).or_default() += 1;
            value
        }
    }

    impl WindowReads for Counting<'_> {
        fn class(&self, window: isize) -> String {
            self.count("class", self.desk.class(window))
        }
        fn owner(&self, window: isize) -> (u32, u32) {
            self.count("owner", self.desk.owner(window))
        }
        fn console_host(&self, window: isize) -> Option<u32> {
            self.count("console_host", self.desk.console_host(window))
        }
        fn parent(&self, window: isize) -> isize {
            self.count("parent", self.desk.parent(window))
        }
        fn root(&self, window: isize) -> isize {
            self.count("root", self.desk.root(window))
        }
        fn root_owner(&self, window: isize) -> isize {
            self.count("root_owner", self.desk.root_owner(window))
        }
        fn is_child(&self, parent: isize, child: isize) -> bool {
            self.count("is_child", self.desk.is_child(parent, child))
        }
        fn topmost(&self, window: isize) -> bool {
            self.count("topmost", self.desk.topmost(window))
        }
        fn desktop(&self) -> isize {
            self.count("desktop", self.desk.desktop())
        }
        fn foreground(&self) -> isize {
            self.count("foreground", self.desk.foreground())
        }
        fn active_window(&self) -> isize {
            self.count("active_window", self.desk.active_window())
        }
        fn hidden_frame(&self, window: isize) -> bool {
            self.count("hidden_frame", self.desk.hidden_frame(window))
        }
    }

    /// A top-level window's facts read through [`ReadOnce`], its class
    /// given as the hook read it: each of the window's facts is read once,
    /// though the window is its own root and root owner, and the facts of
    /// the moment each time they are asked; the facts are those read
    /// without it.
    #[test]
    fn read_once_reads_each_fact_of_a_window_once_and_the_moment_each_time() {
        let mut desk = Desk::default();
        desk.add(DESKTOP, "#32769", 0, 0);
        desk.add(1, "MockApp", DESKTOP, 0);
        desk.add(3, "Other", DESKTOP, 0);
        desk.foreground = 3;
        desk.active = 3;
        let counting = Counting {
            desk: &desk,
            counts: RefCell::new(BTreeMap::new()),
        };
        let once = ReadOnce::new(&counting).with_class(1, "MockApp".to_owned());
        let facts = WindowPart::read(&once, 1, None).with_foreground(&once);
        assert_eq!(once.class(1), "MockApp");
        assert_eq!(once.owner(1), (100, 200));
        assert_eq!(
            facts,
            WindowPart::read(&desk, 1, None).with_foreground(&desk)
        );
        assert_eq!(
            counting.counts.into_inner(),
            BTreeMap::from([
                ("active_window", 1),
                ("class", 1),
                ("desktop", 1),
                ("foreground", 1),
                ("hidden_frame", 1),
                ("is_child", 3),
                ("owner", 1),
                ("parent", 1),
                ("root", 1),
                ("root_owner", 2),
                ("topmost", 1),
            ])
        );
    }

    #[test]
    fn the_desktop_window_is_marked() {
        let mut desk = Desk::default();
        desk.add(DESKTOP, "#32769", 0, 0);
        assert!(
            WindowPart::read(&desk, DESKTOP, None)
                .with_foreground(&desk)
                .desktop
        );
    }
}
