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

use crossbeam_channel::Sender;

use crate::driver::SynthError;
use crate::manager::{DriverState, QueueEvent};
use crate::settings::{SettingDescriptor, SettingId, SettingValue, SynthChoice, SynthId};
use crate::{SpeechSettingsHost, SwitchDone};

/// Persists the active synthesizer's settings.
///
/// Called by [`commit`](SpeechSettingsHost::commit) with the active synth id,
/// whether it is the user's choice, and its current values. It is not the
/// user's choice when it was started in place of a configured synthesizer
/// that could not start: its settings are saved, but the configured choice
/// is kept, to be tried again at the next start, as NVDA does. Returns a
/// human-readable message on failure, which the host surfaces as
/// [`SynthError::Setting`].
pub type PersistFn =
    Box<dyn Fn(&SynthId, bool, &[(SettingId, SettingValue)]) -> Result<(), String> + Send + Sync>;

/// The shared, cloneable form of [`PersistFn`] the host stores.
type PersistArc =
    Arc<dyn Fn(&SynthId, bool, &[(SettingId, SettingValue)]) -> Result<(), String> + Send + Sync>;

/// The mirror of the active synthesizer's settings state.
struct HostState {
    synths: Vec<SynthChoice>,
    active: SynthChoice,
    /// The active synthesizer is the user's choice, not a fallback.
    chosen: bool,
    descriptors: Vec<SettingDescriptor>,
    current: Vec<(SettingId, SettingValue)>,
    committed: Vec<(SettingId, SettingValue)>,
    /// What a synthesizer switch under way owes once it ends; `None` when
    /// no switch is under way.
    switch: Option<Owed>,
}

/// What the end of a synthesizer switch owes for a commit or revert asked
/// for while it was under way.
#[derive(Clone, Copy, Default)]
struct Owed {
    /// A commit was made: the synthesizer the switch starts is persisted
    /// too, once it has started.
    commit: bool,
    /// A revert was asked for: carried out only if the switch fails, since
    /// the synthesizer it starts has its own values.
    revert: bool,
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
        chosen: bool,
        persist: PersistFn,
    ) -> Self {
        let state = HostState {
            synths,
            active: initial.choice.clone(),
            chosen,
            descriptors: initial.descriptors.clone(),
            current: initial.values.clone(),
            committed: initial.values.clone(),
            switch: None,
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

    /// A switch has ended with `outcome`: the mirror takes the new
    /// synthesizer when it started, and a commit or revert asked for during
    /// the switch is carried out for the synthesizer now active. NVDA's
    /// switch blocks its GUI, so its settings dialog can never be closed
    /// during one (`docs/parity.md`, "Settings dialog during a synthesizer
    /// switch"); this host's switch does not block, so a commit made
    /// meanwhile must still save what the switch ended with.
    fn switch_ended(&self, outcome: Result<DriverState, SynthError>) -> Result<(), SynthError> {
        let owed = {
            let mut guard = self.shared.lock().expect("settings mirror poisoned");
            let owed = guard.switch.take().unwrap_or_default();
            if let Ok(state) = &outcome {
                guard.active = state.choice.clone();
                guard.chosen = true;
                guard.descriptors.clone_from(&state.descriptors);
                guard.current.clone_from(&state.values);
                guard.committed.clone_from(&state.values);
            }
            owed
        };
        match outcome {
            Ok(_) => {
                if owed.commit
                    && let Err(error) = self.commit()
                {
                    tracing::warn!(target: "verbatim::speech", %error, "could not save the synthesizer the switch started");
                }
                Ok(())
            }
            Err(error) => {
                if owed.revert {
                    self.revert();
                }
                Err(error)
            }
        }
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

    fn switch_synthesizer(&self, id: &SynthId, done: SwitchDone) {
        {
            let mut guard = self.shared.lock().expect("settings mirror poisoned");
            guard.switch = Some(Owed::default());
        }
        let host = self.clone();
        // Runs on the synth thread once the switch has finished; the mirror
        // describes the new synthesizer before `done` hears of it, and a
        // commit or revert asked for meanwhile is settled by the outcome.
        let reply = Box::new(move |outcome: Result<DriverState, SynthError>| {
            let outcome = host.switch_ended(outcome);
            done(outcome);
        });
        if let Err(crossbeam_channel::SendError(QueueEvent::SwitchSynth { reply, .. })) =
            self.queue_tx.send(QueueEvent::SwitchSynth {
                id: id.clone(),
                reply,
            })
        {
            reply(Err(SynthError::Unavailable(
                "speech pipeline stopped".to_owned(),
            )));
        }
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
        let (active_id, chosen, current) =
            self.with_state(|state| (state.active.id.clone(), state.chosen, state.current.clone()));
        (self.persist)(&active_id, chosen, &current).map_err(SynthError::Setting)?;
        let mut guard = self.shared.lock().expect("settings mirror poisoned");
        let current = guard.current.clone();
        guard.committed = current;
        if let Some(owed) = &mut guard.switch {
            *owed = Owed {
                commit: true,
                revert: false,
            };
        }
        Ok(())
    }

    fn revert(&self) {
        let restored = {
            let mut guard = self.shared.lock().expect("settings mirror poisoned");
            let state = &mut *guard;
            if let Some(owed) = &mut state.switch {
                // Restored, if at all, when the switch ends: queued now, the
                // previous synthesizer's values would reach the new one.
                owed.revert = true;
                return;
            }
            state.current.clone_from(&state.committed);
            state.current.clone()
        };
        let _ = self.queue_tx.send(QueueEvent::RestoreSettings(restored));
    }
}
