//! Thin libtest wrapper (milestone M3 Track B) around the
//! `notepad_and_verbatim_menu` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`. The scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/notepad_and_verbatim_menu.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) keeps discovering and running it by
//! name, unchanged from before the restructuring.

#[test]
fn notepad_and_verbatim_menu() {
    verbatim_e2e::registry::run_named("notepad_and_verbatim_menu");
}
