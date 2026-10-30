//! The [`SpeechSettingsHost`] implementation backed by the running
//! [`SpeechManager`](crate::SpeechManager), and the mirror it reads
//! (`docs/design/ownership-output.md`, "The mirror follows the synth
//! thread").
//!
//! The synth thread owns the driver's setting values, the committed values
//! and whether the active synthesizer is the user's choice, and it alone
//! writes the mirror: after each change, commit, revert or switch it reads
//! every value back from the driver, so the mirror holds what the driver
//! has, side effects included. The host reads the mirror without a thread
//! hop and posts every change, commit, revert and switch to the speech
//! queue, which passes them to the synth thread in order; a change's
//! answer, numbered as the change was, goes to the settings GUI as well as
//! into the mirror. Persistence is the synth thread's too, through the
//! injected [`SpeechSettingsStore`]; this crate never depends on the config
//! layer.

use std::sync::{Arc, Mutex, PoisonError};

use crate::driver::SynthError;
use crate::inbox::SpeechInbox;
use crate::manager::QueueEvent;
use crate::settings::{SettingDescriptor, SettingId, SettingValue, SynthChoice, SynthId};
use crate::{SettingsAnswer, SettingsAnswered, SpeechSettingsHost, SwitchDone};

/// The speech section of the settings, owned by the synth thread
/// (`docs/design/ownership-output.md`, "One saver, one writer per
/// section"): what a synthesizer starts with, and where a commit saves.
pub trait SpeechSettingsStore: Send {
    /// The saved setting values of `synth`, applied whenever it starts, at
    /// startup or on a switch. A value the synthesizer refuses is skipped
    /// with a warning, and the synthesizer keeps its own value for it.
    fn saved(&self, synth: &SynthId) -> Vec<(SettingId, SettingValue)>;

    /// Saves `values`, read from the driver, as `synth`'s, and `synth` as
    /// the configured synthesizer when `chosen`: it is not the user's choice
    /// when it was started in place of a configured synthesizer that could
    /// not start, whose choice is kept, to be tried again at the next
    /// start, as NVDA does. A later [`saved`](Self::saved) sees them.
    fn persist(&mut self, synth: &SynthId, chosen: bool, values: &[(SettingId, SettingValue)]);
}

/// What the settings GUI reads: the active synthesizer's settings as the
/// synth thread last read them from the driver. Written only by the synth
/// thread; a reader copies what it needs under the lock, which is never
/// held across a call.
pub(crate) struct Mirror {
    state: Mutex<MirrorState>,
    /// Where the synth thread's answers go, once the GUI has said.
    answered: Mutex<Option<Answered>>,
}

/// Where answers go, shared so it is called outside the mirror's lock.
type Answered = Arc<dyn Fn(SettingsAnswer) + Send + Sync>;

#[derive(Default)]
struct MirrorState {
    synths: Vec<SynthChoice>,
    active: Option<SynthChoice>,
    descriptors: Vec<SettingDescriptor>,
    values: Vec<(SettingId, SettingValue)>,
}

impl Mirror {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(MirrorState::default()),
            answered: Mutex::new(None),
        }
    }

    fn read<R>(&self, read: impl FnOnce(&MirrorState) -> R) -> R {
        read(&self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn write(&self, write: impl FnOnce(&mut MirrorState)) {
        write(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// The synth thread: `active` is now the active synthesizer, among
    /// `synths`, with these descriptors and values.
    pub(crate) fn set_driver(
        &self,
        synths: Vec<SynthChoice>,
        active: SynthChoice,
        descriptors: Vec<SettingDescriptor>,
        values: Vec<(SettingId, SettingValue)>,
    ) {
        self.write(|state| {
            state.synths = synths;
            state.active = Some(active);
            state.descriptors = descriptors;
            state.values = values;
        });
    }

    /// The synth thread: the driver's values, as just read from it.
    pub(crate) fn set_values(&self, values: Vec<(SettingId, SettingValue)>) {
        self.write(|state| state.values = values);
    }

    /// The synth thread: answers change `change` with the driver's
    /// `values`, into the mirror and to the GUI.
    pub(crate) fn answer(&self, change: u64, values: Vec<(SettingId, SettingValue)>) {
        self.set_values(values.clone());
        let answered = self
            .answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(answered) = answered {
            answered(SettingsAnswer { change, values });
        }
    }
}

/// A cloneable settings-GUI handle over the pipeline.
///
/// Every clone shares one mirror and one inbox to the pipeline.
#[derive(Clone)]
pub struct SettingsHost {
    mirror: Arc<Mirror>,
    queue_tx: Arc<dyn SpeechInbox>,
}

impl SettingsHost {
    pub(crate) fn new(queue_tx: Arc<dyn SpeechInbox>, mirror: Arc<Mirror>) -> Self {
        Self { mirror, queue_tx }
    }

    /// Posts `event` to the speech queue.
    fn post(&self, event: QueueEvent) -> Result<(), SynthError> {
        crate::inbox::post(&*self.queue_tx, event)
            .map_err(|_| SynthError::Unavailable("speech pipeline stopped".to_owned()))
    }
}

/// Validates a value against the descriptor with the given id, returning the
/// descriptor-appropriate [`SynthError::Setting`] when it does not fit.
pub(crate) fn validate(
    descriptors: &[SettingDescriptor],
    id: &SettingId,
    value: &SettingValue,
) -> Result<(), SynthError> {
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.id() == id)
        .ok_or_else(|| SynthError::Setting(format!("unknown setting {id}")))?;
    match (descriptor, value) {
        (SettingDescriptor::Numeric { min, max, .. }, SettingValue::Number(number)) => {
            if (*min..=*max).contains(number) {
                Ok(())
            } else {
                Err(SynthError::Setting(format!(
                    "setting {id} value {number} outside {min}..={max}"
                )))
            }
        }
        (SettingDescriptor::Choice { options, .. }, SettingValue::Choice(choice)) => {
            if options.iter().any(|(option_id, _)| option_id == choice) {
                Ok(())
            } else {
                Err(SynthError::Setting(format!(
                    "setting {id} has no option {choice}"
                )))
            }
        }
        (SettingDescriptor::Toggle { .. }, SettingValue::Toggle(_)) => Ok(()),
        _ => Err(SynthError::Setting(format!(
            "setting {id} value has the wrong type"
        ))),
    }
}

impl SpeechSettingsHost for SettingsHost {
    fn synthesizers(&self) -> Vec<SynthChoice> {
        self.mirror.read(|state| state.synths.clone())
    }

    fn active_synthesizer(&self) -> SynthChoice {
        self.mirror.read(|state| {
            state.active.clone().unwrap_or_else(|| SynthChoice {
                id: SynthId::new(""),
                display_name: String::new(),
            })
        })
    }

    fn switch_synthesizer(&self, id: &SynthId, done: SwitchDone) {
        if let Err(QueueEvent::SwitchSynth { reply, .. }) = crate::inbox::post(
            &*self.queue_tx,
            QueueEvent::SwitchSynth {
                id: id.clone(),
                reply: done,
            },
        ) {
            reply(Err(SynthError::Unavailable(
                "speech pipeline stopped".to_owned(),
            )));
        }
    }

    fn setting_descriptors(&self) -> Vec<SettingDescriptor> {
        self.mirror.read(|state| state.descriptors.clone())
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        self.mirror.read(|state| {
            state
                .values
                .iter()
                .find(|(existing, _)| existing == id)
                .map(|(_, value)| value.clone())
        })
    }

    fn set_setting(
        &self,
        id: &SettingId,
        value: SettingValue,
        change: u64,
    ) -> Result<(), SynthError> {
        self.mirror
            .read(|state| validate(&state.descriptors, id, &value))?;
        self.post(QueueEvent::ApplySetting {
            id: id.clone(),
            value,
            change,
        })
    }

    fn commit(&self) -> Result<(), SynthError> {
        self.post(QueueEvent::Commit)
    }

    fn revert(&self) {
        let _ = self.post(QueueEvent::Revert);
    }

    fn answer_to(&self, answered: SettingsAnswered) {
        *self
            .mirror
            .answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(answered));
    }
}
