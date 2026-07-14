//! Data-driven synthesizer settings, mirroring NVDA's driver-setting model
//! (`nvda/source/autoSettingsUtils/driverSetting.py`) so the settings GUI
//! generates its controls from descriptors and future synths get controls
//! for free.

use serde::{Deserialize, Serialize};

/// Stable identifier of a synthesizer, used in config and the synth list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SynthId(pub String);

impl SynthId {
    /// Wraps an identifier string such as `onecore`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for SynthId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One entry in the synthesizer list: id plus display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthChoice {
    /// Stable identifier.
    pub id: SynthId,
    /// Human-readable name shown in the Select Synthesizer dialog.
    pub display_name: String,
}

/// Stable identifier of one driver setting, such as `rate` or `voice`.
///
/// The conventional ids — `voice`, `variant`, `rate`, `rate-boost`, `pitch`,
/// `inflection`, `volume` — match NVDA's factory settings so persisted
/// values stay portable across drivers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingId(pub String);

impl SettingId {
    /// Wraps an identifier string such as `rate`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for SettingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The value of one driver setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingValue {
    /// Value of a [`SettingDescriptor::Numeric`] setting.
    Number(i32),
    /// Selected option id of a [`SettingDescriptor::Choice`] setting.
    Choice(String),
    /// Value of a [`SettingDescriptor::Toggle`] setting.
    Toggle(bool),
}

/// Describes one driver setting so the GUI can generate a control for it.
///
/// `label_key` fields are Fluent message ids resolved through
/// `verbatim-i18n`, never display text — the localization rule applies to
/// descriptors too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingDescriptor {
    /// A numeric setting rendered as a slider. NVDA convention: 0 to 100
    /// with small step 1, normal step 5, large step 10.
    Numeric {
        /// Stable setting id.
        id: SettingId,
        /// Fluent message id of the control label.
        label_key: String,
        /// Smallest value.
        min: i32,
        /// Largest value.
        max: i32,
        /// Arrow-key step.
        small_step: i32,
        /// Step used by the future settings ring.
        normal_step: i32,
        /// Page-up and page-down step.
        large_step: i32,
    },
    /// A string setting with enumerated options, rendered as a combo box —
    /// voice and variant.
    Choice {
        /// Stable setting id.
        id: SettingId,
        /// Fluent message id of the control label.
        label_key: String,
        /// Option pairs of stable option id and display name, in display
        /// order. Option display names come from the driver (voice names)
        /// and are not localized further.
        options: Vec<(String, String)>,
    },
    /// A boolean setting rendered as a check box — rate boost.
    Toggle {
        /// Stable setting id.
        id: SettingId,
        /// Fluent message id of the control label.
        label_key: String,
    },
}

impl SettingDescriptor {
    /// A numeric descriptor with the NVDA-standard 0 to 100 range and steps.
    #[must_use]
    pub fn standard_numeric(id: impl Into<String>, label_key: impl Into<String>) -> Self {
        Self::Numeric {
            id: SettingId::new(id),
            label_key: label_key.into(),
            min: 0,
            max: 100,
            small_step: 1,
            normal_step: 5,
            large_step: 10,
        }
    }

    /// The setting's stable id.
    #[must_use]
    pub fn id(&self) -> &SettingId {
        match self {
            Self::Numeric { id, .. } | Self::Choice { id, .. } | Self::Toggle { id, .. } => id,
        }
    }

    /// The Fluent message id of the control label.
    #[must_use]
    pub fn label_key(&self) -> &str {
        match self {
            Self::Numeric { label_key, .. }
            | Self::Choice { label_key, .. }
            | Self::Toggle { label_key, .. } => label_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_numeric_uses_nvda_range_and_steps() {
        let descriptor = SettingDescriptor::standard_numeric("rate", "setting-rate");
        let SettingDescriptor::Numeric {
            min,
            max,
            small_step,
            normal_step,
            large_step,
            ..
        } = descriptor
        else {
            panic!("expected numeric descriptor");
        };
        assert_eq!((min, max), (0, 100));
        assert_eq!((small_step, normal_step, large_step), (1, 5, 10));
    }

    #[test]
    fn descriptor_accessors_reach_every_variant() {
        let toggle = SettingDescriptor::Toggle {
            id: SettingId::new("rate-boost"),
            label_key: "setting-rate-boost".into(),
        };
        assert_eq!(toggle.id().0, "rate-boost");
        assert_eq!(toggle.label_key(), "setting-rate-boost");
    }
}
