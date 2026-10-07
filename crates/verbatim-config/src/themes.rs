//! Theme files (`phase6-design.md`, "Themes and configuration profiles"
//! and "Packaging").
//!
//! A theme is a directory in the user's themes folder, named by the
//! theme's id: a TOML manifest, `theme.toml`, holding the theme's name,
//! author, description, version, gain, voice styles, and the indications
//! where it differs from the default theme, beside its sound files. The
//! built-in default theme has no directory: it is defined in code
//! (`verbatim_model::Theme::builtin_default`), and its sounds are the
//! shared `sounds` folder beside the program. A theme names a sound by
//! file name; a file the theme's directory does not have is looked for in
//! the shared sounds.
//!
//! The theme panel's buttons are file operations here: [`new_theme`]
//! ("New theme based on this"), [`rename_theme`], [`remove_theme`],
//! [`add_sound`] (the Sound list's "Browse..."), [`import_theme`], and
//! [`export_theme`], which read and write a theme package, the directory
//! zipped. [`save_theme`] writes an edited theme; the built-in default
//! theme is never written, since editing it makes a new theme.
//!
//! The manifest:
//!
//! ```toml
//! id = "proofreading"
//! name = "Proofreading"
//! author = "A. Writer"
//! description = "Spelling errors by sound alone."
//! version = "1"
//! gain = 100
//!
//! [voice_styles.emphasis]
//! pitch = 20
//!
//! [indications.spelling-error]
//! report = "sound"
//! sound = "textError.wav"
//! gain = 80
//!
//! [indications.role-link]
//! report = "speech-and-sound"
//! sound = { frequency = 880, duration = 40 }
//! voice = "emphasis"
//! ```
//!
//! Indications are named by their catalogue ids
//! (`verbatim_model::Indication::id`); `report` is `off`, `speech`,
//! `sound`, or `speech-and-sound`; `sound` is a file name or a tone;
//! `gain` is in percent; `words` replaces the indication's words; `voice`
//! names one of the theme's voice styles. An indication this version does
//! not know is reported as a problem and otherwise ignored, so a theme from
//! a later version still loads.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use verbatim_model::{
    DEFAULT_GAIN, Indication, IndicationSetting, SoundSource, Theme, ThemeProblem, VoiceStyle,
    is_plain_file_name,
};

use crate::package;

/// The manifest's file name inside a theme's directory.
pub const MANIFEST: &str = "theme.toml";

/// The longest theme id.
const MAX_ID_LEN: usize = 64;

/// A theme as loaded: the theme, its directory (`None` for the built-in
/// default theme), and the problems found loading it, for the theme
/// panel's description and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTheme {
    /// The theme. Its id is its directory's name.
    pub theme: Theme,
    /// Its directory, holding its manifest and sounds.
    pub dir: Option<PathBuf>,
    /// What is wrong with it: unknown indications, sounds missing, and the
    /// problems `Theme::problems` finds. Sounds that are present but
    /// cannot be decoded are found by whatever decodes them.
    pub problems: Vec<ThemeProblem>,
}

impl LoadedTheme {
    /// The built-in default theme.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            theme: Theme::builtin_default(),
            dir: None,
            problems: Vec::new(),
        }
    }

    /// Where the sound file `file` of this theme is: in its directory, or
    /// else in the shared `sounds_dir`; `None` when neither has it or the
    /// name is not a plain file name.
    #[must_use]
    pub fn sound_path(&self, file: &str, sounds_dir: &Path) -> Option<PathBuf> {
        sound_path(self.dir.as_deref(), sounds_dir, file)
    }
}

/// Error from a theme file operation.
#[derive(Debug)]
#[non_exhaustive]
pub enum ThemeError {
    /// Reading or writing a file failed.
    Io(PathBuf, io::Error),
    /// A manifest is not valid TOML for a theme.
    Parse(PathBuf, toml::de::Error),
    /// Writing a manifest as TOML failed.
    Serialize(toml::ser::Error),
    /// A theme id that is not 1 to 64 lowercase letters, digits, hyphens,
    /// and underscores.
    InvalidId(String),
    /// The built-in default theme cannot be written, renamed, removed,
    /// given sounds, or exported.
    Builtin,
    /// A theme with this id is already installed.
    AlreadyExists(String),
    /// No theme with this id is installed.
    NotFound(String),
    /// A sound to add is not a WAV file.
    NotAWav(PathBuf),
    /// A theme package could not be read or written.
    Package(PathBuf, String),
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "theme I/O error at {}: {error}", path.display()),
            Self::Parse(path, error) => {
                write!(f, "theme parse error in {}: {error}", path.display())
            }
            Self::Serialize(error) => write!(f, "theme serialize error: {error}"),
            Self::InvalidId(id) => write!(f, "`{id}` is not a valid theme id"),
            Self::Builtin => write!(f, "the built-in default theme cannot be changed"),
            Self::AlreadyExists(id) => write!(f, "a theme with the id `{id}` is installed"),
            Self::NotFound(id) => write!(f, "no theme with the id `{id}` is installed"),
            Self::NotAWav(path) => write!(f, "{} is not a WAV file", path.display()),
            Self::Package(path, error) => {
                write!(f, "theme package {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for ThemeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, error) => Some(error),
            Self::Parse(_, error) => Some(error),
            Self::Serialize(error) => Some(error),
            _ => None,
        }
    }
}

/// The manifest as written: indications by id, so an id this version does
/// not know still parses.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
    #[serde(default = "default_gain")]
    gain: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    voice_styles: BTreeMap<String, VoiceStyle>,
    #[serde(default)]
    indications: BTreeMap<String, IndicationSetting>,
}

const fn default_gain() -> u16 {
    DEFAULT_GAIN
}

/// Whether `id` can name a theme: 1 to 64 lowercase ASCII letters, digits,
/// hyphens, and underscores, so it is also a safe directory name. The
/// built-in default theme's id is valid but taken.
#[must_use]
pub fn is_valid_theme_id(id: &str) -> bool {
    (1..=MAX_ID_LEN).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Parses a manifest into a theme and the problems found in it: unknown
/// indications (left out) and those `Theme::problems` finds.
///
/// # Errors
///
/// Returns the TOML error when the text is not a theme manifest.
pub fn parse_theme(text: &str) -> Result<(Theme, Vec<ThemeProblem>), toml::de::Error> {
    let manifest: Manifest = toml::from_str(text)?;
    let mut problems = Vec::new();
    let mut indications = BTreeMap::new();
    for (id, setting) in manifest.indications {
        match Indication::from_id(&id) {
            Some(indication) => {
                indications.insert(indication, setting);
            }
            None => problems.push(ThemeProblem::UnknownIndication { id }),
        }
    }
    let theme = Theme {
        id: manifest.id,
        name: manifest.name,
        author: manifest.author,
        description: manifest.description,
        version: manifest.version,
        gain: manifest.gain,
        voice_styles: manifest.voice_styles,
        indications,
    };
    problems.extend(theme.problems());
    Ok((theme, problems))
}

/// A theme as manifest text.
///
/// # Errors
///
/// Returns the TOML error when the theme cannot be written as TOML.
pub fn theme_to_toml(theme: &Theme) -> Result<String, toml::ser::Error> {
    let manifest = Manifest {
        id: theme.id.clone(),
        name: theme.name.clone(),
        author: theme.author.clone(),
        description: theme.description.clone(),
        version: theme.version.clone(),
        gain: theme.gain,
        voice_styles: theme.voice_styles.clone(),
        indications: theme
            .indications
            .iter()
            .map(|(indication, setting)| (indication.id(), setting.clone()))
            .collect(),
    };
    toml::to_string_pretty(&manifest)
}

/// Where the sound file `file` is: in `theme_dir`, or else in the shared
/// `sounds_dir`; `None` when neither has it or the name is not a plain
/// file name.
#[must_use]
pub fn sound_path(theme_dir: Option<&Path>, sounds_dir: &Path, file: &str) -> Option<PathBuf> {
    if !is_plain_file_name(file) {
        return None;
    }
    theme_dir
        .map(|dir| dir.join(file))
        .into_iter()
        .chain(std::iter::once(sounds_dir.join(file)))
        .find(|path| path.is_file())
}

/// The problems with `theme` that can be found without decoding its
/// sounds: those `Theme::problems` finds, and every sound file that is
/// neither in `theme_dir` nor in `sounds_dir`.
#[must_use]
pub fn validate_theme(
    theme: &Theme,
    theme_dir: Option<&Path>,
    sounds_dir: &Path,
) -> Vec<ThemeProblem> {
    let mut problems = theme.problems();
    for (indication, setting) in &theme.indications {
        if let Some(SoundSource::File(file)) = &setting.sound
            && is_plain_file_name(file)
            && sound_path(theme_dir, sounds_dir, file).is_none()
        {
            problems.push(ThemeProblem::MissingSound {
                indication: *indication,
                file: file.clone(),
            });
        }
    }
    problems
}

/// Loads the theme in `dir`. Its id is the directory's name, whatever the
/// manifest says.
///
/// # Errors
///
/// Returns [`ThemeError::Io`] when the manifest cannot be read and
/// [`ThemeError::Parse`] when it is not a theme manifest.
pub fn load_theme(dir: &Path, sounds_dir: &Path) -> Result<LoadedTheme, ThemeError> {
    let path = dir.join(MANIFEST);
    let text = fs::read_to_string(&path).map_err(|error| ThemeError::Io(path.clone(), error))?;
    let (mut theme, mut problems) =
        parse_theme(&text).map_err(|error| ThemeError::Parse(path, error))?;
    if let Some(id) = dir.file_name().and_then(|name| name.to_str()) {
        id.clone_into(&mut theme.id);
    }
    // `parse_theme` found the problems that need no files.
    problems.extend(
        validate_theme(&theme, Some(dir), sounds_dir)
            .into_iter()
            .filter(|problem| matches!(problem, ThemeProblem::MissingSound { .. })),
    );
    Ok(LoadedTheme {
        theme,
        dir: Some(dir.to_path_buf()),
        problems,
    })
}

/// The theme with id `id`: the built-in default theme for `default`, and
/// otherwise the one in the themes folder.
///
/// # Errors
///
/// Returns [`ThemeError::InvalidId`] for an id no theme can have,
/// [`ThemeError::NotFound`] when no such theme is installed, and the
/// errors of [`load_theme`].
pub fn find_theme(
    themes_dir: &Path,
    sounds_dir: &Path,
    id: &str,
) -> Result<LoadedTheme, ThemeError> {
    if id == Theme::DEFAULT_ID {
        return Ok(LoadedTheme::builtin());
    }
    if !is_valid_theme_id(id) {
        return Err(ThemeError::InvalidId(id.to_owned()));
    }
    let dir = themes_dir.join(id);
    if !dir.join(MANIFEST).is_file() {
        return Err(ThemeError::NotFound(id.to_owned()));
    }
    load_theme(&dir, sounds_dir)
}

/// Every installed theme, for the theme panel's list: the built-in default
/// theme first, then the themes folder's by name. A directory that cannot
/// be loaded is returned as an error beside the list, not left out
/// silently; a missing themes folder is no themes.
#[must_use]
pub fn list_themes(themes_dir: &Path, sounds_dir: &Path) -> (Vec<LoadedTheme>, Vec<ThemeError>) {
    let mut themes = Vec::new();
    let mut errors = Vec::new();
    if let Ok(entries) = fs::read_dir(themes_dir) {
        for entry in entries.flatten() {
            let dir = entry.path();
            let id = entry.file_name();
            let Some(id) = id.to_str() else { continue };
            if !dir.is_dir() || !is_valid_theme_id(id) || id == Theme::DEFAULT_ID {
                continue;
            }
            match load_theme(&dir, sounds_dir) {
                Ok(theme) => themes.push(theme),
                Err(error) => errors.push(error),
            }
        }
    }
    themes.sort_by(|a, b| {
        (a.theme.name.to_lowercase(), &a.theme.id).cmp(&(b.theme.name.to_lowercase(), &b.theme.id))
    });
    themes.insert(0, LoadedTheme::builtin());
    (themes, errors)
}

/// Writes `theme`'s manifest to its directory in the themes folder,
/// creating the directory if need be, and returns the directory.
///
/// # Errors
///
/// Returns [`ThemeError::Builtin`] for the built-in default theme,
/// [`ThemeError::InvalidId`] for an id no theme can have, and
/// [`ThemeError::Io`] or [`ThemeError::Serialize`] when writing fails.
pub fn save_theme(themes_dir: &Path, theme: &Theme) -> Result<PathBuf, ThemeError> {
    if theme.is_builtin() {
        return Err(ThemeError::Builtin);
    }
    if !is_valid_theme_id(&theme.id) {
        return Err(ThemeError::InvalidId(theme.id.clone()));
    }
    let dir = themes_dir.join(&theme.id);
    fs::create_dir_all(&dir).map_err(|error| ThemeError::Io(dir.clone(), error))?;
    let text = theme_to_toml(theme).map_err(ThemeError::Serialize)?;
    write_atomic(&dir.join(MANIFEST), text.as_bytes())?;
    Ok(dir)
}

/// An id for a new theme named `name`: the name in lowercase with anything
/// but letters and digits made a hyphen, numbered when a theme already has
/// it.
#[must_use]
pub fn theme_id_for_name(themes_dir: &Path, name: &str) -> String {
    let mut base = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let mut base = base.trim_end_matches('-').to_owned();
    base.truncate(MAX_ID_LEN - 4);
    if base.is_empty() {
        "theme".clone_into(&mut base);
    }
    let taken = |id: &str| id == Theme::DEFAULT_ID || themes_dir.join(id).exists();
    if !taken(&base) {
        return base;
    }
    let mut number = 2_u32;
    loop {
        let id = format!("{base}-{number}");
        if !taken(&id) {
            return id;
        }
        number += 1;
    }
}

/// Makes a new theme named `name` based on `base`: a copy of its
/// settings, voice styles, gain, and description, and of the sound files in
/// its directory; a theme based on the built-in default theme starts with
/// no differences from it. Returns the new theme as loaded.
///
/// # Errors
///
/// Returns [`ThemeError::Io`] or [`ThemeError::Serialize`] when writing
/// fails.
pub fn new_theme(
    themes_dir: &Path,
    sounds_dir: &Path,
    base: &LoadedTheme,
    name: &str,
) -> Result<LoadedTheme, ThemeError> {
    let id = theme_id_for_name(themes_dir, name);
    let mut theme = Theme::new(id, name);
    theme.description.clone_from(&base.theme.description);
    theme.gain = base.theme.gain;
    theme.voice_styles.clone_from(&base.theme.voice_styles);
    if !base.theme.is_builtin() {
        theme.indications.clone_from(&base.theme.indications);
    }
    let dir = save_theme(themes_dir, &theme)?;
    if let Some(base_dir) = &base.dir {
        for file in sound_files(&theme) {
            let source = base_dir.join(&file);
            if source.is_file() {
                let target = dir.join(&file);
                fs::copy(&source, &target).map_err(|error| ThemeError::Io(target, error))?;
            }
        }
    }
    load_theme(&dir, sounds_dir)
}

/// Gives the theme `id` a new name; its id, and so every reference to it,
/// stays.
///
/// # Errors
///
/// As [`find_theme`] and [`save_theme`].
pub fn rename_theme(
    themes_dir: &Path,
    sounds_dir: &Path,
    id: &str,
    name: &str,
) -> Result<LoadedTheme, ThemeError> {
    let mut loaded = find_theme(themes_dir, sounds_dir, id)?;
    if loaded.theme.is_builtin() {
        return Err(ThemeError::Builtin);
    }
    name.clone_into(&mut loaded.theme.name);
    save_theme(themes_dir, &loaded.theme)?;
    Ok(loaded)
}

/// Removes the theme `id` and its sounds. Whether a profile still uses it
/// is the caller's to check.
///
/// # Errors
///
/// Returns [`ThemeError::Builtin`] for the built-in default theme,
/// [`ThemeError::NotFound`] when it is not installed, and
/// [`ThemeError::Io`] when removing fails.
pub fn remove_theme(themes_dir: &Path, id: &str) -> Result<(), ThemeError> {
    if id == Theme::DEFAULT_ID {
        return Err(ThemeError::Builtin);
    }
    if !is_valid_theme_id(id) {
        return Err(ThemeError::InvalidId(id.to_owned()));
    }
    let dir = themes_dir.join(id);
    if !dir.join(MANIFEST).is_file() {
        return Err(ThemeError::NotFound(id.to_owned()));
    }
    fs::remove_dir_all(&dir).map_err(|error| ThemeError::Io(dir, error))
}

/// Copies the WAV file at `file` into the theme `id`'s directory and
/// returns the name the theme can use it by: its own name, or that name
/// numbered when the theme already has a different file by it.
///
/// # Errors
///
/// Returns [`ThemeError::NotAWav`] for a file not named `.wav`,
/// [`ThemeError::Builtin`] for the built-in default theme,
/// [`ThemeError::NotFound`] when the theme is not installed, and
/// [`ThemeError::Io`] when copying fails.
pub fn add_sound(themes_dir: &Path, id: &str, file: &Path) -> Result<String, ThemeError> {
    if id == Theme::DEFAULT_ID {
        return Err(ThemeError::Builtin);
    }
    let not_a_wav = || ThemeError::NotAWav(file.to_path_buf());
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_plain_file_name(name))
        .ok_or_else(not_a_wav)?;
    let (stem, extension) = name.rsplit_once('.').ok_or_else(not_a_wav)?;
    if !extension.eq_ignore_ascii_case("wav") {
        return Err(not_a_wav());
    }
    if !is_valid_theme_id(id) || !themes_dir.join(id).join(MANIFEST).is_file() {
        return Err(ThemeError::NotFound(id.to_owned()));
    }
    let dir = themes_dir.join(id);
    let contents = fs::read(file).map_err(|error| ThemeError::Io(file.to_path_buf(), error))?;
    // A name the theme does not have, or has for this same sound.
    let free = |candidate: &str| {
        fs::read(dir.join(candidate)).map_or(true, |existing| existing == contents)
    };
    let mut chosen = name.to_owned();
    let mut number = 2_u32;
    while !free(&chosen) {
        chosen = format!("{stem}-{number}.{extension}");
        number += 1;
    }
    let target = dir.join(&chosen);
    fs::write(&target, &contents).map_err(|error| ThemeError::Io(target, error))?;
    Ok(chosen)
}

/// Installs the theme package at `package` into the themes folder: its
/// manifest and the WAV files beside it, nothing else. Returns the theme as
/// loaded.
///
/// # Errors
///
/// Returns [`ThemeError::Package`] when the package cannot be read or has
/// no manifest, [`ThemeError::Parse`] when the manifest is not one,
/// [`ThemeError::InvalidId`] or [`ThemeError::AlreadyExists`] when its id
/// is unusable or taken (the built-in default theme's included), and
/// [`ThemeError::Io`] when installing fails; nothing is left installed
/// then.
pub fn import_theme(
    package: &Path,
    themes_dir: &Path,
    sounds_dir: &Path,
) -> Result<LoadedTheme, ThemeError> {
    let files = package::read(package, MANIFEST, |name| {
        name == MANIFEST
            || name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("wav"))
    })
    .map_err(|error| ThemeError::Package(package.to_path_buf(), error))?;
    let manifest = files
        .iter()
        .find(|(name, _)| name == MANIFEST)
        .map(|(_, contents)| String::from_utf8_lossy(contents).into_owned())
        .ok_or_else(|| ThemeError::Package(package.to_path_buf(), format!("no {MANIFEST}")))?;
    let (theme, _) =
        parse_theme(&manifest).map_err(|error| ThemeError::Parse(package.join(MANIFEST), error))?;
    if !is_valid_theme_id(&theme.id) {
        return Err(ThemeError::InvalidId(theme.id));
    }
    let dir = themes_dir.join(&theme.id);
    if theme.is_builtin() || dir.exists() {
        return Err(ThemeError::AlreadyExists(theme.id));
    }
    // Unpacked beside its place and moved there whole, so a failure part
    // way leaves nothing installed.
    let staging = themes_dir.join(format!(".{}.import", theme.id));
    let _ = fs::remove_dir_all(&staging);
    let installed = fs::create_dir_all(&staging)
        .map_err(|error| ThemeError::Io(staging.clone(), error))
        .and_then(|()| {
            for (name, contents) in &files {
                let path = staging.join(name);
                fs::write(&path, contents).map_err(|error| ThemeError::Io(path, error))?;
            }
            fs::rename(&staging, &dir).map_err(|error| ThemeError::Io(dir.clone(), error))
        });
    if let Err(error) = installed {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    load_theme(&dir, sounds_dir)
}

/// Writes `theme` as a theme package at `package`: its manifest and the
/// sound files its directory holds that it uses. Sounds it takes from the
/// shared sounds are not included, since every installation has them.
///
/// # Errors
///
/// Returns [`ThemeError::Builtin`] for the built-in default theme,
/// [`ThemeError::Serialize`] when the manifest cannot be written, and
/// [`ThemeError::Package`] when writing the package fails.
pub fn export_theme(theme: &LoadedTheme, package: &Path) -> Result<(), ThemeError> {
    let Some(dir) = &theme.dir else {
        return Err(ThemeError::Builtin);
    };
    let manifest = dir.join(MANIFEST);
    let mut files = vec![(MANIFEST.to_owned(), manifest)];
    for file in sound_files(&theme.theme) {
        let path = dir.join(&file);
        if path.is_file() {
            files.push((file, path));
        }
    }
    package::write(package, &files)
        .map_err(|error| ThemeError::Package(package.to_path_buf(), error))
}

/// The plain file names of the sounds `theme` names, each once.
fn sound_files(theme: &Theme) -> Vec<String> {
    let mut files: Vec<String> = theme
        .indications
        .values()
        .filter_map(|setting| match &setting.sound {
            Some(SoundSource::File(file)) if is_plain_file_name(file) => Some(file.clone()),
            _ => None,
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Writes `contents` to `path` through a temporary file renamed over it.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), ThemeError> {
    let mut temp = path.to_path_buf();
    temp.set_extension("toml.tmp");
    fs::write(&temp, contents).map_err(|error| ThemeError::Io(temp.clone(), error))?;
    // On Windows, std::fs::rename replaces an existing destination.
    fs::rename(&temp, path).map_err(|error| ThemeError::Io(path.to_path_buf(), error))
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Presentation, Role, Tone};

    use super::*;

    /// The test's own temp folder ([`crate::tests::TempRoot`]), holding
    /// empty themes and sounds folders.
    fn temp_root(name: &str) -> crate::tests::TempRoot {
        let root = crate::tests::TempRoot::new(&format!("themes-{name}"));
        fs::create_dir(root.join("themes")).expect("create themes");
        fs::create_dir(root.join("sounds")).expect("create sounds");
        root
    }

    fn proofreading() -> Theme {
        let mut theme = Theme::new("proofreading", "Proofreading");
        theme.author = "A. Writer".to_owned();
        theme.voice_styles.insert(
            "emphasis".to_owned(),
            VoiceStyle {
                pitch: 20,
                ..VoiceStyle::default()
            },
        );
        theme.indications.insert(
            Indication::SpellingError,
            IndicationSetting {
                report: Presentation::Sound,
                sound: Some(SoundSource::File("ding.wav".to_owned())),
                gain: 80,
                ..IndicationSetting::default()
            },
        );
        theme.indications.insert(
            Indication::Role(Role::Link),
            IndicationSetting {
                report: Presentation::SpeechAndSound,
                sound: Some(SoundSource::Tone(Tone {
                    frequency_hz: 880,
                    duration_ms: 40,
                })),
                words: Some("lnk".to_owned()),
                voice: Some("emphasis".to_owned()),
                ..IndicationSetting::default()
            },
        );
        theme
    }

    #[test]
    fn a_theme_round_trips_through_its_manifest() {
        let theme = proofreading();
        let text = theme_to_toml(&theme).expect("writes");
        assert!(text.contains("[indications.spelling-error]"), "{text}");
        assert!(text.contains("report = \"speech-and-sound\""), "{text}");
        let (back, problems) = parse_theme(&text).expect("parses");
        assert_eq!(back, theme);
        assert_eq!(problems, Vec::new());
    }

    #[test]
    fn the_documented_manifest_parses() {
        let text = "id = \"proofreading\"\nname = \"Proofreading\"\n\n\
            [voice_styles.emphasis]\npitch = 20\n\n\
            [indications.spelling-error]\nreport = \"sound\"\nsound = \"textError.wav\"\ngain = 80\n\n\
            [indications.role-link]\nreport = \"speech-and-sound\"\n\
            sound = { frequency = 880, duration = 40 }\nvoice = \"emphasis\"\n";
        let (theme, problems) = parse_theme(text).expect("parses");
        assert_eq!(problems, Vec::new());
        assert_eq!(theme.gain, DEFAULT_GAIN);
        assert_eq!(
            theme.setting(Indication::Role(Role::Link)).sound,
            Some(SoundSource::Tone(Tone {
                frequency_hz: 880,
                duration_ms: 40
            }))
        );
    }

    #[test]
    fn unknown_indications_and_bad_settings_are_problems_not_errors() {
        let text = "id = \"later\"\nname = \"Later\"\n\
            [indications.role-hologram]\nreport = \"sound\"\n\
            [indications.description]\nreport = \"sound\"\n";
        let (theme, problems) = parse_theme(text).expect("parses");
        assert_eq!(
            problems,
            vec![
                ThemeProblem::UnknownIndication {
                    id: "role-hologram".to_owned()
                },
                ThemeProblem::SoundOnlyWithoutSound {
                    indication: Indication::Description
                },
            ]
        );
        assert_eq!(theme.indications.len(), 1);
        assert!(parse_theme("name = 3").is_err(), "not a manifest at all");
    }

    #[test]
    fn a_missing_sound_is_a_problem_and_a_shared_one_is_found() {
        let root = temp_root("missing-sound");
        let themes = root.join("themes");
        let sounds = root.join("sounds");
        save_theme(&themes, &proofreading()).expect("saves");
        let loaded = find_theme(&themes, &sounds, "proofreading").expect("loads");
        assert_eq!(
            loaded.problems,
            vec![ThemeProblem::MissingSound {
                indication: Indication::SpellingError,
                file: "ding.wav".to_owned()
            }]
        );
        fs::write(sounds.join("ding.wav"), b"shared").expect("write a shared sound");
        let loaded = find_theme(&themes, &sounds, "proofreading").expect("loads");
        assert_eq!(loaded.problems, Vec::new());
        assert_eq!(
            loaded.sound_path("ding.wav", &sounds),
            Some(sounds.join("ding.wav"))
        );
    }

    #[test]
    fn themes_are_listed_default_first_and_found_by_id() {
        let root = temp_root("list");
        let themes = root.join("themes");
        let sounds = root.join("sounds");
        let mut zebra = Theme::new("zebra", "Zebra");
        zebra.description = "Stripes".to_owned();
        save_theme(&themes, &zebra).expect("saves");
        save_theme(&themes, &proofreading()).expect("saves");
        fs::create_dir_all(themes.join("broken")).expect("dir");
        fs::write(themes.join("broken").join(MANIFEST), "not = [toml").expect("write");

        let (list, errors) = list_themes(&themes, &sounds);
        let ids: Vec<&str> = list.iter().map(|loaded| loaded.theme.id.as_str()).collect();
        assert_eq!(ids, ["default", "proofreading", "zebra"]);
        let [ThemeError::Parse(path, _)] = errors.as_slice() else {
            panic!("only the broken theme's manifest is reported: {errors:?}");
        };
        assert_eq!(path, &themes.join("broken").join(MANIFEST));
        assert!(
            find_theme(&themes, &sounds, "default")
                .expect("builtin")
                .dir
                .is_none()
        );
        assert!(matches!(
            find_theme(&themes, &sounds, "absent"),
            Err(ThemeError::NotFound(_))
        ));
        assert!(matches!(
            find_theme(&themes, &sounds, "../escape"),
            Err(ThemeError::InvalidId(_))
        ));
    }

    #[test]
    fn the_default_theme_is_never_written() {
        let root = temp_root("builtin");
        let themes = root.join("themes");
        assert!(matches!(
            save_theme(&themes, &Theme::builtin_default()),
            Err(ThemeError::Builtin)
        ));
        assert!(matches!(
            remove_theme(&themes, "default"),
            Err(ThemeError::Builtin)
        ));
        assert!(matches!(
            export_theme(&LoadedTheme::builtin(), &root.join("default.zip")),
            Err(ThemeError::Builtin)
        ));
    }

    #[test]
    fn a_new_theme_copies_its_base_and_its_sounds() {
        let root = temp_root("new");
        let themes = root.join("themes");
        let sounds = root.join("sounds");
        let dir = save_theme(&themes, &proofreading()).expect("saves");
        fs::write(dir.join("ding.wav"), b"ding").expect("a sound");
        let base = find_theme(&themes, &sounds, "proofreading").expect("loads");

        let copy = new_theme(&themes, &sounds, &base, "Proofreading at night").expect("made");
        assert_eq!(copy.theme.id, "proofreading-at-night");
        assert_eq!(copy.theme.name, "Proofreading at night");
        assert_eq!(copy.theme.indications, base.theme.indications);
        assert_eq!(copy.problems, Vec::new());
        assert_eq!(
            fs::read(themes.join("proofreading-at-night").join("ding.wav")).expect("copied"),
            b"ding"
        );

        let from_default =
            new_theme(&themes, &sounds, &LoadedTheme::builtin(), "Proofreading").expect("made");
        assert_eq!(
            from_default.theme.id, "proofreading-2",
            "the id is numbered"
        );
        assert!(from_default.theme.indications.is_empty());

        let renamed = rename_theme(&themes, &sounds, "proofreading-2", "Plain").expect("renamed");
        assert_eq!(renamed.theme.id, "proofreading-2");
        assert_eq!(
            find_theme(&themes, &sounds, "proofreading-2")
                .expect("loads")
                .theme
                .name,
            "Plain"
        );
        remove_theme(&themes, "proofreading-2").expect("removed");
        assert!(!themes.join("proofreading-2").exists());
    }

    #[test]
    fn a_sound_is_added_under_a_free_name() {
        let root = temp_root("add-sound");
        let themes = root.join("themes");
        save_theme(&themes, &proofreading()).expect("saves");
        let first = root.join("ding.wav");
        fs::write(&first, b"one").expect("write");
        assert_eq!(
            add_sound(&themes, "proofreading", &first).expect("adds"),
            "ding.wav"
        );
        assert_eq!(
            add_sound(&themes, "proofreading", &first).expect("adds"),
            "ding.wav",
            "the same file again is the same sound"
        );
        let other = root.join("other").join("ding.wav");
        fs::create_dir_all(other.parent().expect("parent")).expect("dir");
        fs::write(&other, b"two").expect("write");
        assert_eq!(
            add_sound(&themes, "proofreading", &other).expect("adds"),
            "ding-2.wav"
        );
        let text = root.join("notes.txt");
        fs::write(&text, b"text").expect("write");
        assert!(matches!(
            add_sound(&themes, "proofreading", &text),
            Err(ThemeError::NotAWav(_))
        ));
    }

    #[test]
    fn a_theme_exported_and_imported_is_the_same_theme() {
        let root = temp_root("export-import");
        let themes = root.join("themes");
        let sounds = root.join("sounds");
        let dir = save_theme(&themes, &proofreading()).expect("saves");
        fs::write(dir.join("ding.wav"), b"ding").expect("a sound");
        fs::write(dir.join("unused.wav"), b"unused").expect("a sound it does not use");
        let original = find_theme(&themes, &sounds, "proofreading").expect("loads");
        let package = root.join("proofreading.zip");
        export_theme(&original, &package).expect("exports");

        assert!(matches!(
            import_theme(&package, &themes, &sounds),
            Err(ThemeError::AlreadyExists(_))
        ));
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("dir");
        let imported = import_theme(&package, &elsewhere, &sounds).expect("imports");
        assert_eq!(imported.theme, original.theme);
        assert_eq!(imported.problems, Vec::new());
        let installed = elsewhere.join("proofreading");
        assert_eq!(
            fs::read(installed.join("ding.wav")).expect("unpacked"),
            b"ding"
        );
        assert!(!installed.join("unused.wav").exists());
    }

    #[test]
    fn a_package_zipped_as_a_folder_imports_and_stray_paths_are_left_out() {
        use std::io::Write as _;
        let root = temp_root("import-folder");
        let themes = root.join("themes");
        let sounds = root.join("sounds");
        let package = root.join("calm.zip");
        {
            let file = fs::File::create(&package).expect("create");
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for (name, contents) in [
                ("calm/theme.toml", "id = \"calm\"\nname = \"Calm\"\n"),
                ("calm/soft.wav", "soft"),
                ("calm/readme.txt", "not a theme file"),
                ("calm/nested/deep.wav", "deep"),
            ] {
                zip.start_file(name, options).expect("start");
                zip.write_all(contents.as_bytes()).expect("write");
            }
            zip.finish().expect("finish");
        }
        let imported = import_theme(&package, &themes, &sounds).expect("imports");
        assert_eq!(imported.theme.name, "Calm");
        let dir = themes.join("calm");
        let mut names: Vec<String> = fs::read_dir(&dir)
            .expect("installed")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect();
        names.sort();
        assert_eq!(names, ["soft.wav", "theme.toml"]);
        assert!(!themes.join(".calm.import").exists());

        let empty = root.join("empty.zip");
        zip::ZipWriter::new(fs::File::create(&empty).expect("create"))
            .finish()
            .expect("finish");
        assert!(matches!(
            import_theme(&empty, &themes, &sounds),
            Err(ThemeError::Package(_, _))
        ));
    }
}
