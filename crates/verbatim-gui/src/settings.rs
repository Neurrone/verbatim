//! The settings dialog's model: its categories, the Speech page generated
//! from the settings host's descriptors, what a change to one of the
//! page's controls sets, and the Select Synthesizer dialog's choice.
//!
//! Everything here is pure apart from reading the host. The C++ layer
//! builds widgets from these models and reports changes back by control
//! index, which [`SpeechControls`] turns into setting values.

use verbatim_speech::{
    SettingDescriptor, SettingId, SettingValue, SettingsAnswer, SpeechSettingsHost, SynthChoice,
    SynthId,
};

use crate::bridge::ffi;
use crate::plan::accessible_name;

/// The settings dialog's categories and buttons: Speech, Theme, and
/// Terminal. The
/// dialog is written over the list, so adding a category is a matter of
/// adding it here and a page for its kind in C++.
pub(crate) fn dialog() -> ffi::SettingsDialog {
    let category = |name: String, kind| ffi::Category {
        title: verbatim_i18n::t!("settings-title-with-category", category = name.as_str()),
        name,
        kind,
    };
    ffi::SettingsDialog {
        categories: vec![
            category(
                verbatim_i18n::t!("settings-category-speech"),
                ffi::CategoryKind::Speech,
            ),
            category(
                verbatim_i18n::t!("settings-category-theme"),
                ffi::CategoryKind::Theme,
            ),
            category(
                verbatim_i18n::t!("settings-category-terminal"),
                ffi::CategoryKind::Terminal,
            ),
        ],
        categories_label: verbatim_i18n::t!("settings-categories-label"),
        ok: verbatim_i18n::t!("button-ok"),
        cancel: verbatim_i18n::t!("button-cancel"),
        apply: verbatim_i18n::t!("button-apply"),
    }
}

/// A change the user made to one of the Speech page's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlChange {
    /// A slider moved to this value.
    Number(i32),
    /// A combo box selected the option at this index.
    Option(usize),
    /// A check box was set or cleared.
    Toggle(bool),
}

/// The Speech page's current controls: the descriptors they were built from, so
/// a change reported by index becomes a setting value, and each change
/// sent and not yet answered by the synth thread
/// (`books/dev/src/explanation/design/ownership.md`).
#[derive(Debug, Default)]
pub(crate) struct SpeechControls {
    generation: u32,
    descriptors: Vec<SettingDescriptor>,
    /// The number of the last change sent.
    last_change: u64,
    /// Each setting changed and not yet answered: its latest change's
    /// number and the value it sent.
    outstanding: Vec<(SettingId, u64, SettingValue)>,
}

/// A control to set to the driver's value: control `control` of the page
/// built as `generation`, to `value` as [`GuiThread::set_speech_control`]
/// takes it.
///
/// [`GuiThread::set_speech_control`]: crate::bridge::GuiThread::set_speech_control
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Snap {
    pub(crate) generation: u32,
    pub(crate) control: usize,
    pub(crate) value: i32,
}

impl SpeechControls {
    /// Builds the Speech page from the host, replacing the current
    /// controls: changes reported for the previous set are ignored from
    /// now on, and so are answers to changes made through it.
    pub(crate) fn rebuild(&mut self, host: &dyn SpeechSettingsHost) -> ffi::SpeechPage {
        self.generation = self.generation.wrapping_add(1);
        self.outstanding.clear();
        self.descriptors = host.setting_descriptors();
        let group = verbatim_i18n::t!("speech-synthesizer-group");
        ffi::SpeechPage {
            generation: self.generation,
            synthesizer_field_name: accessible_name(&group),
            synthesizer_group: group,
            synthesizer_name: host.active_synthesizer().display_name,
            change: verbatim_i18n::t!("speech-change-synth"),
            controls: self
                .descriptors
                .iter()
                .map(|descriptor| control(descriptor, host.setting(descriptor.id()).as_ref()))
                .collect(),
        }
    }

    /// The setting a change to control `index` of `generation` sets, or
    /// `None` when the control is gone or the change does not fit it.
    pub(crate) fn setting_for(
        &self,
        generation: u32,
        index: usize,
        change: ControlChange,
    ) -> Option<(SettingId, SettingValue)> {
        if generation != self.generation {
            return None;
        }
        let descriptor = self.descriptors.get(index)?;
        let value = match (descriptor, change) {
            (SettingDescriptor::Numeric { .. }, ControlChange::Number(value)) => {
                SettingValue::Number(value)
            }
            (SettingDescriptor::Choice { options, .. }, ControlChange::Option(option)) => {
                SettingValue::Choice(options.get(option)?.0.clone())
            }
            (SettingDescriptor::Toggle { .. }, ControlChange::Toggle(checked)) => {
                SettingValue::Toggle(checked)
            }
            _ => return None,
        };
        Some((descriptor.id().clone(), value))
    }

    /// Numbers a change of `id` to `value` about to be sent, and keeps it
    /// as outstanding until answered.
    pub(crate) fn sent(&mut self, id: &SettingId, value: &SettingValue) -> u64 {
        self.last_change += 1;
        let change = self.last_change;
        self.outstanding.retain(|(pending, _, _)| pending != id);
        self.outstanding.push((id.clone(), change, value.clone()));
        change
    }

    /// Change `change` was not sent after all.
    pub(crate) fn unsent(&mut self, change: u64) {
        self.outstanding
            .retain(|(_, pending, _)| *pending != change);
    }

    /// Takes the synth thread's answer: every setting whose latest change
    /// it answered is no longer outstanding, and when the driver's value
    /// differs from the value sent, its control is to snap back to the
    /// driver's. A setting changed again since keeps its newer change
    /// outstanding, so an older answer never overwrites a newer request;
    /// a setting not changed is left as shown.
    pub(crate) fn answered(&mut self, answer: &SettingsAnswer) -> Vec<Snap> {
        let mut snaps = Vec::new();
        let (answered, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.outstanding)
            .into_iter()
            .partition(|(_, change, _)| *change <= answer.change);
        self.outstanding = waiting;
        for (id, _, sent) in answered {
            let Some((_, value)) = answer.values.iter().find(|(answered, _)| *answered == id)
            else {
                continue;
            };
            if *value == sent {
                continue;
            }
            let Some(control) = self
                .descriptors
                .iter()
                .position(|descriptor| *descriptor.id() == id)
            else {
                continue;
            };
            if let Some(value) = control_value(&self.descriptors[control], value) {
                snaps.push(Snap {
                    generation: self.generation,
                    control,
                    value,
                });
            }
        }
        snaps
    }
}

/// `value` as the control built from `descriptor` shows it: a slider's
/// value, a choice's option index, or a check box's state as 1 or 0;
/// `None` for a value of another shape, or a choice that is no option.
fn control_value(descriptor: &SettingDescriptor, value: &SettingValue) -> Option<i32> {
    match (descriptor, value) {
        (SettingDescriptor::Numeric { .. }, SettingValue::Number(number)) => Some(*number),
        (SettingDescriptor::Choice { options, .. }, SettingValue::Choice(choice)) => options
            .iter()
            .position(|(option, _)| option == choice)
            .and_then(|index| i32::try_from(index).ok()),
        (SettingDescriptor::Toggle { .. }, SettingValue::Toggle(checked)) => {
            Some(i32::from(*checked))
        }
        _ => None,
    }
}

/// The widget description for `descriptor`, showing the driver's `value`.
/// A value that does not fit the descriptor is the driver's error: it is
/// logged and the control shown empty (a slider at its minimum, no option
/// chosen, unchecked). A label key with no message is logged and the
/// setting's id shown in its place.
fn control(descriptor: &SettingDescriptor, value: Option<&SettingValue>) -> ffi::SettingControl {
    let shown = value
        .as_ref()
        .and_then(|value| control_value(descriptor, value));
    if shown.is_none() && value.is_some() {
        tracing::error!(
            target: verbatim_model::DEFECT_TARGET,
            setting = %descriptor.id(),
            ?value,
            "the synthesizer's value does not fit its setting"
        );
    }
    let label = verbatim_i18n::message(descriptor.label_key()).unwrap_or_else(|| {
        tracing::error!(target: verbatim_model::DEFECT_TARGET, key = descriptor.label_key(), "a setting's label has no message");
        descriptor.id().to_string()
    });
    let mut control = ffi::SettingControl {
        kind: ffi::ControlKind::Slider,
        name: String::new(),
        label,
        min: 0,
        max: 0,
        value: 0,
        line_size: 0,
        page_size: 0,
        options: Vec::new(),
        selection: -1,
        checked: false,
    };
    match descriptor {
        SettingDescriptor::Numeric {
            min,
            max,
            small_step,
            large_step,
            ..
        } => {
            control.min = *min;
            control.max = *max;
            control.value = shown.unwrap_or(*min).clamp(*min, *max);
            control.line_size = *small_step;
            control.page_size = *large_step;
        }
        SettingDescriptor::Choice { options, .. } => {
            control.kind = ffi::ControlKind::Choice;
            control.options = options.iter().map(|(_, shown)| shown.clone()).collect();
            control.selection = shown.unwrap_or(-1);
        }
        SettingDescriptor::Toggle { .. } => {
            // A check box carries its own label, so it has no separate
            // label to borrow a name from; without one it is announced by
            // wxWidgets' default window name, "check".
            control.kind = ffi::ControlKind::Toggle;
            control.name = accessible_name(&control.label);
            control.checked = shown == Some(1);
        }
    }
    control
}

/// The Select Synthesizer dialog over the host's synthesizers.
pub(crate) fn synthesizer_picker(host: &dyn SpeechSettingsHost) -> ffi::SynthesizerPicker {
    let synthesizers = host.synthesizers();
    let active = host.active_synthesizer();
    ffi::SynthesizerPicker {
        title: verbatim_i18n::t!("select-synth-title"),
        label: verbatim_i18n::t!("select-synth-label"),
        ok: verbatim_i18n::t!("button-ok"),
        cancel: verbatim_i18n::t!("button-cancel"),
        active: synthesizers
            .iter()
            .position(|synthesizer| synthesizer.id == active.id)
            .unwrap_or(0),
        names: synthesizers
            .into_iter()
            .map(|synthesizer| synthesizer.display_name)
            .collect(),
        switching: verbatim_i18n::t!("select-synth-switching"),
    }
}

/// What the dialog is told once the switch to the synthesizer named `name`
/// has ended with `outcome`: a failure is shown in NVDA's words
/// (`gui/settingsDialogs.py`, `_synthWarningDialog`).
pub(crate) fn switch_outcome(
    name: &str,
    outcome: &Result<(), verbatim_speech::SynthError>,
) -> ffi::SynthesizerSwitch {
    match outcome {
        Ok(()) => ffi::SynthesizerSwitch {
            switched: true,
            error: String::new(),
            error_title: String::new(),
        },
        Err(_) => ffi::SynthesizerSwitch {
            switched: false,
            error: verbatim_i18n::t!("synth-error", name = name),
            error_title: verbatim_i18n::t!("synth-error-title"),
        },
    }
}

/// The synthesizer to switch to when the user picked the one at `index`:
/// `None` when it is the active one already or out of range.
pub(crate) fn synthesizer_to_switch_to(
    synthesizers: &[SynthChoice],
    active: &SynthId,
    index: usize,
) -> Option<SynthId> {
    synthesizers
        .get(index)
        .filter(|chosen| chosen.id != *active)
        .map(|chosen| chosen.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use verbatim_speech::{SettingDescriptor, SynthError};

    /// A host with one setting of each kind and two synthesizers.
    struct Host {
        active: Mutex<SynthId>,
    }

    impl Host {
        fn new() -> Self {
            Self {
                active: Mutex::new(SynthId::new("espeak")),
            }
        }
    }

    fn synthesizers() -> Vec<SynthChoice> {
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

    impl SpeechSettingsHost for Host {
        fn synthesizers(&self) -> Vec<SynthChoice> {
            synthesizers()
        }
        fn active_synthesizer(&self) -> SynthChoice {
            let active = self.active.lock().unwrap().clone();
            synthesizers()
                .into_iter()
                .find(|synthesizer| synthesizer.id == active)
                .unwrap()
        }
        fn switch_synthesizer(&self, id: &SynthId, done: verbatim_speech::SwitchDone) {
            *self.active.lock().unwrap() = id.clone();
            done.answer(Ok(()));
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
                SettingDescriptor::Toggle {
                    id: SettingId::new("rate-boost"),
                    label_key: "setting-rate-boost".into(),
                },
            ]
        }
        fn setting(&self, id: &SettingId) -> Option<SettingValue> {
            match id.0.as_str() {
                "voice" => Some(SettingValue::Choice("david".into())),
                "rate" => Some(SettingValue::Number(42)),
                "rate-boost" => Some(SettingValue::Toggle(true)),
                _ => None,
            }
        }
        fn set_setting(
            &self,
            _id: &SettingId,
            _value: SettingValue,
            _change: u64,
        ) -> Result<(), SynthError> {
            Ok(())
        }
        fn commit(&self) -> Result<(), SynthError> {
            Ok(())
        }
        fn revert(&self) {}
        fn answer_to(&self, _answered: verbatim_speech::SettingsAnswered) {}
    }

    /// The synth thread's answer to change `change`, with the driver's
    /// rate at `rate` and the host's other values.
    fn answer(change: u64, rate: i32) -> SettingsAnswer {
        SettingsAnswer {
            change,
            values: vec![
                (
                    SettingId::new("voice"),
                    SettingValue::Choice("david".into()),
                ),
                (SettingId::new("rate"), SettingValue::Number(rate)),
                (SettingId::new("rate-boost"), SettingValue::Toggle(true)),
            ],
        }
    }

    #[test]
    fn a_refused_change_snaps_its_control_back_to_the_drivers_value() {
        let mut controls = SpeechControls::default();
        let page = controls.rebuild(&Host::new());
        let rate = SettingId::new("rate");
        let change = controls.sent(&rate, &SettingValue::Number(60));
        assert_eq!(
            controls.answered(&answer(change, 50)),
            [Snap {
                generation: page.generation,
                control: 1,
                value: 50,
            }]
        );
    }

    #[test]
    fn an_answer_to_an_older_change_leaves_the_control_to_the_newer_ones_answer() {
        let mut controls = SpeechControls::default();
        let page = controls.rebuild(&Host::new());
        let rate = SettingId::new("rate");
        let first = controls.sent(&rate, &SettingValue::Number(60));
        let second = controls.sent(&rate, &SettingValue::Number(70));
        // The first change was refused; the second is still outstanding,
        // so the first's answer does not move the control.
        assert_eq!(controls.answered(&answer(first, 50)), []);
        // The driver clamped the second to 65: the control takes it.
        assert_eq!(
            controls.answered(&answer(second, 65)),
            [Snap {
                generation: page.generation,
                control: 1,
                value: 65,
            }]
        );
    }

    #[test]
    fn the_speech_page_describes_each_descriptor() {
        let mut controls = SpeechControls::default();
        let page = controls.rebuild(&Host::new());
        assert_eq!(page.synthesizer_name, "eSpeak NG");
        assert_eq!(
            page.synthesizer_field_name,
            accessible_name(&page.synthesizer_group),
            "the read-only field is named by its group, without a mnemonic"
        );
        let kinds: Vec<_> = page.controls.iter().map(|control| control.kind).collect();
        assert_eq!(
            kinds,
            [
                ffi::ControlKind::Choice,
                ffi::ControlKind::Slider,
                ffi::ControlKind::Toggle
            ]
        );
        let voice = &page.controls[0];
        assert_eq!(voice.options, ["Microsoft Hazel", "Microsoft David"]);
        assert_eq!(voice.selection, 1);
        let rate = &page.controls[1];
        assert_eq!((rate.min, rate.max, rate.value), (0, 100, 42));
        assert_eq!((rate.line_size, rate.page_size), (1, 10));
        let boost = &page.controls[2];
        assert!(boost.checked);
        assert_eq!(boost.name, accessible_name(&boost.label));
        assert!(
            !boost.name.is_empty() && !boost.name.contains('&'),
            "a check box gets a clean accessible name: {:?}",
            boost.name
        );
    }

    /// A slider's value is clamped into its range; a value the host lacks
    /// or that does not fit its setting leaves the control empty; a choice
    /// that is no option selects nothing.
    #[test]
    fn a_control_shows_only_a_value_that_fits_it() {
        let rate = SettingDescriptor::standard_numeric("rate", "setting-rate");
        let voice = SettingDescriptor::Choice {
            id: SettingId::new("voice"),
            label_key: "setting-voice".into(),
            options: vec![("hazel".into(), "Microsoft Hazel".into())],
        };
        let boost = SettingDescriptor::Toggle {
            id: SettingId::new("rate-boost"),
            label_key: "setting-rate-boost".into(),
        };
        assert_eq!(control(&rate, Some(&SettingValue::Number(150))).value, 100);
        assert_eq!(control(&rate, None).value, 0);
        assert_eq!(control(&rate, Some(&SettingValue::Toggle(true))).value, 0);
        let gone = SettingValue::Choice("david".into());
        assert_eq!(control(&voice, Some(&gone)).selection, -1);
        assert!(!control(&boost, Some(&SettingValue::Number(1))).checked);
    }

    #[test]
    fn changes_become_setting_values() {
        let mut controls = SpeechControls::default();
        let generation = controls.rebuild(&Host::new()).generation;
        assert_eq!(
            controls.setting_for(generation, 0, ControlChange::Option(0)),
            Some((
                SettingId::new("voice"),
                SettingValue::Choice("hazel".into())
            ))
        );
        assert_eq!(
            controls.setting_for(generation, 1, ControlChange::Number(7)),
            Some((SettingId::new("rate"), SettingValue::Number(7)))
        );
        assert_eq!(
            controls.setting_for(generation, 2, ControlChange::Toggle(false)),
            Some((SettingId::new("rate-boost"), SettingValue::Toggle(false)))
        );
    }

    #[test]
    fn changes_that_do_not_fit_are_ignored() {
        let mut controls = SpeechControls::default();
        let generation = controls.rebuild(&Host::new()).generation;
        assert_eq!(
            controls.setting_for(generation, 0, ControlChange::Option(5)),
            None,
            "an option past the end"
        );
        assert_eq!(
            controls.setting_for(generation, 9, ControlChange::Number(1)),
            None,
            "a control past the end"
        );
        assert_eq!(
            controls.setting_for(generation, 1, ControlChange::Toggle(true)),
            None,
            "a change of the wrong kind"
        );
    }

    #[test]
    fn changes_from_replaced_controls_are_ignored() {
        let mut controls = SpeechControls::default();
        let old = controls.rebuild(&Host::new()).generation;
        let new = controls.rebuild(&Host::new()).generation;
        assert_ne!(old, new);
        assert_eq!(controls.setting_for(old, 1, ControlChange::Number(7)), None);
        assert!(
            controls
                .setting_for(new, 1, ControlChange::Number(7))
                .is_some()
        );
    }

    #[test]
    fn the_picker_starts_on_the_active_synthesizer() {
        let picker = synthesizer_picker(&Host::new());
        assert_eq!(picker.names, ["Windows OneCore", "eSpeak NG"]);
        assert_eq!(picker.active, 1);
    }

    #[test]
    fn a_failed_switch_is_reported_in_nvdas_words() {
        let failed = switch_outcome(
            "Windows OneCore voices",
            &Err(SynthError::Unavailable("no answer".to_owned())),
        );
        assert!(!failed.switched);
        assert_eq!(
            (failed.error.as_str(), failed.error_title.as_str()),
            (
                "Could not load the Windows OneCore voices synthesizer.",
                "Synthesizer Error"
            )
        );
        let switched = switch_outcome("eSpeak NG", &Ok(()));
        assert!(switched.switched);
        assert_eq!(switched.error, "");
    }

    #[test]
    fn only_a_different_synthesizer_is_switched_to() {
        let active = SynthId::new("espeak");
        assert_eq!(
            synthesizer_to_switch_to(&synthesizers(), &active, 0),
            Some(SynthId::new("onecore"))
        );
        assert_eq!(
            synthesizer_to_switch_to(&synthesizers(), &active, 1),
            None,
            "the active synthesizer needs no switch and no rebuild"
        );
        assert_eq!(synthesizer_to_switch_to(&synthesizers(), &active, 2), None);
    }

    #[test]
    fn the_dialog_opens_on_speech_then_lists_theme_and_terminal() {
        let dialog = dialog();
        let kinds: Vec<_> = dialog
            .categories
            .iter()
            .map(|category| category.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                ffi::CategoryKind::Speech,
                ffi::CategoryKind::Theme,
                ffi::CategoryKind::Terminal
            ]
        );
        let names: Vec<(&str, &str)> = dialog
            .categories
            .iter()
            .map(|category| (category.name.as_str(), category.title.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("Speech", "Verbatim Settings: Speech"),
                ("Theme", "Verbatim Settings: Theme"),
                ("Terminal", "Verbatim Settings: Terminal"),
            ]
        );
    }

    /// Keeps the target of every event logged while it is the thread's
    /// subscriber.
    #[derive(Default)]
    struct Targets(Mutex<Vec<String>>);

    impl tracing::Subscriber for Targets {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            self.0
                .lock()
                .unwrap()
                .push(event.metadata().target().to_owned());
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// A driver value that does not fit its setting, and a label with no
    /// message, are Verbatim's own defects, every driver being Verbatim's:
    /// both are logged where the end-to-end suite fails a run on them.
    #[test]
    fn a_value_that_does_not_fit_and_a_missing_label_are_logged_as_defects() {
        let descriptor = SettingDescriptor::Choice {
            id: SettingId::new("voice"),
            label_key: "setting-no-such-label".into(),
            options: vec![("hazel".into(), "Microsoft Hazel".into())],
        };
        let targets = std::sync::Arc::new(Targets::default());
        tracing::subscriber::with_default(std::sync::Arc::clone(&targets), || {
            control(&descriptor, Some(&SettingValue::Number(3)));
        });
        assert_eq!(
            *targets.0.lock().unwrap(),
            [verbatim_model::DEFECT_TARGET, verbatim_model::DEFECT_TARGET]
        );
    }
}
