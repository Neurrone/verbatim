//! The functional core (architecture section 2).
//!
//! Interaction logic lives here as a deterministic reducer that advances a
//! state in place by one input and returns a list of effects; the
//! imperative shell executes the effects. Determinism is the point: a
//! recorded triple of initial state, input sequence, and fetch replies
//! replays to identical effects, which is what turns field bugs into unit
//! tests.
//!
//! M0 shipped the flight-recorder skeleton and M1 the reducer; M2 adds
//! [`dump`], the on-disk format that turns a live flight-recorder ring into
//! a committed replay fixture.

#![forbid(unsafe_code)]

mod applications;
pub mod dump;
mod editing;
pub mod flight_recorder;
pub mod focus_rules;
mod foreground;
mod keys;
mod recorder;
mod reduce;
mod review;
mod review_text;
mod say_all;
mod shell;
mod state;
mod terminal;
mod terminal_model;
mod text;

pub use dump::{
    DUMP_FORMAT_VERSION, DumpContents, DumpHeader, DumpReadError, read_dump, write_dump,
};
pub use flight_recorder::FlightRecorder;
pub use focus_rules::is_selection_container;
pub use recorder::{RecordedInput, ReducerRecorder, replay};
pub use reduce::{SNAP_RESULTS_ACTIVITY, reduce};
pub use state::SrState;
pub use terminal_model::{read_at, read_of, snapshot_of};
