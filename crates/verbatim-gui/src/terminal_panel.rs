//! The settings dialog's Terminal page (`books/dev/src/reference/crates/verbatim-gui.md`;
//! `docs/parity/system.md`, "The terminal settings"): its model, pure apart from the
//! [`TerminalHost`] it reads and changes the settings through.
//!
//! The page has four controls, top to bottom: "Report new output", a check
//! box; "Lines spoken in full" and "Last lines to speak", sliders from 1 to
//! [`MAX_TERMINAL_LINES`]; and a check box for speaking passwords typed in
//! terminals. It opens on the reader settings as they are now, so a
//! Verbatim+5 toggle made before it opened is shown.
//!
//! Unlike the Speech and Theme pages, a change here waits for Apply or OK,
//! which write all four values, as NVDA's settings panels write every
//! control on save. Cancel drops the changes not yet applied; Core never
//! saw them, so nothing else needs restoring.

use std::sync::Arc;

use verbatim_i18n::messages::{self, TerminalLabels};
use verbatim_model::{MAX_TERMINAL_LINES, ReaderSettings, TerminalLines};

use crate::bridge::ffi;
use crate::plan::accessible_name;

/// What the Terminal page needs from the rest of Verbatim. The app
/// implements it over its configuration store and Core's reducer.
pub trait TerminalHost: Send + Sync {
    /// The reader settings as they are now.
    fn reader_settings(&self) -> ReaderSettings;
    /// Writes `change` into Core's reader settings and saves the result.
    fn change(&self, change: TerminalChange);
}

/// The Terminal page's four values, written together into the reader
/// settings by [`apply_to`](Self::apply_to).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalChange {
    /// "Report new output".
    pub report_output: bool,
    /// "Lines spoken in full".
    pub full_lines: TerminalLines,
    /// "Last lines to speak".
    pub last_lines: TerminalLines,
    /// Speak passwords typed in terminals.
    pub speak_passwords: bool,
}

impl TerminalChange {
    /// The terminal settings of `settings`.
    fn of(settings: &ReaderSettings) -> Self {
        Self {
            report_output: settings.report_terminal_output,
            full_lines: settings.terminal_full_lines,
            last_lines: settings.terminal_last_lines,
            speak_passwords: settings.speak_terminal_passwords,
        }
    }

    /// Writes the four values into `settings`, leaving the other reader
    /// settings as they are.
    pub fn apply_to(&self, settings: &mut ReaderSettings) {
        settings.report_terminal_output = self.report_output;
        settings.terminal_full_lines = self.full_lines;
        settings.terminal_last_lines = self.last_lines;
        settings.speak_terminal_passwords = self.speak_passwords;
    }
}

/// A slider's value as a line limit.
fn line_limit(value: i32) -> TerminalLines {
    TerminalLines::new(u16::try_from(value.max(1)).unwrap_or(MAX_TERMINAL_LINES))
}

/// The Terminal page's state.
pub(crate) struct TerminalPanel {
    host: Arc<dyn TerminalHost>,
    labels: TerminalLabels,
    /// The settings as the controls show them.
    values: TerminalChange,
}

impl TerminalPanel {
    /// The page with the settings as they are now.
    pub(crate) fn open(host: Arc<dyn TerminalHost>) -> Self {
        let values = TerminalChange::of(&host.reader_settings());
        Self {
            host,
            labels: messages::terminal_labels(),
            values,
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
            full_lines: i32::from(self.values.full_lines.get()),
            last_lines_label: labels.last_lines.clone(),
            last_lines: i32::from(self.values.last_lines.get()),
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

    /// OK or Apply: writes the page's four values.
    pub(crate) fn apply(&self) {
        self.host.change(self.values);
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
            terminal_full_lines: TerminalLines::new(50),
            terminal_last_lines: TerminalLines::new(10),
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

    /// OK writes all four of the page's values, as NVDA's panels write
    /// every control on save: "Report new output", still checked on the
    /// page, is on again after Verbatim+5 turned it off while the page was
    /// open, and a reader setting off the page is kept.
    #[test]
    fn ok_writes_all_four_values() {
        let host = FakeHost::new(ReaderSettings {
            follow_caret: false,
            ..ReaderSettings::default()
        });
        let mut panel = open(&host);
        // Verbatim+5 in Core, with the page open.
        host.settings.lock().unwrap().report_terminal_output = false;
        panel.set_full_lines(60);
        panel.apply();
        let settings = host.reader_settings();
        assert!(settings.report_terminal_output);
        assert_eq!(settings.full_lines(), 60);
        assert_eq!(settings.last_lines(), 30);
        assert!(!settings.speak_terminal_passwords);
        assert!(!settings.follow_caret, "a setting off the page is kept");
    }
}
