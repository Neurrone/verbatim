//! Demonstration: editing in Windows 11 Notepad (milestone M4 items 3 and
//! 4), recorded by `cargo xtask demo` for `videos/demos`: exactly the
//! `notepad_editing` scenario's walk, so it shows nothing that scenario
//! does not test. The caret moves by character, word, and line; a line is
//! selected and unselected with Shift; characters are typed and echoed,
//! and deleted with Backspace and Delete, each saying what it deleted; and
//! End and Backspace name the line break they meet.

pub(crate) use super::editing::{notepad_body as body, notepad_setup as setup, teardown};
