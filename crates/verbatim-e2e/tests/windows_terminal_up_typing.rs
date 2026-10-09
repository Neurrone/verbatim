//! Thin libtest wrapper around the `windows_terminal_up_typing` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/terminal_key_timing.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn windows_terminal_up_typing() {
    verbatim_e2e::registry::run_named("windows_terminal_up_typing");
}
