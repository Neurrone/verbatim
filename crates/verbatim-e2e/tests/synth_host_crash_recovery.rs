//! Thin libtest wrapper around the `synth_host_crash_recovery` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/synth_host_crash_recovery.rs`.

#[test]
fn synth_host_crash_recovery() {
    verbatim_e2e::registry::run_named("synth_host_crash_recovery");
}
