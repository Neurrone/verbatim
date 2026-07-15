//! Thin libtest wrapper (milestone M3 Track B) around the `notepad_focus`
//! scenario registered in `crates/verbatim-e2e/src/registry.rs`. The
//! scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/notepad_focus.rs`; this file exists
//! only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e` job, and
//! plain libtest filtering) keeps discovering and running it by name,
//! unchanged from before the restructuring.

#[test]
fn notepad_focus() {
    verbatim_e2e::registry::run_named("notepad_focus");
}
