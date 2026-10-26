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
//!   and says which path answered; for terminals, [`terminal_snapshot`]:
//!   the text from where the last read said to start to the document's
//!   end in one `GetText`, with the screen, the caret, and the screen's
//!   colours ([`ColourRun`]) after a key; and, for
//!   caret reports, [`caret_read_remote`], [`caret_read_classic`], and
//!   [`caret_read`]: the caret, the evidence a caret key moved it, its line
//!   and another unit at it, the formatting of the text to be spoken, and
//!   how the selection changed; for the text protocol's other requests,
//!   [`text_units`] (a unit read after a movement, or several ahead for
//!   say-all), [`text_range`] (the text between two points, or selecting
//!   it), and [`text_location`]; for object navigation,
//!   [`navigation_step`]: the neighbor and the nearest window; for a
//!   selection inside an element the focus controls,
//!   [`controlled_selection`]; and, for the outpost finding its own copy
//!   of an element the focus listener read, [`element_by_path`]: the
//!   element reached from one the caller has by the runtime ids below it;
//!   and, for the control plane's tree dump, [`tree_dump`]: an element's
//!   subtree, each element's children read in one call.
//!
//! The crate is Windows-specific and GPL like NVDA, from which the
//! instruction table and builder design are ported.
//! `docs/crates/verbatim-uia-rops.md` is the reviewer's guide.

mod builder;
mod caret;
mod controlled;
mod error;
mod focus;
mod instruction;
mod navigate;
mod opcode;
mod operation;
mod path;
mod terminal_colours;
mod terminal_find;
mod terminal_rows;
mod terminal_snapshot;
mod text;
mod tree;

pub use builder::{Builder, Index, Numeric, Ordered, Reg, kind};
pub use caret::{
    ANNOTATION_GRAMMAR_ERROR, ANNOTATION_SPELLING_ERROR, Attributes, CaretAnswer, CaretLineQuery,
    CaretQuery, CaretReadFn, FormatSpan, MAX_COMPARED_TEXT, MAX_RUNS, RangeEnd, Run, RunAttributes,
    SelectionTextChange, TextAttribute, UnitRead, caret_read, caret_read_classic,
    caret_read_remote,
};
pub use controlled::{
    Controlled, ControlledQuery, ControlledSelectionFn, controlled_selection,
    controlled_selection_classic, controlled_selection_remote,
};
pub use error::{Error, Failure};
pub use focus::{
    Ancestry, FailedHop, FocusAncestry, FocusAncestryFn, FocusQuery, Path, focus_ancestry,
    focus_ancestry_classic, focus_ancestry_remote, runtime_id_key,
};
pub use instruction::{Instruction, OperandId, TypeTest};
pub use navigate::{
    NavigationStepFn, Step, StepQuery, navigation_step, navigation_step_classic,
    navigation_step_remote,
};
pub use opcode::{
    Comparison, NavigationDirection, Opcode, PointProperty, RectProperty, Status, pattern_method,
    pattern_related_object_method,
};
pub use operation::{Operation, Outcome, Read, Value, counting, work};
pub use path::{
    ElementByPathFn, PathQuery, element_by_path, element_by_path_classic, element_by_path_remote,
};
pub use terminal_colours::{COLOUR_BUDGET, COLOUR_CHUNK, ColourRun, row_width, screen_height};
pub use terminal_find::{
    FIND_CALLS, FIND_MATCHES, FindMatch, FindQuery, terminal_find, terminal_find_classic,
    terminal_find_remote,
};
pub use terminal_rows::{
    MOST_RUNS, RowsAnswer, RowsQuery, TerminalRowsFn, terminal_rows, terminal_rows_classic,
    terminal_rows_remote,
};
pub use terminal_snapshot::{
    Snapshot, SnapshotQuery, TerminalSnapshotFn, terminal_snapshot, terminal_snapshot_classic,
    terminal_snapshot_remote,
};
pub use text::{
    ChangesUnread, FoundPoint, LocationAnswer, LocationQuery, Movement, Position, RangeAction,
    RangeAnswer, RangeQuery, SelectionAfter, TextFrom, TextLocationFn, TextRangeFn, TextTarget,
    TextUnitsFn, UnitText, UnitsAnswer, UnitsQuery, text_location, text_location_classic,
    text_location_remote, text_range, text_range_classic, text_range_remote, text_units,
    text_units_classic, text_units_remote,
};
pub use tree::{
    CHILDREN_PER_RUN, TreeDumpFn, TreeQuery, tree_dump, tree_dump_classic, tree_dump_remote,
};
