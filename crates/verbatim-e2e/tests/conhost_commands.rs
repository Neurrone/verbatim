//! Thin libtest wrapper around the `conhost_commands` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_commands.rs`.

#[test]
fn conhost_commands() {
    verbatim_e2e::registry::run_named("conhost_commands");
}
