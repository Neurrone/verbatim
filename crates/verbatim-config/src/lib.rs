//! Configuration (milestone M1 layout, amended during M1 review).
//!
//! Verbatim is portable: everything lives next to `verbatim.exe`.
//! `settings.toml` is the configuration — global settings (Verbatim modifier
//! keys, locale, logging) plus the base profile's sections, speech being the
//! first. A `profiles` folder holds named profiles: sparse overlays that
//! override only the keys they mention, mirroring NVDA's model where the
//! main configuration file is the base and profiles are diffs over it.
//! Because the base lives in `settings.toml`, every file in `profiles` is a
//! user profile — a profile named `Base` collides with nothing.
//!
//! Resolution is layered: an [`ActiveConfig`] view resolves each setting
//! through the active overlays (most specific first) down to the base in
//! `settings.toml`. M1 activates no overlays; manual and triggered profiles
//! later are additive. Global settings are structurally outside the profile
//! system — [`Profile`] has no fields for them, so no profile can override
//! the modifier keys or language.
//!
//! Writes are atomic — write a temporary file, then rename over the target.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use verbatim_model::{ReaderSettings, SayAllUnit, ThemeOptions, TypingEcho};

mod package;
pub mod themes;

/// Which keys act as the Verbatim modifier (NVDA's `NVDAModifierKeys`). The
/// default adds Caps Lock to NVDA's two Insert keys, a deliberate
/// difference (`docs/parity.md`, "NVDA-modifier semantics").
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

/// Which keyboard layout's gesture bindings are active — NVDA's desktop
/// (numpad-based) and laptop (no numpad assumed, shift/control chords on the
/// main key block instead) layouts. `verbatim-input`'s `bindings_for` reads
/// this to choose a binding table; that crate redeclares its own layout
/// enum to stay decoupled from configuration, the same pattern `VerbatimKeys`
/// and `DecisionConfig` already follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardLayout {
    /// The numpad-based layout, and the default.
    #[default]
    Desktop,
    /// The layout for keyboards without a numpad.
    Laptop,
}

/// Keyboard configuration within `settings.toml`. Global, like
/// [`VerbatimKeys`]: [`Profile`] has no field for it, so no profile can
/// override the active layout. Exposed only in the file for M3; a GUI
/// choice arrives with M8's gesture-remapping work.
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
/// provider or measuring. Global, like [`KeyboardConfig`]. Read at startup
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
/// review; applied uniformly by [`Settings::for_e2e`] so the VM (`OneCore`)
/// and runner-direct (capture synth) paths speak at the same rate.
pub const E2E_RATE: i64 = 80;

impl Settings {
    /// The fixed configuration shared by the E2E suite's own runner-direct
    /// staging (`verbatim-e2e`'s `Scenario::launch`) and `cargo xtask vm
    /// deploy`'s guest staging (`write_synth_settings`): [`Settings::default`]
    /// with the speech synthesizer overridden to `synth_id` and that
    /// synthesizer's `rate` set to [`E2E_RATE`].
    ///
    /// Both call sites need the same guarantee: a run's configuration is
    /// always these fixed values, never whatever a previous run or a
    /// developer's own `settings.toml` happened to leave behind, so neither
    /// one may load an existing file and mutate it — they build this and
    /// write it fresh. `xtask` cannot depend on `verbatim-e2e` (and should
    /// not, to avoid a dev-tooling dependency edge into a test-only crate),
    /// so this constructor lives here in `verbatim-config`, which both
    /// already depend on, rather than being duplicated in both places. If
    /// its shape ever needs to change, keep both call sites in lockstep.
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
/// section of `settings.toml` (`phase6-design.md`, "Themes and
/// configuration profiles"): the id of a theme in the `themes` folder, or
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

/// A profile's theme section: whatever it sets overrides the base, and
/// what it leaves unset comes from the base. A profile that names no theme
/// uses the base settings' theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileTheme {
    /// The theme's id.
    pub id: Option<String>,
    /// The volume of sounds relative to speech, from 0 to 100.
    pub sound_volume: Option<u8>,
    /// Whether sounds play during say-all.
    pub sounds_during_say_all: Option<bool>,
    /// Whether indications reported by sound alone are spoken as well.
    pub speak_sounded_indications: Option<bool>,
}

/// The contents of `settings.toml`: global settings plus the base profile's
/// sections.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Requested UI and speech language, a BCP 47 tag such as `de`; `None`
    /// follows the Windows preferred languages. Global: profiles cannot
    /// override it.
    pub locale: Option<String>,
    /// Tracing filter override, same syntax as `RUST_LOG`. Global.
    pub log_filter: Option<String>,
    /// The Verbatim modifier keys. Global.
    pub verbatim_keys: VerbatimKeys,
    /// The active keyboard layout. Global.
    pub keyboard: KeyboardConfig,
    /// Developer settings for the UIA client. Global.
    pub uia: UiaConfig,
    /// The base profile's speech configuration; named profiles override it
    /// per setting.
    pub speech: SpeechConfig,
    /// The reader settings the reducer reads, the `[reader]` section:
    /// typing echo, whether the review cursor follows the caret, say-all's
    /// reading unit, keeping the display on during say-all, and the
    /// Terminal settings (speaking passwords, reporting new output, and the
    /// flood policy's two limits), each with NVDA's default where NVDA has
    /// the setting ([`ReaderSettings`]). The base profile's only, until
    /// profiles grow them (M8).
    pub reader: ReaderSettings,
    /// The base profile's theme and the settings that go with it.
    pub theme: ThemeConfig,
}

/// One named profile — the contents of one file in the `profiles` folder.
///
/// Sparse by design: a profile overrides only what it mentions. This type
/// deliberately has no global fields, so a profile file cannot change the
/// modifier keys or language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Speech configuration carried by this profile.
    pub speech: SpeechConfig,
    /// The theme this profile uses, and the settings that go with it.
    pub theme: ProfileTheme,
}

/// A resolved, read-only view over the active overlays and the base, most
/// specific first. M1 activates no overlays.
#[derive(Clone, Copy, Debug)]
pub struct ActiveConfig<'a> {
    /// Active overlays, most specific first.
    overlays: &'a [Profile],
    /// The base profile's speech section, from `settings.toml`.
    base: &'a SpeechConfig,
    /// The base profile's theme section, from `settings.toml`.
    base_theme: &'a ThemeConfig,
}

impl ActiveConfig<'_> {
    /// Speech sections in resolution order: overlays first, base last.
    fn speech_layers(&self) -> impl Iterator<Item = &SpeechConfig> {
        self.overlays
            .iter()
            .map(|profile| &profile.speech)
            .chain(std::iter::once(self.base))
    }

    /// The active synthesizer id, from the most specific layer that sets one.
    #[must_use]
    pub fn synthesizer(&self) -> Option<&str> {
        self.speech_layers()
            .find_map(|speech| speech.synthesizer.as_deref())
    }

    /// One persisted setting value for a synthesizer, from the most
    /// specific layer that sets it.
    #[must_use]
    pub fn synth_setting(&self, synth_id: &str, setting_id: &str) -> Option<&ConfigValue> {
        self.speech_layers()
            .find_map(|speech| speech.synth_settings.get(synth_id)?.get(setting_id))
    }

    /// The id of the theme in use: the most specific layer's that names
    /// one, and otherwise the base settings' (`default` unless changed).
    #[must_use]
    pub fn theme_id(&self) -> &str {
        self.overlays
            .iter()
            .find_map(|profile| profile.theme.id.as_deref())
            .unwrap_or(&self.base_theme.id)
    }

    /// The settings that go with the theme, each from the most specific
    /// layer that sets it.
    #[must_use]
    pub fn theme_options(&self) -> ThemeOptions {
        let base = self.base_theme.options;
        let themes = || self.overlays.iter().map(|profile| &profile.theme);
        ThemeOptions {
            sound_volume: themes()
                .find_map(|theme| theme.sound_volume)
                .unwrap_or(base.sound_volume)
                .min(100),
            sounds_during_say_all: themes()
                .find_map(|theme| theme.sounds_during_say_all)
                .unwrap_or(base.sounds_during_say_all),
            speak_sounded_indications: themes()
                .find_map(|theme| theme.speak_sounded_indications)
                .unwrap_or(base.speak_sounded_indications),
        }
    }

    /// All persisted setting values for a synthesizer, resolved across
    /// layers (most specific layer wins per setting).
    #[must_use]
    pub fn synth_settings(&self, synth_id: &str) -> BTreeMap<String, ConfigValue> {
        let mut resolved = BTreeMap::new();
        let layers: Vec<&SpeechConfig> = self.speech_layers().collect();
        for speech in layers.into_iter().rev() {
            if let Some(settings) = speech.synth_settings.get(synth_id) {
                for (id, value) in settings {
                    resolved.insert(id.clone(), value.clone());
                }
            }
        }
        resolved
    }
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

/// Loads, holds, and saves `settings.toml` and the active profile overlays.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    root: PathBuf,
    settings: Settings,
    /// Active overlays, most specific first. M1 activates none; named
    /// profiles from the `profiles` folder join here later.
    overlays: Vec<Profile>,
}

impl ConfigStore {
    /// File name of the settings file next to the executable.
    pub const SETTINGS_FILE: &'static str = "settings.toml";
    /// Folder name of the named-profiles folder next to the executable.
    pub const PROFILES_DIR: &'static str = "profiles";
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
            overlays: Vec::new(),
        })
    }

    /// The settings: global fields plus the base profile's sections.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Mutable access to the settings; call [`save_settings`](Self::save_settings)
    /// to persist.
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// The resolved view over the active overlays and the base.
    #[must_use]
    pub fn active(&self) -> ActiveConfig<'_> {
        ActiveConfig {
            overlays: &self.overlays,
            base: &self.settings.speech,
            base_theme: &self.settings.theme,
        }
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
    /// `settings.toml` and empty `profiles` and `themes` folders — so users
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
        let profiles_dir = self.root.join(Self::PROFILES_DIR);
        fs::create_dir_all(&profiles_dir).map_err(|error| ConfigError::Io(profiles_dir, error))?;
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
        write_toml_atomic(&self.root.join(Self::SETTINGS_FILE), &self.settings)
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
            overlays: Vec::new(),
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

fn write_toml_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(value).map_err(ConfigError::Serialize)?;
    let mut temp = path.to_path_buf();
    temp.set_extension("toml.tmp");
    fs::write(&temp, text).map_err(|error| ConfigError::Io(temp.clone(), error))?;
    // On Windows, std::fs::rename replaces an existing destination.
    fs::rename(&temp, path).map_err(|error| ConfigError::Io(path.to_path_buf(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join("verbatim-config-tests")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    #[test]
    fn missing_files_load_as_defaults() {
        let root = temp_root("defaults");
        let store = ConfigStore::load(&root).expect("loads");
        assert_eq!(store.settings(), &Settings::default());
        assert!(store.active().synthesizer().is_none());
    }

    #[test]
    fn save_and_reload_round_trips() {
        let root = temp_root("roundtrip");
        let mut store = ConfigStore::load(&root).expect("loads");
        store.settings_mut().locale = Some("de".into());
        store.settings_mut().verbatim_keys.numpad_insert = false;
        store.settings_mut().speech.synthesizer = Some("onecore".into());
        store.settings_mut().speech.synth_settings.insert(
            "onecore".into(),
            BTreeMap::from([
                ("rate".into(), ConfigValue::Integer(60)),
                ("voice".into(), ConfigValue::Text("Microsoft David".into())),
                ("rate-boost".into(), ConfigValue::Flag(true)),
            ]),
        );
        store.save_settings().expect("saves settings");

        let reloaded = ConfigStore::load(&root).expect("reloads");
        assert_eq!(reloaded.settings().locale.as_deref(), Some("de"));
        assert!(!reloaded.settings().verbatim_keys.numpad_insert);
        let active = reloaded.active();
        assert_eq!(active.synthesizer(), Some("onecore"));
        assert_eq!(
            active.synth_setting("onecore", "rate"),
            Some(&ConfigValue::Integer(60))
        );
        assert_eq!(
            active.synth_setting("onecore", "rate-boost"),
            Some(&ConfigValue::Flag(true))
        );
    }

    #[test]
    fn ensure_files_exist_creates_missing_pieces_and_leaves_existing_ones() {
        let root = temp_root("ensure");
        let store = ConfigStore::load(&root).expect("loads");
        store.ensure_files_exist().expect("creates files");
        assert!(root.join(ConfigStore::SETTINGS_FILE).is_file());
        assert!(root.join(ConfigStore::PROFILES_DIR).is_dir());
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
    fn a_profile_file_cannot_carry_global_settings() {
        // Unknown sections in a profile are ignored by the schema, so a
        // profile cannot smuggle in modifier keys or a locale.
        let profile: Profile = toml::from_str(
            "locale = \"de\"\n[verbatim_keys]\ncaps_lock = false\n[speech]\nsynthesizer = \"onecore\"\n",
        )
        .expect("parses, ignoring global keys");
        assert_eq!(profile.speech.synthesizer.as_deref(), Some("onecore"));
    }

    #[test]
    fn keyboard_layout_defaults_to_desktop() {
        assert_eq!(Settings::default().keyboard.layout, KeyboardLayout::Desktop);
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
        assert_eq!(settings.reader, ReaderSettings::default());
        // A file written before these settings existed reads them as the
        // defaults.
        let old: Settings =
            toml::from_str("[keyboard]\nlayout = \"laptop\"\n").expect("parses an older file");
        assert!(old.keyboard.speech_interrupt_for_enter);
        assert_eq!(old.reader.speak_typed_characters, TypingEcho::Always);
    }

    #[test]
    fn reader_settings_round_trip_through_toml() {
        let root = temp_root("reader-roundtrip");
        let mut store = ConfigStore::load(&root).expect("loads");
        store.settings_mut().reader.speak_typed_words = TypingEcho::EditControls;
        store.settings_mut().reader.say_all_unit = SayAllUnit::Line;
        store.settings_mut().reader.follow_caret = false;
        store.settings_mut().keyboard.speech_interrupt_for_enter = false;
        store.save_settings().expect("saves settings");

        let text =
            fs::read_to_string(root.join(ConfigStore::SETTINGS_FILE)).expect("reads the file");
        assert!(
            text.contains("speak_typed_words = \"edit_controls\""),
            "{text}"
        );
        let reloaded = ConfigStore::load(&root).expect("reloads");
        assert_eq!(
            reloaded.settings().reader.speak_typed_words,
            TypingEcho::EditControls
        );
        assert_eq!(reloaded.settings().reader.say_all_unit, SayAllUnit::Line);
        assert!(!reloaded.settings().reader.follow_caret);
        assert!(!reloaded.settings().keyboard.speech_interrupt_for_enter);
    }

    #[test]
    fn terminal_settings_read_from_the_reader_section() {
        let settings: Settings = toml::from_str(
            "[reader]\nreport_terminal_output = false\nterminal_full_lines = 50\nterminal_last_lines = 10\n",
        )
        .expect("parses");
        assert!(!settings.reader.report_terminal_output);
        assert_eq!(settings.reader.full_lines(), 50);
        assert_eq!(settings.reader.last_lines(), 10);
        // Left out, each takes its default.
        let defaults: Settings = toml::from_str("[reader]\n").expect("parses");
        assert!(defaults.reader.report_terminal_output);
        assert_eq!(defaults.reader.full_lines(), 30);
        assert_eq!(defaults.reader.last_lines(), 30);
    }

    #[test]
    fn keyboard_layout_round_trips_through_toml() {
        let root = temp_root("keyboard-roundtrip");
        let mut store = ConfigStore::load(&root).expect("loads");
        store.settings_mut().keyboard.layout = KeyboardLayout::Laptop;
        store.save_settings().expect("saves settings");

        let reloaded = ConfigStore::load(&root).expect("reloads");
        assert_eq!(reloaded.settings().keyboard.layout, KeyboardLayout::Laptop);
    }

    #[test]
    fn a_profile_file_cannot_carry_a_keyboard_section() {
        // Unknown sections in a profile are ignored by the schema, so a
        // profile cannot smuggle in a keyboard layout override; Profile has
        // no field for it at all.
        let profile: Profile = toml::from_str(
            "[keyboard]\nlayout = \"laptop\"\n[speech]\nsynthesizer = \"onecore\"\n",
        )
        .expect("parses, ignoring the keyboard section");
        assert_eq!(profile.speech.synthesizer.as_deref(), Some("onecore"));
    }

    #[test]
    fn layered_resolution_prefers_overlays_over_the_base() {
        let mut settings = Settings::default();
        settings.speech.synthesizer = Some("onecore".into());
        settings.speech.synth_settings.insert(
            "onecore".into(),
            BTreeMap::from([
                ("rate".into(), ConfigValue::Integer(50)),
                ("pitch".into(), ConfigValue::Integer(50)),
            ]),
        );
        let mut overlay = Profile::default();
        overlay.speech.synth_settings.insert(
            "onecore".into(),
            BTreeMap::from([("rate".into(), ConfigValue::Integer(90))]),
        );

        let overlays = vec![overlay];
        let active = ActiveConfig {
            overlays: &overlays,
            base: &settings.speech,
            base_theme: &settings.theme,
        };
        assert_eq!(active.synthesizer(), Some("onecore"));
        assert_eq!(
            active.synth_setting("onecore", "rate"),
            Some(&ConfigValue::Integer(90)),
            "overlay wins for settings it carries"
        );
        let resolved = active.synth_settings("onecore");
        assert_eq!(resolved.get("pitch"), Some(&ConfigValue::Integer(50)));
        assert_eq!(resolved.get("rate"), Some(&ConfigValue::Integer(90)));
    }

    #[test]
    fn the_theme_resolves_from_the_profile_then_the_base_then_the_default() {
        let mut settings = Settings::default();
        let none: Vec<Profile> = Vec::new();
        let active = ActiveConfig {
            overlays: &none,
            base: &settings.speech,
            base_theme: &settings.theme,
        };
        assert_eq!(active.theme_id(), "default", "nothing chosen");
        assert_eq!(active.theme_options(), ThemeOptions::default());

        settings.theme.id = "calm".to_owned();
        settings.theme.options.sound_volume = 70;
        let silent = Profile::default();
        let proofreading: Profile =
            toml::from_str("[theme]\nid = \"proofreading\"\nspeak_sounded_indications = true\n")
                .expect("parses");
        let overlays = vec![silent.clone()];
        let active = ActiveConfig {
            overlays: &overlays,
            base: &settings.speech,
            base_theme: &settings.theme,
        };
        assert_eq!(
            active.theme_id(),
            "calm",
            "a profile naming none uses the base's"
        );
        assert_eq!(active.theme_options().sound_volume, 70);

        let overlays = vec![proofreading, silent];
        let active = ActiveConfig {
            overlays: &overlays,
            base: &settings.speech,
            base_theme: &settings.theme,
        };
        assert_eq!(active.theme_id(), "proofreading");
        let options = active.theme_options();
        assert!(options.speak_sounded_indications);
        assert_eq!(
            options.sound_volume, 70,
            "the base's, which the profile leaves"
        );
    }

    #[test]
    fn the_theme_section_round_trips_through_toml() {
        let mut settings = Settings::default();
        settings.theme.id = "calm".to_owned();
        settings.theme.options.sounds_during_say_all = false;
        let text = toml::to_string_pretty(&settings).expect("serializes");
        assert!(text.contains("[theme]\nid = \"calm\""), "{text}");
        let back: Settings = toml::from_str(&text).expect("parses");
        assert_eq!(back.theme, settings.theme);
        let old: Settings = toml::from_str("locale = \"de\"\n").expect("an older file parses");
        assert_eq!(old.theme, ThemeConfig::default());
    }
}
