//! Flight-recorder integration for the reducer (architecture section 9).
//!
//! The recorder holds recent reducer inputs, each paired with how many
//! effects reducing it produced, and the reducer state from just before the
//! oldest of them; [`replay`] proves that feeding the same inputs to the
//! same initial state always reproduces the same effects, which is what
//! turns a flight-recorder dump from a live session into a regression test.

use std::io;

use serde::{Deserialize, Serialize};
use verbatim_model::{Effect, Input};

use crate::flight_recorder::FlightRecorder;
use crate::reduce::reduce;
use crate::state::SrState;

/// One input recorded from a live reducer thread, alongside how many
/// effects reducing it produced. Serializable so it survives the trip to
/// disk in a flight-recorder dump (see [`crate::dump`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedInput {
    /// The input as it was fed to the reducer.
    pub input: Input,
    /// Number of effects the reducer emitted for this input.
    pub effect_count: usize,
}

impl RecordedInput {
    /// An estimate of the memory this entry holds, in bytes: its inline size
    /// plus the length of its compact JSON form. The JSON length tracks the
    /// heap the entry owns, which its strings dominate, within a small
    /// factor, needs no size accounting kept in step with every model type,
    /// and is exactly what the entry costs in a dump. It is counted without
    /// building the JSON, so estimating allocates nothing.
    #[must_use]
    pub fn estimated_bytes(&self) -> usize {
        let mut counter = ByteCounter(0);
        // Serializing plain data into a sink that never fails cannot fail;
        // were it to, the bytes counted so far would still serve.
        let _ = serde_json::to_writer(&mut counter, self);
        size_of::<Self>() + counter.0
    }
}

/// A writer that only counts the bytes written to it.
struct ByteCounter(usize);

impl io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A flight recorder specialized for reducer inputs, whose checkpoints are
/// reducer states.
pub type ReducerRecorder = FlightRecorder<RecordedInput, SrState>;

impl ReducerRecorder {
    /// The entry bound the shell uses: 1,024 inputs, several minutes of
    /// ordinary use, which is what a field bug's dump needs to show how the
    /// state was reached.
    pub const DEFAULT_MAX_ENTRIES: usize = 1024;

    /// The byte bound the shell uses: 8 MiB. A focus event with a ten-deep
    /// ancestor chain is about 3 KB by [`RecordedInput::estimated_bytes`],
    /// so 1,024 ordinary inputs stay well under it and the entry bound
    /// governs normal use; inputs that carry text bodies (a terminal's
    /// output, a document's lines) reach this bound first, which keeps the
    /// recorder's memory fixed whatever the applications send.
    pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

    /// A recorder with the shell's bounds, starting from `initial`.
    #[must_use]
    pub fn with_default_bounds(initial: SrState) -> Self {
        Self::new(Self::DEFAULT_MAX_ENTRIES, Self::DEFAULT_MAX_BYTES, initial)
    }

    /// Records one reducer step: the input, how many effects it produced,
    /// and `state`, the reducer state after it, which becomes a checkpoint
    /// when this step closes a segment of the window (see
    /// [`crate::flight_recorder`] for the rule). The state is cloned only
    /// then, and its clone shares everything large.
    pub fn record_input(&mut self, input: Input, effect_count: usize, state: &SrState) {
        let entry = RecordedInput {
            input,
            effect_count,
        };
        let bytes = entry.estimated_bytes();
        self.record(entry, bytes, || state.clone());
    }

    /// The recorded inputs, oldest first, with their effect counts dropped —
    /// ready to feed to [`replay`] from [`FlightRecorder::checkpoint`].
    #[must_use]
    pub fn dump_inputs(&self) -> Vec<Input> {
        self.entries().map(|entry| entry.input.clone()).collect()
    }
}

/// Replays `inputs` against `initial` and returns the effects produced by
/// each step, in order.
///
/// The reducer is deterministic, so this always reproduces exactly what a
/// live session saw: the same initial state and input sequence yield the
/// same effects, every time.
#[must_use]
pub fn replay(initial: &SrState, inputs: &[Input]) -> Vec<Vec<Effect>> {
    let mut state = initial.clone();
    let mut all_effects = Vec::with_capacity(inputs.len());
    for input in inputs {
        all_effects.push(reduce(&mut state, input));
    }
    all_effects
}
