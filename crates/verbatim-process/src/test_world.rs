//! The test world (prototype, `prototype/speech-level-experiment`): the
//! desktop facts a speech-level test supplies to every process in the loop,
//! in place of the system's.
//!
//! A test creates a named section of shared memory and names it in
//! [`ENV`]; every process Verbatim starts inherits the environment, so
//! Core, the supervisor, the listener and every outpost open the same
//! section the first time they ask, and read the facts from it. With the
//! variable unset, [`world`] is `None` and every caller reads the system.
//!
//! The facts: the window in front and the window with the keyboard focus,
//! which no window on a desktop made for a test has; the test clock; and
//! the listener's injected focus event, a window whose focused element the
//! listener takes as a UI Automation focus event's sender.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_ALL_ACCESS, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// The environment variable naming the world's section.
pub const ENV: &str = "VERBATIM_TEST_WORLD";

/// The section's layout, the same in every process.
#[repr(C)]
struct Shared {
    /// The window in front, 0 for none.
    foreground: AtomicIsize,
    /// The window with the keyboard focus, 0 for none.
    focus_window: AtomicIsize,
    /// The test clock, in microseconds, moved on only by the test.
    clock_us: AtomicU64,
    /// The window whose focused element the listener is told of as a UI
    /// Automation focus event, with each [`World::tell_focus`].
    told_focus_window: AtomicIsize,
}

/// One process's view of the world.
pub struct World {
    shared: &'static Shared,
    name: String,
    /// Set by the test each time it tells the listener of a focus.
    told: HANDLE,
    /// Set by the listener once it has taken a told focus in, as its focus
    /// callback takes a focus event in.
    taken: HANDLE,
}

// SAFETY: the view is mapped for the life of the process and only read and
// written through atomics; the event handle may be waited on from any
// thread.
unsafe impl Send for World {}
// SAFETY: as above.
unsafe impl Sync for World {}

static WORLD: OnceLock<Option<World>> = OnceLock::new();

/// The world this process runs in, when a test gives one.
///
/// # Panics
///
/// When [`ENV`] names a world that cannot be opened.
#[must_use]
pub fn world() -> Option<&'static World> {
    WORLD
        .get_or_init(|| {
            let name = std::env::var(ENV).ok()?;
            match World::open(&name) {
                Ok(world) => Some(world),
                Err(error) => panic!("the test world {name} could not be opened: {error}"),
            }
        })
        .as_ref()
}

/// The section's size.
const SIZE: u32 = 4096;

impl World {
    /// Creates the world `name` (`Local\` names one for this session), for
    /// the test that owns it; the handles are kept for the life of the
    /// process.
    ///
    /// # Errors
    ///
    /// The system's error when the section or the event cannot be made.
    pub fn create(name: &str) -> windows::core::Result<Self> {
        // SAFETY: a section backed by the paging file, no pointer passed.
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                SIZE,
                &HSTRING::from(name),
            )
        }?;
        // SAFETY: the event's name is a valid string.
        let told = unsafe { CreateEventW(None, false, false, &HSTRING::from(told_event(name))) }?;
        // SAFETY: as above.
        let taken = unsafe { CreateEventW(None, false, false, &HSTRING::from(taken_event(name))) }?;
        let world = Self::map(section, name, (told, taken))?;
        world
            .shared
            .clock_us
            .store(crate::clock::system_now_us(), Ordering::SeqCst);
        Ok(world)
    }

    fn open(name: &str) -> windows::core::Result<Self> {
        // SAFETY: the name is a valid string.
        let section =
            unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(name)) }?;
        // SAFETY: as above.
        let told =
            unsafe { OpenEventW(EVENT_ALL_ACCESS, false, &HSTRING::from(told_event(name))) }?;
        // SAFETY: as above.
        let taken =
            unsafe { OpenEventW(EVENT_ALL_ACCESS, false, &HSTRING::from(taken_event(name))) }?;
        Self::map(section, name, (told, taken))
    }

    fn map(
        section: HANDLE,
        name: &str,
        (told, taken): (HANDLE, HANDLE),
    ) -> windows::core::Result<Self> {
        // SAFETY: the section was just opened with read and write access.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        if view.Value.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        // The section's handle stays open for the life of the process: a
        // view alone keeps the section but not its name, which the other
        // processes open it by.
        let _ = section;
        // SAFETY: the view is page aligned, zeroed when made, at least
        // `SIZE` bytes, mapped for the life of the process, and `Shared` is
        // atomics only.
        let shared = unsafe { &*view.Value.cast::<Shared>() };
        Ok(Self {
            shared,
            name: name.to_owned(),
            told,
            taken,
        })
    }

    /// The world's name, to put in [`ENV`].
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The control plane's pipe in this world.
    #[must_use]
    pub fn control_pipe(&self) -> String {
        let suffix = self.name.rsplit('\\').next().unwrap_or(&self.name);
        format!(r"\\.\pipe\verbatim-control-{suffix}")
    }

    /// The window in front, 0 for none.
    #[must_use]
    pub fn foreground(&self) -> isize {
        self.shared.foreground.load(Ordering::SeqCst)
    }

    /// The window with the keyboard focus, `None` for none.
    #[must_use]
    pub fn focus_window(&self) -> Option<isize> {
        let window = self.shared.focus_window.load(Ordering::SeqCst);
        (window != 0).then_some(window)
    }

    /// Puts `window` in front, with the keyboard focus.
    pub fn set_foreground(&self, window: isize) {
        self.shared.foreground.store(window, Ordering::SeqCst);
        self.shared.focus_window.store(window, Ordering::SeqCst);
    }

    /// The test clock, in microseconds.
    #[must_use]
    pub fn now_us(&self) -> u64 {
        self.shared.clock_us.load(Ordering::SeqCst)
    }

    /// Moves the test clock on by `us`.
    pub fn pass_us(&self, us: u64) {
        self.shared.clock_us.fetch_add(us, Ordering::SeqCst);
    }

    /// Tells the listener of a UI Automation focus event whose sender is
    /// the element `window`'s provider reports focused, and waits until
    /// the listener has taken it in, as its focus callback takes an event
    /// in, at most `bound_ms`; answers whether it did.
    ///
    /// # Panics
    ///
    /// When the event cannot be set.
    #[must_use]
    pub fn tell_focus(&self, window: isize, bound_ms: u32) -> bool {
        self.shared
            .told_focus_window
            .store(window, Ordering::SeqCst);
        // SAFETY: the handle is open for the life of the process.
        unsafe { SetEvent(self.told) }.expect("the told event is set");
        // SAFETY: as above.
        let waited = unsafe { WaitForSingleObject(self.taken, bound_ms) };
        waited == WAIT_OBJECT_0
    }

    /// The listener's word that it has taken in the focus told of last.
    ///
    /// # Panics
    ///
    /// When the event cannot be set.
    pub fn told_focus_taken(&self) {
        // SAFETY: the handle is open for the life of the process.
        unsafe { SetEvent(self.taken) }.expect("the taken event is set");
    }

    /// Waits for the test to tell of a focus, and answers its window: the
    /// listener's stand-in for UI Automation's delivery of a focus event.
    #[must_use]
    pub fn next_told_focus(&self) -> isize {
        // SAFETY: the handle is open for the life of the process.
        unsafe { WaitForSingleObject(self.told, u32::MAX) };
        self.shared.told_focus_window.load(Ordering::SeqCst)
    }
}

fn told_event(name: &str) -> String {
    format!("{name}.told")
}

fn taken_event(name: &str) -> String {
    format!("{name}.taken")
}
