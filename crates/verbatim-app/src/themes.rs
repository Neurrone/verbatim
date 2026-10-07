//! The theme in use (`phase6-design.md`, "Themes: one model for verbosity,
//! speech, and sounds"): loading the one the configuration names and making
//! a theme active, which the speech pipeline presents with and the reducer
//! fetches for.
//!
//! The configuration names the theme by id: the base settings' `[theme]`,
//! or a profile's once M8 activates profiles. Making one active decodes its
//! sounds (a missing or unreadable one is logged, and its indications are
//! spoken instead), hands it to the speech manager's theme handle, and
//! tells the reducer what it wants fetched (`Input::Fetches`), which the
//! reducer thread passes on to every outpost.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crossbeam_channel::Sender;
use verbatim_audio::Sound;
use verbatim_config::themes::{self, LoadedTheme};
use verbatim_config::{ConfigStore, ThemeConfig};
use verbatim_gui::ThemeHost;
use verbatim_model::{Earcon, Input, ThemeOptions, Utterance};
use verbatim_speech::{ActiveTheme, SpeechManager};

use crate::ShellCommand;

/// The settings dialog's Theme page's view of Verbatim: the configuration
/// for where themes are and which one is chosen, and the speech manager and
/// reducer for making one active.
pub(crate) struct AppThemeHost {
    pub(crate) store: Arc<Mutex<ConfigStore>>,
    pub(crate) manager: Arc<SpeechManager>,
    pub(crate) commands: Sender<ShellCommand>,
}

impl AppThemeHost {
    fn store(&self) -> std::sync::MutexGuard<'_, ConfigStore> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ThemeHost for AppThemeHost {
    fn themes_dir(&self) -> PathBuf {
        self.store().themes_dir()
    }

    fn sounds_dir(&self) -> PathBuf {
        self.store().sounds_dir()
    }

    fn configured(&self) -> (String, ThemeOptions) {
        let store = self.store();
        let active = store.active();
        (active.theme_id().to_owned(), active.theme_options())
    }

    fn activate(&self, theme: &LoadedTheme, options: ThemeOptions) {
        let sounds_dir = self.sounds_dir();
        activate(&self.manager, &self.commands, theme, &sounds_dir, options);
    }

    fn set_options(&self, options: ThemeOptions) {
        self.manager.themes().set_options(options);
    }

    /// Saves the choice in the base settings, the profile the dialog edits
    /// until M8 activates profiles.
    fn persist(&self, id: &str, options: ThemeOptions) -> Result<(), String> {
        let mut store = self.store();
        store.settings_mut().theme = ThemeConfig {
            id: id.to_owned(),
            options,
        };
        store.save_settings().map_err(|error| error.to_string())
    }

    fn play(&self, sound: &Sound, gain: f32) {
        if let Err(error) = self.manager.play_sound(sound, gain) {
            tracing::warn!(%error, "a sound from the theme page cannot be played");
        }
    }

    fn speak(&self, utterance: Utterance) {
        self.manager.speak(utterance);
    }

    fn play_earcon(&self, earcon: Earcon) {
        self.manager.play_earcon(earcon);
    }
}

/// The theme the configuration names, with the settings that go with it.
/// A theme that cannot be loaded is logged, and the built-in default theme
/// is used in its place, so speech never depends on a theme file.
pub(crate) fn configured(store: &ConfigStore) -> (LoadedTheme, ThemeOptions) {
    let active = store.active();
    let id = active.theme_id().to_owned();
    let options = active.theme_options();
    let loaded = themes::find_theme(&store.themes_dir(), &store.sounds_dir(), &id)
        .unwrap_or_else(|error| {
            tracing::warn!(%error, theme = id, "the configured theme cannot be loaded; using the default theme");
            LoadedTheme::builtin()
        });
    (loaded, options)
}

/// Makes `loaded` ready to present with `options`: decodes its sounds,
/// found in its own directory or the shared `sounds_dir`. Every problem
/// found loading it is logged.
pub(crate) fn prepare(
    loaded: &LoadedTheme,
    sounds_dir: &Path,
    options: ThemeOptions,
) -> ActiveTheme {
    for problem in &loaded.problems {
        tracing::warn!(theme = %loaded.theme.id, %problem, "theme problem");
    }
    let active = ActiveTheme::new(
        loaded.theme.clone(),
        |file| loaded.sound_path(file, sounds_dir),
        options,
    );
    tracing::info!(theme = %loaded.theme.id, ?options, fetches = ?active.fetches(), "theme in use");
    active
}

/// Tells the reducer what `theme` wants fetched (`Input::Fetches`).
pub(crate) fn send_fetches(commands: &Sender<ShellCommand>, theme: &ActiveTheme) {
    if commands
        .send(ShellCommand::Input(Box::new(Input::Fetches(
            theme.fetches(),
        ))))
        .is_err()
    {
        tracing::debug!("the reducer thread is gone; the theme's fetches are not sent");
    }
}

/// Makes `loaded` the active theme with `options` ([`prepare`]): sets it
/// on the speech manager, so the next thing spoken uses it, and tells the
/// reducer what it wants fetched.
pub(crate) fn activate(
    manager: &SpeechManager,
    commands: &Sender<ShellCommand>,
    loaded: &LoadedTheme,
    sounds_dir: &Path,
    options: ThemeOptions,
) {
    let active = prepare(loaded, sounds_dir, options);
    send_fetches(commands, &active);
    manager.themes().set(active);
}
