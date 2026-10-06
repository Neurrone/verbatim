//! The reader settings Core's reducer reads (milestone M4): typing echo,
//! whether the review cursor follows the caret, and say-all's reading
//! unit. They are NVDA's settings with NVDA's defaults; `verbatim-config`
//! stores them, and the shell hands them to the reducer as
//! `Input::Settings`. The reducer changes some of them itself when the
//! user toggles one with a key (Verbatim+2, 3, 5, and 6), and reports the new
//! values as `Effect::SettingsChanged` for the shell to save. The terminal
//! settings (milestone M4 item 9) are here too: whether new output is
//! reported, and the flood policy's two limits.

use serde::{Deserialize, Serialize};

/// When typed characters or words are echoed: NVDA's "Speak typed
/// characters" and "Speak typed words" choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypingEcho {
    /// Never.
    Off,
    /// Only in edit controls and other places text can be typed.
    EditControls,
    /// Always.
    #[default]
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
    /// Speak typed characters (NVDA's default: always).
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
    /// Verbatim+5, default on; `phase6-design.md`, "M4: text, editing, and
    /// terminals"). NVDA's nearest setting is "Report dynamic content
    /// changes"; this one covers terminals until live regions arrive.
    pub report_terminal_output: bool,
    /// "Lines spoken in full": up to this many lines of terminal output
    /// waiting to be spoken are all spoken (default 30, at most
    /// [`MAX_TERMINAL_LINES`]).
    pub terminal_full_lines: u16,
    /// "Last lines to speak": when more terminal output is waiting than
    /// [`Self::terminal_full_lines`], the older lines are replaced by
    /// "skipped N lines" and this many of the newest are kept (default 30,
    /// at most [`MAX_TERMINAL_LINES`]).
    pub terminal_last_lines: u16,
}

/// The most lines either terminal limit may be set to, and so the most an
/// outpost reads for one change.
pub const MAX_TERMINAL_LINES: u16 = 100;

/// The default of both terminal limits ("30 and 30", `phase6-design.md`,
/// "The flood policy, reconsidered").
pub const DEFAULT_TERMINAL_LINES: u16 = 30;

impl ReaderSettings {
    /// "Lines spoken in full", within 1 and [`MAX_TERMINAL_LINES`].
    #[must_use]
    pub fn full_lines(&self) -> usize {
        usize::from(self.terminal_full_lines.clamp(1, MAX_TERMINAL_LINES))
    }

    /// "Last lines to speak", within 1 and [`MAX_TERMINAL_LINES`].
    #[must_use]
    pub fn last_lines(&self) -> usize {
        usize::from(self.terminal_last_lines.clamp(1, MAX_TERMINAL_LINES))
    }

    /// How many of a change's newest terminal lines an outpost reads: as
    /// many as either limit can keep.
    #[must_use]
    pub fn terminal_read_lines(&self) -> u16 {
        u16::try_from(self.full_lines().max(self.last_lines())).unwrap_or(MAX_TERMINAL_LINES)
    }
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            speak_typed_characters: TypingEcho::Always,
            speak_typed_words: TypingEcho::Off,
            follow_caret: true,
            say_all_unit: SayAllUnit::Sentence,
            keep_display_on: true,
            speak_terminal_passwords: false,
            report_terminal_output: true,
            terminal_full_lines: DEFAULT_TERMINAL_LINES,
            terminal_last_lines: DEFAULT_TERMINAL_LINES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_nvdas() {
        let settings = ReaderSettings::default();
        assert_eq!(settings.speak_typed_characters, TypingEcho::Always);
        assert_eq!(settings.speak_typed_words, TypingEcho::Off);
        assert!(settings.follow_caret);
        assert_eq!(settings.say_all_unit, SayAllUnit::Sentence);
        assert!(settings.keep_display_on);
        assert!(!settings.speak_terminal_passwords);
        assert!(settings.report_terminal_output);
        assert_eq!(settings.full_lines(), 30);
        assert_eq!(settings.last_lines(), 30);
        assert_eq!(settings.terminal_read_lines(), 30);
    }

    #[test]
    fn the_terminal_limits_stay_in_range() {
        let settings = ReaderSettings {
            terminal_full_lines: 0,
            terminal_last_lines: 500,
            ..ReaderSettings::default()
        };
        assert_eq!(settings.full_lines(), 1);
        assert_eq!(settings.last_lines(), usize::from(MAX_TERMINAL_LINES));
        assert_eq!(settings.terminal_read_lines(), MAX_TERMINAL_LINES);
    }

    #[test]
    fn the_echo_toggle_cycles_through_all_three() {
        assert_eq!(TypingEcho::Off.next(), TypingEcho::EditControls);
        assert_eq!(TypingEcho::EditControls.next(), TypingEcho::Always);
        assert_eq!(TypingEcho::Always.next(), TypingEcho::Off);
    }
}
