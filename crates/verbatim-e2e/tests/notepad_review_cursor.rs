//! Thin libtest wrapper around the `notepad_review_cursor` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/notepad_review_cursor.rs`.

#[test]
fn notepad_review_cursor() {
    verbatim_e2e::registry::run_named("notepad_review_cursor");
}
