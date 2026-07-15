//! Widget-free planning logic for the settings GUI.
//!
//! Everything here is pure: it maps the frozen `verbatim-speech` descriptor
//! model onto a description of the controls the GUI should build, and it
//! carries the small bits of bookkeeping (category cycling, the settings
//! dialog singleton guard) that are easy to get wrong and worth testing
//! without a display. The widget code in [`crate::dialog`] consumes these
//! plans; it never re-derives them.

use verbatim_speech::{SettingDescriptor, SettingId, SettingValue};

/// A plan for one generated control on the Speech page.
///
/// One [`SettingDescriptor`] maps to exactly one of these, with the initial
/// value already resolved from the host so the widget code only has to build
/// and wire, never decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlPlan {
    /// A labelled slider for a [`SettingDescriptor::Numeric`] setting.
    Slider {
        /// The setting this control drives.
        id: SettingId,
        /// Fluent message id for the control label.
        label_key: String,
        /// Inclusive minimum.
        min: i32,
        /// Inclusive maximum.
        max: i32,
        /// Arrow-key step (wxWidgets calls this the line size).
        small_step: i32,
        /// Page-up and page-down step.
        large_step: i32,
        /// Initial slider value, clamped into `min..=max`.
        initial: i32,
    },
    /// A labelled combo box for a [`SettingDescriptor::Choice`] setting.
    Choice {
        /// The setting this control drives.
        id: SettingId,
        /// Fluent message id for the control label.
        label_key: String,
        /// Option pairs of stable id and display name, in display order.
        options: Vec<(String, String)>,
        /// Index into `options` of the current value, when it matches one.
        selected: Option<usize>,
    },
    /// A check box for a [`SettingDescriptor::Toggle`] setting.
    Toggle {
        /// The setting this control drives.
        id: SettingId,
        /// Fluent message id for the control label.
        label_key: String,
        /// Initial checked state.
        initial: bool,
    },
}

impl ControlPlan {
    /// The setting id this control drives.
    #[must_use]
    pub fn id(&self) -> &SettingId {
        match self {
            Self::Slider { id, .. } | Self::Choice { id, .. } | Self::Toggle { id, .. } => id,
        }
    }

    /// The Fluent message id for this control's label.
    #[must_use]
    pub fn label_key(&self) -> &str {
        match self {
            Self::Slider { label_key, .. }
            | Self::Choice { label_key, .. }
            | Self::Toggle { label_key, .. } => label_key,
        }
    }
}

/// Builds the [`ControlPlan`] for one descriptor, resolving its initial value.
///
/// `value` is the host's current value for the setting (from
/// `SpeechSettingsHost::setting`). A value of the wrong shape for the
/// descriptor — which a well-behaved host never produces — falls back to a
/// safe default (the slider minimum, no selection, or unchecked) rather than
/// panicking, so a driver bug degrades one control instead of the GUI.
#[must_use]
pub fn plan_for(descriptor: &SettingDescriptor, value: Option<SettingValue>) -> ControlPlan {
    match descriptor {
        SettingDescriptor::Numeric {
            id,
            label_key,
            min,
            max,
            small_step,
            large_step,
            ..
        } => {
            let raw = match value {
                Some(SettingValue::Number(n)) => n,
                _ => *min,
            };
            ControlPlan::Slider {
                id: id.clone(),
                label_key: label_key.clone(),
                min: *min,
                max: *max,
                small_step: *small_step,
                large_step: *large_step,
                initial: raw.clamp(*min, *max),
            }
        }
        SettingDescriptor::Choice {
            id,
            label_key,
            options,
        } => {
            let selected = match value {
                Some(SettingValue::Choice(current)) => options
                    .iter()
                    .position(|(option_id, _)| *option_id == current),
                _ => None,
            };
            ControlPlan::Choice {
                id: id.clone(),
                label_key: label_key.clone(),
                options: options.clone(),
                selected,
            }
        }
        SettingDescriptor::Toggle { id, label_key } => ControlPlan::Toggle {
            id: id.clone(),
            label_key: label_key.clone(),
            initial: matches!(value, Some(SettingValue::Toggle(true))),
        },
    }
}

/// Strips wxWidgets accelerator markers from a label so it can serve as an
/// accessible name.
///
/// A single `&` marks the next character as the mnemonic and is not part of the
/// visible text; `&&` is a literal ampersand. Controls whose accessible name we
/// set explicitly need this stripped form, or the marker is spoken aloud.
#[must_use]
pub fn accessible_name(label: &str) -> String {
    let mut name = String::with_capacity(label.len());
    let mut characters = label.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '&' {
            if characters.peek() == Some(&'&') {
                characters.next();
                name.push('&');
            }
            // A lone `&` is the mnemonic marker; drop it.
        } else {
            name.push(character);
        }
    }
    name
}

/// The next index when cycling categories with wraparound.
///
/// `forward` moves toward the end (Ctrl+Tab), `!forward` toward the start
/// (Ctrl+Shift+Tab). Returns `current` unchanged for an empty list.
#[must_use]
pub fn cycle_index(current: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return current;
    }
    if forward {
        (current + 1) % len
    } else {
        (current + len - 1) % len
    }
}

/// The list selection a freshly opened list dialog starts with: the first
/// item when the list is non-empty, so keyboard users land on a selected
/// item rather than an unselected list (mirroring NVDA's systrayList).
#[must_use]
pub fn initial_list_selection(item_count: usize) -> Option<usize> {
    (item_count > 0).then_some(0)
}

/// The singleton guard for the modeless settings dialog.
///
/// A second request to open settings must focus the existing window rather
/// than spawn a duplicate, mirroring NVDA. This models the decision purely so
/// it is testable; the widget code holds the real dialog handle alongside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialogGuard {
    open: bool,
}

/// What opening the settings dialog should do, per the singleton rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// No dialog exists; create and show one.
    Create,
    /// A dialog already exists; raise and focus it.
    FocusExisting,
}

impl DialogGuard {
    /// A guard with no dialog open yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an open request and reports what the caller should do.
    pub fn request_open(&mut self) -> OpenAction {
        if self.open {
            OpenAction::FocusExisting
        } else {
            self.open = true;
            OpenAction::Create
        }
    }

    /// Records that the dialog has closed, so the next request creates a new one.
    pub fn closed(&mut self) {
        self.open = false;
    }

    /// Whether a dialog is currently considered open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_speech::SettingId;

    #[test]
    fn numeric_descriptor_plans_a_slider_with_clamped_initial() {
        let descriptor = SettingDescriptor::standard_numeric("rate", "setting-rate");
        let plan = plan_for(&descriptor, Some(SettingValue::Number(150)));
        assert_eq!(
            plan,
            ControlPlan::Slider {
                id: SettingId::new("rate"),
                label_key: "setting-rate".into(),
                min: 0,
                max: 100,
                small_step: 1,
                large_step: 10,
                // 150 is out of the 0..=100 range and is clamped to the max.
                initial: 100,
            }
        );
    }

    #[test]
    fn numeric_descriptor_without_value_starts_at_minimum() {
        let descriptor = SettingDescriptor::Numeric {
            id: SettingId::new("pitch"),
            label_key: "setting-pitch".into(),
            min: 20,
            max: 80,
            small_step: 1,
            normal_step: 5,
            large_step: 10,
        };
        let ControlPlan::Slider { initial, .. } = plan_for(&descriptor, None) else {
            panic!("numeric descriptor must plan a slider");
        };
        assert_eq!(initial, 20);
    }

    #[test]
    fn choice_descriptor_resolves_the_selected_index() {
        let descriptor = SettingDescriptor::Choice {
            id: SettingId::new("voice"),
            label_key: "setting-voice".into(),
            options: vec![
                ("hazel".into(), "Microsoft Hazel".into()),
                ("david".into(), "Microsoft David".into()),
            ],
        };
        let plan = plan_for(&descriptor, Some(SettingValue::Choice("david".into())));
        let ControlPlan::Choice { selected, .. } = plan else {
            panic!("choice descriptor must plan a choice");
        };
        assert_eq!(selected, Some(1));
    }

    #[test]
    fn choice_descriptor_with_unknown_value_selects_nothing() {
        let descriptor = SettingDescriptor::Choice {
            id: SettingId::new("voice"),
            label_key: "setting-voice".into(),
            options: vec![("hazel".into(), "Microsoft Hazel".into())],
        };
        let plan = plan_for(&descriptor, Some(SettingValue::Choice("missing".into())));
        let ControlPlan::Choice { selected, .. } = plan else {
            panic!("choice descriptor must plan a choice");
        };
        assert_eq!(selected, None);
    }

    #[test]
    fn toggle_descriptor_reads_its_boolean() {
        let descriptor = SettingDescriptor::Toggle {
            id: SettingId::new("rate-boost"),
            label_key: "setting-rate-boost".into(),
        };
        assert_eq!(
            plan_for(&descriptor, Some(SettingValue::Toggle(true))),
            ControlPlan::Toggle {
                id: SettingId::new("rate-boost"),
                label_key: "setting-rate-boost".into(),
                initial: true,
            }
        );
        let ControlPlan::Toggle { initial, .. } = plan_for(&descriptor, None) else {
            panic!("toggle descriptor must plan a toggle");
        };
        assert!(!initial, "a missing toggle value defaults to unchecked");
    }

    #[test]
    fn mismatched_value_shape_falls_back_instead_of_panicking() {
        let numeric = SettingDescriptor::standard_numeric("rate", "setting-rate");
        let ControlPlan::Slider { initial, .. } =
            plan_for(&numeric, Some(SettingValue::Toggle(true)))
        else {
            panic!("numeric descriptor must plan a slider");
        };
        assert_eq!(initial, 0, "a wrong-shaped value falls back to the minimum");
    }

    #[test]
    fn accessible_name_drops_mnemonic_markers() {
        // The live GUI exposed this: a check box whose label kept its `&` marker
        // reported the wrong accessible name, so the name we set must be clean.
        assert_eq!(accessible_name("Rate boos&t"), "Rate boost");
        assert_eq!(accessible_name("&Voice"), "Voice");
        assert_eq!(accessible_name("V&olume"), "Volume");
        assert_eq!(accessible_name("Synthesizer"), "Synthesizer");
        assert_eq!(
            accessible_name("Salt && Pepper"),
            "Salt & Pepper",
            "a doubled ampersand is a literal one"
        );
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        assert_eq!(cycle_index(0, 3, true), 1);
        assert_eq!(cycle_index(2, 3, true), 0, "forward wraps past the end");
        assert_eq!(cycle_index(0, 3, false), 2, "backward wraps past the start");
        assert_eq!(cycle_index(1, 3, false), 0);
        assert_eq!(cycle_index(0, 0, true), 0, "an empty list stays put");
        assert_eq!(
            cycle_index(5, 1, true),
            0,
            "a single category maps to itself"
        );
    }

    #[test]
    fn list_selection_starts_on_the_first_item() {
        assert_eq!(initial_list_selection(3), Some(0));
        assert_eq!(initial_list_selection(1), Some(0));
        assert_eq!(
            initial_list_selection(0),
            None,
            "an empty list has nothing to select"
        );
    }

    #[test]
    fn dialog_guard_enforces_a_single_instance() {
        let mut guard = DialogGuard::new();
        assert!(!guard.is_open());
        assert_eq!(guard.request_open(), OpenAction::Create);
        assert!(guard.is_open());
        assert_eq!(
            guard.request_open(),
            OpenAction::FocusExisting,
            "a second request while open focuses the existing dialog"
        );
        guard.closed();
        assert_eq!(
            guard.request_open(),
            OpenAction::Create,
            "after closing, the next request creates a fresh dialog"
        );
    }
}
