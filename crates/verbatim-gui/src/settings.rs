//! The settings dialog's model: its categories, the Speech page generated
//! from the settings host's descriptors, what a change to one of the
//! page's controls sets, and the Select Synthesizer dialog's choice.
//!
//! Everything here is pure apart from reading the host. The C++ layer
//! builds widgets from these models and reports changes back by control
//! index, which [`SpeechControls`] turns into setting values.

use verbatim_i18n::messages;
use verbatim_speech::{
    SettingId, SettingValue, SettingsAnswer, SpeechSettingsHost, SynthChoice, SynthId,
};

use crate::bridge::ffi;
use crate::plan::{ControlPlan, accessible_name, plan_for};

/// The settings dialog's categories and buttons: Speech, Theme, and
/// Terminal. The
/// dialog is written over the list, so adding a category is a matter of
/// adding it here and a page for its kind in C++.
pub(crate) fn dialog() -> ffi::SettingsDialog {
    let category = |name: String, kind| ffi::Category {
        title: messages::settings_title_with_category(&name),
        name,
        kind,
    };
    ffi::SettingsDialog {
        categories: vec![
            category(
                messages::settings_category_speech(),
                ffi::CategoryKind::Speech,
            ),
            category(
                messages::settings_category_theme(),
                ffi::CategoryKind::Theme,
            ),
            category(
                messages::settings_category_terminal(),
                ffi::CategoryKind::Terminal,
            ),
        ],
        categories_label: messages::settings_categories_label(),
        ok: messages::button_ok(),
        cancel: messages::button_cancel(),
        apply: messages::button_apply(),
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

/// The Speech page's current controls: the plans they were built from, so
/// a change reported by index becomes a setting value, and each change
/// sent and not yet answered by the synth thread
/// (`docs/design/ownership-output.md`, "The mirror follows the synth
/// thread").
#[derive(Debug, Default)]
pub(crate) struct SpeechControls {
    generation: u32,
    plans: Vec<ControlPlan>,
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
        self.plans = host
            .setting_descriptors()
            .iter()
            .map(|descriptor| plan_for(descriptor, host.setting(descriptor.id())))
            .collect();
        let group = messages::speech_synthesizer_group();
        ffi::SpeechPage {
            generation: self.generation,
            synthesizer_field_name: accessible_name(&group),
            synthesizer_group: group,
            synthesizer_name: host.active_synthesizer().display_name,
            change: messages::speech_change_synth(),
            controls: self.plans.iter().map(control).collect(),
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
        let plan = self.plans.get(index)?;
        let value = match (plan, change) {
            (ControlPlan::Slider { .. }, ControlChange::Number(value)) => {
                SettingValue::Number(value)
            }
            (ControlPlan::Choice { options, .. }, ControlChange::Option(option)) => {
                SettingValue::Choice(options.get(option)?.0.clone())
            }
            (ControlPlan::Toggle { .. }, ControlChange::Toggle(checked)) => {
                SettingValue::Toggle(checked)
            }
            _ => return None,
        };
        Some((plan.id().clone(), value))
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
            let Some(control) = self.plans.iter().position(|plan| *plan.id() == id) else {
                continue;
            };
            if let Some(value) = control_value(&self.plans[control], value) {
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

/// `value` as the control `plan` built shows it: a slider's value, a
/// choice's option index, or a check box's state as 1 or 0.
fn control_value(plan: &ControlPlan, value: &SettingValue) -> Option<i32> {
    match (plan, value) {
        (ControlPlan::Slider { .. }, SettingValue::Number(number)) => Some(*number),
        (ControlPlan::Choice { options, .. }, SettingValue::Choice(choice)) => options
            .iter()
            .position(|(option, _)| option == choice)
            .and_then(|index| i32::try_from(index).ok()),
        (ControlPlan::Toggle { .. }, SettingValue::Toggle(checked)) => Some(i32::from(*checked)),
        _ => None,
    }
}

/// The widget description for one planned control, its label resolved.
fn control(plan: &ControlPlan) -> ffi::SettingControl {
    let label = verbatim_i18n::message(plan.label_key());
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
    match plan {
        ControlPlan::Slider {
            min,
            max,
            small_step,
            large_step,
            initial,
            ..
        } => {
            control.min = *min;
            control.max = *max;
            control.value = *initial;
            control.line_size = *small_step;
            control.page_size = *large_step;
        }
        ControlPlan::Choice {
            options, selected, ..
        } => {
            control.kind = ffi::ControlKind::Choice;
            control.options = options.iter().map(|(_, shown)| shown.clone()).collect();
            control.selection = selected
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(-1);
        }
        ControlPlan::Toggle { initial, .. } => {
            // A check box carries its own label, so it has no separate
            // label to borrow a name from; without one it is announced by
            // wxWidgets' default window name, "check".
            control.kind = ffi::ControlKind::Toggle;
            control.name = accessible_name(&control.label);
            control.checked = *initial;
        }
    }
    control
}

/// The Select Synthesizer dialog over the host's synthesizers.
pub(crate) fn synthesizer_picker(host: &dyn SpeechSettingsHost) -> ffi::SynthesizerPicker {
    let synthesizers = host.synthesizers();
    let active = host.active_synthesizer();
    ffi::SynthesizerPicker {
        title: messages::select_synth_title(),
        label: messages::select_synth_label(),
        ok: messages::button_ok(),
        cancel: messages::button_cancel(),
        active: synthesizers
            .iter()
            .position(|synthesizer| synthesizer.id == active.id)
            .unwrap_or(0),
        names: synthesizers
            .into_iter()
            .map(|synthesizer| synthesizer.display_name)
            .collect(),
        switching: messages::select_synth_switching(),
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
            error: messages::synth_error(name),
            error_title: messages::synth_error_title(),
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
            done(Ok(()));
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
}
