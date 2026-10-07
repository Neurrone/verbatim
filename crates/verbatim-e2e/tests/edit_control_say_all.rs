//! Thin libtest wrapper around the `edit_control_say_all` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/edit_control_say_all.rs`.

#[test]
fn edit_control_say_all() {
    verbatim_e2e::registry::run_named("edit_control_say_all");
}
