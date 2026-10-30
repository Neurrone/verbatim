//! Flight-recorder integration for the reducer (architecture section 9).
//!
//! The recorder holds recent reducer inputs, each paired with how many
//! effects reducing it produced, and the reducer state from just before the
//! oldest of them; [`replay`] proves that feeding the same inputs to the
//! same initial state always reproduces the same effects, which is what
//! turns a flight-recorder dump from a live session into a regression test.

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
    /// plus the heap it owns, which its strings dominate. The heap is found
    /// by walking the entry's fields with [`HeapCounter`], a serde
    /// serializer that formats nothing: it adds each string's length and,
    /// for each element of a list or map, the element's inline size, so it
    /// needs no size accounting kept in step with every model type, and
    /// costs one pass over the fields, with no text written and nothing
    /// allocated. A boxed value's own allocation is not seen, so the
    /// estimate is a little low for inputs that box their parts; their
    /// strings, which are what grows, are all counted.
    #[must_use]
    pub fn estimated_bytes(&self) -> usize {
        let mut counter = HeapCounter(0);
        // Counting cannot fail: the counter has no error to raise.
        let _ = self.serialize(&mut counter);
        size_of::<Self>() + counter.0
    }
}

/// A serde serializer that only adds up the heap a value owns
/// ([`RecordedInput::estimated_bytes`]): the length of every string and
/// byte string, and the inline size of every element of a sequence and of
/// every key and value of a map. A struct's fields and an option's content
/// are inline in their parent, so they add only the heap they own.
struct HeapCounter(usize);

/// The error [`HeapCounter`] never raises.
#[derive(Debug)]
struct NeverFails;

impl std::fmt::Display for NeverFails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("counting heap bytes cannot fail")
    }
}

impl std::error::Error for NeverFails {}

impl serde::ser::Error for NeverFails {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        NeverFails
    }
}

impl HeapCounter {
    /// Adds an element held on the heap: its inline size, then what it
    /// owns.
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        self.0 += size_of_val(value);
        value.serialize(self)
    }
}

impl serde::Serializer for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, _: bool) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_f32(self, _: f32) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_f64(self, _: f64) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_char(self, _: char) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_str(self, value: &str) -> Result<(), NeverFails> {
        self.0 += value.len();
        Ok(())
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), NeverFails> {
        self.0 += value.len();
        Ok(())
    }
    fn serialize_none(self) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), NeverFails> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), NeverFails> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), NeverFails> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), NeverFails> {
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, NeverFails> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, NeverFails> {
        Ok(self)
    }
}

impl serde::ser::SerializeSeq for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        self.element(value)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeTuple for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeTupleStruct for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeTupleVariant for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeMap for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NeverFails> {
        self.element(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NeverFails> {
        self.element(value)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeStruct for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), NeverFails> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), NeverFails> {
        Ok(())
    }
}

impl serde::ser::SerializeStructVariant for &mut HeapCounter {
    type Ok = ();
    type Error = NeverFails;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), NeverFails> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), NeverFails> {
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
    /// ancestor chain is a few kilobytes by [`RecordedInput::estimated_bytes`],
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

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::TraceId;

    /// A typed-text input carrying `text`.
    fn typed(text: String) -> Input {
        Input::CharacterTyped {
            key_seq: verbatim_model::KeySeq::NONE_YET,
            trace_id: TraceId::mint(),
            text,
            at_ms: 0,
        }
    }

    #[test]
    fn an_inputs_text_is_counted_in_its_estimate() {
        let small = RecordedInput {
            input: typed(String::new()),
            effect_count: 0,
        };
        let large = RecordedInput {
            input: typed("x".repeat(100_000)),
            effect_count: 0,
        };
        assert_eq!(small.estimated_bytes(), size_of::<RecordedInput>());
        assert_eq!(
            large.estimated_bytes(),
            size_of::<RecordedInput>() + 100_000
        );
    }

    #[test]
    fn inputs_carrying_text_are_bounded_by_bytes() {
        // Each a little over 1 MiB, far below the entry bound: the window
        // drops whole segments of half the 8 MiB bound, four entries each,
        // so four are kept.
        let mut recorder = ReducerRecorder::with_default_bounds(SrState::new());
        for _ in 0..20 {
            recorder.record_input(typed("x".repeat(1 << 20)), 0, &SrState::new());
        }
        assert_eq!(recorder.len(), 4);
        assert!(recorder.bytes() <= ReducerRecorder::DEFAULT_MAX_BYTES);
    }
}
