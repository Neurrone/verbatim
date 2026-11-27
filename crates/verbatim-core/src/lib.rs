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

mod announcements;
mod applications;
pub mod batch;
#[cfg(test)]
#[path = "../tests/support/caret_batches.rs"]
pub(crate) mod caret_batches;
mod caret_location;
mod caret_tell;
mod changes;
#[cfg(test)]
mod desk_tests;
mod desktop;
mod details;
pub mod dump;
mod editing;
mod entries;
mod event_queue;
pub mod flight_recorder;
mod focus;
pub mod focus_rules;
#[cfg(test)]
mod focus_speech_tests;
mod foreground;
pub mod held_tokens;
mod input_desktop;
mod keys;
mod navigator;
mod now;
pub mod outside;
mod progress;
#[cfg(test)]
#[path = "../tests/support/readers.rs"]
pub(crate) mod readers;
mod recorder;
mod reduce;
mod report_focus;
mod review_text;
mod say_all;
mod shell;
mod state;
pub mod supervisor;
pub mod terminal;
mod terminal_model;
mod text;
#[cfg(test)]
#[path = "../tests/support/text_batches.rs"]
pub(crate) mod text_batches;
mod text_facts;
mod text_request;
mod wants;

pub use dump::{
    DUMP_FORMAT_VERSION, DumpContents, DumpHeader, DumpReadError, UNKNOWN_COMMIT, read_dump,
    write_dump,
};
pub use entries::Runtime;
pub use flight_recorder::{FlightRecorder, FlightSnapshot};
pub use held_tokens::{HeldTokens, ListenerView, View, ViewSend};
pub use recorder::{RecordedInput, ReducerRecorder, replay};
pub use reduce::{SNAP_RESULTS_ACTIVITY, reduce};
pub use state::{Readiness, SrState};
pub use terminal_model::{read_at, read_of, read_start, snapshot_of};
