//! Thin libtest wrapper around the `lock_key_announcements` scenario.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn lock_key_announcements() {
    verbatim_e2e::registry::run_named("lock_key_announcements");
}
