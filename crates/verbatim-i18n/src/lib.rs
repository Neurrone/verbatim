//! Localization for Verbatim, built on Fluent via `i18n-embed`.
//!
//! English resources are compiled into the binary as the permanent fallback,
//! so Verbatim can always speak even with a missing or damaged installation.
//! Other locales are loaded at startup from a `locale` folder (expected next
//! to the executable), which keeps translation updates decoupled from
//! binary releases.
//!
//! The localization rule from the roadmap applies workspace-wide: no
//! hardcoded user-visible strings, ever. Every user-visible string resolves
//! through [`loader`], and message lookups go through the compile-time
//! checked `fl!` macro so a typo in a message ID fails the build.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{AssetsMultiplexor, I18nAssets, I18nEmbedError, LanguageLoader};
use rust_embed::RustEmbed;
use unic_langid::LanguageIdentifier;

/// English Fluent resources compiled into the binary.
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct EmbeddedLocalizations;

/// The process-wide language loader.
///
/// Starts with embedded English loaded as the fallback; call
/// [`load_locale_dir`] during startup to add the user's languages.
pub fn loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();
    LOADER.get_or_init(new_loader)
}

/// Creates a fresh loader with embedded English loaded as the fallback.
///
/// Production code uses the shared [`loader`]; this exists so tests can
/// exercise locale loading without mutating process-wide state.
///
/// # Panics
///
/// Panics when the embedded English resources fail to load — that means the
/// binary itself is broken (a bad `.ftl` was compiled in), so there is no
/// sensible recovery.
#[must_use]
pub fn new_loader() -> FluentLanguageLoader {
    let loader = fluent_language_loader!();
    loader
        .load_fallback_language(&EmbeddedLocalizations)
        .expect("embedded English localization must load");
    // Fluent wraps interpolated arguments in Unicode bidi isolation marks
    // (U+2068 and U+2069) by default, which protects visual rendering of
    // mixed-direction text. Verbatim's Fluent output is primarily *spoken*:
    // invisible marks embedded in speech text would leak into dictionary
    // and symbol processing, character navigation, and braille, so they are
    // disabled globally.
    loader.set_use_isolating(false);
    loader
}

/// Loads `requested` languages into the shared [`loader`] from a locale
/// folder, keeping embedded English as the fallback for missing messages.
///
/// The languages actually loaded are negotiated against what the folder
/// provides, so requesting a language with no translations quietly resolves
/// to the fallback rather than failing. Returns the negotiated list, most
/// preferred first.
///
/// # Errors
///
/// Returns an error when the folder does not exist or a negotiated
/// language's resources fail to load or parse.
pub fn load_locale_dir(
    locale_dir: &Path,
    requested: &[LanguageIdentifier],
) -> Result<Vec<LanguageIdentifier>, LocaleError> {
    load_locale_dir_into(loader(), locale_dir, requested)
}

/// [`load_locale_dir`] against an explicit loader instance.
///
/// # Errors
///
/// Same conditions as [`load_locale_dir`].
pub fn load_locale_dir_into(
    loader: &FluentLanguageLoader,
    locale_dir: &Path,
    requested: &[LanguageIdentifier],
) -> Result<Vec<LanguageIdentifier>, LocaleError> {
    // Highest priority first: a translation in the locale folder wins, and
    // the embedded resources keep the fallback language available (the
    // loader refuses to load without it).
    let assets = AssetsMultiplexor::new([
        Box::new(LocaleDirAssets::try_new(locale_dir)?) as Box<dyn I18nAssets + Send + Sync>,
        Box::new(EmbeddedLocalizations),
    ]);
    i18n_embed::select(loader, &assets, requested).map_err(LocaleError::Load)
}

/// Error from [`load_locale_dir`].
#[derive(Debug)]
pub enum LocaleError {
    /// The locale folder does not exist or is not a directory.
    NotADirectory(PathBuf),
    /// Loading or parsing localization resources failed.
    Load(I18nEmbedError),
}

impl fmt::Display for LocaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADirectory(path) => {
                write!(f, "locale folder {} is not a directory", path.display())
            }
            Self::Load(error) => write!(f, "failed to load localizations: {error}"),
        }
    }
}

impl std::error::Error for LocaleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotADirectory(_) => None,
            Self::Load(error) => Some(error),
        }
    }
}

/// A locale folder on disk: one subfolder per language, each holding that
/// language's Fluent resources — for example `locale/de/verbatim.ftl`.
///
/// Deliberately not `i18n_embed::FileSystemAssets`: that type's
/// `filenames_iter` yields bare file names without the language folder
/// (i18n-embed 0.16), so language negotiation can never discover a language
/// from it. This implementation yields `language/file` paths, the layout the
/// loader expects.
struct LocaleDirAssets {
    base_dir: PathBuf,
}

impl LocaleDirAssets {
    fn try_new(base_dir: &Path) -> Result<Self, LocaleError> {
        if !base_dir.is_dir() {
            return Err(LocaleError::NotADirectory(base_dir.to_path_buf()));
        }
        Ok(Self {
            base_dir: base_dir.to_path_buf(),
        })
    }
}

impl I18nAssets for LocaleDirAssets {
    fn get_files(&self, file_path: &str) -> Vec<Cow<'_, [u8]>> {
        std::fs::read(self.base_dir.join(file_path))
            .map(Cow::from)
            .into_iter()
            .collect()
    }

    fn filenames_iter(&self) -> Box<dyn Iterator<Item = String> + '_> {
        let mut filenames = Vec::new();
        if let Ok(languages) = std::fs::read_dir(&self.base_dir) {
            for language_entry in languages.flatten() {
                let Some(language) = language_entry.file_name().to_str().map(String::from) else {
                    continue;
                };
                let Ok(files) = std::fs::read_dir(language_entry.path()) else {
                    continue;
                };
                for file_entry in files.flatten() {
                    if let Some(file_name) = file_entry.file_name().to_str() {
                        filenames.push(format!("{language}/{file_name}"));
                    }
                }
            }
        }
        Box::new(filenames.into_iter())
    }
}

/// The startup announcement — Verbatim's first user-visible string, here to
/// prove the localization pipeline end-to-end from M0.
#[must_use]
pub fn startup_message() -> String {
    i18n_embed_fl::fl!(loader(), "startup-message")
}

/// Resolves a message by runtime id — the path for ids that arrive as data,
/// such as setting-descriptor label keys. Statically known ids should use
/// the typed functions in [`messages`] instead, which are compile-time
/// checked.
#[must_use]
pub fn message(id: &str) -> String {
    loader().get(id)
}

/// Typed accessors for the M1 user-visible strings. Each goes through the
/// compile-time-checked `fl!` macro, so a missing message id fails the
/// build.
pub mod messages {
    use super::loader;
    use i18n_embed_fl::fl;

    /// Tooltip of the system tray icon.
    #[must_use]
    pub fn tray_tooltip() -> String {
        fl!(loader(), "tray-tooltip")
    }

    /// The Settings item of the Verbatim menu.
    #[must_use]
    pub fn menu_settings() -> String {
        fl!(loader(), "menu-settings")
    }

    /// The Exit item of the Verbatim menu.
    #[must_use]
    pub fn menu_exit() -> String {
        fl!(loader(), "menu-exit")
    }

    /// Spoken confirmation that `text` was copied to the clipboard, as NVDA
    /// words it: the text itself, or its length when it is 1024 characters
    /// or more, which a synthesizer may take too long to speak.
    #[must_use]
    pub fn clipboard_copied(text: &str) -> String {
        let count = text.chars().count();
        let spoken = if count < 1024 {
            text.to_owned()
        } else {
            fl!(loader(), "clipboard-characters", count = count)
        };
        fl!(loader(), "clipboard-copied", text = spoken)
    }

    /// A lock key's new state ("caps lock on"): `key` is
    /// `"toggle-caps-lock"`, `"toggle-num-lock"`, or `"toggle-scroll-lock"`.
    #[must_use]
    pub fn toggle_key_state(key: &str, on: bool) -> String {
        let loader = loader();
        let key = match key {
            "toggle-num-lock" => fl!(loader, "toggle-num-lock"),
            "toggle-scroll-lock" => fl!(loader, "toggle-scroll-lock"),
            _ => fl!(loader, "toggle-caps-lock"),
        };
        if on {
            fl!(loader, "toggle-state-on", key = key)
        } else {
            fl!(loader, "toggle-state-off", key = key)
        }
    }

    /// Spoken notice that a clipboard copy failed.
    #[must_use]
    pub fn clipboard_copy_failed() -> String {
        fl!(loader(), "clipboard-copy-failed")
    }

    /// Base title of the settings dialog.
    #[must_use]
    pub fn settings_title() -> String {
        fl!(loader(), "settings-title")
    }

    /// Settings dialog title carrying the active category name.
    #[must_use]
    pub fn settings_title_with_category(category: &str) -> String {
        fl!(
            loader(),
            "settings-title-with-category",
            category = category
        )
    }

    /// Label of the category list in the settings dialog.
    #[must_use]
    pub fn settings_categories_label() -> String {
        fl!(loader(), "settings-categories-label")
    }

    /// Name of the Speech settings category.
    #[must_use]
    pub fn settings_category_speech() -> String {
        fl!(loader(), "settings-category-speech")
    }

    /// The OK button.
    #[must_use]
    pub fn button_ok() -> String {
        fl!(loader(), "button-ok")
    }

    /// The Cancel button.
    #[must_use]
    pub fn button_cancel() -> String {
        fl!(loader(), "button-cancel")
    }

    /// The Apply button.
    #[must_use]
    pub fn button_apply() -> String {
        fl!(loader(), "button-apply")
    }

    /// Label of the synthesizer group on the Speech page.
    #[must_use]
    pub fn speech_synthesizer_group() -> String {
        fl!(loader(), "speech-synthesizer-group")
    }

    /// The button opening the Select Synthesizer dialog.
    #[must_use]
    pub fn speech_change_synth() -> String {
        fl!(loader(), "speech-change-synth")
    }

    /// Title of the system tray items list dialog.
    #[must_use]
    pub fn tray_list_title() -> String {
        fl!(loader(), "tray-list-title")
    }

    /// Label above the list in the system tray items dialog.
    #[must_use]
    pub fn tray_list_label() -> String {
        fl!(loader(), "tray-list-label")
    }

    /// Title of the taskbar items list dialog.
    #[must_use]
    pub fn taskbar_list_title() -> String {
        fl!(loader(), "taskbar-list-title")
    }

    /// Label above the list in the taskbar items dialog.
    #[must_use]
    pub fn taskbar_list_label() -> String {
        fl!(loader(), "taskbar-list-label")
    }

    /// The Left Click button of the tray and taskbar list dialogs.
    #[must_use]
    pub fn tray_list_left_click() -> String {
        fl!(loader(), "tray-list-left-click")
    }

    /// The Left Double Click button of the tray and taskbar list dialogs.
    #[must_use]
    pub fn tray_list_left_double_click() -> String {
        fl!(loader(), "tray-list-left-double-click")
    }

    /// The Right Click button of the tray and taskbar list dialogs.
    #[must_use]
    pub fn tray_list_right_click() -> String {
        fl!(loader(), "tray-list-right-click")
    }

    /// Title of the Select Synthesizer dialog.
    #[must_use]
    pub fn select_synth_title() -> String {
        fl!(loader(), "select-synth-title")
    }

    /// Label of the synthesizer choice in the Select Synthesizer dialog.
    #[must_use]
    pub fn select_synth_label() -> String {
        fl!(loader(), "select-synth-label")
    }
}

/// The localized wording of a fixed reader message — a navigation edge and
/// its siblings — used by the speech pipeline when it renders
/// `SegmentContent::Message` spans. Wording matches NVDA's.
#[must_use]
pub fn message_text(message: verbatim_model::Message) -> String {
    use verbatim_model::Message;
    let loader = loader();
    match message {
        Message::NoNextObject => i18n_embed_fl::fl!(loader, "message-no-next-object"),
        Message::NoPreviousObject => i18n_embed_fl::fl!(loader, "message-no-previous-object"),
        Message::NoContainingObject => i18n_embed_fl::fl!(loader, "message-no-containing-object"),
        Message::NoObjectsInside => i18n_embed_fl::fl!(loader, "message-no-objects-inside"),
        Message::Top => i18n_embed_fl::fl!(loader, "message-top"),
        Message::Bottom => i18n_embed_fl::fl!(loader, "message-bottom"),
        Message::Left => i18n_embed_fl::fl!(loader, "message-left"),
        Message::Right => i18n_embed_fl::fl!(loader, "message-right"),
        Message::Blank => i18n_embed_fl::fl!(loader, "message-blank"),
        Message::MoveToFocus => i18n_embed_fl::fl!(loader, "message-move-to-focus"),
        Message::NoNavigatorObject => i18n_embed_fl::fl!(loader, "message-no-navigator-object"),
        Message::Activate => i18n_embed_fl::fl!(loader, "message-activate"),
        Message::NoAction => i18n_embed_fl::fl!(loader, "message-no-action"),
        Message::Invoke => i18n_embed_fl::fl!(loader, "message-invoke"),
        Message::Space => i18n_embed_fl::fl!(loader, "message-space"),
        Message::StartMarked => i18n_embed_fl::fl!(loader, "message-start-marked"),
        Message::NoStartMarker => i18n_embed_fl::fl!(loader, "message-no-start-marker"),
        Message::StartMarkerElsewhere => {
            i18n_embed_fl::fl!(loader, "message-start-marker-elsewhere")
        }
        Message::CaretMovesReview => i18n_embed_fl::fl!(loader, "message-caret-moves-review"),
        Message::CaretDoesNotMoveReview => {
            i18n_embed_fl::fl!(loader, "message-caret-does-not-move-review")
        }
        Message::NotSupported => i18n_embed_fl::fl!(loader, "message-not-supported"),
        Message::NoCaret => i18n_embed_fl::fl!(loader, "message-no-caret"),
        // `Message` is non_exhaustive; an unmapped future message speaks
        // nothing rather than crashing the pipeline.
        _ => String::new(),
    }
}

/// The localized wording of a reader message with values in it
/// (`SegmentContent::Phrase`), used by the speech pipeline when it renders
/// utterances. Wording matches NVDA's.
#[must_use]
pub fn phrase_text(phrase: &verbatim_model::Phrase) -> String {
    use verbatim_model::Phrase;
    let loader = loader();
    match phrase {
        Phrase::Selected(text) => {
            i18n_embed_fl::fl!(loader, "phrase-selected", text = selection_text(text))
        }
        Phrase::Unselected(text) => {
            i18n_embed_fl::fl!(loader, "phrase-unselected", text = selection_text(text))
        }
        Phrase::Positioned { x, y } => i18n_embed_fl::fl!(
            loader,
            "phrase-positioned",
            x = x.to_string(),
            y = y.to_string()
        ),
        Phrase::SpeakTypedCharacters(mode) => i18n_embed_fl::fl!(
            loader,
            "phrase-speak-typed-characters",
            mode = typing_echo_name(*mode)
        ),
        Phrase::SpeakTypedWords(mode) => i18n_embed_fl::fl!(
            loader,
            "phrase-speak-typed-words",
            mode = typing_echo_name(*mode)
        ),
        Phrase::SkippedLines(1) => {
            i18n_embed_fl::fl!(loader, "phrase-skipped-line", count = "1")
        }
        Phrase::SkippedLines(count) => {
            i18n_embed_fl::fl!(loader, "phrase-skipped-lines", count = count.to_string())
        }
        // `Phrase` is non_exhaustive; an unmapped future phrase speaks
        // nothing rather than crashing the pipeline.
        _ => String::new(),
    }
}

/// The spoken form of text named in a selection announcement: the text, a
/// lone character by its name, or a count of characters.
fn selection_text(text: &verbatim_model::SelectionText) -> String {
    use verbatim_model::SelectionText;
    match text {
        SelectionText::Text(text) => text.clone(),
        SelectionText::Character(character) => {
            character_name(character, None).unwrap_or_else(|| character.clone())
        }
        SelectionText::Characters(count) => {
            i18n_embed_fl::fl!(loader(), "phrase-characters", count = count.to_string())
        }
    }
}

/// The localized name of a typing echo choice, as its toggle announces it.
#[must_use]
pub fn typing_echo_name(mode: verbatim_model::TypingEcho) -> String {
    use verbatim_model::TypingEcho;
    let loader = loader();
    match mode {
        TypingEcho::Off => i18n_embed_fl::fl!(loader, "typing-echo-off"),
        TypingEcho::EditControls => i18n_embed_fl::fl!(loader, "typing-echo-edit-controls"),
        TypingEcho::Always => i18n_embed_fl::fl!(loader, "typing-echo-always"),
    }
}

/// The name a character is spoken by on its own, from the character table
/// of `language` (a BCP 47 tag; `None`, or a language with no table loaded,
/// uses the loaded languages, English last): "comma" for a comma, "space"
/// for a space. `None` when the character has no name, as letters and
/// digits have none, or when `character` is more than one code point. The
/// table is the `character-name-` messages of each locale's Fluent file,
/// keyed by code point, so another language's names are data, not code
/// (`phase6-design.md`, "Internationalization in the text model").
#[must_use]
pub fn character_name(character: &str, language: Option<&str>) -> Option<String> {
    let code = single_code_point(character)?;
    lookup(&format!("character-name-{code:04x}"), language)
}

/// The description a character is spoken by when it is asked for twice
/// ("Alpha" for a), from the character table of `language`; a capital
/// letter has its small letter's description. `None` when the table has
/// none for it.
#[must_use]
pub fn character_description(character: &str, language: Option<&str>) -> Option<String> {
    let code = single_code_point(character)?;
    let mut lower = char::from_u32(code)?.to_lowercase();
    let first = lower.next()?;
    if lower.next().is_some() {
        return None;
    }
    lookup(
        &format!("character-description-{:04x}", u32::from(first)),
        language,
    )
}

/// The code point of a one-code-point string.
fn single_code_point(character: &str) -> Option<u32> {
    let mut chars = character.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(u32::from(first))
}

/// A table message by runtime id, in `language` when it parses, falling
/// back to the loaded languages; `None` when no loaded language has it.
fn lookup(id: &str, language: Option<&str>) -> Option<String> {
    let loader = loader();
    if let Some(language) = language.and_then(|tag| tag.parse::<LanguageIdentifier>().ok()) {
        let chosen = loader.select_languages(&[language]);
        if chosen.has(id) {
            return Some(chosen.get(id));
        }
    }
    loader.has(id).then(|| loader.get(id))
}

/// The localized spoken name of a role, used by the speech pipeline when it
/// renders utterance tokens.
#[must_use]
pub fn role_name(role: verbatim_model::Role) -> String {
    use verbatim_model::Role;
    let loader = loader();
    match role {
        Role::Window => i18n_embed_fl::fl!(loader, "role-window"),
        Role::Dialog => i18n_embed_fl::fl!(loader, "role-dialog"),
        Role::Pane => i18n_embed_fl::fl!(loader, "role-pane"),
        Role::PropertyPage => i18n_embed_fl::fl!(loader, "role-property-page"),
        Role::Group => i18n_embed_fl::fl!(loader, "role-group"),
        Role::MenuBar => i18n_embed_fl::fl!(loader, "role-menu-bar"),
        Role::Menu => i18n_embed_fl::fl!(loader, "role-menu"),
        Role::MenuItem => i18n_embed_fl::fl!(loader, "role-menu-item"),
        Role::Button => i18n_embed_fl::fl!(loader, "role-button"),
        Role::ToggleButton => i18n_embed_fl::fl!(loader, "role-toggle-button"),
        Role::CheckBox => i18n_embed_fl::fl!(loader, "role-check-box"),
        Role::RadioButton => i18n_embed_fl::fl!(loader, "role-radio-button"),
        Role::ComboBox => i18n_embed_fl::fl!(loader, "role-combo-box"),
        Role::List => i18n_embed_fl::fl!(loader, "role-list"),
        Role::ListItem => i18n_embed_fl::fl!(loader, "role-list-item"),
        Role::Slider => i18n_embed_fl::fl!(loader, "role-slider"),
        Role::SpinButton => i18n_embed_fl::fl!(loader, "role-spin-button"),
        Role::TabControl => i18n_embed_fl::fl!(loader, "role-tab-control"),
        Role::Tab => i18n_embed_fl::fl!(loader, "role-tab"),
        Role::StaticText => i18n_embed_fl::fl!(loader, "role-static-text"),
        Role::EditableText => i18n_embed_fl::fl!(loader, "role-editable-text"),
        Role::Link => i18n_embed_fl::fl!(loader, "role-link"),
        Role::ToolBar => i18n_embed_fl::fl!(loader, "role-tool-bar"),
        Role::StatusBar => i18n_embed_fl::fl!(loader, "role-status-bar"),
        Role::Tree => i18n_embed_fl::fl!(loader, "role-tree"),
        Role::TreeItem => i18n_embed_fl::fl!(loader, "role-tree-item"),
        Role::SplitButton => i18n_embed_fl::fl!(loader, "role-split-button"),
        Role::DropDownButton => i18n_embed_fl::fl!(loader, "role-drop-down-button"),
        Role::MenuButton => i18n_embed_fl::fl!(loader, "role-menu-button"),
        Role::Graphic => i18n_embed_fl::fl!(loader, "role-graphic"),
        Role::ProgressBar => i18n_embed_fl::fl!(loader, "role-progress-bar"),
        Role::ScrollBar => i18n_embed_fl::fl!(loader, "role-scroll-bar"),
        Role::Table => i18n_embed_fl::fl!(loader, "role-table"),
        Role::Row => i18n_embed_fl::fl!(loader, "role-row"),
        Role::Cell => i18n_embed_fl::fl!(loader, "role-cell"),
        Role::ColumnHeader => i18n_embed_fl::fl!(loader, "role-column-header"),
        Role::RowHeader => i18n_embed_fl::fl!(loader, "role-row-header"),
        Role::Header => i18n_embed_fl::fl!(loader, "role-header"),
        Role::HeaderItem => i18n_embed_fl::fl!(loader, "role-header-item"),
        Role::DataGrid => i18n_embed_fl::fl!(loader, "role-data-grid"),
        Role::DataItem => i18n_embed_fl::fl!(loader, "role-data-item"),
        Role::Calendar => i18n_embed_fl::fl!(loader, "role-calendar"),
        Role::ToolTip => i18n_embed_fl::fl!(loader, "role-tool-tip"),
        Role::TitleBar => i18n_embed_fl::fl!(loader, "role-title-bar"),
        Role::Separator => i18n_embed_fl::fl!(loader, "role-separator"),
        Role::Document => i18n_embed_fl::fl!(loader, "role-document"),
        Role::Application => i18n_embed_fl::fl!(loader, "role-application"),
        Role::Alert => i18n_embed_fl::fl!(loader, "role-alert"),
        Role::HotkeyField => i18n_embed_fl::fl!(loader, "role-hotkey-field"),
        Role::Thumb => i18n_embed_fl::fl!(loader, "role-thumb"),
        Role::Terminal => i18n_embed_fl::fl!(loader, "role-terminal"),
        _ => i18n_embed_fl::fl!(loader, "role-unknown"),
    }
}

/// The localized spoken name of a state, or `None` for states that are
/// never announced (focusable, selectable, checkable).
#[must_use]
pub fn state_name(state: verbatim_model::State) -> Option<String> {
    use verbatim_model::State;
    let loader = loader();
    Some(match state {
        State::Focused => i18n_embed_fl::fl!(loader, "state-focused"),
        State::Offscreen => i18n_embed_fl::fl!(loader, "state-offscreen"),
        State::Selected => i18n_embed_fl::fl!(loader, "state-selected"),
        State::Checked => i18n_embed_fl::fl!(loader, "state-checked"),
        State::Mixed => i18n_embed_fl::fl!(loader, "state-mixed"),
        State::Disabled => i18n_embed_fl::fl!(loader, "state-disabled"),
        State::ReadOnly => i18n_embed_fl::fl!(loader, "state-read-only"),
        State::Expanded => i18n_embed_fl::fl!(loader, "state-expanded"),
        State::Collapsed => i18n_embed_fl::fl!(loader, "state-collapsed"),
        State::Pressed => i18n_embed_fl::fl!(loader, "state-pressed"),
        State::HasPopup => i18n_embed_fl::fl!(loader, "state-has-popup"),
        State::Busy => i18n_embed_fl::fl!(loader, "state-busy"),
        State::Protected => i18n_embed_fl::fl!(loader, "state-protected"),
        State::Required => i18n_embed_fl::fl!(loader, "state-required"),
        State::InvalidEntry => i18n_embed_fl::fl!(loader, "state-invalid-entry"),
        _ => return None,
    })
}

/// The localized announcement for the notable absence of a state — "not
/// checked" for an unchecked check box — or `None` when the absence is not
/// announced.
#[must_use]
pub fn negated_state_name(state: verbatim_model::State) -> Option<String> {
    use verbatim_model::State;
    let loader = loader();
    Some(match state {
        State::Checked => i18n_embed_fl::fl!(loader, "state-not-checked"),
        State::Selected => i18n_embed_fl::fl!(loader, "state-not-selected"),
        State::Pressed => i18n_embed_fl::fl!(loader, "state-not-pressed"),
        _ => return None,
    })
}

/// The localized "2 of 5" phrase for a position within a set.
///
/// Both numbers pass as pre-rendered strings, not Fluent numbers, so no
/// locale applies digit grouping to what is an ordinal position.
#[must_use]
pub fn position_in_set(position: u32, set_size: u32) -> String {
    i18n_embed_fl::fl!(
        loader(),
        "object-position-in-set",
        position = position.to_string(),
        set_size = set_size.to_string()
    )
}

/// The localized "level 3" phrase for a nesting level.
#[must_use]
pub fn level(level: u32) -> String {
    i18n_embed_fl::fl!(loader(), "object-level", level = level.to_string())
}

/// The localized name of the built-in default theme, which has no name of
/// its own in the theme model.
#[must_use]
pub fn theme_default_name() -> String {
    i18n_embed_fl::fl!(loader(), "theme-default-name")
}

/// The localized description of the built-in default theme.
#[must_use]
pub fn theme_default_description() -> String {
    i18n_embed_fl::fl!(loader(), "theme-default-description")
}

/// The localized name of an indication category, as the theme panel lists
/// it ("Text formatting").
#[must_use]
pub fn indication_category_name(category: verbatim_model::IndicationCategory) -> String {
    use verbatim_model::IndicationCategory;
    let loader = loader();
    match category {
        IndicationCategory::Roles => i18n_embed_fl::fl!(loader, "indication-category-roles"),
        IndicationCategory::States => i18n_embed_fl::fl!(loader, "indication-category-states"),
        IndicationCategory::Properties => {
            i18n_embed_fl::fl!(loader, "indication-category-properties")
        }
        IndicationCategory::TextFormatting => {
            i18n_embed_fl::fl!(loader, "indication-category-text-formatting")
        }
        IndicationCategory::Structure => {
            i18n_embed_fl::fl!(loader, "indication-category-structure")
        }
        IndicationCategory::Events => i18n_embed_fl::fl!(loader, "indication-category-events"),
    }
}

/// The localized name of an indication, as the theme panel lists it: a
/// role's or state's spoken name ("link", "not checked"), and the
/// indication's own name otherwise ("spelling error").
#[must_use]
pub fn indication_name(indication: verbatim_model::Indication) -> String {
    use verbatim_model::Indication;
    let loader = loader();
    match indication {
        Indication::Role(role) => role_name(role),
        Indication::State(state) => state_name(state).unwrap_or_else(|| indication.id()),
        Indication::NegatedState(state) => {
            negated_state_name(state).unwrap_or_else(|| indication.id())
        }
        Indication::Description => i18n_embed_fl::fl!(loader, "indication-description"),
        Indication::Shortcut => i18n_embed_fl::fl!(loader, "indication-shortcut"),
        Indication::Position => i18n_embed_fl::fl!(loader, "indication-position"),
        Indication::Level => i18n_embed_fl::fl!(loader, "indication-level"),
        Indication::SpellingError => i18n_embed_fl::fl!(loader, "indication-spelling-error"),
        Indication::GrammarError => i18n_embed_fl::fl!(loader, "indication-grammar-error"),
        Indication::FontName => i18n_embed_fl::fl!(loader, "indication-font-name"),
        Indication::FontSize => i18n_embed_fl::fl!(loader, "indication-font-size"),
        Indication::Color => i18n_embed_fl::fl!(loader, "indication-color"),
        Indication::Capital => i18n_embed_fl::fl!(loader, "indication-capital"),
        Indication::Blank => i18n_embed_fl::fl!(loader, "indication-blank"),
        Indication::SkippedLines => i18n_embed_fl::fl!(loader, "indication-skipped-lines"),
        Indication::AppNotResponding => {
            i18n_embed_fl::fl!(loader, "indication-app-not-responding")
        }
        Indication::Start => i18n_embed_fl::fl!(loader, "indication-start"),
        Indication::Exit => i18n_embed_fl::fl!(loader, "indication-exit"),
        Indication::Error => i18n_embed_fl::fl!(loader, "indication-error"),
        Indication::BrowseMode => i18n_embed_fl::fl!(loader, "indication-browse-mode"),
        Indication::FocusMode => i18n_embed_fl::fl!(loader, "indication-focus-mode"),
        Indication::SuggestionsOpened => {
            i18n_embed_fl::fl!(loader, "indication-suggestions-opened")
        }
        Indication::SuggestionsClosed => {
            i18n_embed_fl::fl!(loader, "indication-suggestions-closed")
        }
        Indication::Progress => i18n_embed_fl::fl!(loader, "indication-progress"),
        // `Indication` is non_exhaustive; a future indication is listed by
        // its id until it is given a name here.
        other => other.id(),
    }
}

/// The localized name of a way of reporting an indication ("speech and
/// sound").
#[must_use]
pub fn presentation_name(presentation: verbatim_model::Presentation) -> String {
    use verbatim_model::Presentation;
    let loader = loader();
    match presentation {
        Presentation::Off => i18n_embed_fl::fl!(loader, "presentation-off"),
        Presentation::Speech => i18n_embed_fl::fl!(loader, "presentation-speech"),
        Presentation::Sound => i18n_embed_fl::fl!(loader, "presentation-sound"),
        Presentation::SpeechAndSound => {
            i18n_embed_fl::fl!(loader, "presentation-speech-and-sound")
        }
    }
}

/// The spoken words for a formatting span (`SegmentContent::Format`):
/// NVDA's "spelling error" and "out of spelling error", and a font name,
/// size, or color as the application words it.
#[must_use]
pub fn format_text(format: &verbatim_model::TextFormat) -> String {
    use verbatim_model::TextFormat;
    let loader = loader();
    match format {
        TextFormat::SpellingError => i18n_embed_fl::fl!(loader, "format-spelling-error"),
        TextFormat::NotSpellingError => i18n_embed_fl::fl!(loader, "format-not-spelling-error"),
        TextFormat::GrammarError => i18n_embed_fl::fl!(loader, "format-grammar-error"),
        TextFormat::NotGrammarError => i18n_embed_fl::fl!(loader, "format-not-grammar-error"),
        TextFormat::FontName(text) | TextFormat::FontSize(text) | TextFormat::Color(text) => {
            text.clone()
        }
        // `TextFormat` is non_exhaustive; an unmapped future format speaks
        // nothing rather than crashing the pipeline.
        _ => String::new(),
    }
}

/// The spoken words for an event a theme reports by speech ("browse
/// mode", "40 percent").
#[must_use]
pub fn earcon_text(earcon: verbatim_model::Earcon) -> String {
    use verbatim_model::Earcon;
    let loader = loader();
    match earcon {
        Earcon::AppNotResponding => i18n_embed_fl::fl!(loader, "earcon-app-not-responding"),
        Earcon::Start => i18n_embed_fl::fl!(loader, "earcon-start"),
        Earcon::Exit => i18n_embed_fl::fl!(loader, "earcon-exit"),
        Earcon::Error => i18n_embed_fl::fl!(loader, "earcon-error"),
        Earcon::BrowseMode => i18n_embed_fl::fl!(loader, "earcon-browse-mode"),
        Earcon::FocusMode => i18n_embed_fl::fl!(loader, "earcon-focus-mode"),
        Earcon::SuggestionsOpened => i18n_embed_fl::fl!(loader, "earcon-suggestions-opened"),
        Earcon::SuggestionsClosed => i18n_embed_fl::fl!(loader, "earcon-suggestions-closed"),
        Earcon::Progress(percent) => {
            i18n_embed_fl::fl!(loader, "earcon-progress", percent = percent.to_string())
        }
        // `Earcon` is non_exhaustive; an unmapped future earcon speaks
        // nothing rather than crashing the pipeline.
        _ => String::new(),
    }
}

/// A problem found loading a theme, worded for the theme panel's
/// description.
#[must_use]
pub fn theme_problem_text(problem: &verbatim_model::ThemeProblem) -> String {
    use verbatim_model::ThemeProblem;
    let loader = loader();
    match problem {
        ThemeProblem::UnknownIndication { id } => {
            i18n_embed_fl::fl!(loader, "theme-problem-unknown-indication", id = id.as_str())
        }
        ThemeProblem::SoundOnlyWithoutSound { indication } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-sound-only-without-sound",
            indication = indication_name(*indication)
        ),
        ThemeProblem::MissingSound { indication, file } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-missing-sound",
            indication = indication_name(*indication),
            file = file.as_str()
        ),
        ThemeProblem::UnreadableSound {
            indication,
            file,
            reason,
        } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-unreadable-sound",
            indication = indication_name(*indication),
            file = file.as_str(),
            reason = reason.as_str()
        ),
        ThemeProblem::InvalidSoundName { indication, file } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-invalid-sound-name",
            indication = indication_name(*indication),
            file = file.as_str()
        ),
        ThemeProblem::UnknownVoiceStyle { indication, style } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-unknown-voice-style",
            indication = indication_name(*indication),
            style = style.as_str()
        ),
        ThemeProblem::GainTooHigh {
            indication: Some(indication),
        } => i18n_embed_fl::fl!(
            loader,
            "theme-problem-gain-too-high",
            indication = indication_name(*indication)
        ),
        ThemeProblem::GainTooHigh { indication: None } => {
            i18n_embed_fl::fl!(loader, "theme-problem-theme-gain-too-high")
        }
        // `ThemeProblem` is non_exhaustive; a future problem is described
        // in its diagnostic English until it is given wording here.
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_english_resolves() {
        assert_eq!(startup_message(), "Verbatim is starting.");
    }

    #[test]
    fn locale_dir_overrides_language_and_keeps_fallback() {
        let dir = std::env::temp_dir().join("verbatim-i18n-test-locale");
        std::fs::create_dir_all(dir.join("de")).expect("create locale dir");
        std::fs::write(
            dir.join("de").join("verbatim.ftl"),
            "startup-message = Verbatim startet.\n",
        )
        .expect("write German resource");

        let loader = new_loader();
        let german: LanguageIdentifier = "de".parse().expect("valid language id");
        load_locale_dir_into(&loader, &dir, &[german]).expect("locale dir loads");

        assert_eq!(
            loader.get("startup-message"),
            "Verbatim startet.",
            "requested language wins for translated messages"
        );
    }

    #[test]
    fn missing_translation_falls_back_to_embedded_english() {
        let dir = std::env::temp_dir().join("verbatim-i18n-test-locale-empty");
        std::fs::create_dir_all(&dir).expect("create locale dir");

        let loader = new_loader();
        let untranslated: LanguageIdentifier = "fr".parse().expect("valid language id");
        load_locale_dir_into(&loader, &dir, &[untranslated]).expect("locale dir loads");

        assert_eq!(
            loader.get("startup-message"),
            "Verbatim is starting.",
            "embedded English serves messages the requested language lacks"
        );
    }

    /// The English source of every message, for tests that enumerate ids.
    const ENGLISH_FTL: &str = include_str!("../i18n/en/verbatim.ftl");

    /// Message ids parsed from the embedded English resources. Assumes the
    /// single-line message convention documented at the top of the file.
    fn english_message_ids() -> Vec<&'static str> {
        ENGLISH_FTL
            .lines()
            .filter(|line| {
                line.starts_with(|c: char| c.is_ascii_alphanumeric()) && line.contains('=')
            })
            .filter_map(|line| line.split_once('=').map(|(id, _)| id.trim()))
            .collect()
    }

    /// The pseudo-locale test required from M1 (roadmap, localization
    /// track): every message resolves through a generated pseudo-locale,
    /// proving no string bypasses the loader and every id is translatable.
    #[test]
    fn pseudo_locale_covers_every_message() {
        let ids = english_message_ids();
        assert!(
            ids.len() > 40,
            "the M1 string inventory should be present, found {}",
            ids.len()
        );

        // Generate the pseudo-locale: every message value wrapped in
        // distinctive brackets.
        let pseudo: String = ENGLISH_FTL
            .lines()
            .map(|line| {
                let is_message =
                    line.starts_with(|c: char| c.is_ascii_alphanumeric()) && line.contains('=');
                if is_message {
                    let (id, value) = line.split_once('=').expect("message line has =");
                    format!("{id}= [!! {} !!]\n", value.trim())
                } else {
                    format!("{line}\n")
                }
            })
            .collect();

        let dir = std::env::temp_dir().join("verbatim-i18n-test-pseudo");
        std::fs::create_dir_all(dir.join("en-XA")).expect("create pseudo locale dir");
        std::fs::write(dir.join("en-XA").join("verbatim.ftl"), pseudo).expect("write pseudo ftl");

        let loader = new_loader();
        let pseudo_locale: LanguageIdentifier = "en-XA".parse().expect("valid language id");
        load_locale_dir_into(&loader, &dir, &[pseudo_locale]).expect("pseudo locale loads");

        for id in ids {
            let resolved = loader.get(id);
            assert!(
                resolved.starts_with("[!!") || resolved.starts_with("\u{2068}[!!"),
                "message {id} did not resolve through the pseudo-locale: {resolved:?}"
            );
        }
    }

    #[test]
    fn a_lock_keys_state_is_worded_as_nvda_words_it() {
        assert_eq!(
            messages::toggle_key_state("toggle-caps-lock", true),
            "caps lock on"
        );
        assert_eq!(
            messages::toggle_key_state("toggle-num-lock", false),
            "num lock off"
        );
    }

    #[test]
    fn the_clipboard_confirmation_is_nvdas() {
        assert_eq!(
            messages::clipboard_copied("Name Ann"),
            "Copied to clipboard: Name Ann"
        );
        assert_eq!(
            messages::clipboard_copied(&"x".repeat(1500)),
            "Copied to clipboard: 1500 characters"
        );
        assert_eq!(messages::clipboard_copy_failed(), "Unable to copy");
    }

    #[test]
    fn role_and_state_names_resolve() {
        assert_eq!(role_name(verbatim_model::Role::Slider), "slider");
        assert_eq!(role_name(verbatim_model::Role::Tree), "tree view");
        assert_eq!(role_name(verbatim_model::Role::TreeItem), "tree view item");
        assert_eq!(
            role_name(verbatim_model::Role::ToggleButton),
            "toggle button"
        );
        assert_eq!(
            state_name(verbatim_model::State::Checked).as_deref(),
            Some("checked")
        );
        assert_eq!(
            negated_state_name(verbatim_model::State::Checked).as_deref(),
            Some("not checked")
        );
        assert_eq!(
            state_name(verbatim_model::State::Pressed).as_deref(),
            Some("pressed")
        );
        assert_eq!(
            negated_state_name(verbatim_model::State::Pressed).as_deref(),
            Some("not pressed")
        );
        assert_eq!(state_name(verbatim_model::State::Focusable), None);
        assert_eq!(
            state_name(verbatim_model::State::Focused).as_deref(),
            Some("focused")
        );
        assert_eq!(
            messages::settings_title_with_category("Speech"),
            "Verbatim Settings: Speech",
            "no bidi isolation marks: the loader disables Fluent's argument \
             isolation because this output is primarily spoken (see new_loader)"
        );
    }

    #[test]
    fn every_indication_and_category_has_a_name() {
        use verbatim_model::{Indication, IndicationCategory, Presentation, Role, State};
        for indication in Indication::catalogue() {
            let name = indication_name(indication);
            // A name, not the id it falls back to: ids are kebab case.
            assert!(
                !name.is_empty() && !name.contains('-'),
                "{indication}: {name}"
            );
        }
        for category in IndicationCategory::ALL {
            assert_ne!(indication_category_name(category), String::new());
        }
        assert_eq!(
            presentation_name(Presentation::SpeechAndSound),
            "speech and sound"
        );
        assert_eq!(indication_name(Indication::Role(Role::Link)), "link");
        assert_eq!(
            indication_name(Indication::NegatedState(State::Checked)),
            "not checked"
        );
    }

    #[test]
    fn theme_words_are_worded() {
        use verbatim_model::{Earcon, Indication, Phrase, TextFormat, ThemeProblem};
        assert_eq!(format_text(&TextFormat::SpellingError), "spelling error");
        assert_eq!(
            format_text(&TextFormat::NotSpellingError),
            "out of spelling error"
        );
        assert_eq!(earcon_text(Earcon::Progress(40)), "40 percent");
        assert_eq!(phrase_text(&Phrase::SkippedLines(1)), "skipped 1 line");
        assert_eq!(phrase_text(&Phrase::SkippedLines(120)), "skipped 120 lines");
        assert_eq!(
            theme_problem_text(&ThemeProblem::MissingSound {
                indication: Indication::SpellingError,
                file: "x.wav".to_owned()
            }),
            "spelling error: the sound x.wav is missing"
        );
    }

    #[test]
    fn tray_list_messages_resolve() {
        assert_eq!(messages::tray_list_title(), "System Tray Icons");
        assert_eq!(messages::tray_list_label(), "&Icons");
        assert_eq!(messages::taskbar_list_title(), "Taskbar Buttons");
        assert_eq!(messages::taskbar_list_label(), "&Buttons");
        assert_eq!(messages::tray_list_left_click(), "&Left Click");
        assert_eq!(
            messages::tray_list_left_double_click(),
            "Left &Double Click"
        );
        assert_eq!(messages::tray_list_right_click(), "&Right Click");
    }

    #[test]
    fn the_character_table_names_symbols_and_not_letters() {
        assert_eq!(character_name(",", None).as_deref(), Some("comma"));
        assert_eq!(character_name(" ", None).as_deref(), Some("space"));
        assert_eq!(
            character_name("\u{207B}", None).as_deref(),
            Some("superscript minus")
        );
        assert_eq!(
            character_name("\u{215C}", None).as_deref(),
            Some("three eighths")
        );
        assert_eq!(character_name("a", None), None);
        assert_eq!(character_name("ab", None), None);
        // A language with no table of its own falls back to English.
        assert_eq!(character_name(".", Some("fr")).as_deref(), Some("dot"));
    }

    #[test]
    fn character_descriptions_are_the_phonetic_alphabet() {
        assert_eq!(character_description("a", None).as_deref(), Some("Alpha"));
        assert_eq!(character_description("X", None).as_deref(), Some("X-ray"));
        assert_eq!(character_description(",", None), None);
    }

    #[test]
    fn selection_and_toggle_phrases_are_nvdas() {
        use verbatim_model::{Phrase, SelectionText, TypingEcho};
        assert_eq!(
            phrase_text(&Phrase::Selected(SelectionText::Text("hello".into()))),
            "selected hello"
        );
        assert_eq!(
            phrase_text(&Phrase::Unselected(SelectionText::Character(",".into()))),
            "unselected comma"
        );
        assert_eq!(
            phrase_text(&Phrase::Selected(SelectionText::Characters(600))),
            "selected 600 characters"
        );
        assert_eq!(
            phrase_text(&Phrase::Positioned { x: 10, y: 20 }),
            "Positioned at 10, 20"
        );
        assert_eq!(
            phrase_text(&Phrase::SpeakTypedCharacters(TypingEcho::EditControls)),
            "speak typed characters only in edit controls"
        );
        assert_eq!(
            message_text(verbatim_model::Message::NoStartMarker),
            "No start marker set"
        );
        assert_eq!(role_name(verbatim_model::Role::Terminal), "terminal");
    }

    #[test]
    fn object_detail_phrases_resolve_without_isolation_marks() {
        assert_eq!(position_in_set(2, 5), "2 of 5");
        assert_eq!(level(3), "level 3");
    }
}
