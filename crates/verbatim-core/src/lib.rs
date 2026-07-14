//! The functional core (architecture section 2).
//!
//! Interaction logic lives here as a pure reducer mapping a state and an
//! input to a new state and a list of effects; the imperative shell executes
//! the effects. Determinism is the point: a recorded triple of initial
//! state, input sequence, and fetch replies replays to identical effects,
//! which is what turns field bugs into unit tests.
//!
//! M0 shipped the flight-recorder skeleton and M1 the reducer; M2 adds
//! [`dump`], the on-disk format that turns a live flight-recorder ring into
//! a committed replay fixture.

pub mod dump;
pub mod flight_recorder;
mod recorder;
mod reduce;
mod state;

pub use dump::{
    DUMP_FORMAT_VERSION, DumpContents, DumpHeader, DumpReadError, read_dump, write_dump,
};
pub use flight_recorder::FlightRecorder;
pub use recorder::{RecordedInput, ReducerRecorder, replay};
pub use reduce::reduce;
pub use state::SrState;
