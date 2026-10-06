//! Thin libtest wrapper around the `notepad_spelling_errors` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/notepad_spelling_errors.rs`.

#[test]
fn notepad_spelling_errors() {
    verbatim_e2e::registry::run_named("notepad_spelling_errors");
}
