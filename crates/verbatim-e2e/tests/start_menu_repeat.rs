//! Thin libtest wrapper around the `start_menu_repeat` diagnostic scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`. The scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/start_menu_repeat.rs`; this
//! file exists so `cargo test -p verbatim-e2e start_menu_repeat -- --exact`
//! — which is exactly how `cargo xtask vm test --scenario start_menu_repeat`
//! dispatches it — discovers and runs it by name.
//!
//! `start_menu_repeat` is a diagnostic in `registry::Group::Diagnostic`, so
//! `registry::select` excludes it from the no-filter default run; this wrapper
//! only makes it reachable by name, it does not put it back into the gate.

#[test]
fn start_menu_repeat() {
    verbatim_e2e::registry::run_named("start_menu_repeat");
}
