//! Thin libtest wrapper (milestone M3 Track B) around the
//! `m1_exit_regression` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`. The scripted walk itself —
//! Verbatim+V, the menu, and the Speech settings dialog — lives in
//! `crates/verbatim-e2e/src/scenarios/m1_exit_regression.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) keeps discovering and running it by
//! name, unchanged from before the restructuring.

#[test]
fn m1_exit_regression() {
    verbatim_e2e::registry::run_named("m1_exit_regression");
}
