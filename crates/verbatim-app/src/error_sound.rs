//! The error sound: every error Verbatim logs is reported as the error
//! event (`Earcon::Error`), which the default theme plays as NVDA's
//! `error.wav`, as NVDA plays it for a logged error.
//!
//! A tracing layer sees every event at error level, on whatever thread
//! logged it, and only sends the earcon on to the reducer thread, which
//! plays it: playing it on the logging thread could reenter the speech
//! pipeline or the mixer from inside one of their own error paths. Until
//! the reducer thread's channel is set, errors play nothing.

use std::sync::OnceLock;

use crossbeam_channel::Sender;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use verbatim_model::Earcon;

use crate::ShellCommand;

/// Where the error earcon is sent: the reducer thread's command channel,
/// set once at startup.
static ERRORS: OnceLock<Sender<ShellCommand>> = OnceLock::new();

/// Sends every logged error's earcon to `commands` from now on.
pub(crate) fn report_to(commands: Sender<ShellCommand>) {
    let _ = ERRORS.set(commands);
}

/// The tracing layer reporting each error-level event as the error
/// earcon.
pub(crate) struct ErrorSoundLayer;

impl<S: Subscriber> Layer<S> for ErrorSoundLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if *event.metadata().level() == Level::ERROR
            && let Some(commands) = ERRORS.get()
        {
            // An unbounded send never waits; a closed channel means
            // Verbatim is exiting, and the sound no longer matters.
            let _ = commands.send(ShellCommand::PlayEarcon(Earcon::Error));
        }
    }
}
