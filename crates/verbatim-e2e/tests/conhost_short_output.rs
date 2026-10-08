//! Thin libtest wrapper around the `conhost_short_output` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_short_output.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn conhost_short_output() {
    verbatim_e2e::registry::run_named("conhost_short_output");
}
