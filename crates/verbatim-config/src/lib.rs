//! Configuration.
//!
//! Verbatim is portable: everything lives next to `verbatim.exe`.
//! `settings.toml` is the configuration: the Verbatim modifier keys, the
//! locale, logging, and the speech, reader and theme sections.
//! Configuration profiles are milestone M8.3's.
//!
//! Writes are atomic: a temporary file, then a rename over the target.

#![forbid(unsafe_code)]
// Non-test code never panics, overflows or truncates silently: arithmetic
// is checked or saturating, indexing goes through `get`, and casts through
// `From` or `TryFrom`. Tests are exempt, since a panic is how a test fails.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::exit,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::exit,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::string_slice,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::unwrap_in_result,
        clippy::unwrap_used
    )
)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use verbatim_model::{
    KeyboardLayout, ReaderSettings, SayAllUnit, ThemeOptions, TypingEcho, VerbatimKeys,
};

mod package;
pub mod themes;

/// Keyboard configuration within `settings.toml`. Exposed only in the file;
/// a GUI choice arrives with M8.1's gesture-remapping work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// The active gesture-binding layout.
    pub layout: KeyboardLayout,
    /// NVDA's "Speech interrupt for typed characters", on by default: a
    /// typed character, or Shift, cuts speech off. Off, typing leaves speech
    /// alone and Shift no longer pauses it (`docs/nvda/input.md`, "What a
    /// key press does to speech").
    pub speech_interrupt_for_characters: bool,
    /// NVDA's "Speech interrupt for Enter key", on by default: Enter cuts
    /// speech off. Off, Enter leaves speech alone.
    pub speech_interrupt_for_enter: bool,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: KeyboardLayout::Desktop,
            speech_interrupt_for_characters: true,
            speech_interrupt_for_enter: true,
        }
    }
}

/// UIA configuration within `settings.toml`, for developers: the defaults
/// are what Verbatim should do, and a setting here is for diagnosing a
/// provider or measuring. Read at startup
/// and handed to each outpost as it is spawned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiaConfig {
    /// Whether outposts read a UIA focus's ancestors with a remote
    /// operation, one round trip run inside the application's provider,
    /// where the provider supports one (`verbatim-uia-rops`). On by
    /// default; off forces the classic walk, one round trip per ancestor,
    /// for diagnosing a provider and for before-and-after measurements.
    pub remote_operations: bool,
}

impl Default for UiaConfig {
    fn default() -> Self {
        Self {
            remote_operations: true,
        }
    }
}

/// The speech rate the E2E suite runs at, on every synthesizer's shared
/// `0..=100` numeric scale. Deliberately brisk so a recorded run is quick to
/// review; applied by [`Settings::for_e2e`] to every run.
pub const E2E_RATE: i64 = 80;

impl Settings {
    /// The fixed configuration the E2E suite launches Verbatim with
    /// (`verbatim-e2e`'s `Scenario::launch`): [`Settings::default`] with the
    /// speech synthesizer overridden to `synth_id` and that synthesizer's
    /// `rate` set to [`E2E_RATE`]. A run's configuration is always these
    /// fixed values, never whatever a previous run or a developer's own
    /// `settings.toml` happened to leave behind, so no run loads an existing
    /// file and changes it: each builds this and writes it fresh.
    #[must_use]
    pub fn for_e2e(synth_id: &str) -> Self {
        let mut settings = Self::default();
        settings.speech.synthesizer = Some(synth_id.to_owned());
        settings.speech.synth_settings.insert(
            synth_id.to_owned(),
            BTreeMap::from([("rate".to_owned(), ConfigValue::Integer(E2E_RATE))]),
        );
        settings
    }
}

/// One persisted setting value; the TOML-facing analog of the speech
/// crate's setting values (an integer for sliders, a string for choices, a
/// boolean for toggles).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    /// A toggle value.
    Flag(bool),
    /// A slider value.
    Integer(i64),
    /// A choice's selected option id.
    Text(String),
}

/// Speech configuration within the base or a profile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechConfig {
    /// Active synthesizer id, such as `onecore`.
    pub synthesizer: Option<String>,
    /// Per-synthesizer setting values, keyed by synthesizer id and then by
    /// setting id, so switching synths never loses the other synth's values.
    pub synth_settings: BTreeMap<String, BTreeMap<String, ConfigValue>>,
}

/// The theme in use and the settings that go with it, the `[theme]`
/// section of `settings.toml` (`books/dev/src/reference/crates/verbatim-config.md`, "Themes"):
/// the id of a theme in the `themes` folder, or
/// `default` for the built-in default theme, plus the sound volume and the
/// say-all and learning checkboxes, which are ordinary settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// The theme's id.
    pub id: String,
    /// Sound volume, say-all sounds, and speaking sounded indications.
    #[serde(flatten)]
    pub options: ThemeOptions,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            id: verbatim_model::Theme::DEFAULT_ID.to_owned(),
            options: ThemeOptions::default(),
        }
    }
}

/// The contents of `settings.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Requested UI and speech language, a BCP 47 tag such as `de`; `None`
    /// is embedded English.
    pub locale: Option<String>,
    /// Tracing filter override, same syntax as `RUST_LOG`.
    pub log_filter: Option<String>,
    /// The Verbatim modifier keys.
    pub verbatim_keys: VerbatimKeys,
    /// The active keyboard layout.
    pub keyboard: KeyboardConfig,
    /// Developer settings for the UIA client.
    pub uia: UiaConfig,
    /// The speech configuration.
    pub speech: SpeechConfig,
    /// The reader settings the reducer reads, the `[reader]` section:
    /// typing echo, whether the review cursor follows the caret, say-all's
    /// reading unit, keeping the display on during say-all, and the
    /// Terminal settings (speaking passwords, reporting new output, and the
    /// flood policy's two limits), each with NVDA's default where NVDA has
    /// the setting ([`ReaderSettings`]).
    pub reader: ReaderSettings,
    /// The theme and the settings that go with it.
    pub theme: ThemeConfig,
}

/// Error from loading or saving configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// Reading or writing a file failed.
    Io(PathBuf, io::Error),
    /// A file exists but is not valid TOML for its schema.
    Parse(PathBuf, toml::de::Error),
    /// Serializing settings to TOML failed.
    Serialize(toml::ser::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "config I/O error at {}: {error}", path.display()),
            Self::Parse(path, error) => {
                write!(f, "config parse error in {}: {error}", path.display())
            }
            Self::Serialize(error) => write!(f, "config serialize error: {error}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, error) => Some(error),
            Self::Parse(_, error) => Some(error),
            Self::Serialize(error) => Some(error),
        }
    }
}

/// Loads, holds, and saves `settings.toml`.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    root: PathBuf,
    settings: Settings,
}

impl ConfigStore {
    /// File name of the settings file next to the executable.
    pub const SETTINGS_FILE: &'static str = "settings.toml";
    /// Folder name of the user's themes folder next to the executable: one
    /// directory per theme, named by its id ([`themes`]).
    pub const THEMES_DIR: &'static str = "themes";
    /// Folder name of the shared sounds next to the executable, which the
    /// built-in default theme plays and any theme may name.
    pub const SOUNDS_DIR: &'static str = "sounds";

    /// Loads configuration rooted at `root` (the folder containing
    /// `verbatim.exe`). A missing settings file loads as defaults; it is
    /// created by [`ensure_files_exist`](Self::ensure_files_exist), not by
    /// load.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] for an unreadable file and
    /// [`ConfigError::Parse`] for a file that exists but does not parse; a
    /// corrupt config is surfaced, never silently replaced.
    pub fn load(root: &Path) -> Result<Self, ConfigError> {
        let settings = read_toml_or_default(&root.join(Self::SETTINGS_FILE))?;
        Ok(Self {
            root: root.to_path_buf(),
            settings,
        })
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Mutable access to the settings; call [`save_settings`](Self::save_settings)
    /// to persist.
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// The user's themes folder.
    #[must_use]
    pub fn themes_dir(&self) -> PathBuf {
        self.root.join(Self::THEMES_DIR)
    }

    /// The shared sounds folder.
    #[must_use]
    pub fn sounds_dir(&self) -> PathBuf {
        self.root.join(Self::SOUNDS_DIR)
    }

    /// Writes any missing pieces of the on-disk layout with current values —
    /// `settings.toml` and an empty `themes` folder — so users
    /// can find and edit them without first changing a setting through the
    /// GUI.
    /// Existing files, including corrupt ones, are never touched.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] when writing fails.
    pub fn ensure_files_exist(&self) -> Result<(), ConfigError> {
        if !self.root.join(Self::SETTINGS_FILE).exists() {
            self.save_settings()?;
        }

        let themes_dir = self.themes_dir();
        fs::create_dir_all(&themes_dir).map_err(|error| ConfigError::Io(themes_dir, error))?;
        Ok(())
    }

    /// Persists `settings.toml` atomically.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] when writing fails.
    pub fn save_settings(&self) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(&self.settings).map_err(ConfigError::Serialize)?;
        write_atomic(&self.root.join(Self::SETTINGS_FILE), text.as_bytes())
            .map_err(|(path, error)| ConfigError::Io(path, error))
    }

    /// Builds a store from `settings` directly, without reading whatever
    /// file (if any) already sits at `root` — the counterpart to
    /// [`load`](Self::load) for callers that need a fixed, deterministic
    /// configuration rather than picking up and mutating existing state
    /// (see [`Settings::for_e2e`]). Call [`save_settings`](Self::save_settings)
    /// afterward to persist it.
    #[must_use]
    pub fn from_settings(root: &Path, settings: Settings) -> Self {
        Self {
            root: root.to_path_buf(),
            settings,
        }
    }
}

fn read_toml_or_default<T>(path: &Path) -> Result<T, ConfigError>
where
    T: Default + for<'de> Deserialize<'de>,
{
    match fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|error| ConfigError::Parse(path.to_path_buf(), error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(ConfigError::Io(path.to_path_buf(), error)),
    }
}

/// Writes `contents` to `path` through a temporary file renamed over it.
/// A failure gives the file that failed with the error.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), (PathBuf, io::Error)> {
    let mut temp = path.to_path_buf();
    temp.set_extension("toml.tmp");
    fs::write(&temp, contents).map_err(|error| (temp.clone(), error))?;
    // On Windows, std::fs::rename replaces an existing destination.
    fs::rename(&temp, path).map_err(|error| (path.to_path_buf(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of one test's own under the system temp folder, named for
    /// the test and this process, so no two tests or runs share one. It is
    /// created empty, failing if it already exists, and removed when the
    /// test ends; a failure to remove it fails the test. A test that failed
    /// leaves its folder for inspection.
    pub(crate) struct TempRoot(PathBuf);

    impl TempRoot {
        pub(crate) fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("verbatim-config-{name}-{}", std::process::id()));
            fs::create_dir(&root).expect("create the test's own temp folder");
            Self(root)
        }
    }

    impl std::ops::Deref for TempRoot {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                fs::remove_dir_all(&self.0).expect("remove the test's temp folder");
            }
        }
    }

    fn temp_root(name: &str) -> TempRoot {
        TempRoot::new(name)
    }

    #[test]
    fn missing_files_load_as_defaults() {
        let root = temp_root("defaults");
        let store = ConfigStore::load(&root).expect("loads");
        assert_eq!(store.settings(), &Settings::default());
    }

    /// Every section survives a save and a reload, and the file names a
    /// value as its documented word.
    #[test]
    fn save_and_reload_round_trips() {
        let root = temp_root("roundtrip");
        let mut store = ConfigStore::load(&root).expect("loads");
        let settings = store.settings_mut();
        settings.locale = Some("de".into());
        settings.verbatim_keys.numpad_insert = false;
        settings.keyboard.layout = KeyboardLayout::Laptop;
        settings.keyboard.speech_interrupt_for_enter = false;
        settings.speech.synthesizer = Some("onecore".into());
        settings.speech.synth_settings.insert(
            "onecore".into(),
            BTreeMap::from([
                ("rate".into(), ConfigValue::Integer(60)),
                ("voice".into(), ConfigValue::Text("Microsoft David".into())),
                ("rate-boost".into(), ConfigValue::Flag(true)),
            ]),
        );
        settings.reader.speak_typed_words = TypingEcho::EditControls;
        settings.reader.say_all_unit = SayAllUnit::Line;
        settings.reader.follow_caret = false;
        settings.theme.id = "calm".to_owned();
        settings.theme.options.sounds_during_say_all = false;
        store.save_settings().expect("saves settings");

        let text =
            fs::read_to_string(root.join(ConfigStore::SETTINGS_FILE)).expect("reads the file");
        let file: toml::Table = text.parse().expect("the file is TOML");
        assert_eq!(
            file["reader"]["speak_typed_words"],
            toml::Value::String("edit_controls".to_owned()),
            "{text}"
        );
        assert_eq!(
            ConfigStore::load(&root).expect("reloads").settings(),
            store.settings()
        );
    }

    #[test]
    fn ensure_files_exist_creates_missing_pieces_and_leaves_existing_ones() {
        let root = temp_root("ensure");
        let store = ConfigStore::load(&root).expect("loads");
        store.ensure_files_exist().expect("creates files");
        assert!(root.join(ConfigStore::SETTINGS_FILE).is_file());

        assert!(root.join(ConfigStore::THEMES_DIR).is_dir());

        // A hand-edited file survives the next startup's ensure call.
        fs::write(
            root.join(ConfigStore::SETTINGS_FILE),
            "[verbatim_keys]\ncaps_lock = false\ninsert = false\nnumpad_insert = true\n",
        )
        .expect("hand edit");
        let store = ConfigStore::load(&root).expect("reloads");
        store.ensure_files_exist().expect("no-op");
        assert!(!store.settings().verbatim_keys.caps_lock);
        assert!(!store.settings().verbatim_keys.insert);
        assert!(store.settings().verbatim_keys.numpad_insert);
    }

    #[test]
    fn corrupt_config_is_an_error_not_a_default() {
        let root = temp_root("corrupt");
        fs::write(root.join(ConfigStore::SETTINGS_FILE), "not = [valid").expect("write");
        assert!(matches!(
            ConfigStore::load(&root),
            Err(ConfigError::Parse(_, _))
        ));
    }

    #[test]
    fn uia_remote_operations_default_on_and_a_file_can_turn_them_off() {
        assert!(Settings::default().uia.remote_operations);
        let settings: Settings =
            toml::from_str("[uia]\nremote_operations = false\n").expect("parses");
        assert!(!settings.uia.remote_operations);
    }

    #[test]
    fn typing_and_reading_settings_default_to_nvdas() {
        let settings = Settings::default();
        assert!(settings.keyboard.speech_interrupt_for_characters);
        assert!(settings.keyboard.speech_interrupt_for_enter);
        // A file written before these settings existed reads them as the
        // defaults.
        let old: Settings =
            toml::from_str("[keyboard]\nlayout = \"laptop\"\n").expect("parses an older file");
        assert!(old.keyboard.speech_interrupt_for_enter);
        assert_eq!(old.reader.speak_typed_characters, TypingEcho::EditControls);
    }
}
