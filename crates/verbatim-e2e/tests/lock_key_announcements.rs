//! Thin libtest wrapper around the `lock_key_announcements` scenario.

#[test]
fn lock_key_announcements() {
    verbatim_e2e::registry::run_named("lock_key_announcements");
}
