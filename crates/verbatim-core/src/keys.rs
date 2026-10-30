//! The key numbers of user inputs, and the stamp that puts them on speech
//! (`docs/design/ownership-input.md`, "Key numbers in Core, not in a side
//! table"): every user input arrives numbered by the keyboard hook's
//! thread, Core keeps the newest pairs of trace and number, and every
//! utterance an input's step speaks is stamped with the number of the key
//! that caused it, so a later key's cancel fences it off in the speech
//! queue.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{Effect, Input, KeySeq, TraceId};

use crate::state::SrState;

/// How many traces' key numbers are kept: far more than a key's speech can
/// be in flight behind.
pub(crate) const KEY_ORIGINS_KEPT: usize = 1024;

/// The key numbers of what recent user inputs caused, by trace, oldest
/// first, at most [`KEY_ORIGINS_KEPT`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeyOrigins(VecDeque<(TraceId, KeySeq)>);

impl KeyOrigins {
    /// Records that what carries `trace` was caused by key `key`, the
    /// oldest pair going when the table is full.
    pub(crate) fn record(&mut self, trace: TraceId, key: KeySeq) {
        if self.0.len() == KEY_ORIGINS_KEPT {
            self.0.pop_front();
        }
        self.0.push_back((trace, key));
    }

    /// The number of the key that caused what carries `trace`, if a recent
    /// one did.
    pub(crate) fn key_of(&self, trace: TraceId) -> Option<KeySeq> {
        self.0
            .iter()
            .rev()
            .find(|(recorded, _)| *recorded == trace)
            .map(|&(_, key)| key)
    }
}

/// The trace and number of a user input: a key, typed text, a gesture's
/// command or script, or a lock key's press. An input from a flight dump
/// made before inputs carried numbers has none.
fn user_key(input: &Input) -> Option<(TraceId, KeySeq)> {
    let (trace_id, key) = match input {
        Input::CaretKey {
            trace_id, key_seq, ..
        }
        | Input::HeldKey {
            trace_id, key_seq, ..
        }
        | Input::CharacterTyped {
            trace_id, key_seq, ..
        }
        | Input::ClearingKey {
            trace_id, key_seq, ..
        }
        | Input::Command {
            trace_id, key_seq, ..
        }
        | Input::Script {
            trace_id, key_seq, ..
        }
        | Input::ToggleKeyPressed {
            trace_id, key_seq, ..
        } => (*trace_id, *key_seq),
        _ => return None,
    };
    (key != KeySeq::NONE_YET).then_some((trace_id, key))
}

/// Records the number `input` carries, before the input is dispatched.
pub(crate) fn record(state: &mut SrState, input: &Input) {
    if let Some((trace_id, key)) = user_key(input) {
        state.key_origins.record(trace_id, key);
    }
}

/// Stamps every [`Effect::Speak`] of `effects`, inside [`Effect::ForKey`]
/// included, with the number of the key that caused it: the key of the
/// trace of the innermost `ForKey` around it, or else of the input's
/// trace; failing that, the key of the utterance's own trace.
pub(crate) fn stamp(state: &SrState, input: &Input, effects: &mut [Effect]) {
    let trace = input.trace_id();
    for effect in effects {
        stamp_one(&state.key_origins, trace, effect);
    }
}

/// [`stamp`] for one effect, executed under `trace`.
fn stamp_one(origins: &KeyOrigins, trace: Option<TraceId>, effect: &mut Effect) {
    match effect {
        Effect::Speak(utterance, key) => {
            *key = trace
                .and_then(|trace| origins.key_of(trace))
                .or_else(|| origins.key_of(utterance.trace_id));
        }
        Effect::ForKey(key_trace, inner) => stamp_one(origins, Some(*key_trace), inner),
        _ => {}
    }
}
