//! Key routing for the settings dialog: what a key press does, decided
//! from the key and the control that has focus, with no widget code, so
//! the rules are tested here and the widget layer only carries them out.
//!
//! The rules (audit item 7, decided with Dickson on 2026-10-06):
//!
//! - Control+Tab and Control+Shift+Tab move to the next and previous
//!   category from any control.
//! - Control+S applies from any control.
//! - Enter or numpad Enter on a focused button activates that button, so
//!   Enter on Cancel cancels and Enter on Apply applies. This differs from
//!   NVDA, whose settings dialogs send every Enter to OK
//!   (`docs/parity.md`).
//! - Enter on the synthesizer's name opens the Select Synthesizer dialog,
//!   as its Change button does.
//! - Enter anywhere else activates OK.
//! - Space on the Theme page's sound choice plays the sound it shows
//!   (`phase6-design.md`, "The settings dialog").
//! - Every other key is left to the focused control.

/// A key the router distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Enter on the main keyboard or the numeric keypad.
    Enter,
    /// Tab.
    Tab,
    /// The S key.
    S,
    /// The space bar.
    Space,
    /// Any other key.
    Other,
}

/// One key press: the key and the modifiers held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// The key pressed.
    pub key: Key,
    /// Whether Control was held.
    pub control: bool,
    /// Whether Shift was held.
    pub shift: bool,
}

/// A button of the settings dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogButton {
    /// OK: apply and close.
    Ok,
    /// Cancel: revert and close.
    Cancel,
    /// Apply: apply and stay open.
    Apply,
    /// Change: open the Select Synthesizer dialog.
    ChangeSynthesizer,
}

/// The control that has focus when a key is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusedControl {
    /// One of the dialog's buttons.
    Button(DialogButton),
    /// The read-only field naming the current synthesizer.
    SynthesizerName,
    /// The Theme page's sound choice.
    SoundChoice,
    /// A button other than the dialog's own, such as the Theme page's
    /// Preview.
    OtherButton,
    /// Any other control: the category list, a slider, a combo box.
    Other,
}

/// What the dialog does with a key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Move to the next category, or the previous one when `forward` is
    /// false.
    CycleCategory {
        /// Whether to move forward.
        forward: bool,
    },
    /// Activate a button, as clicking it would.
    Activate(DialogButton),
    /// Activate the focused button, one that is not the dialog's own.
    ActivateFocused,
    /// Play the sound the Theme page's sound choice shows.
    PlaySound,
    /// Let the focused control handle the key.
    PassThrough,
}

/// Decides what a key press does in the settings dialog.
#[must_use]
pub fn route_key(press: KeyPress, focused: FocusedControl) -> KeyAction {
    match press.key {
        Key::Tab if press.control => KeyAction::CycleCategory {
            forward: !press.shift,
        },
        Key::S if press.control => KeyAction::Activate(DialogButton::Apply),
        Key::Enter if !press.control => match focused {
            FocusedControl::Button(button) => KeyAction::Activate(button),
            FocusedControl::SynthesizerName => KeyAction::Activate(DialogButton::ChangeSynthesizer),
            FocusedControl::OtherButton => KeyAction::ActivateFocused,
            FocusedControl::SoundChoice | FocusedControl::Other => {
                KeyAction::Activate(DialogButton::Ok)
            }
        },
        Key::Space if !press.control && focused == FocusedControl::SoundChoice => {
            KeyAction::PlaySound
        }
        _ => KeyAction::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key) -> KeyPress {
        KeyPress {
            key,
            control: false,
            shift: false,
        }
    }

    fn control(key: Key, shift: bool) -> KeyPress {
        KeyPress {
            key,
            control: true,
            shift,
        }
    }

    #[test]
    fn enter_on_a_button_activates_that_button() {
        for button in [
            DialogButton::Ok,
            DialogButton::Cancel,
            DialogButton::Apply,
            DialogButton::ChangeSynthesizer,
        ] {
            assert_eq!(
                route_key(press(Key::Enter), FocusedControl::Button(button)),
                KeyAction::Activate(button)
            );
        }
    }

    #[test]
    fn enter_on_the_synthesizer_name_opens_the_synthesizer_dialog() {
        assert_eq!(
            route_key(press(Key::Enter), FocusedControl::SynthesizerName),
            KeyAction::Activate(DialogButton::ChangeSynthesizer)
        );
    }

    #[test]
    fn enter_on_another_button_activates_that_button() {
        assert_eq!(
            route_key(press(Key::Enter), FocusedControl::OtherButton),
            KeyAction::ActivateFocused
        );
    }

    #[test]
    fn space_on_the_sound_choice_plays_the_sound() {
        assert_eq!(
            route_key(press(Key::Space), FocusedControl::SoundChoice),
            KeyAction::PlaySound
        );
        assert_eq!(
            route_key(press(Key::Space), FocusedControl::Other),
            KeyAction::PassThrough,
            "space elsewhere is the control's own"
        );
        assert_eq!(
            route_key(press(Key::Enter), FocusedControl::SoundChoice),
            KeyAction::Activate(DialogButton::Ok)
        );
    }

    #[test]
    fn enter_elsewhere_activates_ok() {
        assert_eq!(
            route_key(press(Key::Enter), FocusedControl::Other),
            KeyAction::Activate(DialogButton::Ok)
        );
    }

    #[test]
    fn control_tab_cycles_categories_from_any_control() {
        for focused in [
            FocusedControl::Other,
            FocusedControl::SynthesizerName,
            FocusedControl::Button(DialogButton::Cancel),
        ] {
            assert_eq!(
                route_key(control(Key::Tab, false), focused),
                KeyAction::CycleCategory { forward: true }
            );
            assert_eq!(
                route_key(control(Key::Tab, true), focused),
                KeyAction::CycleCategory { forward: false }
            );
        }
    }

    #[test]
    fn control_s_applies_from_any_control() {
        for focused in [
            FocusedControl::Other,
            FocusedControl::Button(DialogButton::Ok),
        ] {
            assert_eq!(
                route_key(control(Key::S, false), focused),
                KeyAction::Activate(DialogButton::Apply)
            );
        }
    }

    #[test]
    fn other_keys_pass_through() {
        assert_eq!(
            route_key(press(Key::Tab), FocusedControl::Other),
            KeyAction::PassThrough
        );
        assert_eq!(
            route_key(press(Key::S), FocusedControl::Other),
            KeyAction::PassThrough
        );
        assert_eq!(
            route_key(press(Key::Other), FocusedControl::Button(DialogButton::Ok)),
            KeyAction::PassThrough
        );
    }
}
