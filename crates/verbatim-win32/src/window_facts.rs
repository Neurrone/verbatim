//! The window facts a reader's tell carries, which Core's acceptance
//! reads (design `docs/design/readers-tell-core-asks-build1.md`, "Focus and
//! foreground"), and the facts of a node's window, gathered with calls to
//! Windows only and none into an application, for both readers: the
//! listener's tells and the outpost's tells of its followed nodes' changes.
//! The gathering is pure over [`WindowReads`], so every case is a test
//! here; [`SystemReads`] makes the reads.

use verbatim_model::request::NodeWindow;
use verbatim_model::tell::{AcceptanceFacts, Relation, WindowFacet};
use verbatim_model::{Pid, WindowHandle};
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::WindowQueries;

use crate::window::{SystemWindows, window_class_name};

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
            hidden_frame_owned: !hidden_frame && root_owner != 0 && reads.hidden_frame(root_owner),
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
        Window::new(window).is_some_and(|window| SystemWindows { foreground: 0 }.is_topmost(window))
    }

    fn desktop(&self) -> isize {
        crate::window::desktop_window()
    }

    fn foreground(&self) -> isize {
        crate::window::foreground_window_handle()
    }

    fn active_window(&self) -> isize {
        SystemWindows { foreground: 0 }
            .active_window()
            .map_or(0, Window::get)
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
