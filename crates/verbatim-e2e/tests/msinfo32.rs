//! Thin libtest wrapper (milestone M3) around the `msinfo32` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`. The scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/msinfo32.rs`.

#[test]
fn msinfo32() {
    verbatim_e2e::registry::run_named("msinfo32");
}
