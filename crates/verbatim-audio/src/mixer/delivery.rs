//! The delivery thread (`docs/design/ownership-output.md`, "Real-time
//! audio"): it takes the audio thread's records from the outbound ring,
//! runs each source's playback listener with the time the audio thread
//! read, logs the underrun warning, and frees what the audio thread is done
//! with, so that none of this runs on the audio thread.

use rtrb::Consumer;
use tracing::warn;

use super::PlaybackListener;
use super::records::Outbound;
use crate::{Signal, Waker};

/// Runs the delivery thread until the audio thread has stopped and every
/// record has been taken. After draining a full ring it wakes the audio
/// thread, so events held back for room go out without waiting for its
/// next period.
pub(crate) fn run(
    mut ring: Consumer<Outbound>,
    listeners: &[PlaybackListener],
    signal: &dyn Signal,
    audio: &Waker,
) {
    let capacity = ring.buffer().capacity();
    loop {
        let full = ring.slots() == capacity;
        while let Ok(record) = ring.pop() {
            deliver(record, listeners);
        }
        if full {
            audio();
        }
        if ring.is_abandoned() && ring.is_empty() {
            return;
        }
        signal.wait();
    }
}

fn deliver(record: Outbound, listeners: &[PlaybackListener]) {
    match record {
        Outbound::Event { source, event, at } => {
            if let Some(listener) = listeners.get(source) {
                listener(event, at);
            }
        }
        Outbound::Underrun(underruns) => warn!(
            target: "verbatim::audio",
            underruns,
            "the device ran dry in the middle of an utterance: an audible gap"
        ),
        // Freed here, off the audio thread.
        Outbound::Frames(frames) => drop(frames),
        Outbound::Reason(reason) => drop(reason),
    }
}
