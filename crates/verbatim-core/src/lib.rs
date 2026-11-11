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
// Non-test code never panics, overflows or truncates silently: arithmetic
// is checked or saturating, indexing goes through `get`, and casts through
// `From` or `TryFrom`. Tests are exempt, since a panic is how a test fails.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::exit,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::exit,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::string_slice,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::unwrap_in_result,
        clippy::unwrap_used
    )
)]

mod applications;
mod desktop;
mod driver;
pub mod dump;
mod editing;
pub mod flight_recorder;
mod focus_queue;
pub mod focus_rules;
mod foreground;
mod key_line;
mod keys;
mod live_reread;
mod press;
mod proven;
mod queries;
mod recorder;
mod reduce;
mod report_focus;
pub mod requests;
mod review;
mod review_text;
mod say_all;
mod shell;
mod state;
pub mod supervisor;
mod terminal;
mod terminal_model;
mod text;

/// The reducer's state with the functions paused beside it, under the
/// name every caller of [`reduce`] already uses.
pub use driver::Core as SrState;
pub use dump::{
    DUMP_FORMAT_VERSION, DumpContents, DumpHeader, DumpReadError, UNKNOWN_COMMIT, read_dump,
    write_dump,
};
pub use flight_recorder::FlightRecorder;
pub use focus_rules::is_selection_container;
pub use recorder::{RecordedInput, ReducerRecorder, replay};
pub use reduce::{SNAP_RESULTS_ACTIVITY, reduce};
/// The reducer's state alone: what a checkpoint clones, a dump writes and
/// a replay starts from.
pub use state::SrState as Checkpoint;
pub use terminal_model::{read_at, read_of, snapshot_of};
