//! Thin libtest wrapper around the `terminal_editing` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_editing.rs`.

#[test]
fn terminal_editing() {
    verbatim_e2e::registry::run_named("terminal_editing");
}
