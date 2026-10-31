//! The settings dialog's Terminal page's view of Verbatim: the reader
//! settings as the reducer thread publishes them, and Core's reducer, which
//! a change is sent to.
//!
//! A change is merged on the reducer thread into the settings the reducer
//! holds, then given to it as `Input::Settings` and saved, so a toggle key
//! such as Verbatim+5, which the reducer handles on the same thread, is
//! never undone by a change from the dialog.

use std::sync::{Arc, Mutex, PoisonError};

use crate::Commands;
use crate::saver::ReaderWriter;
use verbatim_gui::{TerminalChange, TerminalHost};
use verbatim_model::ReaderSettings;

use crate::ShellCommand;

/// The reader settings as the reducer thread last set them, which it alone
/// writes and the Terminal page reads (`docs/design/ownership.md`): each a copy under a lock never
/// held across a call.
pub(crate) struct ReaderView(Mutex<ReaderSettings>);

impl ReaderView {
    pub(crate) fn new(settings: ReaderSettings) -> Self {
        Self(Mutex::new(settings))
    }

    fn get(&self) -> ReaderSettings {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, settings: ReaderSettings) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = settings;
    }
}

/// The reader section of the settings, which the reducer thread owns: its
/// writer, and the view it publishes.
pub(crate) struct ReaderSection {
    pub(crate) writer: ReaderWriter,
    pub(crate) view: Arc<ReaderView>,
}

impl ReaderSection {
    /// The reducer's reader settings changed: published for the Terminal
    /// page and handed to the saver.
    pub(crate) fn changed(&self, settings: ReaderSettings) {
        self.view.set(settings);
        self.writer.save(settings);
    }
}

/// The Terminal page's host, over the reducer thread's view of its reader
/// settings and its port.
pub(crate) struct AppTerminalHost {
    pub(crate) view: Arc<ReaderView>,
    pub(crate) commands: Commands,
}

impl TerminalHost for AppTerminalHost {
    /// The reducer's own reader settings, as it last published them.
    fn reader_settings(&self) -> ReaderSettings {
        self.view.get()
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
