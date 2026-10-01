//! Thin libtest wrapper around the `focus_churn` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/focus_churn.rs`. This file exists so
//! `cargo test -p verbatim-e2e focus_churn -- --exact` discovers and runs it
//! by name.

#[test]
fn focus_churn() {
    verbatim_e2e::registry::run_named("focus_churn");
}
