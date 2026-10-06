//! Thin libtest wrapper around the `notepad_spelling_errors` demonstration
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/notepad_spelling_errors.rs`.
//! Ignored, as every demonstration is: `cargo xtask demo` runs it with the
//! ignored tests included.

#[test]
#[ignore = "a demonstration of Windows 11 Notepad's spell checker; run it with cargo xtask demo"]
fn notepad_spelling_errors() {
    verbatim_e2e::registry::run_named("notepad_spelling_errors");
}
