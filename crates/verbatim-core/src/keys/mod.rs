//! The user's keys in Core (`books/dev/src/explanation/design/ownership.md`):
//! every key arrives numbered by the keyboard hook's thread and joins the
//! event queue as a press, whose speech carries its entry's fence
//! ([`entry`]); caret keys run their own entry ([`caret_key`]), held
//! strokes wait in Core's stroke line ([`strokes`]), and the focused
//! terminal takes its keys at their arrival (`crate::terminal`).

pub(crate) mod caret_key;
pub(crate) mod entry;
pub(crate) mod strokes;
