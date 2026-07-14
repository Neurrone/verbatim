//! Runs the settings dialog against an in-memory mock speech host so the GUI
//! can be inspected by hand (and read by Verbatim itself over UIA).
//!
//! This opens real windows, so it is not run by `cargo test`; the lead session
//! runs it manually. On launch it posts `OpenSettings` so the dialog appears
//! immediately.

use std::sync::{Arc, Mutex};

use verbatim_gui::{GuiCommand, GuiEvent, run_gui};
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

fn main() {
    let host: Arc<dyn SpeechSettingsHost> = Arc::new(MockHost::new());
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
            }
        }
    });

    run_gui(host, events_tx, move |handle| {
        *handle_slot.lock().unwrap() = Some(handle.clone());
        handle.send(GuiCommand::OpenSettings);
    })
    .expect("GUI ran");
}
