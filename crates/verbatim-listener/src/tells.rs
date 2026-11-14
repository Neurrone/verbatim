//! Measurement prototype (branch `prototype/background-events`): the
//! listener tells Core of every MSAA event of the kinds an outpost hooks
//! for its application, hooked desktop-wide, so that the parent session can
//! measure what ADR 0003's limit "Every background event reaches Core"
//! costs. Not product code.
//!
//! Each event of those kinds that survives NVDA's local drops, checked live
//! as it arrives (a null or invalid window, `MSNHiddenWindowClass`, a
//! destroy of anything but a window, a hung or ghosted window), has the
//! window facts the acceptance test reads gathered (ADR 0003, "Acceptance
//! judged in Core"; `docs/parity/focus-events.md`, "Event acceptance"), and
//! goes to Core as one tell, through the listener's merge-only limiter
//! (one waiting tell per object, kind and raising thread).
//!
//! With `VERBATIM_PROTOTYPE_FOREGROUND_FILTER=1`, read once at start, the
//! ruled fallback is on: a change event is told only from a process of the
//! foreground window's tree (its owner and the owners of its child windows,
//! read again when the foreground window changes or half a second has
//! passed).
//!
//! Every five seconds while events arrive, the counts are logged at info on
//! the `verbatim::prototype` target, with the mean time the drops and the
//! facts took per told event.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GA_ROOT, GA_ROOTOWNER, GetAncestor,
};
use windows::core::BOOL;

use verbatim_ia2::{WinEvent, WinEventKind};
use verbatim_model::protocol::{Tell, TellKind};
use verbatim_model::{Pid, WindowFacts, WindowHandle};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::{Readability, readability, window_class_name, window_owner};

/// The kinds an outpost hooks for its application that the listener did
/// not hook before this prototype, hooked desktop-wide beside the
/// listener's own (`verbatim_ia2::APP_SUBSCRIPTIONS` less the console's
/// five events, which a console host's outpost keeps under ADR 0003).
pub(crate) const ADDED_KINDS: [WinEventKind; 7] = [
    WinEventKind::StateChange,
    WinEventKind::NameChange,
    WinEventKind::DescriptionChange,
    WinEventKind::Selection,
    WinEventKind::Destroy,
    WinEventKind::Caret,
    WinEventKind::TextSelectionChange,
];

/// The variable that turns the ruled fallback on.
const FILTER_VARIABLE: &str = "VERBATIM_PROTOTYPE_FOREGROUND_FILTER";

/// How often the counts are logged.
const SPAN: Duration = Duration::from_secs(5);

/// How long the foreground's processes are kept before they are read again
/// for the same foreground window.
const PROCESSES_KEPT: Duration = Duration::from_millis(500);

/// The window object's id, `OBJID_WINDOW`.
const OBJID_WINDOW: i32 = 0;

/// The tell's kind for an event of `kind`, or `None` for an event that is
/// not told: the foreground change, which has its own report, the ends of
/// menus and the switcher, the desktop's events and the console's.
pub(crate) fn tell_kind(kind: WinEventKind) -> Option<TellKind> {
    Some(match kind {
        WinEventKind::Focus => TellKind::Focus,
        WinEventKind::ValueChange => TellKind::ValueChange,
        WinEventKind::StateChange => TellKind::StateChange,
        WinEventKind::NameChange => TellKind::NameChange,
        WinEventKind::DescriptionChange => TellKind::DescriptionChange,
        WinEventKind::Selection => TellKind::Selection,
        WinEventKind::MenuPopupStart => TellKind::MenuPopupStart,
        WinEventKind::Alert => TellKind::Alert,
        WinEventKind::Show => TellKind::Show,
        WinEventKind::Destroy => TellKind::Destroy,
        WinEventKind::Caret => TellKind::Caret,
        WinEventKind::TextSelectionChange => TellKind::TextSelectionChange,
        WinEventKind::Foreground
        | WinEventKind::MenuEnd
        | WinEventKind::SwitchEnd
        | WinEventKind::ConsoleUpdate
        | WinEventKind::ShellForeground
        | WinEventKind::ConsoleCaret
        | WinEventKind::DesktopSwitch
        | WinEventKind::DesktopName => return None,
    })
}

/// Whether an event of `kind` is a change event, which the fallback
/// filters by the foreground's processes. A focus, a menu opening, an
/// alert, a tooltip's show and a window's destruction are not.
fn is_change(kind: TellKind) -> bool {
    matches!(
        kind,
        TellKind::ValueChange
            | TellKind::StateChange
            | TellKind::NameChange
            | TellKind::DescriptionChange
            | TellKind::Selection
            | TellKind::Caret
            | TellKind::TextSelectionChange
    )
}

/// Whether only this prototype hooks `kind` in the listener, so today's
/// handling never sees it.
pub(crate) fn added(kind: WinEventKind) -> bool {
    ADDED_KINDS.contains(&kind)
}

/// What became of an event offered to [`Teller::offer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Offered {
    /// Not a kind that is told.
    NotTold,
    /// Dropped by one of NVDA's local drops, or from an ignored process.
    Dropped,
    /// Dropped by the fallback's filter.
    Filtered,
    /// Told, with the time its drops and facts took.
    Told(Duration),
}

/// The processes of the foreground window's tree, read with local calls.
struct ForegroundProcesses {
    window: isize,
    read_at: Instant,
    pids: Vec<u32>,
}

impl ForegroundProcesses {
    /// The processes of `foreground`'s tree: its owner and the owners of
    /// its child windows.
    fn read(foreground: isize) -> Self {
        unsafe extern "system" fn visit(window: HWND, pids: LPARAM) -> BOOL {
            // SAFETY: `pids` points to the vector below, alive for the
            // synchronous enumeration.
            let pids = unsafe { &mut *(pids.0 as *mut Vec<u32>) };
            let pid = window_owner(window.0 as isize).1;
            if pid != 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
            BOOL::from(true)
        }
        let mut pids = vec![window_owner(foreground).1];
        if foreground != 0 {
            // SAFETY: `visit` reads only the vector, which outlives the
            // synchronous call.
            unsafe {
                let _ = EnumChildWindows(
                    Some(HWND(foreground as *mut core::ffi::c_void)),
                    Some(visit),
                    LPARAM((&raw mut pids) as isize),
                );
            }
        }
        Self {
            window: foreground,
            read_at: Instant::now(),
            pids,
        }
    }
}

/// Counts of one span.
#[derive(Default)]
struct Span {
    offered: u64,
    told: u64,
    dropped: u64,
    filtered: u64,
    gathering: Duration,
}

/// The event thread's teller: the fallback's switch, the foreground's
/// processes it last read, and the span's counts. Lives on the event
/// thread alone.
pub(crate) struct Teller {
    filter: bool,
    foreground: RefCell<Option<ForegroundProcesses>>,
    span: RefCell<(Instant, Span)>,
}

impl Teller {
    /// A teller, the fallback's switch read from the environment now.
    pub(crate) fn from_environment() -> Self {
        let filter = std::env::var(FILTER_VARIABLE).is_ok_and(|value| value == "1");
        tracing::info!(
            target: "verbatim::prototype",
            filter,
            "the listener tells Core of every event of the hooked kinds"
        );
        Self {
            filter,
            foreground: RefCell::new(None),
            span: RefCell::new((Instant::now(), Span::default())),
        }
    }

    /// Offers `event`, observed at `observed_at_us`: told through `send`
    /// unless it is not a kind that is told, a local drop drops it, `ignored`
    /// names its process, or the fallback filters it. Counts it, and logs
    /// the span's counts once five seconds have passed.
    pub(crate) fn offer(
        &self,
        event: WinEvent,
        ignored: impl Fn(Pid) -> bool,
        send: impl FnOnce(Tell),
    ) -> Offered {
        let offered = self.judge(event, ignored, send);
        self.count(offered);
        offered
    }

    fn judge(
        &self,
        event: WinEvent,
        ignored: impl Fn(Pid) -> bool,
        send: impl FnOnce(Tell),
    ) -> Offered {
        let Some(kind) = tell_kind(event.kind) else {
            return Offered::NotTold;
        };
        let started = Instant::now();
        let WinEvent {
            hwnd,
            id_object,
            id_child,
            thread,
            time,
            ..
        } = event;
        // A destroy is told for a window alone.
        if kind == TellKind::Destroy && (id_object != OBJID_WINDOW || id_child != 0) {
            return Offered::Dropped;
        }
        if hwnd == 0 {
            return Offered::Dropped;
        }
        let (_, owner) = window_owner(hwnd);
        if owner == 0 {
            return Offered::Dropped;
        }
        let class = window_class_name(hwnd);
        if class == "MSNHiddenWindowClass" {
            return Offered::Dropped;
        }
        let application = if class == CONSOLE_WINDOW_CLASS {
            match crate::listener::process_of_thread(thread) {
                Some(host) => host,
                None => return Offered::Dropped,
            }
        } else {
            owner
        };
        if ignored(Pid(application)) || ignored(Pid(owner)) {
            return Offered::Dropped;
        }
        let foreground = verbatim_win32::window::foreground_window_handle();
        if self.filter
            && is_change(kind)
            && !self.in_foreground_processes(foreground, [application, owner])
        {
            return Offered::Filtered;
        }
        if readability(hwnd) != Readability::Readable {
            return Offered::Dropped;
        }
        let facts = window_facts(hwnd, class, foreground);
        send(Tell {
            kind,
            application: Pid(application),
            window: hwnd,
            id_object,
            id_child,
            thread,
            raised_at: time,
            facts,
        });
        Offered::Told(started.elapsed())
    }

    /// Whether either of `pids` is a process of `foreground`'s tree.
    fn in_foreground_processes(&self, foreground: isize, pids: [u32; 2]) -> bool {
        let mut kept = self.foreground.borrow_mut();
        let stale = kept.as_ref().is_none_or(|kept| {
            kept.window != foreground || kept.read_at.elapsed() >= PROCESSES_KEPT
        });
        if stale {
            *kept = Some(ForegroundProcesses::read(foreground));
        }
        kept.as_ref()
            .is_some_and(|kept| pids.iter().any(|pid| kept.pids.contains(pid)))
    }

    fn count(&self, offered: Offered) {
        let mut span = self.span.borrow_mut();
        let (started, counts) = &mut *span;
        match offered {
            Offered::NotTold => return,
            Offered::Dropped => counts.dropped += 1,
            Offered::Filtered => counts.filtered += 1,
            Offered::Told(took) => {
                counts.told += 1;
                counts.gathering += took;
            }
        }
        counts.offered += 1;
        let elapsed = started.elapsed();
        if elapsed >= SPAN {
            let counts = std::mem::take(counts);
            let mean_ns = u64::try_from(counts.gathering.as_nanos())
                .unwrap_or(u64::MAX)
                .checked_div(counts.told)
                .unwrap_or(0);
            tracing::info!(
                target: "verbatim::prototype",
                span_ms = elapsed.as_millis(),
                offered = counts.offered,
                told = counts.told,
                dropped = counts.dropped,
                filtered = counts.filtered,
                gathering_mean_ns = mean_ns,
                "the listener's tells in the span"
            );
            *started = Instant::now();
        }
    }
}

/// The window facts the acceptance test reads, for `handle`, of class
/// `class`, against the foreground window `foreground`: as an outpost
/// gathers them today (`verbatim-outpost`'s `window_facts`), with local
/// calls only.
fn window_facts(handle: isize, class: String, foreground: isize) -> WindowFacts {
    let window = HWND(handle as *mut core::ffi::c_void);
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    // SAFETY: as above.
    let root_owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    let root = if root.0.is_null() { window } else { root };
    let root_owner = if root_owner.0.is_null() {
        root
    } else {
        root_owner
    };
    let relation = verbatim_reader_rules::Window::new(handle).map_or(
        verbatim_core::focus_rules::ForegroundRelation {
            in_foreground: false,
            topmost: false,
            under_active_window: None,
        },
        |window| {
            verbatim_reader_rules::acceptance::foreground_relation(
                window,
                &class,
                &verbatim_win32::window::SystemWindows { foreground },
            )
        },
    );
    WindowFacts {
        top_level: handle_of(root.0 as isize),
        root_owner: handle_of(root_owner.0 as isize),
        topmost: relation.topmost,
        under_active_window: relation.under_active_window,
        in_foreground: relation.in_foreground,
        foreground_window: handle_of(foreground),
        class,
        application: String::new(),
        alt_held: false,
        foreground_is_shell: verbatim_ia2::is_shell_foreground_class(&window_class_name(
            foreground,
        )),
    }
}

/// `handle` as the model's opaque window handle.
fn handle_of(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds told are every kind an outpost hooks but the foreground
    /// change and the console's events; every added kind is told.
    #[test]
    fn every_kind_an_outpost_hooks_is_told_but_the_foreground_and_the_consoles() {
        let told: Vec<WinEventKind> = verbatim_ia2::APP_SUBSCRIPTIONS
            .iter()
            .copied()
            .filter(|&kind| tell_kind(kind).is_some())
            .collect();
        let untold: Vec<WinEventKind> = verbatim_ia2::APP_SUBSCRIPTIONS
            .iter()
            .copied()
            .filter(|&kind| tell_kind(kind).is_none())
            .collect();
        assert_eq!(
            untold,
            [
                WinEventKind::Foreground,
                WinEventKind::ConsoleUpdate,
                WinEventKind::ConsoleCaret
            ]
        );
        assert!(ADDED_KINDS.iter().all(|kind| told.contains(kind)));
        assert!(
            ADDED_KINDS
                .iter()
                .all(|kind| !verbatim_ia2::LISTENER_SUBSCRIPTIONS.contains(kind))
        );
    }

    /// A destroy of an object other than a window is a local drop, before
    /// any read of its window.
    #[test]
    fn a_destroy_of_anything_but_a_window_is_dropped() {
        let teller = Teller {
            filter: false,
            foreground: RefCell::new(None),
            span: RefCell::new((Instant::now(), Span::default())),
        };
        let event = WinEvent {
            kind: WinEventKind::Destroy,
            hwnd: 0x1234,
            id_object: -4,
            id_child: 0,
            thread: 1,
            time: 0,
            raised_ms_ago: 0,
        };
        assert_eq!(
            teller.offer(event, |_| false, |_| panic!("not told")),
            Offered::Dropped
        );
    }

    /// An event with no window is dropped.
    #[test]
    fn an_event_with_no_window_is_dropped() {
        let teller = Teller {
            filter: false,
            foreground: RefCell::new(None),
            span: RefCell::new((Instant::now(), Span::default())),
        };
        let event = WinEvent {
            kind: WinEventKind::NameChange,
            hwnd: 0,
            id_object: -4,
            id_child: 0,
            thread: 1,
            time: 0,
            raised_ms_ago: 0,
        };
        assert_eq!(
            teller.offer(event, |_| false, |_| panic!("not told")),
            Offered::Dropped
        );
    }
}
