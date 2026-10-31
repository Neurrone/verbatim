//! The delivery thread (`docs/design/ownership.md`): it takes the audio thread's records from the outbound ring,
//! runs each source's playback listener with the time the audio thread
//! read, logs the underrun warning, and frees what the audio thread is done
//! with, so that none of this runs on the audio thread.

use rtrb::Consumer;
use tracing::warn;

use verbatim_model::UtteranceEnding;

use super::records::Outbound;
use super::{FORMAT_CHANGED, PlaybackListener};
use crate::{PlaybackEvent, Signal, Waker};

/// Runs the delivery thread until the audio thread has stopped and every
/// record has been taken. After a drain that took at least half the ring
/// it wakes the audio thread, so events held back for room go out without
/// waiting for its next period: the audio thread holds events back only
/// when it finds the ring full, and the drain that empties a full ring
/// takes all of it.
pub(crate) fn run(
    mut ring: Consumer<Outbound>,
    listeners: &[PlaybackListener],
    signal: &dyn Signal,
    audio: &Waker,
) {
    let capacity = ring.buffer().capacity();
    loop {
        let mut took = 0;
        while let Ok(record) = ring.pop() {
            deliver(record, listeners);
            took += 1;
        }
        if took >= capacity / 2 {
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
        Outbound::FormatChanged {
            source,
            utterance,
            trace_id,
            at,
        } => {
            if let Some(listener) = listeners.get(source) {
                let ending = UtteranceEnding::Failed(FORMAT_CHANGED.to_owned());
                let event = PlaybackEvent::Ended {
                    utterance,
                    trace_id,
                    ending,
                };
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
