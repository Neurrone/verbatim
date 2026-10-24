//! The error sound: every error Verbatim logs is reported as the error
//! event (`Earcon::Error`), which the default theme plays as NVDA's
//! `error.wav`, as NVDA plays it for a logged error.
//!
//! A tracing layer sees every event at error level, on whatever thread
//! logged it, and only tells the reducer (`Input::ErrorLogged`), which
//! plays the earcon: playing it on the logging thread could reenter the
//! speech pipeline or the mixer from inside one of their own error paths.
//! Until the reducer thread's port is set, errors play nothing.

use std::sync::OnceLock;

use crate::Commands;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use verbatim_model::Input;

use crate::ShellCommand;

/// Where each logged error is posted: the reducer thread's port, set once
/// at startup.
static ERRORS: OnceLock<Commands> = OnceLock::new();

/// Tells `commands` of every error logged from now on.
pub(crate) fn report_to(commands: Commands) {
    let _ = ERRORS.set(commands);
}

/// The tracing layer telling the reducer of each error-level event.
pub(crate) struct ErrorSoundLayer;

impl<S: Subscriber> Layer<S> for ErrorSoundLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if *event.metadata().level() == Level::ERROR
            && let Some(commands) = ERRORS.get()
        {
            // A post never waits; a refused one means
            // Verbatim is exiting, and the sound no longer matters.
            let _ = commands.send(ShellCommand::Input(Box::new(Input::ErrorLogged)));
        }
    }
}
