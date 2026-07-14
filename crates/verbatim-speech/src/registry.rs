//! The synth driver registry: named factories the pipeline builds active
//! drivers from, and switches between at runtime.
//!
//! Registration records an id and display name alongside the factory, so the
//! synthesizer list can be shown without constructing every driver.

use crate::driver::{SynthDriver, SynthError};
use crate::settings::{SynthChoice, SynthId};

/// Builds a fresh driver instance on demand.
///
/// Invoked on the pipeline's synth thread, so a driver may touch
/// thread-affine resources during construction.
///
/// # Errors
///
/// Returns [`SynthError::Unavailable`] when the synthesizer cannot be
/// initialized.
pub type SynthFactory = Box<dyn Fn() -> Result<Box<dyn SynthDriver>, SynthError> + Send>;

struct RegisteredSynth {
    id: SynthId,
    display_name: String,
    factory: SynthFactory,
}

/// A set of synth driver factories keyed by [`SynthId`], in registration
/// (display) order.
#[derive(Default)]
pub struct SynthRegistry {
    entries: Vec<RegisteredSynth>,
}

impl SynthRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Registers a factory under `id` with the given display name.
    ///
    /// A later registration with the same id replaces the earlier one, so a
    /// composition root can override a default driver.
    pub fn register(
        &mut self,
        id: SynthId,
        display_name: impl Into<String>,
        factory: SynthFactory,
    ) {
        let display_name = display_name.into();
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.id == id) {
            existing.display_name = display_name;
            existing.factory = factory;
        } else {
            self.entries.push(RegisteredSynth {
                id,
                display_name,
                factory,
            });
        }
    }

    /// The registered synthesizers as choices, in display order.
    #[must_use]
    pub fn choices(&self) -> Vec<SynthChoice> {
        self.entries
            .iter()
            .map(|entry| SynthChoice {
                id: entry.id.clone(),
                display_name: entry.display_name.clone(),
            })
            .collect()
    }

    /// Whether a synthesizer with this id is registered.
    #[must_use]
    pub fn contains(&self, id: &SynthId) -> bool {
        self.entries.iter().any(|entry| &entry.id == id)
    }

    /// Builds the driver registered under `id`.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when no driver is registered under
    /// `id`, or when the factory itself fails to initialize the driver.
    pub fn build(&self, id: &SynthId) -> Result<Box<dyn SynthDriver>, SynthError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| &entry.id == id)
            .ok_or_else(|| SynthError::Unavailable(format!("no synthesizer registered as {id}")))?;
        (entry.factory)()
    }
}
