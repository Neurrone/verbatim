//! The foreground record: the window the focus listener last reported in
//! front, which the keyboard hook reads, the foreground object, and the
//! window the user's focus line was last in
//! (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the
//! window title").
//!
//! Every fact of the record has one writer, a variant of
//! [`ForegroundFact`], applied through [`Foreground::apply`]; the reducer
//! reads the record through its accessors only, so a new writer is a new
//! variant, seen where the record is.

use serde::{Deserialize, Serialize};
use verbatim_model::{ForegroundWindow, ObjectKey, WindowHandle};

/// One writer of the foreground record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundFact {
    /// The focus listener reported this window in the system's foreground:
    /// the keyboard hook's view of it; one of Verbatim's own takes the user
    /// out of the entered window.
    Changed(ForegroundWindow),
    /// A whole-window focus was taken, or was refused as the window
    /// holding Core's focus: its window is the foreground object.
    Object(ObjectKey),
    /// A whole-window focus was taken: the window the user's focus line is
    /// in, the focus's own [`Object`](Self::Object) giving the foreground
    /// object.
    Entered(WindowHandle),
    /// A focus that entered the window in front was taken: the window the
    /// user's focus line is in, and its chain's outermost entry, if any, as
    /// the foreground object.
    EnteredBy(WindowHandle, Option<ObjectKey>),
}

/// The foreground record ([module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Foreground {
    /// The foreground object, as the identity rule compares it.
    #[serde(rename = "foreground")]
    object: Option<ObjectKey>,
    /// The window the listener last reported in front.
    #[serde(rename = "foreground_window")]
    window: Option<ForegroundWindow>,
    /// The entered window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entered: Option<WindowHandle>,
}

impl Foreground {
    /// Applies one fact, the record's only write.
    pub(crate) fn apply(&mut self, fact: ForegroundFact) {
        match fact {
            ForegroundFact::Changed(change) => {
                self.window = Some(change);
                // Verbatim's own window in front, its menu's hidden frame
                // or one of its dialogs, takes the user out of the entered
                // window, so the focus of the return says its title even
                // when it is the return's only report.
                if change.own {
                    self.entered = None;
                }
            }
            ForegroundFact::Object(object) => self.object = Some(object),
            ForegroundFact::Entered(window) => self.entered = Some(window),
            ForegroundFact::EnteredBy(window, object) => {
                self.entered = Some(window);
                self.object = object;
            }
        }
    }

    /// The window most recently reported as the foreground, as the
    /// identity rule compares it, whose speech stays valid while it is in
    /// front (`FocusNow`).
    pub(crate) fn object(&self) -> Option<ObjectKey> {
        self.object
    }

    /// The window the focus listener last reported in the system's
    /// foreground (`Input::Foreground`), one of Verbatim's own included:
    /// the keyboard hook's view.
    pub(crate) fn window(&self) -> Option<ForegroundWindow> {
        self.window
    }

    /// The entered window: the top-level window the user's focus line was
    /// last in, set by a whole-window focus taken and a focus taken in the
    /// window in front, never by a foreground change, and cleared by a
    /// foreground change into one of Verbatim's own windows.
    pub(crate) fn entered(&self) -> Option<WindowHandle> {
        self.entered
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{NodeId, Pid};

    use super::*;

    fn key(number: u64) -> ObjectKey {
        ObjectKey::of_id(NodeId::new(number))
    }

    fn change(window: u64, own: bool) -> ForegroundWindow {
        ForegroundWindow {
            window: WindowHandle(window),
            root_owner: WindowHandle(window + 1),
            process: Pid(3),
            observed_at_us: 40,
            own,
            thread: 0,
            console: false,
            integrity: None,
            own_integrity: None,
        }
    }

    /// The listener's change is the keyboard hook's view, and moves
    /// neither the foreground object nor the entered window.
    #[test]
    fn a_foreground_change_is_the_hooks_view() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Entered(WindowHandle(5)));
        record.apply(ForegroundFact::Changed(change(7, false)));
        assert_eq!(record.window(), Some(change(7, false)));
        assert_eq!(
            (record.object(), record.entered()),
            (Some(key(1)), Some(WindowHandle(5)))
        );
    }

    /// A change to one of Verbatim's own windows takes the user out of the
    /// entered window.
    #[test]
    fn a_change_to_verbatims_own_window_leaves_the_entered_window() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Entered(WindowHandle(5)));
        record.apply(ForegroundFact::Changed(change(9, true)));
        assert_eq!(record.window(), Some(change(9, true)));
        assert_eq!(record.entered(), None);
    }

    /// A focus entering a window sets the entered window and its chain's
    /// outermost entry as the foreground object; a whole-window focus sets
    /// the entered window alone.
    #[test]
    fn an_entered_window_is_kept_with_its_object() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Entered(WindowHandle(5)));
        assert_eq!(
            (record.entered(), record.object()),
            (Some(WindowHandle(5)), Some(key(1)))
        );
        record.apply(ForegroundFact::EnteredBy(WindowHandle(6), Some(key(2))));
        assert_eq!(
            (record.entered(), record.object()),
            (Some(WindowHandle(6)), Some(key(2)))
        );
    }

    #[test]
    fn the_foreground_object_is_its_reports_window() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(5)));
        assert_eq!(record.object(), Some(key(5)));
    }
}
