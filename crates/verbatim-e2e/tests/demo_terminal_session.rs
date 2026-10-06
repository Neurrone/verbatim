//! Thin libtest wrapper around the `demo_terminal_session` demonstration registered in
//! `crates/verbatim-e2e/src/registry.rs`; the demonstration itself lives in
//! `crates/verbatim-e2e/src/scenarios/demo_terminal_session.rs`. It is ignored, so the suite
//! never runs it; `cargo xtask demo` runs it with `--include-ignored`.

#[test]
#[ignore = "a demonstration, recorded by cargo xtask demo"]
fn demo_terminal_session() {
    verbatim_e2e::registry::run_named("demo_terminal_session");
}
