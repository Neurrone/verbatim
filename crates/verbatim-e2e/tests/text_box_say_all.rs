//! Thin libtest wrapper around the `text_box_say_all` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/text_box_say_all.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn text_box_say_all() {
    verbatim_e2e::registry::run_named("text_box_say_all");
}
