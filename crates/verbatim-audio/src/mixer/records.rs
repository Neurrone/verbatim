//! What crosses into and out of the audio thread: the records a lane
//! carries in, and the records the outbound ring carries out to the
//! delivery thread (`docs/design/ownership-output.md`, "Real-time audio").

use std::sync::Arc;
use std::time::Instant;

use verbatim_model::{TraceId, UtteranceId};

use crate::PlaybackEvent;

/// One request from a producing thread, in its lane, in the order made.
pub(crate) enum Record {
    /// An utterance that will be written next, registered under the
    /// source's cancel generation the queue thread gave it.
    Register {
        utterance: UtteranceId,
        trace_id: TraceId,
        generation: u64,
    },
    /// `frames` frames of `utterance`, `samples` samples in the device
    /// format of format generation `format`, which follow in the lane's
    /// sample ring.
    Frames {
        utterance: UtteranceId,
        format: u64,
        frames: u64,
        samples: usize,
    },
    /// An index mark at the current end of `utterance`'s audio.
    Mark { utterance: UtteranceId, mark: u64 },
    /// A sound placed at the current end of `utterance`'s audio.
    Sound {
        utterance: UtteranceId,
        voice: NewVoice,
    },
    /// A sound played at once.
    Play { voice: NewVoice },
    /// A sound played at once as the utterance `utterance`, which ends when
    /// the sound has played to its end: the exit sound.
    PlayToEnd {
        utterance: UtteranceId,
        trace_id: TraceId,
        voice: NewVoice,
    },
    /// `utterance` has written all its audio.
    Finish { utterance: UtteranceId },
    /// `utterance` failed, for `reason`.
    Fail {
        utterance: UtteranceId,
        reason: String,
    },
}

/// A sound's frames, converted on the producer's thread for the device
/// format of format generation `format`.
pub(crate) struct NewVoice {
    pub(crate) frames: Arc<[f32]>,
    pub(crate) gain: f32,
    pub(crate) format: u64,
}

/// What the audio thread hands the delivery thread: a report, or memory it
/// is done with, which the delivery thread frees so the audio thread never
/// does.
pub(crate) enum Outbound {
    /// A playback event of source `source`, with the time the audio thread
    /// read in the pass that reported it.
    Event {
        source: usize,
        event: PlaybackEvent,
        at: Instant,
    },
    /// The device ran dry in the middle of an utterance; the count so far.
    Underrun(u64),
    /// A voice's frames, done with.
    Frames(Arc<[f32]>),
    /// A failure's reason, for an utterance already ended.
    Reason(String),
}
