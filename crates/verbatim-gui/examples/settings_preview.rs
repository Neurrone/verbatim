//! Runs the settings dialog against an in-memory mock speech host, a
//! theme host over a temporary themes folder and the repository's sounds,
//! and a terminal host holding the reader settings in memory, so the GUI can be inspected by hand (and read by Verbatim itself over
//! UIA).
//!
//! This opens real windows, so it is not run by `cargo test`; the lead session
//! runs it manually. On launch it posts `OpenSettings` so the dialog appears
//! immediately.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use verbatim_audio::Sound;
use verbatim_config::themes::LoadedTheme;
use verbatim_gui::{GuiCommand, GuiEvent, TerminalChange, TerminalHost, ThemeHost, run_gui};
use verbatim_model::{Earcon, ReaderSettings, Theme, ThemeOptions, Utterance};
use verbatim_speech::{
    SettingDescriptor, SettingId, SettingValue, SpeechSettingsHost, SynthChoice, SynthError,
    SynthId,
};

/// A mock host holding two synthesizers and a handful of settings in memory.
struct MockHost {
    inner: Mutex<Inner>,
}

struct Inner {
    active: SynthId,
    values: Vec<(SettingId, SettingValue)>,
    committed: Vec<(SettingId, SettingValue)>,
}

impl MockHost {
    fn new() -> Self {
        let values = default_values();
        Self {
            inner: Mutex::new(Inner {
                active: SynthId::new("onecore"),
                committed: values.clone(),
                values,
            }),
        }
    }
}

fn default_values() -> Vec<(SettingId, SettingValue)> {
    vec![
        (
            SettingId::new("voice"),
            SettingValue::Choice("hazel".into()),
        ),
        (SettingId::new("rate"), SettingValue::Number(50)),
        (SettingId::new("pitch"), SettingValue::Number(50)),
        (SettingId::new("rate-boost"), SettingValue::Toggle(false)),
    ]
}

impl SpeechSettingsHost for MockHost {
    fn synthesizers(&self) -> Vec<SynthChoice> {
        vec![
            SynthChoice {
                id: SynthId::new("onecore"),
                display_name: "Windows OneCore".into(),
            },
            SynthChoice {
                id: SynthId::new("espeak"),
                display_name: "eSpeak NG".into(),
            },
        ]
    }

    fn active_synthesizer(&self) -> SynthChoice {
        let active = self.inner.lock().unwrap().active.clone();
        self.synthesizers()
            .into_iter()
            .find(|synth| synth.id == active)
            .unwrap()
    }

    fn set_active_synthesizer(&self, id: &SynthId) -> Result<(), SynthError> {
        self.inner.lock().unwrap().active = id.clone();
        Ok(())
    }

    fn setting_descriptors(&self) -> Vec<SettingDescriptor> {
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".into(),
                options: vec![
                    ("hazel".into(), "Microsoft Hazel".into()),
                    ("david".into(), "Microsoft David".into()),
                ],
            },
            SettingDescriptor::standard_numeric("rate", "setting-rate"),
            SettingDescriptor::standard_numeric("pitch", "setting-pitch"),
            SettingDescriptor::Toggle {
                id: SettingId::new("rate-boost"),
                label_key: "setting-rate-boost".into(),
            },
        ]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        self.inner
            .lock()
            .unwrap()
            .values
            .iter()
            .find(|(setting_id, _)| setting_id == id)
            .map(|(_, value)| value.clone())
    }

    fn set_setting(&self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner
            .values
            .iter_mut()
            .find(|(setting_id, _)| setting_id == id)
        {
            entry.1 = value;
        } else {
            inner.values.push((id.clone(), value));
        }
        Ok(())
    }

    fn commit(&self) -> Result<(), SynthError> {
        let mut inner = self.inner.lock().unwrap();
        let snapshot = inner.values.clone();
        inner.committed = snapshot;
        Ok(())
    }

    fn revert(&self) {
        let mut inner = self.inner.lock().unwrap();
        let snapshot = inner.committed.clone();
        inner.values = snapshot;
    }
}

/// A theme host that keeps its themes in a temporary folder and prints what
/// it is asked to do instead of speaking.
struct MockThemeHost {
    themes_dir: PathBuf,
}

impl ThemeHost for MockThemeHost {
    fn themes_dir(&self) -> PathBuf {
        self.themes_dir.clone()
    }

    fn sounds_dir(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sounds")
    }

    fn configured(&self) -> (String, ThemeOptions) {
        (Theme::DEFAULT_ID.to_owned(), ThemeOptions::default())
    }

    fn activate(&self, theme: &LoadedTheme, options: ThemeOptions) {
        println!("theme {} active with {options:?}", theme.theme.id);
    }

    fn set_options(&self, options: ThemeOptions) {
        println!("theme options {options:?}");
    }

    fn persist(&self, id: &str, options: ThemeOptions) -> Result<(), String> {
        println!("theme {id} saved with {options:?}");
        Ok(())
    }

    fn play(&self, sound: &Sound, gain: f32) {
        println!("play a sound of {:?} at {gain}", sound.duration());
    }

    fn speak(&self, utterance: Utterance) {
        println!("speak {:?}", utterance.segments);
    }

    fn play_earcon(&self, earcon: Earcon) {
        println!("event {earcon:?}");
    }
}

/// A terminal host holding the reader settings in memory and printing
/// each change.
struct MockTerminalHost {
    settings: Mutex<ReaderSettings>,
}

impl TerminalHost for MockTerminalHost {
    fn reader_settings(&self) -> ReaderSettings {
        *self.settings.lock().unwrap()
    }

    fn change(&self, change: TerminalChange) {
        println!("terminal settings {change:?}");
        change.apply_to(&mut self.settings.lock().unwrap());
    }
}

fn main() {
    let host: Arc<dyn SpeechSettingsHost> = Arc::new(MockHost::new());
    let theme_host: Arc<dyn ThemeHost> = Arc::new(MockThemeHost {
        themes_dir: std::env::temp_dir().join("verbatim-settings-preview-themes"),
    });
    let terminal_host: Arc<dyn TerminalHost> = Arc::new(MockTerminalHost {
        settings: Mutex::new(ReaderSettings::default()),
    });
    let (events_tx, events_rx) = crossbeam_channel::unbounded::<GuiEvent>();

    // Drain GuiEvents on a background thread; on QuitRequested, ask the GUI to
    // shut down (mirroring what the real app composition root does).
    let handle_slot = Arc::new(Mutex::new(None));
    let handle_slot_bg = handle_slot.clone();
    std::thread::spawn(move || {
        for event in events_rx {
            match event {
                GuiEvent::QuitRequested => {
                    if let Some(handle) = handle_slot_bg.lock().unwrap().as_ref() {
                        let handle: &verbatim_gui::GuiHandle = handle;
                        handle.send(GuiCommand::Shutdown);
                    }
                }
                // The preview opens no shell item list.
                GuiEvent::ShellItemGone(_) => {}
            }
        }
    });

    run_gui(host, theme_host, terminal_host, events_tx, move |handle| {
        *handle_slot.lock().unwrap() = Some(handle.clone());
        handle.send(GuiCommand::OpenSettings);
    })
    .expect("GUI ran");
}
