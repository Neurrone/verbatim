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
}
