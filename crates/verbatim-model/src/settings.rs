//! The reader settings Core's reducer reads (milestone M4): typing echo,
//! whether the review cursor follows the caret, and say-all's reading
//! unit. They are NVDA's settings with NVDA's defaults; `verbatim-config`
//! stores them, and the shell hands them to the reducer as
//! `Input::Settings`. The reducer changes some of them itself when the
//! user toggles one with a key (Verbatim+2, 3, and 6), and reports the new
//! values as `Effect::SettingsChanged` for the shell to save.

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
    }

    #[test]
    fn the_echo_toggle_cycles_through_all_three() {
        assert_eq!(TypingEcho::Off.next(), TypingEcho::EditControls);
        assert_eq!(TypingEcho::EditControls.next(), TypingEcho::Always);
        assert_eq!(TypingEcho::Always.next(), TypingEcho::Off);
    }
}
