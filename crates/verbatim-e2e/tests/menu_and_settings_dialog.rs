//! Thin libtest wrapper (milestone M3 Track B) around the
//! `menu_and_settings_dialog` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`. The scripted walk itself —
//! Verbatim+V, the menu, and the Speech settings dialog — lives in
//! `crates/verbatim-e2e/src/scenarios/menu_and_settings_dialog.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) keeps discovering and running it by
//! name, unchanged from before the restructuring.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn menu_and_settings_dialog() {
    verbatim_e2e::registry::run_named("menu_and_settings_dialog");
}
