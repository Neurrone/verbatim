//! Thin libtest wrapper around the `terminal_review_grid` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_review_grid.rs`.

#[test]
fn terminal_review_grid() {
    verbatim_e2e::registry::run_named("terminal_review_grid");
}
