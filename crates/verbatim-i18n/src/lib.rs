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

    /// Spoken confirmation that text was copied to the clipboard.
    #[must_use]
    pub fn clipboard_copied() -> String {
        fl!(loader(), "clipboard-copied")
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
        _ => i18n_embed_fl::fl!(loader, "role-unknown"),
    }
}

/// The localized spoken name of a state, or `None` for states that are
/// never announced (focused, focusable, selectable, offscreen).
#[must_use]
pub fn state_name(state: verbatim_model::State) -> Option<String> {
    use verbatim_model::State;
    let loader = loader();
    Some(match state {
        State::Selected => i18n_embed_fl::fl!(loader, "state-selected"),
        State::Checked => i18n_embed_fl::fl!(loader, "state-checked"),
        State::Mixed => i18n_embed_fl::fl!(loader, "state-mixed"),
        State::Disabled => i18n_embed_fl::fl!(loader, "state-disabled"),
        State::ReadOnly => i18n_embed_fl::fl!(loader, "state-read-only"),
        State::Expanded => i18n_embed_fl::fl!(loader, "state-expanded"),
        State::Collapsed => i18n_embed_fl::fl!(loader, "state-collapsed"),
        State::Pressed => i18n_embed_fl::fl!(loader, "state-pressed"),
        State::HasPopup => i18n_embed_fl::fl!(loader, "state-has-popup"),
        State::DefaultControl => i18n_embed_fl::fl!(loader, "state-default"),
        State::Busy => i18n_embed_fl::fl!(loader, "state-busy"),
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
    fn role_and_state_names_resolve() {
        assert_eq!(role_name(verbatim_model::Role::Slider), "slider");
        assert_eq!(role_name(verbatim_model::Role::Tree), "tree view");
        assert_eq!(role_name(verbatim_model::Role::TreeItem), "tree view item");
        assert_eq!(
            state_name(verbatim_model::State::Checked).as_deref(),
            Some("checked")
        );
        assert_eq!(
            negated_state_name(verbatim_model::State::Checked).as_deref(),
            Some("not checked")
        );
        assert_eq!(state_name(verbatim_model::State::Focused), None);
        assert_eq!(
            messages::settings_title_with_category("Speech"),
            "Verbatim Settings: Speech",
            "no bidi isolation marks: the loader disables Fluent's argument \
             isolation because this output is primarily spoken (see new_loader)"
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
    fn object_detail_phrases_resolve_without_isolation_marks() {
        assert_eq!(position_in_set(2, 5), "2 of 5");
        assert_eq!(level(3), "level 3");
    }
}
