//! UIA remote operations (architecture section 4): programs that run inside
//! a UIA provider's process, so work that would take many cross-process
//! round trips takes one.
//!
//! Calls Windows' own API, the `WinRT` class
//! `Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, as NVDA
//! does. Three layers:
//!
//! - The instruction set ([`Opcode`], [`Instruction`]) and the typed
//!   program [`Builder`], in which each register's Rust type says what it
//!   holds ([`Reg`] and the [`kind`] markers) and control flow computes its
//!   own jump offsets. Every instruction records the Rust line that emitted
//!   it, so a failing instruction's index maps back to its source.
//! - Execution: an [`Operation`] imports elements and text ranges, runs,
//!   and maps a failure to an [`Error`] carrying the failing instruction's
//!   index and source location; an [`Outcome`] converts requested registers
//!   to Rust values, elements arriving with the cache the program filled;
//!   [`counting`] counts the instructions programs execute, to measure them
//!   against the platform's limit on one run.
//! - Algorithms with a classic implementation behind the same signature:
//!   [`focus_ancestry_remote`] and [`focus_ancestry_classic`], and
//!   [`focus_ancestry`], which call sites use: it runs the remote program
//!   when asked to, falls back to the classic walk when the program fails,
//!   and says which path answered; for the outpost finding its own copy of
//!   an element the focus listener read, [`element_by_path`]: the element
//!   reached from one the caller has by the runtime ids below it; and for
//!   a focus's completion, [`focus_states`]: the focus's states, read once
//!   its event registration is in, with the children of the dialogs it
//!   enters.
//! - A dialog's objects through UI Automation ([`dialog`]), which
//!   `verbatim_reader_rules::dialog` gathers a dialog's own text from.
//! - The request list's executors ([`batch`]): a run of a batch's
//!   fusable steps as programs one builder generates from the operations,
//!   with the fallback rule's reruns, or call by call.
//!
//! The crate is Windows-specific and GPL like NVDA, from which the
//! instruction table and builder design are ported.
//! `books/dev/src/reference/crates/verbatim-uia-rops.md` is the reviewer's guide.

pub mod batch;
mod builder;
mod caret;
pub mod dialog;
mod error;
mod focus;
mod focus_states;
mod instruction;
mod navigate;
mod opcode;
mod operation;
mod path;
mod terminal_colours;
mod text;
mod watch;

pub use builder::{Builder, Index, Numeric, Ordered, Reg, kind};
pub use caret::{
    ANNOTATION_GRAMMAR_ERROR, ANNOTATION_SPELLING_ERROR, Attributes, MAX_COMPARED_TEXT, MAX_RUNS,
    RangeEnd, Run, RunAttributes, SelectionTextChange, TextAttribute, UnitRead,
};
pub use error::{Error, Failure};
pub use focus::{
    Ancestry, FailedHop, FocusAncestry, FocusAncestryFn, FocusQuery, Path, Progress,
    focus_ancestry, focus_ancestry_classic, focus_ancestry_remote, runtime_id_key,
};
pub use focus_states::{
    DIALOG_CHILDREN_PER_RUN, FocusStatesFn, States, StatesQuery, focus_states,
    focus_states_classic, focus_states_remote,
};
pub use instruction::{Instruction, OperandId, TypeTest};
pub use opcode::{
    Comparison, NavigationDirection, Opcode, PointProperty, RectProperty, Status, pattern_method,
    pattern_related_object_method,
};
pub use operation::{Operation, Outcome, Read, Value, counting, work};
pub use path::{
    ElementByPathFn, PathQuery, element_by_path, element_by_path_classic, element_by_path_remote,
};
pub use terminal_colours::{COLOUR_BUDGET, COLOUR_CHUNK, ColourRun};
pub use text::{FoundPoint, Movement, Position, TextFrom, TextTarget, UnitText};
pub use watch::{WatchAnswer, WatchQuery, watch_check_classic};
