//! The audio thread's recovery path, kept out of the loop's module: opening
//! the device, reopening it after an error or a request, and stopping at
//! shutdown. Here the audio thread may log and allocate, since it runs only
//! when the device opens or fails (`docs/design/ownership-output.md`,
//! "Real-time audio").

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tracing::warn;
use verbatim_model::UtteranceEnding;

use super::run::AudioLoop;
use super::store::SAMPLES;
use super::{pack_format, record_generation, source_capacity};
use crate::fallback::Fallback;
use crate::{AudioDevice, AudioError, DeviceFormat, Signal};

/// How long the audio thread waits for the delivery thread to make room at
/// shutdown, between tries.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(10);

/// Whether the mixer can play `format`: a source's capacity of it fits the
/// samples each source reserves.
fn fits(format: DeviceFormat) -> bool {
    source_capacity(format) * u64::from(format.channels)
        <= u64::try_from(SAMPLES).unwrap_or(u64::MAX)
}

/// Opens the device, or real-time silence in its place, and makes ready
/// for its format: the mix and the tap's pending frames get their room,
/// the tap is told, and the format is published.
pub(crate) fn open(audio: &mut AudioLoop, device: &mut Fallback, tap_signal: &Arc<dyn Signal>) {
    let format = device.open_or_silence(fits);
    let samples =
        usize::try_from(format.buffer_frames).unwrap_or(usize::MAX) * usize::from(format.channels);
    audio.mix.clear();
    audio.mix.reserve(samples);
    audio.tap_pending.clear();
    audio.tap_pending.reserve(samples);
    if let Some(tap) = audio.tap.as_mut() {
        tap.opened(format, tap_signal);
    }
    if (format.sample_rate, format.channels) != (audio.format.sample_rate, audio.format.channels) {
        if audio.format.channels != 0 {
            // A device that comes back in a different format cannot play
            // the frames already converted for the old one, so every
            // utterance not yet ended fails.
            let ending = UtteranceEnding::Failed("the audio device changed format".to_owned());
            for store in &mut audio.sources {
                store.end_all(&ending);
            }
        }
        audio.generation = record_generation(audio.generation + 1);
        for store in &mut audio.sources {
            store.set_channels(usize::from(format.channels));
        }
    }
    audio.format = format;
    audio
        .shared
        .format
        .store(pack_format(audio.generation, format), Ordering::Release);
    audio
        .shared
        .buffer_frames
        .store(u64::from(format.buffer_frames), Ordering::Release);
}

/// Recovers from a device error, or a request to reopen when `error` is
/// `None`: discards what was queued, rewinds every source to what had
/// played, reports what is due, and opens the device again. Every producer
/// waiting for room is woken, since a format change frees it all.
pub(crate) fn recover(
    audio: &mut AudioLoop,
    device: &mut Fallback,
    tap_signal: &Arc<dyn Signal>,
    error: Option<AudioError>,
) {
    let error =
        error.unwrap_or_else(|| AudioError::Device("the device asked to be reopened".to_owned()));
    warn!(target: "verbatim::audio", %error, "reopening the audio device");
    device.stop();
    audio.running = false;
    audio.forget_clock();
    audio.written = audio.played;
    audio.tap_pending.clear();
    let played = audio.played;
    for store in &mut audio.sources {
        store.rewind(played);
    }
    audio.fire(Instant::now());
    open(audio, device, tap_signal);
    audio.fire(Instant::now());
    for (store, control) in audio.sources.iter_mut().zip(&audio.controls) {
        store.released_now = false;
        control.released.store(store.released, Ordering::Release);
        control.room.set();
    }
    audio.end_pass();
}

/// Stops at shutdown: the device stops, every utterance still registered
/// ends cancelled, and every ending and every voice leaves through the
/// outbound ring, waiting on the device between tries while the delivery
/// thread makes room. Every producer waiting for room is woken, to find the
/// mixer stopped.
pub(crate) fn shut_down(audio: &mut AudioLoop, device: &mut Fallback) {
    device.stop();
    audio.running = false;
    for store in &mut audio.sources {
        store.end_all(&UtteranceEnding::Cancelled);
    }
    loop {
        audio.fire(Instant::now());
        audio.end_pass();
        let done = audio
            .sources
            .iter()
            .all(|store| store.nothing_retired() && !store.events_pending());
        if done {
            break;
        }
        device.wait(SHUTDOWN_WAIT);
    }
    for control in &audio.controls {
        control.room.set();
    }
    audio.delivery.set();
}
