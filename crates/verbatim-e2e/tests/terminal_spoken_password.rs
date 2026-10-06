//! Thin libtest wrapper around the `terminal_spoken_password` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_commands.rs`.

#[test]
fn terminal_spoken_password() {
    verbatim_e2e::registry::run_named("terminal_spoken_password");
}
