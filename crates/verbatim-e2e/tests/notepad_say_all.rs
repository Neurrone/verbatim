//! Thin libtest wrapper around the `notepad_say_all` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/notepad_say_all.rs`.

#[test]
fn notepad_say_all() {
    verbatim_e2e::registry::run_named("notepad_say_all");
}
