//! `verbatim-synth-host.exe`: one synthesizer, in its own process
//! (decision D18).
//!
//! Core launches it, contained in a kill-on-close job, as
//! `--pipe-in <handle> --pipe-out <handle> --synth <id>`, over two
//! overlapped pipes, and talks to it with the protocol in
//! `verbatim_speech::hosting`. The host builds the named synthesizer's
//! driver and serves Core on its one thread ([`verbatim_synth_host::serve`])
//! until the command pipe closes.
//!
//! The host renders nothing: it only synthesizes, and Core's mixer plays
//! the audio, so recording, device recovery, and mixing stay in one place.

use std::process::ExitCode;
use std::sync::Arc;

use verbatim_process::event::Event;
use verbatim_speech::hosting::synth_ids;
use verbatim_speech::{SynthDriver, SynthError};
use verbatim_synth_host::{parse_args, serve};

/// Writes a line to standard error, as `eprintln!` does, but ignores a
/// failed write instead of panicking: a full disk or a closed pipe under
/// redirected output must not take the process down.
macro_rules! report_error {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

fn main() -> ExitCode {
    // Core redirects this process's output to a log file of its own.
    init_tracing();
    let Some((pipe_in, pipe_out, synth)) = parse_args(&std::env::args().collect::<Vec<_>>()) else {
        report_error!(
            "usage: verbatim-synth-host --pipe-in <handle> --pipe-out <handle> --synth <id>"
        );
        return ExitCode::from(2);
    };
    // SAFETY: Core passes the values of the pipe ends it created for this
    // process, which inherited them and uses them nowhere else;
    // `inherited_pipe_handles` checks that they are distinct open pipes, so
    // a malformed command line fails here.
    let pipes = unsafe { verbatim_process::inherited_pipe_handles(pipe_in, pipe_out) };
    let Ok((commands, replies)) = pipes.inspect_err(|error| report_error!("{error}")) else {
        return ExitCode::from(2);
    };

    serve(commands, replies, |command_event| {
        // Logged before and after building the synthesizer, so a host that
        // never becomes ready shows in its log where it stopped.
        tracing::info!(%synth, "synthesizer host starting");
        let building = std::time::Instant::now();
        let driver = build(&synth, command_event)?;
        tracing::info!(%synth, elapsed_ms = building.elapsed().as_millis(), "synthesizer host ready");
        Ok(driver)
    })
}

/// Builds the driver of the named synthesizer. `OneCore`, which waits for
/// its synthesizer, waits on the command event as well.
fn build(synth: &str, command_event: &Arc<Event>) -> Result<Box<dyn SynthDriver>, SynthError> {
    match synth {
        synth_ids::ESPEAK => Ok(Box::new(verbatim_synth_espeak::EspeakSynth::new()?)),
        synth_ids::ONECORE => Ok(Box::new(
            verbatim_synth_onecore::OneCoreSynth::new()?.interrupted_by(Arc::clone(command_event)),
        )),
        other => Err(SynthError::Unavailable(format!(
            "this host has no synthesizer named {other}"
        ))),
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // A write that fails (a full disk, a closed pipe) is dropped: by
        // default the formatter reports it with `eprint!`, which panics when
        // standard error fails too.
        .log_internal_errors(false)
        .with_writer(std::io::stderr)
        .try_init();
}
