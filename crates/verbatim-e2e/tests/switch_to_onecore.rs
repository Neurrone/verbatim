//! Thin libtest wrapper around the `switch_to_onecore` scenario registered
//! in `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/switch_to_onecore.rs`.

#[test]
fn switch_to_onecore() {
    verbatim_e2e::registry::run_named("switch_to_onecore");
}
