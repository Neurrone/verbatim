//! Thin libtest wrapper around the `explorer_folder_window` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/explorer_folder_window.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn explorer_folder_window() {
    verbatim_e2e::registry::run_named("explorer_folder_window");
}
