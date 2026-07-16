//! Thin libtest wrapper (milestone M3) around the `start_menu` scenario.

#[test]
fn start_menu() {
    verbatim_e2e::registry::run_named("start_menu");
}
