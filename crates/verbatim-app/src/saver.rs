//! The saver (`docs/design/ownership.md`): the one thread that owns `settings.toml` and writes it whole.
//!
//! Each section of the file has one owner, which hands the saver its new
//! value through a section writer: the speech section the synth thread
//! ([`SpeechWriter`]), the reader section the reducer thread
//! ([`ReaderWriter`]), the theme section the GUI thread ([`ThemeWriter`]).
//! A writer's save is one post to the saver's completion port, which takes
//! no lock and never waits, so no thread on the event or speech path waits
//! on a file. The saver takes everything waiting, keeps the newest value of
//! each section, and writes the file once, a temporary file renamed over
//! it; with one thread writing, no section's update is lost to another's.
//! The global fields have no writer while Verbatim runs.

use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Sender, bounded};
use verbatim_config::{ConfigError, ConfigStore, Settings, SpeechConfig, ThemeConfig};
use verbatim_model::ReaderSettings;
use verbatim_process::port_queue::{PortQueue, Poster, port_queue};

/// What a writer or the main thread posts to the saver.
enum Message {
    Speech(SpeechConfig),
    Reader(ReaderSettings),
    Theme(ThemeConfig),
    /// Answer once everything posted before has been written.
    Finish(Sender<()>),
}

/// Where the saver writes the settings: the file on disk, or a test's.
pub(crate) trait SettingsFile: Send + 'static {
    /// Writes `settings` whole.
    ///
    /// # Errors
    ///
    /// Returns the error writing failed with.
    fn write(&mut self, settings: &Settings) -> Result<(), ConfigError>;
}

/// `settings.toml` in the folder Verbatim runs from.
pub(crate) struct DiskFile(pub(crate) PathBuf);

impl SettingsFile for DiskFile {
    fn write(&mut self, settings: &Settings) -> Result<(), ConfigError> {
        ConfigStore::from_settings(&self.0, settings.clone()).save_settings()
    }
}

/// The saver thread's handle, for the main thread to wait at exit until
/// every save has been written.
pub(crate) struct Saver {
    poster: Poster<Message>,
}

/// The speech section's writer, which the synth thread owns.
pub(crate) struct SpeechWriter(Poster<Message>);

/// The reader section's writer, which the reducer thread owns.
pub(crate) struct ReaderWriter(Poster<Message>);

/// The theme section's writer, which the GUI thread owns.
pub(crate) struct ThemeWriter(Poster<Message>);

impl SpeechWriter {
    /// Hands the saver the speech section's new value.
    pub(crate) fn save(&self, speech: SpeechConfig) {
        post(&self.0, Message::Speech(speech));
    }
}

impl ReaderWriter {
    /// Hands the saver the reader section's new value.
    pub(crate) fn save(&self, reader: ReaderSettings) {
        post(&self.0, Message::Reader(reader));
    }
}

impl ThemeWriter {
    /// Hands the saver the theme section's new value.
    pub(crate) fn save(&self, theme: ThemeConfig) {
        post(&self.0, Message::Theme(theme));
    }
}

fn post(poster: &Poster<Message>, message: Message) {
    if poster.send(message).is_err() {
        tracing::warn!("the settings saver has stopped; a change is not saved");
    }
}

impl Saver {
    /// Starts the saver over `settings`, the whole file as loaded at
    /// startup, writing to `file`, and returns it with the one writer of
    /// each section.
    ///
    /// # Errors
    ///
    /// Returns the error making the port or starting the thread.
    pub(crate) fn start(
        file: impl SettingsFile,
        settings: Settings,
    ) -> io::Result<(Self, SpeechWriter, ReaderWriter, ThemeWriter)> {
        let (poster, queue) = port_queue::<Message>()?;
        thread::Builder::new()
            .name("verbatim-settings-saver".to_owned())
            .spawn(move || run(queue, file, settings))?;
        Ok((
            Self {
                poster: poster.clone(),
            },
            SpeechWriter(poster.clone()),
            ReaderWriter(poster.clone()),
            ThemeWriter(poster),
        ))
    }

    /// Waits until everything posted before this call has been written,
    /// or until `deadline`. Returns `false` when the deadline passed first.
    pub(crate) fn finish(&self, deadline: Instant) -> bool {
        let (done_tx, done_rx) = bounded(1);
        self.poster.send(Message::Finish(done_tx)).is_ok()
            && done_rx.recv_deadline(deadline).is_ok()
    }
}

/// The saver thread: takes everything waiting, keeps the newest value of
/// each section, writes the file once, then answers every finish taken.
fn run(mut queue: PortQueue<Message>, mut file: impl SettingsFile, mut settings: Settings) {
    while let Some(first) = queue.recv() {
        let mut changed = Changed::default();
        let mut finished = Vec::new();
        let mut next = Some(first);
        while let Some(message) = next {
            match message {
                Message::Speech(speech) => {
                    settings.speech = speech;
                    changed.speech = true;
                }
                Message::Reader(reader) => {
                    settings.reader = reader;
                    changed.reader = true;
                }
                Message::Theme(theme) => {
                    settings.theme = theme;
                    changed.theme = true;
                }
                Message::Finish(done) => finished.push(done),
            }
            next = queue.try_recv();
        }
        if changed.any()
            && let Err(error) = file.write(&settings)
        {
            changed.report(&error);
        }
        for done in finished {
            let _ = done.send(());
        }
    }
}

/// The sections one write carried.
#[derive(Default)]
struct Changed {
    speech: bool,
    reader: bool,
    theme: bool,
}

impl Changed {
    fn any(&self) -> bool {
        self.speech || self.reader || self.theme
    }

    /// Logs a failed write, for each section it carried.
    fn report(&self, error: &ConfigError) {
        if self.speech {
            tracing::warn!(%error, "could not save the settings");
        }
        if self.reader {
            tracing::warn!(%error, "the reader settings could not be saved");
        }
        if self.theme {
            tracing::warn!(%error, "could not save the theme settings");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    /// What the test file has seen: every write, and whether the next one
    /// is held until released.
    #[derive(Default)]
    struct Seen {
        writes: Vec<Settings>,
        hold: bool,
        /// A write is under way and held.
        holding: bool,
    }

    /// A file that records each write and can hold one open.
    #[derive(Clone, Default)]
    struct TestFile(Arc<(Mutex<Seen>, Condvar)>);

    impl SettingsFile for TestFile {
        fn write(&mut self, settings: &Settings) -> Result<(), ConfigError> {
            let (seen, changed) = &*self.0;
            let mut seen = seen.lock().unwrap();
            seen.holding = seen.hold;
            changed.notify_all();
            seen = changed.wait_while(seen, |seen| seen.hold).unwrap();
            seen.holding = false;
            seen.writes.push(settings.clone());
            changed.notify_all();
            Ok(())
        }
    }

    impl TestFile {
        fn hold(&self) {
            self.0.0.lock().unwrap().hold = true;
        }

        fn release(&self) {
            self.0.0.lock().unwrap().hold = false;
            self.0.1.notify_all();
        }

        /// Waits until a write is under way and held.
        fn wait_holding(&self) {
            let (seen, changed) = &*self.0;
            let (_seen, wait) = changed
                .wait_timeout_while(seen.lock().unwrap(), WAIT, |seen| !seen.holding)
                .unwrap();
            assert!(!wait.timed_out(), "a write is held within {WAIT:?}");
        }

        fn writes(&self) -> Vec<Settings> {
            self.0.0.lock().unwrap().writes.clone()
        }
    }

    fn speech(synthesizer: &str) -> SpeechConfig {
        SpeechConfig {
            synthesizer: Some(synthesizer.to_owned()),
            ..SpeechConfig::default()
        }
    }

    fn reader() -> ReaderSettings {
        ReaderSettings {
            follow_caret: !ReaderSettings::default().follow_caret,
            ..ReaderSettings::default()
        }
    }

    #[test]
    fn a_section_posted_while_a_write_is_held_returns_at_once_and_is_written_next() {
        let file = TestFile::default();
        let (saver, speech_writer, reader_writer, _theme_writer) =
            Saver::start(file.clone(), Settings::default()).unwrap();
        file.hold();
        speech_writer.save(speech("onecore"));
        file.wait_holding();

        // The reader section's save is a post: it returns while the
        // speech section's write is held on the disk.
        reader_writer.save(reader());
        file.release();
        assert!(saver.finish(Instant::now() + WAIT), "the saver finishes");

        let first = Settings {
            speech: speech("onecore"),
            ..Settings::default()
        };
        let second = Settings {
            reader: reader(),
            ..first.clone()
        };
        assert_eq!(file.writes(), [first, second]);
    }

    #[test]
    fn sections_posted_while_a_write_is_held_are_written_in_one_write() {
        let file = TestFile::default();
        let (saver, speech_writer, reader_writer, theme_writer) =
            Saver::start(file.clone(), Settings::default()).unwrap();
        file.hold();
        speech_writer.save(speech("espeak"));
        file.wait_holding();

        let theme = ThemeConfig {
            id: "quiet".to_owned(),
            ..ThemeConfig::default()
        };
        speech_writer.save(speech("onecore"));
        reader_writer.save(reader());
        theme_writer.save(theme.clone());
        file.release();
        assert!(saver.finish(Instant::now() + WAIT), "the saver finishes");

        let first = Settings {
            speech: speech("espeak"),
            ..Settings::default()
        };
        let second = Settings {
            speech: speech("onecore"),
            reader: reader(),
            theme,
            ..Settings::default()
        };
        assert_eq!(file.writes(), [first, second]);
    }
}
