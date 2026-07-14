//! Flight-recorder integration for the reducer (architecture section 9).
//!
//! The recorder holds recent reducer inputs, each paired with how many
//! effects reducing it produced; [`replay`] proves that feeding the same
//! inputs to the same initial state always reproduces the same effects,
//! which is what turns a flight-recorder dump from a live session into a
//! regression test.

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

/// A flight recorder specialized for reducer inputs.
pub type ReducerRecorder = FlightRecorder<RecordedInput>;

impl ReducerRecorder {
    /// Records one reducer step: the input and how many effects it produced.
    pub fn record_input(&mut self, input: Input, effect_count: usize) {
        self.record(RecordedInput {
            input,
            effect_count,
        });
    }

    /// The recorded inputs, oldest first, with their effect counts dropped —
    /// ready to feed to [`replay`].
    #[must_use]
    pub fn dump_inputs(&self) -> Vec<Input> {
        self.snapshot().map(|entry| entry.input.clone()).collect()
    }
}

/// Replays `inputs` against `initial` and returns the effects produced by
/// each step, in order.
///
/// The reducer is pure, so this always reproduces exactly what a live
/// session saw: the same initial state and input sequence yield the same
/// effects, every time.
#[must_use]
pub fn replay(initial: &SrState, inputs: &[Input]) -> Vec<Vec<Effect>> {
    let mut state = initial.clone();
    let mut all_effects = Vec::with_capacity(inputs.len());
    for input in inputs {
        let (next_state, effects) = reduce(&state, input);
        state = next_state;
        all_effects.push(effects);
    }
    all_effects
}
