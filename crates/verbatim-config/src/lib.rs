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

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which keys act as the Verbatim modifier (NVDA's `NVDAModifierKeys`).
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
    /// The base profile's speech configuration; named profiles override it
    /// per setting.
    pub speech: SpeechConfig,
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
}

/// A resolved, read-only view over the active overlays and the base, most
/// specific first. M1 activates no overlays.
#[derive(Clone, Copy, Debug)]
pub struct ActiveConfig<'a> {
    /// Active overlays, most specific first.
    overlays: &'a [Profile],
    /// The base profile's speech section, from `settings.toml`.
    base: &'a SpeechConfig,
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
        }
    }

    /// Writes any missing pieces of the on-disk layout with current values —
    /// `settings.toml` and an empty `profiles` folder — so users can find
    /// and edit them without first changing a setting through the GUI.
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
}
