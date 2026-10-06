//! Thin libtest wrapper around the `terminal_flood` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_flood.rs`.

#[test]
fn terminal_flood() {
    verbatim_e2e::registry::run_named("terminal_flood");
}
