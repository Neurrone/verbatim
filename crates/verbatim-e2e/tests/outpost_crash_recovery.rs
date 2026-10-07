//! Thin libtest wrapper around the `outpost_crash_recovery` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/outpost_crash_recovery.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn outpost_crash_recovery() {
    verbatim_e2e::registry::run_named("outpost_crash_recovery");
}
