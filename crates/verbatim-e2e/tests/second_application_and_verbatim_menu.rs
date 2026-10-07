//! Thin libtest wrapper (milestone M3 Track B) around the
//! `second_application_and_verbatim_menu` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`. The scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/second_application_and_verbatim_menu.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) keeps discovering and running it by
//! name, unchanged from before the restructuring.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn second_application_and_verbatim_menu() {
    verbatim_e2e::registry::run_named("second_application_and_verbatim_menu");
}
