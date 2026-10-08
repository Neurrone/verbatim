//! Thin libtest wrapper around the `spelling_errors` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/spelling_errors.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn spelling_errors() {
    verbatim_e2e::registry::run_named("spelling_errors");
}
