//! The speech section of the settings as the synth thread owns it
//! (`docs/design/ownership-output.md`, "One saver, one writer per
//! section"): read whenever a synthesizer starts, and saved through the
//! section's writer on a commit, with the values the driver has.

use verbatim_config::{ConfigValue, SpeechConfig};
use verbatim_speech::{SettingId, SettingValue, SpeechSettingsStore, SynthId};

use crate::saver::SpeechWriter;

/// The speech section and its writer, moved to the synth thread.
pub(crate) struct SpeechSettings {
    section: SpeechConfig,
    writer: SpeechWriter,
}

impl SpeechSettings {
    /// The speech section as loaded at startup, saved through `writer`.
    pub(crate) fn new(section: SpeechConfig, writer: SpeechWriter) -> Self {
        Self { section, writer }
    }
}

impl SpeechSettingsStore for SpeechSettings {
    /// Saved setting values for a synthesizer, mapped from config values to
    /// driver values.
    fn saved(&self, synth: &SynthId) -> Vec<(SettingId, SettingValue)> {
        self.section
            .synth_settings
            .get(&synth.0)
            .into_iter()
            .flatten()
            .filter_map(|(id, value)| {
                let value = match value {
                    ConfigValue::Integer(number) => {
                        SettingValue::Number(i32::try_from(*number).ok()?)
                    }
                    ConfigValue::Text(option) => SettingValue::Choice(option.clone()),
                    ConfigValue::Flag(flag) => SettingValue::Toggle(*flag),
                };
                Some((SettingId::new(id.clone()), value))
            })
            .collect()
    }

    /// Writes the synthesizer's values into the section, and the
    /// synthesizer as the configured one when it is the user's choice
    /// rather than a fallback, and hands the section to the saver.
    fn persist(&mut self, synth: &SynthId, chosen: bool, values: &[(SettingId, SettingValue)]) {
        if chosen {
            self.section.synthesizer = Some(synth.0.clone());
        }
        let settings = self
            .section
            .synth_settings
            .entry(synth.0.clone())
            .or_default();
        for (id, value) in values {
            let config_value = match value {
                SettingValue::Number(number) => ConfigValue::Integer(i64::from(*number)),
                SettingValue::Choice(option) => ConfigValue::Text(option.clone()),
                SettingValue::Toggle(flag) => ConfigValue::Flag(*flag),
            };
            settings.insert(id.0.clone(), config_value);
        }
        self.writer.save(self.section.clone());
    }
}
