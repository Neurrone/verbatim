//! The lifecycle of the GUI's dialogs as one state machine, with no widget
//! code: which dialogs are open, whether a shell item enumeration is in
//! flight, whether shutdown has begun, and what each request or close
//! means for the hidden frame.
//!
//! The rules:
//!
//! - The settings dialog and the shell item list are singletons. A request
//!   to open one that is already open focuses it instead, as NVDA does.
//! - A shell item list request while an enumeration is in flight is
//!   dropped rather than queued; one that arrives after shutdown began is
//!   ignored, as are enumeration results that arrive after it.
//! - The hidden frame is shown around every popup (NVDA's `prePopup`) and
//!   hidden again (`postPopup`) once the menu has closed and no dialog
//!   still needs it as its visible owner: hiding an owner can take its
//!   owned windows with it.
//!
//! The widget layer asks this machine what to do and carries the answer
//! out; it never keeps its own copy of these facts.

/// One of the GUI's modeless dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// The settings dialog.
    Settings,
    /// The system tray or taskbar item list.
    ShellList,
}

/// What a request to open the settings dialog should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenSettings {
    /// No dialog exists: build and show one.
    Create,
    /// The dialog is open: raise and focus it.
    FocusExisting,
    /// Shutdown has begun: do nothing.
    Ignore,
}

/// What a request for the shell item list should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenShellList {
    /// Start an enumeration; the list is presented when it finishes.
    Enumerate,
    /// The list is open: raise and focus it.
    FocusExisting,
    /// An enumeration is already in flight, or shutdown has begun: do
    /// nothing.
    Ignore,
}

/// Whether the hidden frame should be hidden now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Hide it: nothing needs it as a visible owner any more.
    Hide,
    /// Keep it shown: a dialog it owns is still open.
    Keep,
}

/// The state of the shell item list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ShellList {
    #[default]
    Closed,
    Enumerating,
    Open,
}

/// The GUI's dialog lifecycle.
#[derive(Debug, Default)]
pub(crate) struct Lifecycle {
    settings_open: bool,
    shell_list: ShellList,
    shutting_down: bool,
}

impl Lifecycle {
    /// A lifecycle with nothing open.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Decides a request to open the settings dialog. A `Create` answer
    /// records the dialog as open.
    pub(crate) fn request_settings(&mut self) -> OpenSettings {
        if self.shutting_down {
            OpenSettings::Ignore
        } else if self.settings_open {
            OpenSettings::FocusExisting
        } else {
            self.settings_open = true;
            OpenSettings::Create
        }
    }

    /// Decides a request for the shell item list. An `Enumerate` answer
    /// records the enumeration as in flight.
    pub(crate) fn request_shell_list(&mut self) -> OpenShellList {
        match self.shell_list {
            _ if self.shutting_down => OpenShellList::Ignore,
            ShellList::Open => OpenShellList::FocusExisting,
            ShellList::Enumerating => OpenShellList::Ignore,
            ShellList::Closed => {
                self.shell_list = ShellList::Enumerating;
                OpenShellList::Enumerate
            }
        }
    }

    /// Records that an enumeration finished, `found` saying whether it
    /// produced a list. Returns whether to present it; presenting records
    /// the list as open.
    pub(crate) fn shell_items_arrived(&mut self, found: bool) -> bool {
        if self.shell_list != ShellList::Enumerating {
            return false;
        }
        if found && !self.shutting_down {
            self.shell_list = ShellList::Open;
            true
        } else {
            self.shell_list = ShellList::Closed;
            false
        }
    }

    /// Records that `dialog` closed, and says what to do with the frame.
    pub(crate) fn closed(&mut self, dialog: Dialog) -> Frame {
        match dialog {
            Dialog::Settings => self.settings_open = false,
            Dialog::ShellList => self.shell_list = ShellList::Closed,
        }
        self.frame_after_popup()
    }

    /// What to do with the frame once a popup, the menu or a dialog, is
    /// gone.
    pub(crate) fn frame_after_popup(&self) -> Frame {
        if self.settings_open || self.shell_list == ShellList::Open {
            Frame::Keep
        } else {
            Frame::Hide
        }
    }

    /// Records that shutdown began: every later request is ignored.
    pub(crate) fn shut_down(&mut self) {
        self.shutting_down = true;
        self.settings_open = false;
        self.shell_list = ShellList::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_is_a_singleton() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.request_settings(), OpenSettings::Create);
        assert_eq!(
            lifecycle.request_settings(),
            OpenSettings::FocusExisting,
            "a second request while open focuses the existing dialog"
        );
        assert_eq!(lifecycle.closed(Dialog::Settings), Frame::Hide);
        assert_eq!(
            lifecycle.request_settings(),
            OpenSettings::Create,
            "after closing, the next request builds a fresh dialog"
        );
    }

    #[test]
    fn a_shell_list_request_during_enumeration_is_dropped() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::Enumerate);
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::Ignore);
        assert!(lifecycle.shell_items_arrived(true));
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::FocusExisting);
    }

    #[test]
    fn a_failed_enumeration_presents_nothing_and_allows_a_retry() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::Enumerate);
        assert!(!lifecycle.shell_items_arrived(false));
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::Enumerate);
    }

    #[test]
    fn unrequested_results_are_not_presented() {
        let mut lifecycle = Lifecycle::new();
        assert!(!lifecycle.shell_items_arrived(true));
    }

    #[test]
    fn the_frame_stays_while_any_dialog_needs_it() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(lifecycle.frame_after_popup(), Frame::Hide);
        lifecycle.request_settings();
        assert_eq!(
            lifecycle.frame_after_popup(),
            Frame::Keep,
            "the menu closing must not hide the settings dialog's owner"
        );
        lifecycle.request_shell_list();
        assert!(lifecycle.shell_items_arrived(true));
        assert_eq!(
            lifecycle.closed(Dialog::Settings),
            Frame::Keep,
            "the shell list still needs the frame"
        );
        assert_eq!(lifecycle.closed(Dialog::ShellList), Frame::Hide);
    }

    #[test]
    fn an_enumeration_in_flight_does_not_keep_the_frame() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.request_shell_list();
        assert_eq!(lifecycle.frame_after_popup(), Frame::Hide);
    }

    #[test]
    fn shutdown_ignores_later_requests_and_results() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.request_shell_list();
        lifecycle.shut_down();
        assert!(!lifecycle.shell_items_arrived(true));
        assert_eq!(lifecycle.request_settings(), OpenSettings::Ignore);
        assert_eq!(lifecycle.request_shell_list(), OpenShellList::Ignore);
    }
}
