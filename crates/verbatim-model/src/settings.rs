//! The reader settings Core's reducer reads (milestone M4): typing echo,
//! whether the review cursor follows the caret, and say-all's reading
//! unit. They are NVDA's settings with NVDA's defaults; `verbatim-config`
//! stores them, and the shell hands them to the reducer as
//! `Input::Settings`. The reducer changes some of them itself when the
//! user toggles one with a key (Verbatim+2, 3, 5, and 6), and reports the new
//! values as `Effect::SettingsChanged` for the shell to save. The terminal
//! settings (milestone M4 item 9) are here too: whether new output is
//! reported, and the flood policy's two limits. The keyboard's settings,
//! which the hook reads, are here too, so the configuration and the hook
//! share one declaration of each.

use serde::{Deserialize, Deserializer, Serialize};

/// Which keys act as the Verbatim modifier, as NVDA's setting of its
/// modifier keys does. The
/// default adds Caps Lock to NVDA's two Insert keys, a deliberate
/// difference (`docs/parity/input.md`, "NVDA-modifier semantics").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-key toggles plus one behavior flag, not a state machine"
)]
pub struct VerbatimKeys {
    /// Caps lock acts as the Verbatim key.
    pub caps_lock: bool,
    /// The extended (navigation-cluster) insert key acts as the Verbatim key.
    pub insert: bool,
    /// The numpad insert key acts as the Verbatim key.
    pub numpad_insert: bool,
    /// Passes the Verbatim key's own transitions down the hook chain instead
    /// of swallowing them, so another screen reader running behind Verbatim
    /// (for example NVDA sharing the same modifier) sees the modifier held
    /// and can run its own commands for chords Verbatim leaves unbound.
    /// Only enable while such a screen reader is running: with nothing
    /// behind Verbatim to swallow the key, caps lock reaches the OS and
    /// toggles on every use.
    pub share_modifier: bool,
}

impl Default for VerbatimKeys {
    fn default() -> Self {
        Self {
            caps_lock: true,
            insert: true,
            numpad_insert: true,
            share_modifier: false,
        }
    }
}

/// Which keyboard layout's gesture bindings are active: NVDA's desktop
/// (numpad-based) and laptop (no numpad assumed, shift and control chords
/// on the main key block instead) layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardLayout {
    /// The numpad-based layout, and the default.
    #[default]
    Desktop,
    /// The layout for keyboards without a numpad.
    Laptop,
}

/// When typed characters or words are echoed: NVDA's "Speak typed
/// characters" and "Speak typed words" choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypingEcho {
    /// Never.
    Off,
    /// Only in edit controls and other places text can be typed.
    EditControls,
    /// Always.
    Always,
}

impl TypingEcho {
    /// The next choice in the order NVDA's toggle key cycles through them:
    /// off, only in edit controls, always, then off again.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::EditControls,
            Self::EditControls => Self::Always,
            Self::Always => Self::Off,
        }
    }
}

/// The unit say-all reads by: NVDA's "Say all reads by" setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SayAllUnit {
    /// Sentence by sentence where the text can be split into sentences, and
    /// line by line otherwise (UIA text, terminals). The default.
    #[default]
    Sentence,
    /// Paragraph by paragraph, line by line where there are no paragraphs.
    Paragraph,
    /// Line by line.
    Line,
}

/// The reader settings the reducer reads, with NVDA's defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent settings, each a checkbox the user sets on its own"
)]
pub struct ReaderSettings {
    /// Speak typed characters (NVDA's default: only in edit controls).
    pub speak_typed_characters: TypingEcho,
    /// Speak typed words (NVDA's default: off).
    pub speak_typed_words: TypingEcho,
    /// The review cursor follows the caret ("caret moves review cursor",
    /// NVDA's default: on).
    pub follow_caret: bool,
    /// The unit say-all reads by (NVDA's default: sentence where possible).
    pub say_all_unit: SayAllUnit,
    /// Keep the display on while say-all reads (NVDA's "Prevent display from
    /// turning off during say all", default on).
    pub keep_display_on: bool,
    /// Speak characters typed into a terminal at once, before the terminal
    /// shows them (NVDA's "Speak passwords in all enhanced terminals",
    /// default off): when off, they wait until the terminal's text changes,
    /// so a password prompt that echoes nothing speaks nothing.
    pub speak_terminal_passwords: bool,
    /// Speak new output in terminals ("Report new output", toggled with
    /// Verbatim+5, default on; `docs/parity/system.md`, "The terminal
    /// settings"). NVDA's nearest setting is "Report dynamic content
    /// changes"; this one covers terminals until live regions arrive.
    pub report_terminal_output: bool,
    /// "Lines spoken in full": up to this many lines of terminal output
    /// waiting to be spoken are all spoken (default 30).
    pub terminal_full_lines: TerminalLines,
    /// "Last lines to speak": when more terminal output is waiting than
    /// [`Self::terminal_full_lines`], the older lines are replaced by
    /// "skipped N lines" and this many of the newest are kept (default 30).
    pub terminal_last_lines: TerminalLines,
    /// Indicate a progress bar in a background window as well as one in the
    /// foreground (NVDA's `reportBackgroundProgressBars`, default off;
    /// `docs/parity/changes-and-selection.md`, "Progress bars"). Core alone
    /// reads it: no reader is told it.
    pub report_background_progress_bars: bool,
}

/// The most lines either terminal limit may be set to, and so the most an
/// outpost reads for one change: more than either terminal's history holds
/// (9,031 lines in Windows Terminal, scrollback and screen; 9,001 in the
/// console host), so "Lines spoken in full" may reach past it, as decided
/// on 2026-10-07 and again in the coherence review of 2026-10-09; Core's
/// 10 MB bound on output waiting protects memory.
pub const MAX_TERMINAL_LINES: u16 = 10_000;

/// The default of both terminal limits ("30 and 30", `docs/parity/terminals.md`,
/// "The backlog"): a screenful or more keeps ordinary command output whole
/// and still bounds a flood.
pub const DEFAULT_TERMINAL_LINES: u16 = 30;

/// A terminal line limit, within 1 and [`MAX_TERMINAL_LINES`] however it
/// was written: one outside is brought within as it is made or read from
/// a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TerminalLines(u16);

impl TerminalLines {
    /// `lines`, brought within 1 and [`MAX_TERMINAL_LINES`].
    #[must_use]
    pub fn new(lines: u16) -> Self {
        Self(lines.clamp(1, MAX_TERMINAL_LINES))
    }

    /// The number of lines.
    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for TerminalLines {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        u16::deserialize(deserializer).map(Self::new)
    }
}

impl ReaderSettings {
    /// "Lines spoken in full".
    #[must_use]
    pub fn full_lines(&self) -> usize {
        usize::from(self.terminal_full_lines.get())
    }

    /// "Last lines to speak".
    #[must_use]
    pub fn last_lines(&self) -> usize {
        usize::from(self.terminal_last_lines.get())
    }
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            speak_typed_characters: TypingEcho::EditControls,
            speak_typed_words: TypingEcho::Off,
            follow_caret: true,
            say_all_unit: SayAllUnit::Sentence,
            keep_display_on: true,
            speak_terminal_passwords: false,
            report_terminal_output: true,
            terminal_full_lines: TerminalLines(DEFAULT_TERMINAL_LINES),
            terminal_last_lines: TerminalLines(DEFAULT_TERMINAL_LINES),
            report_background_progress_bars: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_nvdas() {
        let settings = ReaderSettings::default();
        assert_eq!(settings.speak_typed_characters, TypingEcho::EditControls);
        assert_eq!(settings.speak_typed_words, TypingEcho::Off);
        assert!(settings.follow_caret);
        assert_eq!(settings.say_all_unit, SayAllUnit::Sentence);
        assert!(settings.keep_display_on);
        assert!(!settings.speak_terminal_passwords);
        assert!(settings.report_terminal_output);
        assert_eq!(settings.full_lines(), 30);
        assert_eq!(settings.last_lines(), 30);
    }

    /// Limits out of range in a file are read within it, and so is a sound
    /// volume past 100.
    #[test]
    fn bounded_settings_are_read_within_their_bounds() {
        let settings: ReaderSettings =
            serde_json::from_str(r#"{"terminal_full_lines": 0, "terminal_last_lines": 20000}"#)
                .unwrap();
        assert_eq!(settings.full_lines(), 1);
        assert_eq!(settings.last_lines(), usize::from(MAX_TERMINAL_LINES));
        let options: crate::ThemeOptions =
            serde_json::from_str(r#"{"sound_volume": 250}"#).unwrap();
        assert_eq!(options.sound_volume.get(), 100);
    }

    #[test]
    fn the_echo_toggle_cycles_through_all_three() {
        assert_eq!(TypingEcho::Off.next(), TypingEcho::EditControls);
        assert_eq!(TypingEcho::EditControls.next(), TypingEcho::Always);
        assert_eq!(TypingEcho::Always.next(), TypingEcho::Off);
    }
}
