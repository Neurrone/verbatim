//! The settings dialog's Terminal page (`phase6-design.md`, "M4: text,
//! editing, and terminals", Questions): its model, pure apart from the
//! [`TerminalHost`] it reads and changes the settings through.
//!
//! The page has four controls, top to bottom: "Report new output", a check
//! box; "Lines spoken in full" and "Last lines to speak", sliders from 1 to
//! [`MAX_TERMINAL_LINES`]; and a check box for speaking passwords typed in
//! terminals. It opens on the reader settings as they are now, so a
//! Verbatim+5 toggle made before it opened is shown.
//!
//! Unlike the Speech and Theme pages, a change here waits for Apply or OK,
//! as NVDA's settings panels do. Apply sends Core only the settings changed
//! since the page opened or was last applied, so a toggle key pressed while
//! the dialog is open is not undone by applying another setting. Cancel
//! drops the changes not yet applied; Core never saw them, so nothing else
//! needs restoring.

use std::sync::Arc;

use verbatim_i18n::messages::{self, TerminalLabels};
use verbatim_model::{MAX_TERMINAL_LINES, ReaderSettings};

use crate::bridge::ffi;
use crate::plan::accessible_name;

/// What the Terminal page needs from the rest of Verbatim. The app
/// implements it over its configuration store and Core's reducer.
pub trait TerminalHost: Send + Sync {
    /// The reader settings as they are now.
    fn reader_settings(&self) -> ReaderSettings;
    /// Applies `change` to Core's reader settings and saves the result.
    fn change(&self, change: TerminalChange);
}

/// A change to the terminal settings: each field is `Some` when that
/// setting changed, and is merged into the reader settings by
/// [`apply_to`](Self::apply_to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalChange {
    /// "Report new output".
    pub report_output: Option<bool>,
    /// "Lines spoken in full".
    pub full_lines: Option<u16>,
    /// "Last lines to speak".
    pub last_lines: Option<u16>,
    /// Speak passwords typed in terminals.
    pub speak_passwords: Option<bool>,
}

impl TerminalChange {
    /// Whether the change changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Sets the settings this change carries in `settings`, leaving the
    /// others as they are.
    pub fn apply_to(&self, settings: &mut ReaderSettings) {
        if let Some(on) = self.report_output {
            settings.report_terminal_output = on;
        }
        if let Some(lines) = self.full_lines {
            settings.terminal_full_lines = lines;
        }
        if let Some(lines) = self.last_lines {
            settings.terminal_last_lines = lines;
        }
        if let Some(on) = self.speak_passwords {
            settings.speak_terminal_passwords = on;
        }
    }
}

/// The page's four settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Values {
    report_output: bool,
    full_lines: u16,
    last_lines: u16,
    speak_passwords: bool,
}

impl Values {
    /// The terminal settings of `settings`, the limits within range.
    fn of(settings: &ReaderSettings) -> Self {
        Self {
            report_output: settings.report_terminal_output,
            full_lines: settings.terminal_full_lines.clamp(1, MAX_TERMINAL_LINES),
            last_lines: settings.terminal_last_lines.clamp(1, MAX_TERMINAL_LINES),
            speak_passwords: settings.speak_terminal_passwords,
        }
    }

    /// What changed from `before` to `self`.
    fn change_from(self, before: Self) -> TerminalChange {
        fn changed<T: Copy + PartialEq>(now: T, then: T) -> Option<T> {
            (now != then).then_some(now)
        }
        TerminalChange {
            report_output: changed(self.report_output, before.report_output),
            full_lines: changed(self.full_lines, before.full_lines),
            last_lines: changed(self.last_lines, before.last_lines),
            speak_passwords: changed(self.speak_passwords, before.speak_passwords),
        }
    }
}

/// A slider's value as a line limit, within 1 and [`MAX_TERMINAL_LINES`].
fn line_limit(value: i32) -> u16 {
    u16::try_from(value.clamp(1, i32::from(MAX_TERMINAL_LINES))).unwrap_or(MAX_TERMINAL_LINES)
}

/// The Terminal page's state.
pub(crate) struct TerminalPanel {
    host: Arc<dyn TerminalHost>,
    labels: TerminalLabels,
    /// The settings as the controls show them.
    values: Values,
    /// The settings as the page opened with them, or last applied.
    committed: Values,
}

impl TerminalPanel {
    /// The page with the settings as they are now.
    pub(crate) fn open(host: Arc<dyn TerminalHost>) -> Self {
        let values = Values::of(&host.reader_settings());
        Self {
            host,
            labels: messages::terminal_labels(),
            values,
            committed: values,
        }
    }

    /// The page as it is now.
    pub(crate) fn page(&self) -> ffi::TerminalPage {
        let labels = &self.labels;
        ffi::TerminalPage {
            report_output_label: labels.report_output.clone(),
            report_output_name: accessible_name(&labels.report_output),
            report_output: self.values.report_output,
            full_lines_label: labels.full_lines.clone(),
            full_lines: i32::from(self.values.full_lines),
            last_lines_label: labels.last_lines.clone(),
            last_lines: i32::from(self.values.last_lines),
            min_lines: 1,
            max_lines: i32::from(MAX_TERMINAL_LINES),
            speak_passwords_label: labels.speak_passwords.clone(),
            speak_passwords_name: accessible_name(&labels.speak_passwords),
            speak_passwords: self.values.speak_passwords,
        }
    }

    /// "Report new output" was toggled.
    pub(crate) fn set_report_output(&mut self, checked: bool) {
        self.values.report_output = checked;
    }

    /// "Lines spoken in full" moved.
    pub(crate) fn set_full_lines(&mut self, value: i32) {
        self.values.full_lines = line_limit(value);
    }

    /// "Last lines to speak" moved.
    pub(crate) fn set_last_lines(&mut self, value: i32) {
        self.values.last_lines = line_limit(value);
    }

    /// The check box for speaking passwords was toggled.
    pub(crate) fn set_speak_passwords(&mut self, checked: bool) {
        self.values.speak_passwords = checked;
    }

    /// OK or Apply: sends Core the settings changed since the page opened
    /// or was last applied, which Cancel then keeps.
    pub(crate) fn apply(&mut self) {
        let change = self.values.change_from(self.committed);
        if !change.is_empty() {
            self.host.change(change);
        }
        self.committed = self.values;
    }

    /// Cancel: drops the changes not yet applied.
    pub(crate) fn cancel(&mut self) {
        self.values = self.committed;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A host whose settings are changed as Core would change them.
    struct FakeHost {
        settings: Mutex<ReaderSettings>,
        changes: Mutex<Vec<TerminalChange>>,
    }

    impl FakeHost {
        fn new(settings: ReaderSettings) -> Arc<Self> {
            Arc::new(Self {
                settings: Mutex::new(settings),
                changes: Mutex::new(Vec::new()),
            })
        }

        fn changes(&self) -> Vec<TerminalChange> {
            self.changes.lock().unwrap().clone()
        }
    }

    impl TerminalHost for FakeHost {
        fn reader_settings(&self) -> ReaderSettings {
            *self.settings.lock().unwrap()
        }
        fn change(&self, change: TerminalChange) {
            change.apply_to(&mut self.settings.lock().unwrap());
            self.changes.lock().unwrap().push(change);
        }
    }

    fn open(host: &Arc<FakeHost>) -> TerminalPanel {
        TerminalPanel::open(Arc::clone(host) as Arc<dyn TerminalHost>)
    }

    #[test]
    fn the_page_shows_the_settings_as_they_are_now() {
        let host = FakeHost::new(ReaderSettings {
            report_terminal_output: false,
            terminal_full_lines: 50,
            terminal_last_lines: 10,
            speak_terminal_passwords: true,
            ..ReaderSettings::default()
        });
        let page = open(&host).page();
        assert!(!page.report_output);
        assert_eq!((page.full_lines, page.last_lines), (50, 10));
        assert!(page.speak_passwords);
        assert_eq!((page.min_lines, page.max_lines), (1, 10_000));
        for (label, name) in [
            (&page.report_output_label, &page.report_output_name),
            (&page.speak_passwords_label, &page.speak_passwords_name),
        ] {
            assert_eq!(*name, accessible_name(label));
            assert!(!name.is_empty() && !name.contains('&'), "{name:?}");
        }
        assert!(
            host.changes().is_empty(),
            "opening the page changes nothing"
        );
    }

    #[test]
    fn the_limits_stay_between_1_and_10_000() {
        let host = FakeHost::new(ReaderSettings {
            terminal_full_lines: 0,
            terminal_last_lines: 20_000,
            ..ReaderSettings::default()
        });
        let mut panel = open(&host);
        assert_eq!(
            (panel.page().full_lines, panel.page().last_lines),
            (1, 10_000)
        );
        panel.set_full_lines(-4);
        panel.set_last_lines(100_000);
        assert_eq!(
            (panel.page().full_lines, panel.page().last_lines),
            (1, 10_000)
        );
        panel.set_full_lines(42);
        assert_eq!(panel.page().full_lines, 42);
    }

    #[test]
    fn apply_sends_only_what_changed() {
        let host = FakeHost::new(ReaderSettings::default());
        let mut panel = open(&host);
        panel.apply();
        assert!(host.changes().is_empty(), "nothing changed, nothing sent");

        panel.set_report_output(false);
        panel.set_last_lines(12);
        // A change made and undone before applying is no change.
        panel.set_speak_passwords(true);
        panel.set_speak_passwords(false);
        panel.apply();
        assert_eq!(
            host.changes(),
            [TerminalChange {
                report_output: Some(false),
                last_lines: Some(12),
                ..TerminalChange::default()
            }]
        );
        let settings = host.reader_settings();
        assert!(!settings.report_terminal_output);
        assert_eq!(settings.terminal_last_lines, 12);
        assert_eq!(settings.terminal_full_lines, 30);

        // Applying again sends nothing more.
        panel.apply();
        assert_eq!(host.changes().len(), 1);
    }

    #[test]
    fn a_toggle_key_pressed_while_the_page_is_open_is_not_undone() {
        let host = FakeHost::new(ReaderSettings::default());
        let mut panel = open(&host);
        // Verbatim+5 in Core, with the page open.
        host.settings.lock().unwrap().report_terminal_output = false;
        panel.set_full_lines(60);
        panel.apply();
        let settings = host.reader_settings();
        assert!(!settings.report_terminal_output);
        assert_eq!(settings.terminal_full_lines, 60);
    }

    #[test]
    fn cancel_drops_what_was_not_applied() {
        let host = FakeHost::new(ReaderSettings::default());
        let mut panel = open(&host);
        panel.set_full_lines(5);
        panel.apply();
        panel.set_full_lines(80);
        panel.set_report_output(false);
        panel.cancel();
        let page = panel.page();
        assert_eq!(page.full_lines, 5, "back to the applied value");
        assert!(page.report_output);
        assert_eq!(host.changes().len(), 1, "Cancel sends nothing");
        assert_eq!(host.reader_settings().terminal_full_lines, 5);
    }

    #[test]
    fn a_change_merges_only_its_own_settings() {
        let mut settings = ReaderSettings {
            follow_caret: false,
            ..ReaderSettings::default()
        };
        TerminalChange {
            speak_passwords: Some(true),
            full_lines: Some(7),
            ..TerminalChange::default()
        }
        .apply_to(&mut settings);
        assert!(settings.speak_terminal_passwords);
        assert_eq!(settings.terminal_full_lines, 7);
        assert_eq!(settings.terminal_last_lines, 30);
        assert!(settings.report_terminal_output);
        assert!(!settings.follow_caret, "a setting off the page is kept");
    }
}
