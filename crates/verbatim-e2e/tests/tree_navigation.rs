//! Thin libtest wrapper (milestone M3) around the `tree_navigation`
//! scenario registered in `crates/verbatim-e2e/src/registry.rs`. The
//! scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/tree_navigation.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) discovers and runs it by name.

#[test]
fn tree_navigation() {
    verbatim_e2e::registry::run_named("tree_navigation");
}
