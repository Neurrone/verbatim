//! Thin libtest wrapper around the `notepad_typed_words` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/notepad_typed_words.rs`.

#[test]
fn notepad_typed_words() {
    verbatim_e2e::registry::run_named("notepad_typed_words");
}
