//! The foreground record: the foreground object, and the window the
//! user's focus line was last in
//! (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the
//! window title").
//!
//! Every fact of the record has one writer, a variant of
//! [`ForegroundFact`], applied through [`Foreground::apply`]; the reducer
//! reads the record through its accessors only, so a new writer is a new
//! variant, seen where the record is.

use serde::{Deserialize, Serialize};
use verbatim_model::{ObjectKey, WindowHandle};

/// One writer of the foreground record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundFact {
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
    /// The entered window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entered: Option<WindowHandle>,
}

impl Foreground {
    /// Applies one fact, the record's only write.
    pub(crate) fn apply(&mut self, fact: ForegroundFact) {
        match fact {
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

    /// The entered window: the top-level window the user's focus line was
    /// last in, set by a whole-window focus taken and a focus taken in the
    /// window in front, never by a foreground change.
    pub(crate) fn entered(&self) -> Option<WindowHandle> {
        self.entered
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::NodeId;

    use super::*;

    fn key(number: u64) -> ObjectKey {
        ObjectKey::of_id(NodeId::new(number))
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
