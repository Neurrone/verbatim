//! Demonstration: the review cursor over a plain-text table in Windows 11
//! Notepad (milestone M4 item 5), recorded by `cargo xtask demo` for
//! `videos/demos`: exactly the `notepad_review_cursor` scenario's walk, so
//! it shows nothing that scenario does not test. The review cursor reads
//! the table's lines, words, and characters, keeps its column down the
//! table, names the line break at a line's end, and a range marked with
//! the start marker is copied, pasted, and read back.

pub(crate) use super::review_cursor::{notepad_body as body, notepad_setup as setup, teardown};
