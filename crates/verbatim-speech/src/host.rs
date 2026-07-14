//! The [`SpeechSettingsHost`] implementation backed by the running
//! [`SpeechManager`](crate::SpeechManager).
//!
//! The host keeps a mirror of the active synthesizer's descriptors and values
//! so the settings GUI reads without a thread hop; writes validate against the
//! mirror, then reach the driver on the synth thread so a slider drag is
//! audible as it happens. Persistence is delegated: `commit` calls the
//! injected callback, and `revert` restores the last committed values and
//! reapplies them to the driver. This crate never depends on the config layer.

use std::sync::{Arc, Mutex};

use crossbeam_channel::{Sender, bounded};

use crate::SpeechSettingsHost;
use crate::driver::SynthError;
use crate::manager::{DriverState, QueueEvent};
use crate::settings::{SettingDescriptor, SettingId, SettingValue, SynthChoice, SynthId};

/// Persists the active synthesizer's settings.
///
/// Called by [`commit`](SpeechSettingsHost::commit) with the active synth id
/// and its current values. Returns a human-readable message on failure, which
/// the host surfaces as [`SynthError::Setting`].
pub type PersistFn =
    Box<dyn Fn(&SynthId, &[(SettingId, SettingValue)]) -> Result<(), String> + Send + Sync>;

/// The shared, cloneable form of [`PersistFn`] the host stores.
type PersistArc =
    Arc<dyn Fn(&SynthId, &[(SettingId, SettingValue)]) -> Result<(), String> + Send + Sync>;

/// The mirror of the active synthesizer's settings state.
struct HostState {
    synths: Vec<SynthChoice>,
    active: SynthChoice,
    descriptors: Vec<SettingDescriptor>,
    current: Vec<(SettingId, SettingValue)>,
    committed: Vec<(SettingId, SettingValue)>,
}

/// A cloneable settings-GUI handle over the pipeline.
///
/// Every clone shares one mirror and one channel to the pipeline, so changes
/// made through any clone are seen by all.
#[derive(Clone)]
pub struct SettingsHost {
    shared: Arc<Mutex<HostState>>,
    queue_tx: Sender<QueueEvent>,
    persist: PersistArc,
}

impl SettingsHost {
    pub(crate) fn new(
        queue_tx: Sender<QueueEvent>,
        initial: &DriverState,
        synths: Vec<SynthChoice>,
        persist: PersistFn,
    ) -> Self {
        let state = HostState {
            synths,
            active: initial.choice.clone(),
            descriptors: initial.descriptors.clone(),
            current: initial.values.clone(),
            committed: initial.values.clone(),
        };
        Self {
            shared: Arc::new(Mutex::new(state)),
            queue_tx,
            persist: Arc::from(persist),
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&HostState) -> R) -> R {
        let guard = self.shared.lock().expect("settings mirror poisoned");
        f(&guard)
    }
}

/// Validates a value against the descriptor with the given id, returning the
/// descriptor-appropriate [`SynthError::Setting`] when it does not fit.
fn validate(
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

/// Inserts or replaces `id`'s value in a value list, preserving order.
fn upsert(values: &mut Vec<(SettingId, SettingValue)>, id: SettingId, value: SettingValue) {
    if let Some(slot) = values.iter_mut().find(|(existing, _)| *existing == id) {
        slot.1 = value;
    } else {
        values.push((id, value));
    }
}

impl SpeechSettingsHost for SettingsHost {
    fn synthesizers(&self) -> Vec<SynthChoice> {
        self.with_state(|state| state.synths.clone())
    }

    fn active_synthesizer(&self) -> SynthChoice {
        self.with_state(|state| state.active.clone())
    }

    fn set_active_synthesizer(&self, id: &SynthId) -> Result<(), SynthError> {
        let (reply_tx, reply_rx) = bounded::<Result<DriverState, SynthError>>(1);
        self.queue_tx
            .send(QueueEvent::SwitchSynth {
                id: id.clone(),
                reply: reply_tx,
            })
            .map_err(|_| SynthError::Unavailable("speech pipeline stopped".to_owned()))?;
        let state = reply_rx
            .recv()
            .map_err(|_| SynthError::Unavailable("speech pipeline stopped".to_owned()))??;

        let mut guard = self.shared.lock().expect("settings mirror poisoned");
        guard.active = state.choice;
        guard.descriptors = state.descriptors;
        guard.current.clone_from(&state.values);
        guard.committed = state.values;
        Ok(())
    }

    fn setting_descriptors(&self) -> Vec<SettingDescriptor> {
        self.with_state(|state| state.descriptors.clone())
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        self.with_state(|state| {
            state
                .current
                .iter()
                .find(|(existing, _)| existing == id)
                .map(|(_, value)| value.clone())
        })
    }

    fn set_setting(&self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        {
            let mut guard = self.shared.lock().expect("settings mirror poisoned");
            validate(&guard.descriptors, id, &value)?;
            upsert(&mut guard.current, id.clone(), value.clone());
        }
        // Apply to the running driver; it takes effect from the next speak.
        self.queue_tx
            .send(QueueEvent::ApplySetting {
                id: id.clone(),
                value,
            })
            .map_err(|_| SynthError::Unavailable("speech pipeline stopped".to_owned()))?;
        Ok(())
    }

    fn commit(&self) -> Result<(), SynthError> {
        let (active_id, current) =
            self.with_state(|state| (state.active.id.clone(), state.current.clone()));
        (self.persist)(&active_id, &current).map_err(SynthError::Setting)?;
        let mut guard = self.shared.lock().expect("settings mirror poisoned");
        let current = guard.current.clone();
        guard.committed = current;
        Ok(())
    }

    fn revert(&self) {
        let restored = {
            let mut guard = self.shared.lock().expect("settings mirror poisoned");
            let state = &mut *guard;
            state.current.clone_from(&state.committed);
            state.current.clone()
        };
        let _ = self.queue_tx.send(QueueEvent::RestoreSettings(restored));
    }
}
