//! Themes: one model for verbosity, speech, and sounds (`phase6-design.md`,
//! "Themes: one model for verbosity, speech, and sounds").
//!
//! The indication catalogue names every kind of thing Verbatim can report,
//! each with a stable id and a category. A [`Theme`] says, for each
//! indication, how it is reported ([`Presentation`]: off, speech, sound, or
//! speech and sound), with the sound, gain, replacement words, and voice
//! style that go with it. The built-in default theme
//! ([`Theme::builtin_default`]) is complete; every other theme holds only
//! the indications where it differs, and anything it does not mention,
//! including indications later versions add, falls back to the default
//! ([`Theme::setting`]). There are no chains of themes built on themes.
//!
//! The reducer keeps putting every fact into utterances as typed spans
//! (decision D12); the presentation stage at the end of the speech pipeline
//! turns each span into words, a sound, both, or nothing, as the active
//! theme says ([`Indication::of_segment`] finds the span's indication). The
//! reducer consults the theme in one case, to skip fetching details set to
//! off ([`Fetches`]).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::event::Earcon;
use crate::speech::{Message, Phrase, SegmentContent, TextFormat};
use crate::tree::{Role, State};

/// The categories of the catalogue, in the order the theme panel lists
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndicationCategory {
    /// Roles: "link", "button".
    Roles,
    /// States and their negations: "checked", "not checked".
    States,
    /// Properties: description, position, keyboard shortcut, level.
    Properties,
    /// Text formatting: spelling and grammar errors, font, color, capitals.
    TextFormatting,
    /// Structure: blank lines, skipped terminal lines.
    Structure,
    /// Events: start and exit, errors, modes, suggestions, progress, an
    /// application not responding.
    Events,
}

impl IndicationCategory {
    /// Every category, in display order.
    pub const ALL: [Self; 6] = [
        Self::Roles,
        Self::States,
        Self::Properties,
        Self::TextFormatting,
        Self::Structure,
        Self::Events,
    ];
}

/// The states that are spoken, and so are indications: a state the reducer
/// never speaks (focused, focusable, selectable, offscreen, checkable) is
/// not in the catalogue.
const SPOKEN_STATES: [State; 13] = [
    State::Selected,
    State::Checked,
    State::Mixed,
    State::Disabled,
    State::ReadOnly,
    State::Expanded,
    State::Collapsed,
    State::Pressed,
    State::HasPopup,
    State::Busy,
    State::Protected,
    State::Required,
    State::InvalidEntry,
];

/// The states whose absence is spoken: "not checked", "not selected", "not
/// pressed".
const NEGATED_STATES: [State; 3] = [State::Checked, State::Selected, State::Pressed];

/// One kind of thing Verbatim can report: an entry of the indication
/// catalogue. Each has a stable id ([`id`](Self::id)), which theme files
/// use, and a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Indication {
    /// A role, such as "link"; id `role-link`.
    Role(Role),
    /// A state, such as "checked"; id `state-checked`.
    State(State),
    /// A state's absence, such as "not checked"; id `state-not-checked`.
    NegatedState(State),
    /// An object's description; id `description`.
    Description,
    /// An object's keyboard shortcut; id `shortcut`.
    Shortcut,
    /// An object's position in its set, "3 of 7"; id `position`.
    Position,
    /// An object's nesting level, "level 2"; id `level`.
    Level,
    /// A spelling error in text, where it starts and ends; id
    /// `spelling-error`.
    SpellingError,
    /// A grammar error in text; id `grammar-error`.
    GrammarError,
    /// The font of text; id `font-name`.
    FontName,
    /// The size of text; id `font-size`.
    FontSize,
    /// The color of text; id `color`.
    Color,
    /// Bold, italic, and underlined text, NVDA's font attributes; id
    /// `font-attributes`.
    FontAttributes,
    /// A capital letter spoken on its own: speech raises its pitch, a sound
    /// plays a tone; id `capital`.
    Capital,
    /// Nothing to read, "blank"; id `blank`.
    Blank,
    /// Terminal output skipped as too much to read; id `skipped-lines`.
    SkippedLines,
    /// An application is not responding; id `app-not-responding`.
    AppNotResponding,
    /// Verbatim started; id `start`.
    Start,
    /// Verbatim is exiting; id `exit`.
    Exit,
    /// An error was logged; id `error`.
    Error,
    /// Browse mode turned on; id `browse-mode`.
    BrowseMode,
    /// Focus mode turned on; id `focus-mode`.
    FocusMode,
    /// Suggestions appeared; id `suggestions-opened`.
    SuggestionsOpened,
    /// Suggestions went away; id `suggestions-closed`.
    SuggestionsClosed,
    /// A progress bar moved, its tone's pitch rising with the percentage;
    /// id `progress`.
    Progress,
}

/// The fixed ids of the indications that carry no role or state, in
/// catalogue order (which is category order).
const FIXED: [(Indication, &str); 22] = [
    (Indication::Description, "description"),
    (Indication::Shortcut, "shortcut"),
    (Indication::Position, "position"),
    (Indication::Level, "level"),
    (Indication::SpellingError, "spelling-error"),
    (Indication::GrammarError, "grammar-error"),
    (Indication::FontName, "font-name"),
    (Indication::FontSize, "font-size"),
    (Indication::Color, "color"),
    (Indication::FontAttributes, "font-attributes"),
    (Indication::Capital, "capital"),
    (Indication::Blank, "blank"),
    (Indication::SkippedLines, "skipped-lines"),
    (Indication::AppNotResponding, "app-not-responding"),
    (Indication::Start, "start"),
    (Indication::Exit, "exit"),
    (Indication::Error, "error"),
    (Indication::BrowseMode, "browse-mode"),
    (Indication::FocusMode, "focus-mode"),
    (Indication::SuggestionsOpened, "suggestions-opened"),
    (Indication::SuggestionsClosed, "suggestions-closed"),
    (Indication::Progress, "progress"),
];

impl Indication {
    /// The whole catalogue, in display order: by category, and within a
    /// category in the order the theme panel lists it.
    #[must_use]
    pub fn catalogue() -> Vec<Self> {
        let mut all = Vec::with_capacity(Role::ALL.len() + 40);
        all.extend(Role::ALL.iter().map(|role| Self::Role(*role)));
        for state in SPOKEN_STATES {
            all.push(Self::State(state));
            if NEGATED_STATES.contains(&state) {
                all.push(Self::NegatedState(state));
            }
        }
        all.extend(FIXED.iter().map(|(indication, _)| *indication));
        all
    }

    /// The category the indication is listed under.
    #[must_use]
    pub const fn category(self) -> IndicationCategory {
        match self {
            Self::Role(_) => IndicationCategory::Roles,
            Self::State(_) | Self::NegatedState(_) => IndicationCategory::States,
            Self::Description | Self::Shortcut | Self::Position | Self::Level => {
                IndicationCategory::Properties
            }
            Self::SpellingError
            | Self::GrammarError
            | Self::FontName
            | Self::FontSize
            | Self::Color
            | Self::FontAttributes
            | Self::Capital => IndicationCategory::TextFormatting,
            Self::Blank | Self::SkippedLines => IndicationCategory::Structure,
            Self::AppNotResponding
            | Self::Start
            | Self::Exit
            | Self::Error
            | Self::BrowseMode
            | Self::FocusMode
            | Self::SuggestionsOpened
            | Self::SuggestionsClosed
            | Self::Progress => IndicationCategory::Events,
        }
    }

    /// The stable id theme files name the indication by: `role-` and
    /// `state-` followed by the role's or state's name in kebab case,
    /// `state-not-` for a negated state, and a fixed id otherwise (see each
    /// variant). Renaming a `Role` or `State` variant would change its id,
    /// so variants are never renamed.
    #[must_use]
    pub fn id(self) -> String {
        match self {
            Self::Role(role) => format!("role-{}", kebab(&format!("{role:?}"))),
            Self::State(state) => format!("state-{}", kebab(&format!("{state:?}"))),
            Self::NegatedState(state) => format!("state-not-{}", kebab(&format!("{state:?}"))),
            fixed => FIXED
                .iter()
                .find(|(indication, _)| *indication == fixed)
                .map_or_else(String::new, |(_, id)| (*id).to_owned()),
        }
    }

    /// The indication with this id, or `None` for an id this version does
    /// not know (a theme made by a later version can name one).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(name) = id.strip_prefix("role-") {
            return Role::ALL
                .iter()
                .find(|role| kebab(&format!("{role:?}")) == name)
                .map(|role| Self::Role(*role));
        }
        if let Some(name) = id.strip_prefix("state-not-") {
            return State::ALL
                .iter()
                .find(|state| kebab(&format!("{state:?}")) == name)
                .map(|state| Self::NegatedState(*state));
        }
        if let Some(name) = id.strip_prefix("state-") {
            return State::ALL
                .iter()
                .find(|state| kebab(&format!("{state:?}")) == name)
                .map(|state| Self::State(*state));
        }
        FIXED
            .iter()
            .find(|(_, fixed)| *fixed == id)
            .map(|(indication, _)| *indication)
    }

    /// The indication an utterance span reports, or `None` for a span that
    /// is content rather than an indication (a label, a value, literal
    /// text, a character, a mark, and messages other than "blank"), which
    /// is always spoken.
    #[must_use]
    pub fn of_segment(content: &SegmentContent) -> Option<Self> {
        Some(match content {
            SegmentContent::Role(role) => Self::Role(*role),
            SegmentContent::State(state) => Self::State(*state),
            SegmentContent::NegatedState(state) => Self::NegatedState(*state),
            SegmentContent::Description(_) => Self::Description,
            SegmentContent::Shortcut(_) => Self::Shortcut,
            SegmentContent::Position { .. } => Self::Position,
            SegmentContent::Level(_) => Self::Level,
            SegmentContent::SpelledCapital(_) => Self::Capital,
            SegmentContent::Message(Message::Blank) => Self::Blank,
            SegmentContent::Phrase(Phrase::SkippedLines(_) | Phrase::SkippedUncountedLines) => {
                Self::SkippedLines
            }
            SegmentContent::Format(format) => match format {
                TextFormat::SpellingError | TextFormat::NotSpellingError => Self::SpellingError,
                TextFormat::GrammarError | TextFormat::NotGrammarError => Self::GrammarError,
                TextFormat::FontName(_) => Self::FontName,
                TextFormat::FontSize(_) => Self::FontSize,
                TextFormat::Color(_) => Self::Color,
                TextFormat::Bold
                | TextFormat::NotBold
                | TextFormat::Italic
                | TextFormat::NotItalic
                | TextFormat::Underline
                | TextFormat::NotUnderline => Self::FontAttributes,
            },
            _ => return None,
        })
    }

    /// The Events indication of an earcon.
    #[must_use]
    pub const fn of_earcon(earcon: Earcon) -> Self {
        match earcon {
            Earcon::AppNotResponding => Self::AppNotResponding,
            Earcon::Start => Self::Start,
            Earcon::Exit => Self::Exit,
            Earcon::Error => Self::Error,
            Earcon::BrowseMode => Self::BrowseMode,
            Earcon::FocusMode => Self::FocusMode,
            Earcon::SuggestionsOpened => Self::SuggestionsOpened,
            Earcon::SuggestionsClosed => Self::SuggestionsClosed,
            Earcon::Progress(_) => Self::Progress,
        }
    }

    /// How the built-in default theme reports the indication: everything
    /// spoken as NVDA speaks it, and sounds where NVDA plays them by
    /// default (the mode switches, suggestions, errors, start and exit,
    /// and the spelling error sound alongside its words), plus Verbatim's
    /// own cues for an application not responding and skipped terminal
    /// lines, and tones for progress bars. Font name, size, and color are
    /// off, as NVDA's are by default; a capital is raised in pitch.
    #[must_use]
    pub fn default_setting(self) -> IndicationSetting {
        let file = |name: &str| Some(SoundSource::File(name.to_owned()));
        let tone = |frequency_hz, duration_ms| {
            Some(SoundSource::Tone(Tone {
                frequency_hz,
                duration_ms,
            }))
        };
        let (report, sound) = match self {
            Self::SpellingError => (Presentation::SpeechAndSound, file("textError.wav")),
            Self::FontName | Self::FontSize | Self::Color | Self::FontAttributes => {
                (Presentation::Off, None)
            }
            Self::Capital => (Presentation::Speech, tone(1_760, 40)),
            Self::SkippedLines => (Presentation::SpeechAndSound, tone(330, 80)),
            Self::AppNotResponding => (Presentation::Sound, tone(220, 150)),
            Self::Start => (Presentation::Sound, file("start.wav")),
            Self::Exit => (Presentation::Sound, file("exit.wav")),
            Self::Error => (Presentation::Sound, file("error.wav")),
            Self::BrowseMode => (Presentation::Sound, file("browseMode.wav")),
            Self::FocusMode => (Presentation::Sound, file("focusMode.wav")),
            Self::SuggestionsOpened => (Presentation::Sound, file("suggestionsOpened.wav")),
            Self::SuggestionsClosed => (Presentation::Sound, file("suggestionsClosed.wav")),
            Self::Progress => (Presentation::Sound, tone(PROGRESS_BASE_HZ, 40)),
            _ => (Presentation::Speech, None),
        };
        IndicationSetting {
            report,
            sound,
            ..IndicationSetting::default()
        }
    }
}

/// The frequency of the default theme's progress tone at 0 percent.
const PROGRESS_BASE_HZ: u32 = 220;

/// How far a progress tone rises from 0 to 100 percent: three octaves.
const PROGRESS_OCTAVES: f64 = 3.0;

/// The frequency of a progress tone at `percent` (0 to 100), given the
/// theme's tone for progress at 0 percent: rising evenly in pitch by three
/// octaves to 100 percent.
#[must_use]
pub fn progress_frequency(base_hz: u32, percent: u8) -> u32 {
    let percent = f64::from(percent.min(100));
    let frequency = f64::from(base_hz) * (PROGRESS_OCTAVES * percent / 100.0).exp2();
    // Within u32: at most eight times a u32 frequency, and frequencies are
    // validated to at most 20 kHz.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a positive frequency well within u32"
    )]
    let frequency = frequency.round() as u32;
    frequency
}

impl fmt::Display for Indication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id())
    }
}

impl Serialize for Indication {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.id())
    }
}

impl<'de> Deserialize<'de> for Indication {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::from_id(&id)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown indication `{id}`")))
    }
}

/// `MenuItem` as `menu-item`.
fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// How an indication is reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Presentation {
    /// Not at all, and where the reducer can tell, not even fetched.
    Off,
    /// In words.
    #[default]
    Speech,
    /// By its sound alone. With no sound to play (none set, or its file
    /// missing or unreadable), it is spoken instead, so information is
    /// never dropped by accident.
    Sound,
    /// By its sound, then in words.
    SpeechAndSound,
}

impl Presentation {
    /// Every choice, in the order the theme panel lists them.
    pub const ALL: [Self; 4] = [Self::Off, Self::Speech, Self::Sound, Self::SpeechAndSound];

    /// Whether this reports in words.
    #[must_use]
    pub const fn speaks(self) -> bool {
        matches!(self, Self::Speech | Self::SpeechAndSound)
    }

    /// Whether this plays a sound.
    #[must_use]
    pub const fn sounds(self) -> bool {
        matches!(self, Self::Sound | Self::SpeechAndSound)
    }
}

/// Where an indication's sound comes from: a WAV file of the theme, or a
/// tone generated in code. In a theme file, `sound = "textError.wav"` or
/// `sound = { frequency = 880, duration = 40 }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SoundSource {
    /// A WAV file, named relative to the theme's directory, or to the
    /// shared `sounds` directory when the theme does not have it.
    File(String),
    /// A sine tone.
    Tone(Tone),
}

/// A sine tone, generated in code with a short fade in and out so it does
/// not click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tone {
    /// Frequency in hertz.
    #[serde(rename = "frequency")]
    pub frequency_hz: u32,
    /// Length in milliseconds.
    #[serde(rename = "duration")]
    pub duration_ms: u32,
}

/// The gain every sound has unless a theme says otherwise: 100 percent.
pub const DEFAULT_GAIN: u16 = 100;

/// The highest gain a theme may give, in percent.
pub const MAX_GAIN: u16 = 400;

/// How one indication is reported in a theme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicationSetting {
    /// Off, speech, sound, or speech and sound.
    pub report: Presentation,
    /// The sound, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundSource>,
    /// The sound's gain in percent, 100 for as recorded, up to
    /// [`MAX_GAIN`].
    #[serde(default = "default_gain", skip_serializing_if = "is_default_gain")]
    pub gain: u16,
    /// Words spoken in place of the indication's own: they replace a role's
    /// or state's name, and come before what carries content of its own
    /// (a description, a position, a font). `None` for the usual words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    /// The name of one of the theme's voice styles to speak the words in,
    /// or `None` for the voice as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl Default for IndicationSetting {
    fn default() -> Self {
        Self {
            report: Presentation::Speech,
            sound: None,
            gain: DEFAULT_GAIN,
            words: None,
            voice: None,
        }
    }
}

const fn default_gain() -> u16 {
    DEFAULT_GAIN
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_default_gain(gain: &u16) -> bool {
    *gain == DEFAULT_GAIN
}

/// A voice style a theme defines, applied to the words of indications that
/// name it: changes relative to the configured voice, from -100 to 100.
/// Only the pitch is applied so far; rate and volume are kept for when the
/// speech sequence can carry them (milestone M11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceStyle {
    /// Pitch change.
    pub pitch: i32,
    /// Rate change.
    pub rate: i32,
    /// Volume change.
    pub volume: i32,
}

/// A theme: how every indication of the catalogue is reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    /// The id the settings and profiles refer to the theme by, which is
    /// also the name of its directory.
    pub id: String,
    /// Its name, as listed. The built-in default theme's is empty: it is
    /// listed by its localized name.
    pub name: String,
    /// Who made it.
    #[serde(default)]
    pub author: String,
    /// What it is for.
    #[serde(default)]
    pub description: String,
    /// Its version, as its author numbers it.
    #[serde(default)]
    pub version: String,
    /// Gain applied to all its sounds, in percent.
    #[serde(default = "default_gain")]
    pub gain: u16,
    /// Its voice styles, by name.
    #[serde(default)]
    pub voice_styles: BTreeMap<String, VoiceStyle>,
    /// The indications where it differs from the default theme (all of
    /// them, for the default theme itself).
    #[serde(default)]
    pub indications: BTreeMap<Indication, IndicationSetting>,
}

impl Theme {
    /// The id of the built-in default theme.
    pub const DEFAULT_ID: &'static str = "default";

    /// The built-in default theme, complete: every indication of the
    /// catalogue with its [`Indication::default_setting`]. It cannot be
    /// changed in place; editing it makes a new theme based on it.
    #[must_use]
    pub fn builtin_default() -> Self {
        Self {
            id: Self::DEFAULT_ID.to_owned(),
            name: String::new(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            gain: DEFAULT_GAIN,
            voice_styles: BTreeMap::new(),
            indications: Indication::catalogue()
                .into_iter()
                .map(|indication| (indication, indication.default_setting()))
                .collect(),
        }
    }

    /// A new, empty theme: no differences from the default.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            author: String::new(),
            description: String::new(),
            version: String::new(),
            gain: DEFAULT_GAIN,
            voice_styles: BTreeMap::new(),
            indications: BTreeMap::new(),
        }
    }

    /// Whether this is the built-in default theme.
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        self.id == Self::DEFAULT_ID
    }

    /// How the theme reports `indication`: its own setting when it has one,
    /// and the default theme's otherwise.
    #[must_use]
    pub fn setting(&self, indication: Indication) -> IndicationSetting {
        self.indications
            .get(&indication)
            .cloned()
            .unwrap_or_else(|| indication.default_setting())
    }

    /// Whether the theme reports `indication` differently from the default
    /// theme, which the theme panel marks "changed".
    #[must_use]
    pub fn differs(&self, indication: Indication) -> bool {
        self.indications
            .get(&indication)
            .is_some_and(|setting| *setting != indication.default_setting())
    }

    /// What the reducer fetches under this theme: each detail unless its
    /// indication is off.
    #[must_use]
    pub fn fetches(&self) -> Fetches {
        let on = |indication| self.setting(indication).report != Presentation::Off;
        Fetches {
            description: on(Indication::Description),
            shortcut: on(Indication::Shortcut),
            position: on(Indication::Position),
            level: on(Indication::Level),
            spelling_errors: on(Indication::SpellingError),
            grammar_errors: on(Indication::GrammarError),
            font: on(Indication::FontName) || on(Indication::FontSize),
            color: on(Indication::Color),
            font_attributes: on(Indication::FontAttributes),
        }
    }

    /// The problems with this theme that can be found without reading its
    /// sound files: an indication reported by sound alone with no sound,
    /// a voice style that does not exist, a gain above [`MAX_GAIN`], and a
    /// sound file name that is not a plain file name. Sound files that are
    /// missing or cannot be read are found by whatever loads them.
    #[must_use]
    pub fn problems(&self) -> Vec<ThemeProblem> {
        let mut problems = Vec::new();
        if self.gain > MAX_GAIN {
            problems.push(ThemeProblem::GainTooHigh { indication: None });
        }
        for (indication, setting) in &self.indications {
            let indication = *indication;
            if setting.report == Presentation::Sound && setting.sound.is_none() {
                problems.push(ThemeProblem::SoundOnlyWithoutSound { indication });
            }
            if setting.gain > MAX_GAIN {
                problems.push(ThemeProblem::GainTooHigh {
                    indication: Some(indication),
                });
            }
            if let Some(style) = &setting.voice
                && !self.voice_styles.contains_key(style)
            {
                problems.push(ThemeProblem::UnknownVoiceStyle {
                    indication,
                    style: style.clone(),
                });
            }
            if let Some(SoundSource::File(file)) = &setting.sound
                && !is_plain_file_name(file)
            {
                problems.push(ThemeProblem::InvalidSoundName {
                    indication,
                    file: file.clone(),
                });
            }
        }
        problems
    }
}

/// Whether `name` names a file in a directory itself, with no directory
/// part: no separators, not `.` or `..`, and not empty.
#[must_use]
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

/// Something wrong with a theme, found when it is loaded: listed in the
/// theme panel and the log. A theme with problems still loads; an
/// indication whose sound is unavailable is spoken instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ThemeProblem {
    /// The theme names an indication this version does not know.
    UnknownIndication {
        /// The id it names.
        id: String,
    },
    /// An indication is reported by sound alone, with no sound set.
    SoundOnlyWithoutSound {
        /// The indication.
        indication: Indication,
    },
    /// A sound file is not in the theme or the shared sounds.
    MissingSound {
        /// The indication using it.
        indication: Indication,
        /// The file name.
        file: String,
    },
    /// A sound file could not be read or decoded.
    UnreadableSound {
        /// The indication using it.
        indication: Indication,
        /// The file name.
        file: String,
        /// Why.
        reason: String,
    },
    /// A sound file name has a directory in it.
    InvalidSoundName {
        /// The indication using it.
        indication: Indication,
        /// The file name.
        file: String,
    },
    /// An indication names a voice style the theme does not define.
    UnknownVoiceStyle {
        /// The indication.
        indication: Indication,
        /// The style it names.
        style: String,
    },
    /// A gain above [`MAX_GAIN`], used as that maximum: the theme's own when
    /// `indication` is `None`.
    GainTooHigh {
        /// The indication, or `None` for the theme's gain.
        indication: Option<Indication>,
    },
}

impl fmt::Display for ThemeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownIndication { id } => write!(f, "unknown indication `{id}`"),
            Self::SoundOnlyWithoutSound { indication } => {
                write!(f, "{indication} is reported by sound with no sound set")
            }
            Self::MissingSound { indication, file } => {
                write!(f, "{indication}: sound file {file} is missing")
            }
            Self::UnreadableSound {
                indication,
                file,
                reason,
            } => write!(
                f,
                "{indication}: sound file {file} cannot be read: {reason}"
            ),
            Self::InvalidSoundName { indication, file } => {
                write!(f, "{indication}: `{file}` is not a plain file name")
            }
            Self::UnknownVoiceStyle { indication, style } => {
                write!(f, "{indication}: no voice style named `{style}`")
            }
            Self::GainTooHigh {
                indication: Some(indication),
            } => write!(f, "{indication}: gain above {MAX_GAIN} percent"),
            Self::GainTooHigh { indication: None } => {
                write!(f, "theme gain above {MAX_GAIN} percent")
            }
        }
    }
}

/// The details the reducer has fetched, decided by the active theme
/// ([`Theme::fetches`]): a detail whose indication is off is not fetched,
/// so off also saves the cross-process call (`phase6-design.md`, "Themes:
/// one model for verbosity, speech, and sounds"). The shell gives it to the
/// reducer as `Input::Fetches`, and passes the reducer's view of it to every
/// outpost, which leaves out what is not wanted when it reads a node or
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent details, each fetched or not"
)]
pub struct Fetches {
    /// Object descriptions.
    pub description: bool,
    /// Keyboard shortcuts.
    pub shortcut: bool,
    /// Position in set and set size.
    pub position: bool,
    /// Nesting level.
    pub level: bool,
    /// Spelling errors in text.
    pub spelling_errors: bool,
    /// Grammar errors in text.
    pub grammar_errors: bool,
    /// Font name and size of text.
    pub font: bool,
    /// Color of text.
    pub color: bool,
    /// Bold, italic, and underline of text.
    pub font_attributes: bool,
}

impl Default for Fetches {
    /// Everything, as before themes existed.
    fn default() -> Self {
        Self {
            description: true,
            shortcut: true,
            position: true,
            level: true,
            spelling_errors: true,
            grammar_errors: true,
            font: true,
            color: true,
            font_attributes: true,
        }
    }
}

/// Settings that go with the theme but are not part of it: ordinary
/// settings that a configuration profile may change, like the theme's
/// choice itself (`phase6-design.md`, "Themes and configuration
/// profiles").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeOptions {
    /// The volume of sounds relative to speech, from 0 to 100.
    pub sound_volume: u8,
    /// Whether sounds play during say-all.
    pub sounds_during_say_all: bool,
    /// Whether indications reported by sound alone are spoken as well, to
    /// learn a theme's sounds.
    pub speak_sounded_indications: bool,
}

impl Default for ThemeOptions {
    fn default() -> Self {
        Self {
            sound_volume: 100,
            sounds_during_say_all: true,
            speak_sounded_indications: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogue_id_round_trips_and_is_unique() {
        let catalogue = Indication::catalogue();
        let mut ids: Vec<String> = catalogue.iter().map(|i| i.id()).collect();
        for (indication, id) in catalogue.iter().zip(&ids) {
            assert_eq!(Indication::from_id(id), Some(*indication), "{id}");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{id} is a bare TOML key"
            );
        }
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "ids are unique");
    }

    #[test]
    fn ids_are_stable() {
        assert_eq!(Indication::Role(Role::MenuItem).id(), "role-menu-item");
        assert_eq!(Indication::State(State::HasPopup).id(), "state-has-popup");
        assert_eq!(
            Indication::NegatedState(State::Checked).id(),
            "state-not-checked"
        );
        assert_eq!(Indication::SpellingError.id(), "spelling-error");
        assert_eq!(Indication::from_id("state-unheard-of"), None);
    }

    #[test]
    fn the_catalogue_lists_categories_in_order() {
        let catalogue = Indication::catalogue();
        let categories: Vec<IndicationCategory> = catalogue
            .iter()
            .map(|indication| indication.category())
            .collect();
        let mut sorted = categories.clone();
        sorted.sort();
        assert_eq!(categories, sorted);
        for category in IndicationCategory::ALL {
            assert!(categories.contains(&category), "{category:?} has entries");
        }
        assert_eq!(
            catalogue.len(),
            Role::ALL.len() + SPOKEN_STATES.len() + NEGATED_STATES.len() + 22
        );
    }

    #[test]
    fn a_theme_falls_back_to_the_default_for_what_it_does_not_mention() {
        let mut theme = Theme::new("quiet", "Quiet");
        theme.indications.insert(
            Indication::Description,
            IndicationSetting {
                report: Presentation::Off,
                ..IndicationSetting::default()
            },
        );
        assert_eq!(
            theme.setting(Indication::Description).report,
            Presentation::Off
        );
        assert_eq!(
            theme.setting(Indication::SpellingError),
            Indication::SpellingError.default_setting()
        );
        assert!(theme.differs(Indication::Description));
        assert!(!theme.differs(Indication::Level));
        assert!(!theme.fetches().description);
        assert!(theme.fetches().shortcut);
    }

    #[test]
    fn the_default_theme_is_complete_and_without_problems() {
        let theme = Theme::builtin_default();
        assert!(theme.is_builtin());
        for indication in Indication::catalogue() {
            assert!(theme.indications.contains_key(&indication), "{indication}");
            assert!(!theme.differs(indication));
        }
        assert_eq!(theme.problems(), Vec::new());
        let fetches = theme.fetches();
        assert!(fetches.description && fetches.spelling_errors);
        assert!(!fetches.font && !fetches.color);
    }

    #[test]
    fn problems_are_found() {
        let mut theme = Theme::new("broken", "Broken");
        theme.indications.insert(
            Indication::Role(Role::Link),
            IndicationSetting {
                report: Presentation::Sound,
                voice: Some("whisper".to_owned()),
                ..IndicationSetting::default()
            },
        );
        theme.indications.insert(
            Indication::Blank,
            IndicationSetting {
                report: Presentation::Sound,
                sound: Some(SoundSource::File("../escape.wav".to_owned())),
                ..IndicationSetting::default()
            },
        );
        let problems = theme.problems();
        assert!(problems.contains(&ThemeProblem::SoundOnlyWithoutSound {
            indication: Indication::Role(Role::Link)
        }));
        assert!(problems.contains(&ThemeProblem::UnknownVoiceStyle {
            indication: Indication::Role(Role::Link),
            style: "whisper".to_owned()
        }));
        assert!(problems.contains(&ThemeProblem::InvalidSoundName {
            indication: Indication::Blank,
            file: "../escape.wav".to_owned()
        }));
    }

    #[test]
    fn spans_and_earcons_find_their_indications() {
        assert_eq!(
            Indication::of_segment(&SegmentContent::Format(TextFormat::NotSpellingError)),
            Some(Indication::SpellingError)
        );
        assert_eq!(
            Indication::of_segment(&SegmentContent::Message(Message::Blank)),
            Some(Indication::Blank)
        );
        assert_eq!(
            Indication::of_segment(&SegmentContent::Label("OK".to_owned())),
            None
        );
        assert_eq!(
            Indication::of_earcon(Earcon::Progress(50)),
            Indication::Progress
        );
    }

    #[test]
    fn progress_rises_three_octaves() {
        assert_eq!(progress_frequency(220, 0), 220);
        assert_eq!(progress_frequency(220, 100), 1_760);
        assert_eq!(progress_frequency(220, 200), 1_760);
    }
}
