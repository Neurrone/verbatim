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
mod manager;
mod registry;
mod settings;
mod theme;
mod trim;

pub use driver::{
    IndexMark, SoundCue, SpeechItem, SpeechSequence, SynthDriver, SynthError, SynthSink,
};
pub use events::SpeechEvents;
pub use host::{PersistFn, SettingsHost};
pub use manager::{SavedSettingsFn, SpeechControl, SpeechManager, SpeechManagerConfig};
pub use registry::{SynthFactory, SynthRegistry};
pub use settings::{SettingDescriptor, SettingId, SettingValue, SynthChoice, SynthId};
pub use theme::{ActiveTheme, Presenter, ThemeHandle, ThemePresenter};

/// Takes the outcome of [`SpeechSettingsHost::switch_synthesizer`].
pub type SwitchDone = Box<dyn FnOnce(Result<(), SynthError>) + Send>;

/// The live handle the settings GUI uses to inspect and adjust speech.
///
/// Set calls apply immediately to the running synthesizer so slider drags
/// are audible as they happen (NVDA behavior); `commit` persists the current
/// values to the base profile, and `revert` restores the last committed
/// values — the Cancel path.
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

    /// The current value of one setting of the active synthesizer.
    fn setting(&self, id: &SettingId) -> Option<SettingValue>;

    /// Applies a setting to the running synthesizer immediately, without
    /// persisting it.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Setting`] when the value is out of range or
    /// unknown to the driver.
    fn set_setting(&self, id: &SettingId, value: SettingValue) -> Result<(), SynthError>;

    /// Persists the active synthesizer and its current setting values to the
    /// base profile.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Setting`] when writing the profile fails.
    fn commit(&self) -> Result<(), SynthError>;

    /// Restores the last committed synthesizer and setting values,
    /// discarding uncommitted live changes.
    fn revert(&self);
}
