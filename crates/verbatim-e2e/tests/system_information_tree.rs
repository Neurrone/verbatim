//! Thin libtest wrapper (milestone M3) around the `system_information_tree`
//! scenario registered in `crates/verbatim-e2e/src/registry.rs`. The
//! scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/system_information_tree.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) discovers and runs it by name.

#[test]
fn system_information_tree() {
    verbatim_e2e::registry::run_named("system_information_tree");
}
