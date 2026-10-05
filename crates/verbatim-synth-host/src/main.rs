//! `verbatim-synth-host.exe`: one synthesizer, in its own process
//! (decision D18).
//!
//! Core launches it, contained in a kill-on-close job, as
//! `--pipe-in <handle> --pipe-out <handle> --synth <id>`, and talks to it
//! with the protocol in `verbatim_speech::hosting`. The host builds the
//! named synthesizer's driver, sends `Ready` with its description (or
//! `Unavailable` and exits), then serves requests one at a time until the
//! command pipe closes. A reader thread watches the command pipe while an
//! utterance is being spoken, so a `Cancel` reaches the driver at its next
//! push of audio, or at its next `is_cancelled` check.
//!
//! The host renders nothing: it only synthesizes, and Core's mixer plays
//! the audio, so recording, device recovery, and mixing stay in one place.

use std::io::{BufReader, BufWriter, Write};
use std::ops::ControlFlow;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::unbounded;
use verbatim_audio::PcmFormat;
use verbatim_speech::hosting::{
    FromHost, HostDescription, ToHost, read_to_host, synth_ids, write_from_host,
};
use verbatim_speech::{IndexMark, SynthDriver, SynthError, SynthSink};

fn main() -> ExitCode {
    // Core redirects this process's output to a log file of its own.
    init_tracing();
    let Some((pipe_in, pipe_out, synth)) = parse_args(&std::env::args().collect::<Vec<_>>()) else {
        eprintln!("usage: verbatim-synth-host --pipe-in <handle> --pipe-out <handle> --synth <id>");
        return ExitCode::from(2);
    };
    // SAFETY: the values name the pipe ends Core created for this process,
    // which it inherited and nothing else uses.
    let (from_core, to_core) = unsafe { verbatim_process::inherited_pipes(pipe_in, pipe_out) };
    let mut to_core = BufWriter::new(to_core);

    let mut driver = match build(&synth) {
        Ok(driver) => driver,
        Err(error) => {
            tracing::error!(%synth, %error, "the synthesizer could not start");
            let _ = write_from_host(&mut to_core, &FromHost::Unavailable(error.to_string()));
            return ExitCode::FAILURE;
        }
    };
    let description = HostDescription {
        display_name: driver.display_name(),
        places_marks: driver.places_marks(),
        changes_pitch: driver.changes_pitch(),
        settings: driver.supported_settings(),
        values: driver
            .supported_settings()
            .iter()
            .filter_map(|descriptor| {
                let id = descriptor.id().clone();
                driver.setting(&id).map(|value| (id, value))
            })
            .collect(),
    };
    if write_from_host(&mut to_core, &FromHost::Ready(description)).is_err() {
        return ExitCode::FAILURE;
    }
    tracing::info!(%synth, "synthesizer host ready");

    // The reader thread: cancels take effect at once, everything else is
    // served in order on this thread.
    let cancelled = Arc::new(AtomicU64::new(0));
    let (requests_tx, requests) = unbounded();
    {
        let cancelled = Arc::clone(&cancelled);
        let mut from_core = BufReader::new(from_core);
        std::thread::spawn(move || {
            loop {
                let request = match read_to_host(&mut from_core) {
                    Ok(Some(request)) => request,
                    // Core closed the command pipe: it is done with this host.
                    Ok(None) => return,
                    Err(error) => {
                        tracing::error!(%error, "a command from Core could not be read");
                        return;
                    }
                };
                match request {
                    ToHost::Cancel(utterance) => cancelled.store(utterance.0, Ordering::Release),
                    other => {
                        if requests_tx.send(other).is_err() {
                            return;
                        }
                    }
                }
            }
        });
    }

    while let Ok(request) = requests.recv() {
        let reply = match request {
            ToHost::Speak(sequence) => {
                tracing::trace!(target: "verbatim::stage", trace = %sequence.trace_id, stage = "host request");
                let mut sink = PipeSink {
                    to_core: &mut to_core,
                    utterance: sequence.utterance.0,
                    trace_id: sequence.trace_id,
                    cancelled: &cancelled,
                    broken: false,
                    audio: false,
                };
                let result = driver.speak(&sequence, &mut sink);
                if sink.broken {
                    return ExitCode::FAILURE;
                }
                match result {
                    Ok(()) => FromHost::Done,
                    Err(error) => FromHost::Failed(error.to_string()),
                }
            }
            ToHost::SetSetting { id, value } => FromHost::SettingApplied(
                driver
                    .set_setting(&id, value)
                    .err()
                    .map(|error| error.to_string()),
            ),
            // Handled by the reader thread.
            ToHost::Cancel(_) => continue,
        };
        if write_from_host(&mut to_core, &reply).is_err() {
            return ExitCode::FAILURE;
        }
    }
    // Core closed the command pipe.
    ExitCode::SUCCESS
}

/// Builds the driver of the named synthesizer.
fn build(synth: &str) -> Result<Box<dyn SynthDriver>, SynthError> {
    match synth {
        synth_ids::ESPEAK => Ok(Box::new(verbatim_synth_espeak::EspeakSynth::new()?)),
        synth_ids::ONECORE => Ok(Box::new(verbatim_synth_onecore::OneCoreSynth::new()?)),
        other => Err(SynthError::Unavailable(format!(
            "this host has no synthesizer named {other}"
        ))),
    }
}

/// Sends the driver's output to Core.
struct PipeSink<'a, W: Write> {
    to_core: &'a mut W,
    /// The utterance being spoken.
    utterance: u64,
    /// The utterance Core last cancelled.
    cancelled: &'a AtomicU64,
    /// Core is gone: the pipe could not be written.
    broken: bool,
    /// The trace the utterance belongs to, and whether the driver has given
    /// audio yet, for the stage log.
    trace_id: verbatim_model::TraceId,
    audio: bool,
}

impl<W: Write> SynthSink for PipeSink<'_, W> {
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        if self.broken || self.cancelled.load(Ordering::Acquire) == self.utterance {
            return ControlFlow::Break(());
        }
        if !self.audio {
            self.audio = true;
            tracing::trace!(target: "verbatim::stage", trace = %self.trace_id, stage = "host audio");
        }
        // Blocks while Core is behind: the pipe's small buffer is the
        // backpressure that paces the synthesizer.
        if write_from_host(self.to_core, &FromHost::Pcm(format, samples.to_vec())).is_err() {
            self.broken = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn index_reached(&mut self, mark: IndexMark) {
        if !self.broken && write_from_host(self.to_core, &FromHost::Mark(mark)).is_err() {
            self.broken = true;
        }
    }

    fn is_cancelled(&self) -> bool {
        self.broken || self.cancelled.load(Ordering::Acquire) == self.utterance
    }
}

/// `--pipe-in <handle> --pipe-out <handle> --synth <id>`, in any order.
fn parse_args(args: &[String]) -> Option<(usize, usize, String)> {
    let mut pipe_in = None;
    let mut pipe_out = None;
    let mut synth = None;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--pipe-in" => pipe_in = iter.next()?.parse().ok(),
            "--pipe-out" => pipe_out = iter.next()?.parse().ok(),
            "--synth" => synth = iter.next().cloned(),
            _ => return None,
        }
    }
    Some((pipe_in?, pipe_out?, synth?))
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
