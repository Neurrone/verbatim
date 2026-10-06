//! Thin libtest wrapper around the `windows_terminal_commands` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_commands.rs`.

#[test]
fn windows_terminal_commands() {
    verbatim_e2e::registry::run_named("windows_terminal_commands");
}
