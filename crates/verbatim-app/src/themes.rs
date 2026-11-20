//! The theme in use (`books/dev/src/reference/crates/verbatim-model.md`, "Themes"): loading the
//! one the configuration names and making
//! a theme active, which the speech pipeline presents with and Core chooses
//! its reads' details from.
//!
//! The configuration names the theme by id in its `[theme]` section.
//! Making one active decodes its
//! sounds (a missing or unreadable one is logged, and its indications are
//! spoken instead), hands it to the speech manager's theme handle, and
//! tells the reducer the details it reports (`Input::Reported`), from which
//! Core chooses what each of its requests fetches.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::Commands;
use verbatim_audio::{Lane, Sound};
use verbatim_config::themes::{self, LoadedTheme};
use verbatim_config::{ConfigStore, ThemeConfig};
use verbatim_gui::ThemeHost;
use verbatim_model::{Earcon, Input, ThemeOptions, Utterance};
use verbatim_speech::{ActiveTheme, SpeechManager};

use crate::ShellCommand;
use crate::saver::ThemeWriter;

/// The settings dialog's Theme page's view of Verbatim: where themes are,
/// the theme section of the settings, which the GUI thread owns
/// (`books/dev/src/explanation/design/ownership.md`), the GUI's lane to the mixer for the sounds the page plays,
/// and the speech manager and reducer for making a theme active.
pub(crate) struct AppThemeHost {
    pub(crate) themes_dir: PathBuf,
    pub(crate) sounds_dir: PathBuf,
    /// The theme section as last saved, and its writer.
    pub(crate) section: Mutex<(ThemeConfig, ThemeWriter)>,
    /// The GUI thread's lane to the events' source.
    pub(crate) sounds: Mutex<Lane>,
    pub(crate) manager: Arc<SpeechManager>,
    pub(crate) commands: Commands,
}

impl ThemeHost for AppThemeHost {
    fn themes_dir(&self) -> PathBuf {
        self.themes_dir.clone()
    }

    fn sounds_dir(&self) -> PathBuf {
        self.sounds_dir.clone()
    }

    /// The theme section as last saved.
    fn configured(&self) -> (String, ThemeOptions) {
        let section = self.section.lock().unwrap_or_else(PoisonError::into_inner);
        (section.0.id.clone(), section.0.options)
    }

    fn activate(&self, theme: &LoadedTheme, options: ThemeOptions) {
        let sounds_dir = self.sounds_dir();
        activate(&self.manager, &self.commands, theme, &sounds_dir, options);
    }

    fn set_options(&self, options: ThemeOptions) {
        self.manager.themes().set_options(options);
    }

    /// Saves the choice in the settings through the saver: a failed write
    /// is logged there.
    fn persist(&self, id: &str, options: ThemeOptions) {
        let mut section = self.section.lock().unwrap_or_else(PoisonError::into_inner);
        section.0 = ThemeConfig {
            id: id.to_owned(),
            options,
        };
        section.1.save(section.0.clone());
    }

    fn play(&self, sound: &Sound, gain: f32) {
        let played = self
            .sounds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .play(sound, gain);
        if let Err(error) = played {
            tracing::warn!(%error, "a sound from the theme page cannot be played");
        }
    }

    /// A preview spoken, through the reducer, which speaks it
    /// interrupting.
    fn speak(&self, utterance: Utterance) {
        let input = Input::PreviewSpeech {
            trace_id: utterance.trace_id,
            segments: utterance.segments,
        };
        let _ = self.commands.send(ShellCommand::Input(Box::new(input)));
    }

    /// A preview of an event, through the reducer.
    fn play_earcon(&self, earcon: Earcon) {
        let input = Input::PreviewEarcon(earcon);
        let _ = self.commands.send(ShellCommand::Input(Box::new(input)));
    }
}

/// The theme the configuration names, with the settings that go with it.
/// A theme that cannot be loaded is logged, and the built-in default theme
/// is used in its place, so speech never depends on a theme file.
pub(crate) fn configured(store: &ConfigStore) -> (LoadedTheme, ThemeOptions) {
    let id = store.settings().theme.id.clone();
    let options = store.settings().theme.options;
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
    tracing::info!(theme = %loaded.theme.id, ?options, reported = ?active.reported(), "theme in use");
    active
}

/// Tells the reducer the details `theme` reports (`Input::Reported`).
pub(crate) fn send_reported(commands: &Commands, theme: &ActiveTheme) {
    if commands
        .send(ShellCommand::Input(Box::new(Input::Reported(
            theme.reported(),
        ))))
        .is_err()
    {
        tracing::debug!("the reducer thread is gone; the theme's details are not sent");
    }
}

/// Makes `loaded` the active theme with `options` ([`prepare`]): sets it
/// on the speech manager, so the next thing spoken uses it, and tells the
/// reducer the details it reports.
pub(crate) fn activate(
    manager: &SpeechManager,
    commands: &Commands,
    loaded: &LoadedTheme,
    sounds_dir: &Path,
    options: ThemeOptions,
) {
    let active = prepare(loaded, sounds_dir, options);
    send_reported(commands, &active);
    manager.themes().set(active);
}
