//! Demonstration: the settings dialog's keys, recorded by `cargo xtask
//! demo` for `videos/demos`: exactly the `settings_dialog_keys` scenario's
//! walk, so it shows nothing that scenario does not test. Enter on Cancel
//! cancels, Enter on Apply applies and keeps the dialog open, Control+S
//! saves from the rate slider, and Control+Tab and Control+Shift+Tab change
//! category from any control.

pub(crate) use super::settings_dialog_keys::{body, setup, teardown};
