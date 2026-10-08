//! Thin libtest wrapper around the `conhost_footer_flood` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_footer.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn conhost_footer_flood() {
    verbatim_e2e::registry::run_named("conhost_footer_flood");
}
