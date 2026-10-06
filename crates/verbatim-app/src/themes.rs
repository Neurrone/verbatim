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

use std::path::Path;

use crossbeam_channel::Sender;
use verbatim_config::ConfigStore;
use verbatim_config::themes::{self, LoadedTheme};
use verbatim_model::{Input, ThemeOptions};
use verbatim_speech::{ActiveTheme, SpeechManager};

use crate::ShellCommand;

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

/// Makes `loaded` the active theme with `options`: decodes its sounds,
/// found in its own directory or the shared `sounds_dir`; sets it on the
/// speech manager, so the next thing spoken uses it; and tells the reducer
/// what it wants fetched. Every problem found loading it is logged.
pub(crate) fn activate(
    manager: &SpeechManager,
    commands: &Sender<ShellCommand>,
    loaded: &LoadedTheme,
    sounds_dir: &Path,
    options: ThemeOptions,
) {
    for problem in &loaded.problems {
        tracing::warn!(theme = %loaded.theme.id, %problem, "theme problem");
    }
    let active = ActiveTheme::new(
        loaded.theme.clone(),
        |file| loaded.sound_path(file, sounds_dir),
        options,
    );
    let fetches = active.fetches();
    tracing::info!(theme = %loaded.theme.id, ?options, ?fetches, "theme in use");
    manager.themes().set(active);
    if commands
        .send(ShellCommand::Input(Box::new(Input::Fetches(fetches))))
        .is_err()
    {
        tracing::debug!("the reducer thread is gone; the theme's fetches are not sent");
    }
}
