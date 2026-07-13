//! The functional core (architecture section 2).
//!
//! Interaction logic lives here as a pure reducer mapping a state and an
//! input to a new state and a list of effects; the imperative shell executes
//! the effects. Determinism is the point: a recorded triple of initial
//! state, input sequence, and fetch replies replays to identical effects,
//! which is what turns field bugs into unit tests.
//!
//! M0 ships the flight-recorder skeleton; the reducer itself lands with
//! milestone M1.

pub mod flight_recorder;

pub use flight_recorder::FlightRecorder;
