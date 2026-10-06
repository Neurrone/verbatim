//! The settings dialog's Terminal page's view of Verbatim: the reader
//! settings as the configuration has them, and Core's reducer, which a
//! change is sent to.
//!
//! A change is merged on the reducer thread into the settings the reducer
//! holds, then given to it as `Input::Settings` and saved, so a toggle key
//! such as Verbatim+5, which the reducer handles on the same thread, is
//! never undone by a change from the dialog.

use std::sync::{Arc, Mutex, PoisonError};

use crossbeam_channel::Sender;
use verbatim_config::ConfigStore;
use verbatim_gui::{TerminalChange, TerminalHost};
use verbatim_model::ReaderSettings;

use crate::ShellCommand;

/// The Terminal page's host, over the configuration store and the
/// reducer thread's command channel.
pub(crate) struct AppTerminalHost {
    pub(crate) store: Arc<Mutex<ConfigStore>>,
    pub(crate) commands: Sender<ShellCommand>,
}

impl TerminalHost for AppTerminalHost {
    /// The configuration's reader settings, which the reducer thread saves
    /// whenever the reducer changes them, so they are the reducer's own.
    fn reader_settings(&self) -> ReaderSettings {
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .settings()
            .reader
    }

    fn change(&self, change: TerminalChange) {
        if self
            .commands
            .send(ShellCommand::TerminalSettings(change))
            .is_err()
        {
            tracing::debug!("the reducer thread is gone; the terminal settings are not changed");
        }
    }
}
