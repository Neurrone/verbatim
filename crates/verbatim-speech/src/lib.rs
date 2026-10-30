//! Speech pipeline (architecture section 6).
//!
//! Stages in order: utterance, presentation (the active theme flattens it
//! to a [`SpeechSequence`] of words and sounds), silence trimming, synth
//! driver, PCM, and the audio mixer, which plays the sounds at their places
//! and reports when each utterance is heard (decision D17). The
//! seams are the synchronous [`SynthDriver`] contract, the data-driven
//! [`SettingDescriptor`] model mirroring NVDA's driver settings, the
//! [`SpeechEvents`] observer, and the [`SpeechSettingsHost`] handle the GUI
//! talks to.

#![forbid(unsafe_code)]

mod driver;
mod events;
mod host;
pub mod hosting;
mod inbox;
mod manager;
mod registry;
mod settings;
mod theme;
mod trim;

pub use driver::{
    CancelToken, CancelWake, IndexMark, SoundCue, SpeechItem, SpeechSequence, SynthDriver,
    SynthError, SynthSink, SynthTiming,
};
pub use events::SpeechEvents;
pub use host::{SettingsHost, SpeechSettingsStore};
pub use inbox::{QueueItem, SpeechInbox, SpeechInboxTaker, channel_inbox};
pub use manager::{EarconHeard, SpeakToken, SpeechControl, SpeechManager, SpeechManagerConfig};
pub use registry::{SynthFactory, SynthRegistry};
pub use settings::{SettingDescriptor, SettingId, SettingValue, SynthChoice, SynthId};
pub use theme::{ActiveTheme, Presenter, ThemeHandle, ThemePresenter};

/// Takes the outcome of [`SpeechSettingsHost::switch_synthesizer`].
pub type SwitchDone = Box<dyn FnOnce(Result<(), SynthError>) + Send>;

/// The synth thread's answer to a change or a revert: every value the
/// driver has, read back from it, and the number of the latest change the
/// synth thread had applied when it read them
/// ([`SpeechSettingsHost::set_setting`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsAnswer {
    /// The number of the latest change applied, 0 before any.
    pub change: u64,
    /// The driver's values, in descriptor order.
    pub values: Vec<(SettingId, SettingValue)>,
}

/// Takes each [`SettingsAnswer`], on the synth thread.
pub type SettingsAnswered = Box<dyn Fn(SettingsAnswer) + Send + Sync>;

/// The live handle the settings GUI uses to inspect and adjust speech
/// (`docs/design/ownership-output.md`, "The mirror follows the synth
/// thread").
///
/// Set calls apply to the running synthesizer as soon as the synth thread
/// takes them, so slider drags are audible as they happen (NVDA behavior);
/// `commit` saves what the driver then has to the base profile, and
/// `revert` restores the last committed values, the Cancel path. Each is
/// carried out on the synth thread, in order behind every change and
/// switch posted before it, and the values the getters return are the ones
/// the synth thread last read from the driver.
pub trait SpeechSettingsHost: Send + Sync {
    /// Every installed synthesizer, in display order.
    fn synthesizers(&self) -> Vec<SynthChoice>;

    /// The active synthesizer.
    fn active_synthesizer(&self) -> SynthChoice;

    /// Starts switching the active synthesizer to `id`, applying its
    /// persisted settings, and returns at once: nothing waits for the new
    /// synthesizer to start. `done` is called with the outcome, on another
    /// thread, once the switch has finished: `Ok` once the new synthesizer
    /// is active and this host describes it, or [`SynthError::Unavailable`]
    /// when it cannot be started, including a synthesizer host that does
    /// not answer within its time limit, and then the previous synthesizer
    /// stays active.
    fn switch_synthesizer(&self, id: &SynthId, done: SwitchDone);

    /// Setting descriptors for the active synthesizer, in display order.
    fn setting_descriptors(&self) -> Vec<SettingDescriptor>;

    /// The value of one setting of the active synthesizer, as the synth
    /// thread last read it from the driver.
    fn setting(&self, id: &SettingId) -> Option<SettingValue>;

    /// Applies a setting to the running synthesizer without persisting it,
    /// as change number `change`, a number the caller raises with each
    /// change it makes. The synth thread answers it with every value the
    /// driver then has ([`answer_to`](Self::answer_to)), so a refused or
    /// clamped value comes back as the driver's own.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Setting`] when the value is out of range or
    /// unknown to the driver's descriptors, and
    /// [`SynthError::Unavailable`] when the pipeline has stopped.
    fn set_setting(
        &self,
        id: &SettingId,
        value: SettingValue,
        change: u64,
    ) -> Result<(), SynthError>;

    /// Saves the active synthesizer and the values its driver has, once the
    /// synth thread has carried out everything posted before. During a
    /// switch it is carried out after the switch, so what is saved is the
    /// synthesizer the switch ended with.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when the pipeline has stopped.
    fn commit(&self) -> Result<(), SynthError>;

    /// Restores the active synthesizer's committed values, discarding
    /// uncommitted live changes, and answers with the driver's values.
    /// During a switch it is carried out after the switch: a synthesizer
    /// that started has its own saved values as its committed ones, and
    /// after a switch that failed the previous synthesizer's committed
    /// values are restored.
    fn revert(&self);

    /// Gives every answer of the synth thread to `answered`, from now on.
    fn answer_to(&self, answered: SettingsAnswered);
}
