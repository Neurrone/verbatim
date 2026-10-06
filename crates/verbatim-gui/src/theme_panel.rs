//! The settings dialog's Theme page (`phase6-design.md`, "The settings
//! dialog"): its model, pure apart from the theme files and the
//! [`ThemeHost`] it makes changes live through.
//!
//! The page lists the installed themes, the built-in default first, and
//! shows the selected one's description and problems, the settings that go
//! with it (sound volume, sounds during say-all, and speaking indications
//! that play a sound), and its indications as a tree by category, each
//! named with its setting. Below the tree, the selected indication's "report
//! as", sound, words, and voice can be changed, previewed, and reset.
//!
//! Changes apply at once: choosing a theme, changing an indication, or a
//! setting makes it the one speech uses. Apply and OK keep them, saving the
//! changed themes and the choice; Cancel restores the theme and settings
//! the dialog opened with, or last applied. File operations (a new theme,
//! renaming, importing, exporting, removing, and adding a sound) act on the
//! themes folder at once. The built-in default theme cannot be changed: a
//! change to one of its indications first asks for a name, makes a new
//! theme based on it, and is made there.
//!
//! The C++ layer builds the widgets from the [`ffi::ThemePage`],
//! [`ffi::ThemeTreeCategory`], and [`ffi::IndicationControls`] this module
//! builds, with every string resolved, and reports each change back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use verbatim_audio::Sound;
use verbatim_config::themes::{self, LoadedTheme};
use verbatim_i18n::messages::{self, ThemeLabels};
use verbatim_model::{
    Earcon, Indication, IndicationCategory, IndicationSetting, Message, Phrase, Presentation,
    SegmentContent, SoundSource, SpeechPriority, TextFormat, Theme, ThemeOptions, Tone, TraceId,
    Utterance, UtteranceSegment,
};

use crate::bridge::ffi;
use crate::plan::accessible_name;

/// What the Theme page needs from the rest of Verbatim: where themes live,
/// the theme the configuration names, and the live effects of choosing
/// one. The app implements it over its configuration and speech manager.
pub trait ThemeHost: Send + Sync {
    /// The user's themes folder.
    fn themes_dir(&self) -> PathBuf;
    /// The shared sounds folder beside the program.
    fn sounds_dir(&self) -> PathBuf;
    /// The id of the theme the configuration names, and the settings that
    /// go with it.
    fn configured(&self) -> (String, ThemeOptions);
    /// Makes `theme` the one speech presents with, with `options`, at once,
    /// and tells the reducer what it wants fetched.
    fn activate(&self, theme: &LoadedTheme, options: ThemeOptions);
    /// Changes only the settings that go with the active theme.
    fn set_options(&self, options: ThemeOptions);
    /// Saves `id` and `options` as the configuration's theme.
    ///
    /// # Errors
    ///
    /// Why the configuration could not be saved.
    fn persist(&self, id: &str, options: ThemeOptions) -> Result<(), String>;
    /// Plays `sound` at once at `gain`, 1.0 for as recorded.
    fn play(&self, sound: &Sound, gain: f32);
    /// Speaks `utterance` through the active theme.
    fn speak(&self, utterance: Utterance);
    /// Reports `earcon` as the active theme says.
    fn play_earcon(&self, earcon: Earcon);
}

/// A sound the sound choice offers.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SoundChoice {
    /// No sound.
    None,
    /// A generated tone.
    Tone(Tone),
    /// A sound file, by its name.
    File(String),
    /// "Browse...": ask for a file.
    Browse,
}

/// A change waiting for the name of a new theme to make it in, because the
/// theme it was made to is built in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// Set the indication's setting to this.
    Set(Indication, IndicationSetting),
    /// Add the sound file at this path and use it for the indication.
    AddSound(Indication, PathBuf),
}

/// The Theme page's state.
pub(crate) struct ThemePanel {
    host: Arc<dyn ThemeHost>,
    labels: ThemeLabels,
    /// The installed themes, the built-in default first, as on disk.
    themes: Vec<LoadedTheme>,
    /// The selected theme's index in `themes`.
    selected: usize,
    /// Themes changed since the last apply, by id, not yet saved.
    edited: BTreeMap<String, Theme>,
    /// The settings that go with the theme, as they are now.
    options: ThemeOptions,
    /// The theme and settings to restore on Cancel: those the dialog opened
    /// with, or last applied.
    committed: (String, ThemeOptions),
    /// The catalogue, whose indexes identify indications to C++.
    catalogue: Vec<Indication>,
    /// The selected indication.
    indication: Option<Indication>,
    /// The find field's text.
    filter: String,
    /// A change waiting for a new theme's name.
    pending: Option<Pending>,
}

impl ThemePanel {
    /// The page as the configuration has it, with nothing changed.
    pub(crate) fn open(host: Arc<dyn ThemeHost>) -> Self {
        let (id, options) = host.configured();
        let mut panel = Self {
            host,
            labels: messages::theme_labels(),
            themes: Vec::new(),
            selected: 0,
            edited: BTreeMap::new(),
            options,
            committed: (id.clone(), options),
            catalogue: Indication::catalogue(),
            indication: None,
            filter: String::new(),
            pending: None,
        };
        panel.reload(&id);
        panel
    }

    /// Lists the installed themes again and selects `id`, or the built-in
    /// default theme when it is not installed.
    fn reload(&mut self, id: &str) {
        let (themes, errors) =
            themes::list_themes(&self.host.themes_dir(), &self.host.sounds_dir());
        for error in errors {
            tracing::warn!(%error, "a theme in the themes folder cannot be loaded");
        }
        self.themes = themes;
        self.selected = self
            .themes
            .iter()
            .position(|loaded| loaded.theme.id == id)
            .unwrap_or(0);
    }

    /// The selected theme as it is now, with its unsaved changes.
    fn current(&self) -> LoadedTheme {
        let mut loaded = self.themes[self.selected].clone();
        if let Some(edited) = self.edited.get(&loaded.theme.id) {
            loaded.theme = edited.clone();
            loaded.problems =
                themes::validate_theme(&loaded.theme, loaded.dir.as_deref(), &self.sounds_dir());
        }
        loaded
    }

    fn sounds_dir(&self) -> PathBuf {
        self.host.sounds_dir()
    }

    /// Makes the selected theme, as it is now, the one speech uses.
    fn activate(&self) {
        self.host.activate(&self.current(), self.options);
    }

    /// A theme's name as the list shows it.
    fn display_name(theme: &Theme) -> String {
        if theme.is_builtin() {
            verbatim_i18n::theme_default_name()
        } else {
            theme.name.clone()
        }
    }

    /// The page as it is now.
    pub(crate) fn page(&self) -> ffi::ThemePage {
        let current = self.current();
        let labels = &self.labels;
        let name = Self::display_name(&current.theme);
        let builtin = current.theme.is_builtin();
        ffi::ThemePage {
            theme_label: labels.theme.clone(),
            themes: self
                .themes
                .iter()
                .map(|loaded| Self::display_name(&loaded.theme))
                .collect(),
            selected: i32::try_from(self.selected).unwrap_or(0),
            description_label: labels.description.clone(),
            description: self.description(&current),
            volume_label: labels.sound_volume.clone(),
            volume: i32::from(self.options.sound_volume.min(100)),
            say_all_name: accessible_name(&labels.sounds_during_say_all),
            say_all_label: labels.sounds_during_say_all.clone(),
            say_all: self.options.sounds_during_say_all,
            speak_sounded_name: accessible_name(&labels.speak_sounded),
            speak_sounded_label: labels.speak_sounded.clone(),
            speak_sounded: self.options.speak_sounded_indications,
            find_label: labels.find.clone(),
            indications_label: labels.indications.clone(),
            report_label: labels.report_as.clone(),
            sound_label: labels.sound.clone(),
            words_label: labels.words.clone(),
            voice_label: labels.voice.clone(),
            preview: labels.preview.clone(),
            reset: labels.reset.clone(),
            new_theme: labels.new_theme.clone(),
            rename: labels.rename.clone(),
            import_label: labels.import.clone(),
            export_label: labels.export.clone(),
            remove: labels.remove.clone(),
            can_rename: !builtin,
            can_remove: !builtin && current.theme.id != self.committed.0,
            prompts: ffi::ThemePrompts {
                new_title: labels.new_title.clone(),
                new_prompt: messages::theme_new_prompt(&name),
                new_name: messages::theme_copy_name(&name),
                rename_title: labels.rename_title.clone(),
                rename_prompt: messages::theme_rename_prompt(&name),
                import_title: labels.import_title.clone(),
                export_title: messages::theme_export_title(&name),
                export_file: format!("{}.zip", current.theme.id),
                package_filter: labels.package_filter.clone(),
                sound_title: labels.sound_file_title.clone(),
                sound_filter: labels.sound_file_filter.clone(),
                remove_title: labels.remove_title.clone(),
                remove_question: messages::theme_remove_question(&name),
                error_title: labels.error_title.clone(),
                name,
            },
        }
    }

    /// The description field: the theme's description, its author, and
    /// the problems found with it.
    fn description(&self, current: &LoadedTheme) -> String {
        let mut lines = Vec::new();
        if current.theme.is_builtin() {
            lines.push(verbatim_i18n::theme_default_description());
        } else if !current.theme.description.is_empty() {
            lines.push(current.theme.description.clone());
        }
        if !current.theme.author.is_empty() {
            lines.push(messages::theme_author(&current.theme.author));
        }
        if current.problems.is_empty() {
            lines.push(self.labels.no_problems.clone());
        } else {
            lines.push(self.labels.problems.clone());
            lines.extend(
                current
                    .problems
                    .iter()
                    .map(verbatim_i18n::theme_problem_text),
            );
        }
        lines.join("\n")
    }

    /// The indications tree, filtered by the find field.
    pub(crate) fn tree(&self) -> Vec<ffi::ThemeTreeCategory> {
        let theme = self.current().theme;
        let filter = self.filter.to_lowercase();
        IndicationCategory::ALL
            .iter()
            .filter_map(|&category| {
                let items: Vec<ffi::ThemeTreeItem> = self
                    .catalogue
                    .iter()
                    .enumerate()
                    .filter(|(_, indication)| indication.category() == category)
                    .map(|(index, &indication)| ffi::ThemeTreeItem {
                        label: Self::summary(&theme, indication),
                        indication: index,
                    })
                    .filter(|item| item.label.to_lowercase().contains(&filter))
                    .collect();
                (!items.is_empty()).then(|| ffi::ThemeTreeCategory {
                    label: verbatim_i18n::indication_category_name(category),
                    items,
                })
            })
            .collect()
    }

    /// An indication as the tree names it: "link: speech and sound", with
    /// its sound when it plays one, and ", changed" when it differs from
    /// the default theme.
    fn summary(theme: &Theme, indication: Indication) -> String {
        let setting = theme.setting(indication);
        let presentation = verbatim_i18n::presentation_name(setting.report);
        let described = match (&setting.sound, setting.report.sounds()) {
            (Some(sound), true) => {
                messages::theme_setting_with_sound(&presentation, &Self::sound_name(sound))
            }
            _ => presentation,
        };
        messages::theme_indication_summary(
            &verbatim_i18n::indication_name(indication),
            &described,
            theme.differs(indication),
        )
    }

    /// A sound as the page names it: its file name, or the tone.
    fn sound_name(sound: &SoundSource) -> String {
        match sound {
            SoundSource::File(file) => file.clone(),
            SoundSource::Tone(tone) => {
                messages::theme_sound_tone(tone.frequency_hz, tone.duration_ms)
            }
        }
    }

    /// The sounds the sound choice offers for a setting: none, its tone if
    /// it has one, every sound file the theme can use (its own and the
    /// shared ones, and the setting's own even when missing), and Browse.
    fn sound_choices(
        &self,
        current: &LoadedTheme,
        setting: &IndicationSetting,
    ) -> Vec<SoundChoice> {
        let mut files: Vec<String> = current
            .dir
            .iter()
            .map(PathBuf::as_path)
            .chain(std::iter::once(self.sounds_dir().as_path()))
            .flat_map(wav_files)
            .collect();
        if let Some(SoundSource::File(file)) = &setting.sound {
            files.push(file.clone());
        }
        files.sort_by_key(|file| file.to_lowercase());
        files.dedup();
        let mut choices = vec![SoundChoice::None];
        if let Some(SoundSource::Tone(tone)) = &setting.sound {
            choices.push(SoundChoice::Tone(*tone));
        }
        choices.extend(files.into_iter().map(SoundChoice::File));
        choices.push(SoundChoice::Browse);
        choices
    }

    /// The selected indication's controls.
    pub(crate) fn controls(&self) -> ffi::IndicationControls {
        let report_options: Vec<String> = Presentation::ALL
            .iter()
            .map(|presentation| verbatim_i18n::presentation_name(*presentation))
            .collect();
        let Some(indication) = self.indication else {
            return ffi::IndicationControls {
                report_options,
                report: -1,
                report_enabled: false,
                sound_options: Vec::new(),
                sound: -1,
                sound_browse: -1,
                sound_enabled: false,
                words: String::new(),
                words_enabled: false,
                voice_options: Vec::new(),
                voice: -1,
                voice_enabled: false,
                preview_enabled: false,
                reset_enabled: false,
            };
        };
        let current = self.current();
        let setting = current.theme.setting(indication);
        let choices = self.sound_choices(&current, &setting);
        let chosen = match &setting.sound {
            None => SoundChoice::None,
            Some(SoundSource::Tone(tone)) => SoundChoice::Tone(*tone),
            Some(SoundSource::File(file)) => SoundChoice::File(file.clone()),
        };
        let voices: Vec<&String> = current.theme.voice_styles.keys().collect();
        let voice = setting
            .voice
            .as_ref()
            .and_then(|name| voices.iter().position(|style| *style == name))
            .map_or(0, |index| index + 1);
        let speaks = setting.report.speaks();
        ffi::IndicationControls {
            report_options,
            report: index_of(Presentation::ALL.iter().position(|p| *p == setting.report)),
            report_enabled: true,
            sound_options: choices
                .iter()
                .map(|choice| self.choice_name(choice))
                .collect(),
            sound: index_of(choices.iter().position(|choice| *choice == chosen)),
            sound_browse: index_of(Some(choices.len() - 1)),
            sound_enabled: setting.report.sounds(),
            words: setting.words.clone().unwrap_or_default(),
            words_enabled: speaks,
            voice_options: std::iter::once(self.labels.voice_default.clone())
                .chain(voices.iter().map(|style| (*style).clone()))
                .collect(),
            voice: index_of(Some(voice)),
            voice_enabled: speaks,
            preview_enabled: true,
            reset_enabled: current.theme.differs(indication),
        }
    }

    fn choice_name(&self, choice: &SoundChoice) -> String {
        match choice {
            SoundChoice::None => self.labels.sound_none.clone(),
            SoundChoice::Tone(tone) => Self::sound_name(&SoundSource::Tone(*tone)),
            SoundChoice::File(file) => file.clone(),
            SoundChoice::Browse => self.labels.sound_browse.clone(),
        }
    }

    /// Chooses the theme at `index`, which applies at once.
    pub(crate) fn choose(&mut self, index: usize) {
        if index < self.themes.len() && index != self.selected {
            self.selected = index;
            self.activate();
        }
    }

    /// The find field changed.
    pub(crate) fn set_filter(&mut self, text: &str) {
        text.clone_into(&mut self.filter);
    }

    /// An indication was selected by its catalogue index, or none.
    pub(crate) fn select(&mut self, index: Option<usize>) {
        self.indication = index.and_then(|index| self.catalogue.get(index).copied());
    }

    /// Changes the selected indication's setting with `change`.
    fn edit(&mut self, change: impl FnOnce(&mut IndicationSetting)) -> ffi::ThemeEdit {
        let Some(indication) = self.indication else {
            return edit_done();
        };
        let mut setting = self.current().theme.setting(indication);
        change(&mut setting);
        self.set(Pending::Set(indication, setting))
    }

    /// Makes `change` in the selected theme, or, when it is built in, keeps
    /// it waiting for the name of a new theme to make it in.
    fn set(&mut self, change: Pending) -> ffi::ThemeEdit {
        let current = self.current();
        if current.theme.is_builtin() {
            let name = Self::display_name(&current.theme);
            self.pending = Some(change);
            return ffi::ThemeEdit {
                needs_name: true,
                prompt_title: self.labels.new_title.clone(),
                prompt: messages::theme_copy_prompt(&name),
                suggested_name: messages::theme_copy_name(&name),
                error: String::new(),
            };
        }
        let mut theme = current.theme;
        let (indication, setting) = match change {
            Pending::Set(indication, setting) => (indication, setting),
            Pending::AddSound(indication, path) => {
                match themes::add_sound(&self.host.themes_dir(), &theme.id, &path) {
                    Ok(file) => {
                        // The theme's own sound files changed on disk.
                        let id = theme.id.clone();
                        self.reload(&id);
                        let mut setting = theme.setting(indication);
                        setting.sound = Some(SoundSource::File(file));
                        (indication, setting)
                    }
                    Err(error) => return edit_failed(&error.to_string()),
                }
            }
        };
        // A sound alone with no sound chosen yet keeps being spoken until
        // one is, and the description lists the problem meanwhile.
        if setting == indication.default_setting() {
            theme.indications.remove(&indication);
        } else {
            theme.indications.insert(indication, setting);
        }
        self.edited.insert(theme.id.clone(), theme);
        self.activate();
        edit_done()
    }

    /// "Report as" changed to the option at `option`.
    pub(crate) fn set_report(&mut self, option: usize) -> ffi::ThemeEdit {
        let Some(&report) = Presentation::ALL.get(option) else {
            return edit_done();
        };
        self.edit(|setting| setting.report = report)
    }

    /// The sound choice changed to the option at `option`.
    pub(crate) fn set_sound(&mut self, option: usize) -> ffi::ThemeEdit {
        let Some(indication) = self.indication else {
            return edit_done();
        };
        let current = self.current();
        let setting = current.theme.setting(indication);
        let sound = match self.sound_choices(&current, &setting).get(option) {
            Some(SoundChoice::None) => None,
            Some(SoundChoice::Tone(tone)) => Some(SoundSource::Tone(*tone)),
            Some(SoundChoice::File(file)) => Some(SoundSource::File(file.clone())),
            Some(SoundChoice::Browse) | None => return edit_done(),
        };
        if sound == setting.sound {
            return edit_done();
        }
        let edit = self.edit(|setting| setting.sound = sound);
        self.play_sound();
        edit
    }

    /// A sound file was chosen through Browse: it is copied into the theme
    /// and used for the indication.
    pub(crate) fn add_sound(&mut self, path: &Path) -> ffi::ThemeEdit {
        let Some(indication) = self.indication else {
            return edit_done();
        };
        self.set(Pending::AddSound(indication, path.to_path_buf()))
    }

    /// The words field changed.
    pub(crate) fn set_words(&mut self, text: &str) -> ffi::ThemeEdit {
        let words = (!text.is_empty()).then(|| text.to_owned());
        if self
            .indication
            .is_some_and(|indication| self.current().theme.setting(indication).words == words)
        {
            return edit_done();
        }
        self.edit(|setting| setting.words = words)
    }

    /// The voice choice changed to the option at `option`: 0 is the
    /// default voice, then each voice style.
    pub(crate) fn set_voice(&mut self, option: usize) -> ffi::ThemeEdit {
        let voice = option
            .checked_sub(1)
            .and_then(|index| self.current().theme.voice_styles.keys().nth(index).cloned());
        self.edit(|setting| setting.voice = voice)
    }

    /// Reset: the selected indication goes back to the default theme's
    /// setting.
    pub(crate) fn reset(&mut self) -> ffi::ThemeEdit {
        let Some(indication) = self.indication else {
            return edit_done();
        };
        if self.current().theme.is_builtin() {
            return edit_done();
        }
        self.set(Pending::Set(indication, indication.default_setting()))
    }

    /// The answer to a name prompt: the waiting change is made in a new
    /// theme named `name` based on the selected one, which becomes the
    /// selected theme; or it is dropped.
    pub(crate) fn named(&mut self, name: &str, accepted: bool) -> ffi::ThemeEdit {
        let Some(change) = self.pending.take() else {
            return edit_done();
        };
        if !accepted || name.trim().is_empty() {
            return edit_done();
        }
        let error = self.new_theme(name);
        if !error.is_empty() {
            return edit_failed(&error);
        }
        self.set(change)
    }

    /// Preview: a sample of the selected indication, through the theme.
    pub(crate) fn preview(&self) {
        let Some(indication) = self.indication else {
            return;
        };
        match preview_of(indication) {
            Preview::Speech(segments) => self.host.speak(Utterance {
                trace_id: TraceId::mint(),
                priority: SpeechPriority::Interrupt,
                segments,
                source: None,
                validity: None,
                say_all: false,
            }),
            Preview::Event(earcon) => self.host.play_earcon(earcon),
        }
    }

    /// Plays the selected indication's sound, as the sound choice shows
    /// it.
    pub(crate) fn play_sound(&self) {
        let Some(indication) = self.indication else {
            return;
        };
        let current = self.current();
        let setting = current.theme.setting(indication);
        let Some(source) = &setting.sound else {
            return;
        };
        let sound = match source {
            SoundSource::Tone(tone) => Sound::tone(tone.frequency_hz, tone.duration_ms),
            SoundSource::File(file) => match current.sound_path(file, &self.sounds_dir()) {
                Some(path) => Sound::from_wav_file(&path),
                None => return,
            },
        };
        match sound {
            Ok(sound) => self.host.play(&sound, self.gain(&current.theme, &setting)),
            Err(error) => tracing::warn!(%error, "a theme sound cannot be played"),
        }
    }

    /// The gain a sound of `setting` plays at, as speech plays it.
    fn gain(&self, theme: &Theme, setting: &IndicationSetting) -> f32 {
        let percent = |gain: u16| f32::from(gain.min(verbatim_model::MAX_GAIN)) / 100.0;
        percent(theme.gain) * percent(setting.gain) * f32::from(self.options.sound_volume.min(100))
            / 100.0
    }

    /// The sound volume slider moved: the new volume applies at once, and
    /// a short sample plays at it.
    pub(crate) fn set_volume(&mut self, volume: i32) {
        self.options.sound_volume = u8::try_from(volume.clamp(0, 100)).unwrap_or(100);
        self.host.set_options(self.options);
        let theme = self.current().theme;
        match Sound::tone(SAMPLE_TONE.frequency_hz, SAMPLE_TONE.duration_ms) {
            Ok(sound) => self
                .host
                .play(&sound, self.gain(&theme, &IndicationSetting::default())),
            Err(error) => tracing::warn!(%error, "the volume sample cannot be played"),
        }
    }

    /// The "play sounds during say all" check box was toggled.
    pub(crate) fn set_say_all(&mut self, checked: bool) {
        self.options.sounds_during_say_all = checked;
        self.host.set_options(self.options);
    }

    /// The "also speak indications that play a sound" check box was
    /// toggled.
    pub(crate) fn set_speak_sounded(&mut self, checked: bool) {
        self.options.speak_sounded_indications = checked;
        self.host.set_options(self.options);
    }

    /// A new theme named `name` based on the selected one, as it is now;
    /// it becomes the selected theme. Returns why it failed, or empty.
    pub(crate) fn new_theme(&mut self, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            return String::new();
        }
        match themes::new_theme(
            &self.host.themes_dir(),
            &self.sounds_dir(),
            &self.current(),
            name,
        ) {
            Ok(created) => {
                self.reload(&created.theme.id);
                self.activate();
                String::new()
            }
            Err(error) => error.to_string(),
        }
    }

    /// Renames the selected theme. Returns why it failed, or empty.
    pub(crate) fn rename(&mut self, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            return String::new();
        }
        let id = self.current().theme.id;
        match themes::rename_theme(&self.host.themes_dir(), &self.sounds_dir(), &id, name) {
            Ok(_) => {
                if let Some(edited) = self.edited.get_mut(&id) {
                    name.clone_into(&mut edited.name);
                }
                self.reload(&id);
                String::new()
            }
            Err(error) => error.to_string(),
        }
    }

    /// Installs the package at `path`; the theme becomes the selected one.
    /// Returns why it failed, or empty.
    pub(crate) fn import(&mut self, path: &Path) -> String {
        match themes::import_theme(path, &self.host.themes_dir(), &self.sounds_dir()) {
            Ok(imported) => {
                self.reload(&imported.theme.id);
                self.activate();
                String::new()
            }
            Err(error) => error.to_string(),
        }
    }

    /// Writes the selected theme, as saved, as a package at `path`.
    /// Returns why it failed, or empty.
    pub(crate) fn export(&self, path: &Path) -> String {
        match themes::export_theme(&self.themes[self.selected], path) {
            Ok(()) => String::new(),
            Err(error) => error.to_string(),
        }
    }

    /// Removes the selected theme, unless the configuration uses it; the
    /// built-in default theme becomes the selected one. Returns why it
    /// failed, or empty.
    pub(crate) fn remove(&mut self) -> String {
        let current = self.current().theme;
        if current.id == self.committed.0 {
            return messages::theme_in_use(&Self::display_name(&current));
        }
        match themes::remove_theme(&self.host.themes_dir(), &current.id) {
            Ok(()) => {
                self.edited.remove(&current.id);
                self.reload(Theme::DEFAULT_ID);
                self.activate();
                String::new()
            }
            Err(error) => error.to_string(),
        }
    }

    /// OK or Apply: saves the changed themes and the choice, which Cancel
    /// then restores. Returns why something could not be saved, or empty.
    pub(crate) fn apply(&mut self) -> String {
        let themes_dir = self.host.themes_dir();
        let mut failures = Vec::new();
        for theme in self.edited.values() {
            if let Err(error) = themes::save_theme(&themes_dir, theme) {
                failures.push(error.to_string());
            }
        }
        self.edited.clear();
        let id = self.themes[self.selected].theme.id.clone();
        if let Err(error) = self.host.persist(&id, self.options) {
            failures.push(error);
        }
        self.reload(&id);
        self.committed = (id, self.options);
        failures.join("\n")
    }

    /// Cancel: drops every unsaved change and restores the theme and
    /// settings last applied.
    pub(crate) fn cancel(&mut self) {
        self.edited.clear();
        self.pending = None;
        let (id, options) = self.committed.clone();
        self.options = options;
        self.reload(&id);
        self.activate();
    }
}

/// A short tone played as the sound volume changes.
const SAMPLE_TONE: Tone = Tone {
    frequency_hz: 880,
    duration_ms: 60,
};

/// A change made.
fn edit_done() -> ffi::ThemeEdit {
    edit_failed("")
}

/// A change that failed, for the reason given.
fn edit_failed(error: &str) -> ffi::ThemeEdit {
    ffi::ThemeEdit {
        needs_name: false,
        prompt_title: String::new(),
        prompt: String::new(),
        suggested_name: String::new(),
        error: error.to_owned(),
    }
}

/// `index` as C++ takes a selection, -1 for none.
fn index_of(index: Option<usize>) -> i32 {
    index
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

/// The names of the WAV files in `dir`.
fn wav_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("wav"))
        })
        .collect()
}

/// What Preview does for an indication.
#[derive(Debug, PartialEq, Eq)]
enum Preview {
    /// Speaks a sample with the indication in it.
    Speech(Vec<UtteranceSegment>),
    /// Reports the event the indication is.
    Event(Earcon),
}

/// A sample of `indication`: a sample object with the role, state, or
/// property, misspelled sample text, a capital letter, and so on, spoken
/// through the theme so its sound is heard in context; an event is
/// reported as itself.
fn preview_of(indication: Indication) -> Preview {
    let [sample, description, font, size, color] = messages::theme_preview_samples();
    let label = || UtteranceSegment::label(sample.clone());
    let span = |content| UtteranceSegment::new(content);
    let segments = match indication {
        Indication::Role(role) => vec![label(), span(SegmentContent::Role(role))],
        Indication::State(state) => vec![label(), span(SegmentContent::State(state))],
        Indication::NegatedState(state) => {
            vec![label(), span(SegmentContent::NegatedState(state))]
        }
        Indication::Description => vec![label(), span(SegmentContent::Description(description))],
        Indication::Shortcut => vec![label(), span(SegmentContent::Shortcut("Alt+S".to_owned()))],
        Indication::Position => vec![
            label(),
            span(SegmentContent::Position {
                position: 2,
                set_size: Some(5),
            }),
        ],
        Indication::Level => vec![label(), span(SegmentContent::Level(2))],
        Indication::SpellingError => vec![
            span(SegmentContent::Format(TextFormat::SpellingError)),
            UtteranceSegment::text(sample),
            span(SegmentContent::Format(TextFormat::NotSpellingError)),
        ],
        Indication::GrammarError => vec![
            span(SegmentContent::Format(TextFormat::GrammarError)),
            UtteranceSegment::text(sample),
            span(SegmentContent::Format(TextFormat::NotGrammarError)),
        ],
        Indication::FontName => vec![
            span(SegmentContent::Format(TextFormat::FontName(font))),
            UtteranceSegment::text(sample),
        ],
        Indication::FontSize => vec![
            span(SegmentContent::Format(TextFormat::FontSize(size))),
            UtteranceSegment::text(sample),
        ],
        Indication::Color => vec![
            span(SegmentContent::Format(TextFormat::Color(color))),
            UtteranceSegment::text(sample),
        ],
        Indication::FontAttributes => vec![
            span(SegmentContent::Format(TextFormat::Bold)),
            UtteranceSegment::text(sample),
            span(SegmentContent::Format(TextFormat::NotBold)),
        ],
        Indication::Capital => vec![span(SegmentContent::SpelledCapital("A".to_owned()))],
        Indication::Blank => vec![span(SegmentContent::Message(Message::Blank))],
        Indication::SkippedLines => vec![span(SegmentContent::Phrase(Phrase::SkippedLines(12)))],
        other => {
            return earcon_of(other).map_or_else(|| Preview::Speech(vec![label()]), Preview::Event);
        }
    };
    Preview::Speech(segments)
}

/// The event an Events indication reports.
fn earcon_of(indication: Indication) -> Option<Earcon> {
    Some(match indication {
        Indication::AppNotResponding => Earcon::AppNotResponding,
        Indication::Start => Earcon::Start,
        Indication::Exit => Earcon::Exit,
        Indication::Error => Earcon::Error,
        Indication::BrowseMode => Earcon::BrowseMode,
        Indication::FocusMode => Earcon::FocusMode,
        Indication::SuggestionsOpened => Earcon::SuggestionsOpened,
        Indication::SuggestionsClosed => Earcon::SuggestionsClosed,
        Indication::Progress => Earcon::Progress(50),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use verbatim_model::Role;

    use super::*;

    /// A host over temporary folders that records what it was asked to do.
    struct FakeHost {
        themes_dir: PathBuf,
        sounds_dir: PathBuf,
        configured: Mutex<(String, ThemeOptions)>,
        activated: Mutex<Vec<(Theme, ThemeOptions)>>,
        options: Mutex<Vec<ThemeOptions>>,
        played: AtomicU32,
        spoken: Mutex<Vec<Utterance>>,
        events: Mutex<Vec<Earcon>>,
    }

    impl FakeHost {
        fn new(name: &str) -> Arc<Self> {
            let root = std::env::temp_dir().join(format!(
                "verbatim-gui-theme-panel-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let sounds_dir = root.join("sounds");
            std::fs::create_dir_all(&sounds_dir).expect("create the sounds folder");
            write_wav(&sounds_dir.join("textError.wav"));
            write_wav(&sounds_dir.join("start.wav"));
            Arc::new(Self {
                themes_dir: root.join("themes"),
                sounds_dir,
                configured: Mutex::new((Theme::DEFAULT_ID.to_owned(), ThemeOptions::default())),
                activated: Mutex::new(Vec::new()),
                options: Mutex::new(Vec::new()),
                played: AtomicU32::new(0),
                spoken: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
            })
        }

        fn last_activated(&self) -> (Theme, ThemeOptions) {
            self.activated
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("a theme was activated")
        }
    }

    fn write_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create the WAV");
        writer.write_sample(1_000_i16).expect("write a sample");
        writer.finalize().expect("finish the WAV");
    }

    impl ThemeHost for FakeHost {
        fn themes_dir(&self) -> PathBuf {
            self.themes_dir.clone()
        }
        fn sounds_dir(&self) -> PathBuf {
            self.sounds_dir.clone()
        }
        fn configured(&self) -> (String, ThemeOptions) {
            self.configured.lock().unwrap().clone()
        }
        fn activate(&self, theme: &LoadedTheme, options: ThemeOptions) {
            self.activated
                .lock()
                .unwrap()
                .push((theme.theme.clone(), options));
        }
        fn set_options(&self, options: ThemeOptions) {
            self.options.lock().unwrap().push(options);
        }
        fn persist(&self, id: &str, options: ThemeOptions) -> Result<(), String> {
            *self.configured.lock().unwrap() = (id.to_owned(), options);
            Ok(())
        }
        fn play(&self, _sound: &Sound, _gain: f32) {
            self.played.fetch_add(1, Ordering::Relaxed);
        }
        fn speak(&self, utterance: Utterance) {
            self.spoken.lock().unwrap().push(utterance);
        }
        fn play_earcon(&self, earcon: Earcon) {
            self.events.lock().unwrap().push(earcon);
        }
    }

    fn catalogue_index(indication: Indication) -> usize {
        Indication::catalogue()
            .iter()
            .position(|entry| *entry == indication)
            .expect("in the catalogue")
    }

    const LINK: Indication = Indication::Role(Role::Link);

    /// The label of `indication` in the tree.
    fn label_of(panel: &ThemePanel, indication: Indication) -> String {
        let index = catalogue_index(indication);
        panel
            .tree()
            .into_iter()
            .flat_map(|category| category.items)
            .find(|item| item.indication == index)
            .map(|item| item.label)
            .expect("listed in the tree")
    }

    /// A panel whose selected theme is a user theme named "Mine", with the
    /// link selected.
    fn with_user_theme(host: &Arc<FakeHost>) -> ThemePanel {
        let mut panel = ThemePanel::open(Arc::clone(host) as Arc<dyn ThemeHost>);
        assert_eq!(panel.new_theme("Mine"), "");
        panel.select(Some(catalogue_index(LINK)));
        panel
    }

    #[test]
    fn the_page_lists_the_default_theme_first_and_selects_the_configured_one() {
        let host = FakeHost::new("open");
        let panel = ThemePanel::open(Arc::clone(&host) as Arc<dyn ThemeHost>);
        let page = panel.page();
        assert_eq!(page.themes, [verbatim_i18n::theme_default_name()]);
        assert_eq!(page.selected, 0);
        assert!(!page.can_rename && !page.can_remove, "the built-in theme");
        assert!(
            page.description
                .contains(&verbatim_i18n::theme_default_description())
        );
        assert_eq!(page.volume, 100);
        assert!(page.say_all && !page.speak_sounded);
        assert!(
            host.activated.lock().unwrap().is_empty(),
            "opening the page changes nothing"
        );
    }

    #[test]
    fn the_tree_lists_each_category_and_names_each_setting() {
        let host = FakeHost::new("tree");
        let mut panel = ThemePanel::open(Arc::clone(&host) as Arc<dyn ThemeHost>);
        let tree = panel.tree();
        assert_eq!(tree.len(), IndicationCategory::ALL.len());
        assert_eq!(
            tree.iter()
                .map(|category| category.items.len())
                .sum::<usize>(),
            Indication::catalogue().len()
        );
        assert_eq!(label_of(&panel, LINK), "link: speech");
        assert_eq!(
            label_of(&panel, Indication::SpellingError),
            "spelling error: speech and sound (textError.wav)"
        );
        panel.set_filter("SPELLING");
        let filtered = panel.tree();
        assert_eq!(filtered.len(), 1, "only text formatting has a match");
        assert_eq!(filtered[0].items.len(), 1);
    }

    #[test]
    fn a_change_to_the_built_in_theme_is_made_in_a_new_theme_once_named() {
        let host = FakeHost::new("copy");
        let mut panel = ThemePanel::open(Arc::clone(&host) as Arc<dyn ThemeHost>);
        panel.select(Some(catalogue_index(LINK)));
        let sound = Presentation::ALL
            .iter()
            .position(|presentation| *presentation == Presentation::Sound)
            .unwrap();
        let edit = panel.set_report(sound);
        assert!(edit.needs_name, "the built-in theme cannot be changed");
        assert!(host.activated.lock().unwrap().is_empty());

        let edit = panel.named("Sounds", false);
        assert!(!edit.needs_name && edit.error.is_empty());
        assert_eq!(panel.page().themes.len(), 1, "cancelled: no theme made");

        panel.set_report(sound);
        let edit = panel.named("Sounds", true);
        assert!(edit.error.is_empty(), "{}", edit.error);
        let page = panel.page();
        assert_eq!(
            page.themes[usize::try_from(page.selected).unwrap()],
            "Sounds"
        );
        assert!(page.can_rename);
        let (active, _) = host.last_activated();
        assert_eq!(active.name, "Sounds");
        assert_eq!(active.setting(LINK).report, Presentation::Sound);
        assert_eq!(label_of(&panel, LINK), "link: sound, changed");
    }

    #[test]
    fn the_controls_follow_the_setting() {
        let host = FakeHost::new("controls");
        let mut panel = with_user_theme(&host);
        let controls = panel.controls();
        assert_eq!(controls.report, 1, "speech");
        assert!(!controls.sound_enabled, "speech plays no sound");
        assert!(controls.words_enabled && controls.voice_enabled);
        assert!(!controls.reset_enabled, "nothing changed yet");
        assert_eq!(controls.sound_options[0], "none");
        assert_eq!(
            usize::try_from(controls.sound_browse).unwrap(),
            controls.sound_options.len() - 1
        );

        panel.set_report(2);
        let controls = panel.controls();
        assert!(controls.sound_enabled);
        assert!(
            !controls.words_enabled && !controls.voice_enabled,
            "a sound alone is not spoken"
        );
        assert!(controls.reset_enabled);

        let start = controls
            .sound_options
            .iter()
            .position(|option| option == "start.wav")
            .expect("the shared sounds are offered");
        assert_eq!(panel.set_sound(start).error, "");
        assert_eq!(
            host.last_activated().0.setting(LINK).sound,
            Some(SoundSource::File("start.wav".to_owned()))
        );
        assert_eq!(
            host.played.load(Ordering::Relaxed),
            1,
            "choosing a sound plays it"
        );

        panel.reset();
        assert_eq!(
            host.last_activated().0.setting(LINK),
            LINK.default_setting()
        );
        assert!(!panel.controls().reset_enabled);
    }

    #[test]
    fn apply_saves_the_theme_and_the_choice_and_cancel_restores_them() {
        let host = FakeHost::new("apply");
        let mut panel = with_user_theme(&host);
        panel.set_report(0);
        panel.set_volume(40);
        assert_eq!(panel.apply(), "");
        let (id, options) = host.configured();
        assert_eq!(id, "mine");
        assert_eq!(options.sound_volume, 40);
        let saved = themes::find_theme(&host.themes_dir, &host.sounds_dir, "mine").unwrap();
        assert_eq!(saved.theme.setting(LINK).report, Presentation::Off);

        // A later change, cancelled, is undone, live and on disk.
        panel.choose(0);
        panel.set_say_all(false);
        panel.cancel();
        let (active, options) = host.last_activated();
        assert_eq!(active.id, "mine");
        assert!(options.sounds_during_say_all);
        assert_eq!(panel.page().themes[1], "Mine");
    }

    #[test]
    fn the_configured_theme_cannot_be_removed_and_another_can() {
        let host = FakeHost::new("remove");
        let mut panel = with_user_theme(&host);
        assert_eq!(panel.apply(), "");
        assert!(!panel.page().can_remove);
        assert_ne!(panel.remove(), "", "the configured theme stays");

        assert_eq!(panel.new_theme("Other"), "");
        assert!(panel.page().can_remove);
        assert_eq!(panel.remove(), "");
        let page = panel.page();
        assert_eq!(page.themes.len(), 2, "the default and Mine remain");
        assert_eq!(page.selected, 0, "the default theme is selected");
    }

    #[test]
    fn rename_export_and_import_act_on_the_themes_folder() {
        let host = FakeHost::new("files");
        let mut panel = with_user_theme(&host);
        assert_eq!(panel.rename("Renamed"), "");
        assert!(panel.page().themes.contains(&"Renamed".to_owned()));
        let package = host.themes_dir.with_file_name("mine.zip");
        assert_eq!(panel.export(&package), "");
        panel.choose(0);
        panel.cancel();
        assert_eq!(panel.page().themes.len(), 2);
        assert!(!panel.import(&package).is_empty(), "the id is taken");
        themes::remove_theme(&host.themes_dir, "mine").unwrap();
        assert_eq!(panel.import(&package), "");
        let page = panel.page();
        assert_eq!(
            page.themes[usize::try_from(page.selected).unwrap()],
            "Renamed"
        );
    }

    #[test]
    fn preview_speaks_a_sample_and_reports_an_event() {
        let host = FakeHost::new("preview");
        let mut panel = ThemePanel::open(Arc::clone(&host) as Arc<dyn ThemeHost>);
        panel.select(Some(catalogue_index(LINK)));
        panel.preview();
        let spoken = host.spoken.lock().unwrap().clone();
        assert_eq!(spoken.len(), 1);
        assert!(
            spoken[0]
                .segments
                .iter()
                .any(|segment| segment.content == SegmentContent::Role(Role::Link))
        );
        panel.select(Some(catalogue_index(Indication::Exit)));
        panel.preview();
        assert_eq!(*host.events.lock().unwrap(), [Earcon::Exit]);
    }

    #[test]
    fn every_event_previews_as_itself_and_everything_else_speaks() {
        for indication in Indication::catalogue() {
            let preview = preview_of(indication);
            if indication.category() == IndicationCategory::Events {
                assert!(matches!(preview, Preview::Event(earcon)
                    if Indication::of_earcon(earcon) == indication));
            } else {
                assert!(
                    matches!(&preview, Preview::Speech(segments)
                        if segments.iter().any(|segment|
                            Indication::of_segment(&segment.content) == Some(indication))),
                    "{indication:?} previews as {preview:?}"
                );
            }
        }
    }
}
