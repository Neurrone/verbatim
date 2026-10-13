//! Narrowing the outpost's `WinEvent` hook to what windows read through UIA
//! need (`docs/design/wake-ups.md`, "Narrowing the outpost's hook").
//!
//! An out-of-context hook wakes the event thread for every event it is
//! subscribed to, before anything can drop it. The value, state, name,
//! description, selection, text selection and caret location events of a
//! window read through UIA are dropped on arrival ([`super::Context`]'s
//! `take_in_win_event`), and the worker drops them too: in a console
//! flood they are most of the console host's events, nearly all of them UI
//! Automation's own MSAA copies of its events. So while the application's
//! focus window and foreground window are both known to be read through
//! UIA, those kinds are unhooked ([`NARROWED`]), and as soon as either
//! moves to a window that is read through MSAA, or whose backend is not
//! yet decided, they are hooked again. Everything else stays hooked for
//! the outpost's life, since the worker uses it for UIA windows too: focus,
//! foreground, a menu opening, alerts, tooltips, a top-level window shown,
//! a window destroyed, and the console host's updates and caret.
//!
//! The decision is the event thread's own, on the thread that owns the
//! hooks: its focus and foreground windows are those of the focus,
//! menu-opening and foreground events its hook delivers, and, before the
//! hook has delivered one, those of the facts the listener routed, which
//! the reader hands it as nudges. It decides again whenever one of those
//! changes, when the worker has just kept a verdict of UIA for a window,
//! and when Core changes the backend override.
//!
//! What can be lost: an event of a narrowed kind raised by a window read
//! through MSAA after the focus or foreground moved into it but before the
//! event thread has taken that focus or foreground event and hooked the
//! kinds again, typically a fraction of a millisecond; and such events
//! from a window of the application that is read through MSAA but is
//! neither its focus window nor its foreground window, which the worker
//! accepts only from a topmost window such as a menu or a drop-down list,
//! whose own focus or menu-opening event hooks them again.

use verbatim_ia2::{WinEvent, WinEventKind};

use super::Context;
use super::window;
use crate::arbitration::{EventWindows, WindowClasses};
use std::sync::atomic::Ordering;

/// The kinds unhooked while the application's focus and foreground windows
/// are both read through UIA: those its event thread drops for a window
/// read through UIA, and that the worker uses for no such window.
pub(super) const NARROWED: &[WinEventKind] = &[
    WinEventKind::ValueChange,
    WinEventKind::StateChange,
    WinEventKind::NameChange,
    WinEventKind::DescriptionChange,
    WinEventKind::Selection,
    WinEventKind::Caret,
    WinEventKind::TextSelectionChange,
];

/// What the event thread is nudged with, from another thread of the
/// outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Nudge {
    /// The listener routed a focus, or a menu opening, in window `0`.
    RoutedFocus(isize),
    /// The listener routed a foreground change to window `0`.
    RoutedForeground(isize),
    /// Decide again: a window's backend may have been decided.
    Decide,
}

impl Nudge {
    /// The nudge as the event thread's message carries it.
    pub(super) fn words(self) -> (usize, isize) {
        match self {
            Self::RoutedFocus(hwnd) => (1, hwnd),
            Self::RoutedForeground(hwnd) => (2, hwnd),
            Self::Decide => (3, 0),
        }
    }

    /// The nudge a message carries.
    pub(super) fn from_words(word: usize, hwnd: isize) -> Option<Self> {
        match word {
            1 => Some(Self::RoutedFocus(hwnd)),
            2 => Some(Self::RoutedForeground(hwnd)),
            3 => Some(Self::Decide),
            _ => None,
        }
    }
}

/// A window the event thread noted as the focus's or the foreground's.
#[derive(Clone, Copy, Debug)]
struct Noted {
    hwnd: isize,
    /// Whether the outpost's own hook delivered it. The listener forwards
    /// only events raised before the hook was in, so one it routed never
    /// replaces a window the hook noted.
    from_hook: bool,
}

/// The event thread's own record, which only it touches: its windows, the
/// application's focus and foreground windows, and whether the hook is
/// narrowed.
#[derive(Default)]
pub(super) struct EventThreadState {
    /// What the event thread knows of each window it takes events from.
    pub(super) windows: EventWindows,
    focus: Option<Noted>,
    foreground: Option<Noted>,
    narrowed: bool,
}

impl EventThreadState {
    /// Notes the window of a focus, menu-opening or foreground event the
    /// hook delivered, and decides again.
    pub(super) fn note_win_event(&mut self, event: &WinEvent, context: &Context) {
        if event.hwnd == 0 {
            return;
        }
        let noted = Some(Noted {
            hwnd: event.hwnd,
            from_hook: true,
        });
        match event.kind {
            WinEventKind::Focus | WinEventKind::MenuPopupStart => self.focus = noted,
            WinEventKind::Foreground => self.foreground = noted,
            _ => return,
        }
        self.decide(context);
    }

    /// Takes `nudge` from another thread of the outpost, and decides
    /// again.
    pub(super) fn nudged(&mut self, nudge: Nudge, context: &Context) {
        let routed = |slot: &mut Option<Noted>, hwnd: isize| {
            if hwnd != 0 && !slot.is_some_and(|noted| noted.from_hook) {
                *slot = Some(Noted {
                    hwnd,
                    from_hook: false,
                });
            }
        };
        match nudge {
            Nudge::RoutedFocus(hwnd) => routed(&mut self.focus, hwnd),
            Nudge::RoutedForeground(hwnd) => routed(&mut self.foreground, hwnd),
            Nudge::Decide => {}
        }
        self.decide(context);
    }

    /// Narrows the hook while the focus and foreground windows are both
    /// known to be read through UIA, and hooks every kind again otherwise.
    fn decide(&mut self, context: &Context) {
        let overrides = context.override_changes.load(Ordering::Acquire);
        let mut known = |hwnd: isize| {
            self.windows.known_to_use_uia(
                (&context.arbitrator, overrides),
                (hwnd, window::window_thread(hwnd)),
                WindowClasses::of,
            )
        };
        let narrow = match (self.focus, self.foreground) {
            (Some(focus), Some(foreground)) => known(focus.hwnd) && known(foreground.hwnd),
            _ => false,
        };
        if narrow == self.narrowed {
            return;
        }
        let suspended = if narrow { NARROWED } else { &[] };
        match verbatim_ia2::suspend_kinds(suspended) {
            Ok(()) => {
                self.narrowed = narrow;
                tracing::debug!(
                    narrow,
                    focus = self.focus.map(|noted| noted.hwnd),
                    foreground = self.foreground.map(|noted| noted.hwnd),
                    "the hook's change events of windows read through MSAA {}",
                    if narrow { "unhooked" } else { "hooked again" }
                );
            }
            // Left as it was believed to be, so the next decision makes
            // the rest of the change.
            Err(error) => tracing::warn!(error, narrow, "the hook could not be changed"),
        }
    }
}
