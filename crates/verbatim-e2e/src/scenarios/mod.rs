//! The registered scenarios' actual setup, body, and teardown logic, one
//! module per scenario. [`crate::registry::SCENARIOS`] is what wires each of
//! these into a named, grouped [`crate::registry::ScenarioDef`]; the
//! `#[test]` wrappers under `crates/verbatim-e2e/tests/` call
//! [`crate::registry::run_named`] with the matching name, rather than
//! calling into these modules directly.

pub(crate) mod m1_exit_regression;
pub(crate) mod msinfo32;
pub(crate) mod multi_outpost_switch;
pub(crate) mod notepad_focus;
pub(crate) mod object_navigation;
