//! Thin libtest wrapper (milestone M3 Track B) around the
//! `multi_outpost_switch` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`. The scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/multi_outpost_switch.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) keeps discovering and running it by
//! name, unchanged from before the restructuring.

#[test]
fn multi_outpost_switch() {
    verbatim_e2e::registry::run_named("multi_outpost_switch");
}
