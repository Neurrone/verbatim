//! Thin libtest wrapper (milestone M3) around the `object_navigation`
//! scenario registered in `crates/verbatim-e2e/src/registry.rs`. The
//! scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/object_navigation.rs`; this file
//! exists only so `cargo test -p verbatim-e2e` (runner-direct CI's `e2e`
//! job, and plain libtest filtering) discovers and runs it by name.

#[test]
fn object_navigation() {
    verbatim_e2e::registry::run_named("object_navigation");
}
