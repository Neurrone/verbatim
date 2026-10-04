//! Thin libtest wrapper (milestone M3) around the `start_menu_search` scenario.

#[test]
fn start_menu_search() {
    verbatim_e2e::registry::run_named("start_menu_search");
}
