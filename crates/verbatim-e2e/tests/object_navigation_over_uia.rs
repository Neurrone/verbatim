//! Thin libtest wrapper around the `object_navigation_over_uia` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/object_navigation_over_uia.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn object_navigation_over_uia() {
    verbatim_e2e::registry::run_named("object_navigation_over_uia");
}
