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
//!   to Rust values, elements arriving with the cache the program filled.
//! - Algorithms with a classic implementation behind the same signature:
//!   [`focus_ancestry_remote`] and [`focus_ancestry_classic`], and
//!   [`focus_ancestry`], which call sites use: it runs the remote program
//!   when asked to, falls back to the classic walk when the program fails,
//!   and says which path answered.
//!   `terminal_tail` (the anchor line, the count of lines to the end, and
//!   the last lines' text) follows with milestone M4's terminals.
//!
//! The crate is Windows-specific and GPL like NVDA, from which the
//! instruction table and builder design are ported.
//! `docs/crates/verbatim-uia-rops.md` is the reviewer's guide.

mod builder;
mod error;
mod focus;
mod instruction;
mod opcode;
mod operation;

pub use builder::{Builder, Index, Numeric, Ordered, Reg, kind};
pub use error::{Error, Failure};
pub use focus::{
    Ancestry, FocusAncestry, FocusAncestryFn, FocusQuery, LEFT_OUT_WHEN_UNSUPPORTED, Path,
    focus_ancestry, focus_ancestry_classic, focus_ancestry_remote, runtime_id_key,
};
pub use instruction::{Instruction, OperandId, TypeTest};
pub use opcode::{
    Comparison, NavigationDirection, Opcode, PointProperty, RectProperty, Status,
    pattern_related_object_method,
};
pub use operation::{Operation, Outcome, Read, Value};
