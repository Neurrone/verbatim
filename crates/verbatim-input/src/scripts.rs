//! The M3 script vocabulary and binding tables (`docs/roadmap.md`'s M3
//! object-navigation bullet, transcribed exactly).
//!
//! [`ScriptAction`] names every M3 command abstractly, independent of any
//! particular key; [`bindings_for`] returns the complete gesture table for a
//! chosen [`KeyboardLayout`]. Verbatim+V (the menu) is not part of this
//! table — it stays bound the way it already is today, outside M3's
//! object-navigation vocabulary.
//!
//! [`crate::GestureMap`] is a plain membership set, not generic over the
//! bound action, so this module hands the application data it can adapt: a
//! list of gesture and action pairs, rather than a ready-built router.

use verbatim_model::GestureId;

/// Which physical keyboard layout's gesture bindings are active — NVDA's
/// desktop (numpad-based) and laptop (no numpad assumed) layouts.
///
/// Redeclared here rather than depending on `verbatim_config::KeyboardLayout`
/// so this crate stays decoupled from configuration, the same pattern
/// [`crate::DecisionConfig`] already follows for `verbatim_config::VerbatimKeys`;
/// the application maps one to the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyboardLayout {
    /// The numpad-based layout, and the default.
    #[default]
    Desktop,
    /// The layout for keyboards without a numpad, using shift and control
    /// chords on the main key block instead.
    Laptop,
}

/// Every M3 command a gesture can bind to, abstractly — the vocabulary
/// object navigation, review-cursor text reading, the time-and-date command,
/// and the system tray list script all bind against.
///
/// Report current object carries NVDA's full press semantics: the first
/// press reports the object, the second (within the multi-press window)
/// spells it, and the third copies its name and value to the clipboard.
/// Speak time and show tray list follow the same pattern: the first press of
/// Verbatim+F12 speaks the time and the second speaks the date; the first
/// press of Verbatim+F11 opens the system tray list and the second opens the
/// taskbar list. None of that repeat-count dispatch lives here — it is the
/// consumer's job, driven by [`crate::EmittedGesture`]'s `repeat` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptAction {
    /// Report the current (navigator) object; repeated presses spell then
    /// copy it.
    ReportCurrentObject,
    /// Move the navigator object to its parent.
    MoveToParent,
    /// Move the navigator object to its next sibling.
    MoveToNextSibling,
    /// Move the navigator object to its previous sibling.
    MoveToPreviousSibling,
    /// Move the navigator object to its first child.
    MoveToFirstChild,
    /// Move the review cursor back to the current focus.
    MoveReviewCursorToFocus,
    /// Activate (invoke) the current navigator object.
    ActivateCurrentObject,
    /// Move the review cursor to the top of the review area.
    ReviewTop,
    /// Move the review cursor to the previous line.
    ReviewPreviousLine,
    /// Report the review cursor's current line.
    ReviewCurrentLine,
    /// Move the review cursor to the next line.
    ReviewNextLine,
    /// Move the review cursor to the previous word.
    ReviewPreviousWord,
    /// Report the review cursor's current word.
    ReviewCurrentWord,
    /// Move the review cursor to the next word.
    ReviewNextWord,
    /// Move the review cursor to the start of the current line.
    ReviewStartOfLine,
    /// Move the review cursor to the previous character.
    ReviewPreviousCharacter,
    /// Report the review cursor's current character.
    ReviewCurrentCharacter,
    /// Move the review cursor to the next character.
    ReviewNextCharacter,
    /// Move the review cursor to the end of the current line.
    ReviewEndOfLine,
    /// Move the review cursor to the bottom of the review area.
    ReviewBottom,
    /// Speak the time; pressed twice quickly, the date.
    SpeakTime,
    /// Open the system tray list; pressed twice quickly, the taskbar list.
    ShowTrayList,
}

/// One raw `(gesture identifier, action)` pair before parsing, so the tables
/// below read as plain data.
type RawBinding = (&'static str, ScriptAction);

/// The desktop layout's M3 bindings, in `docs/roadmap.md` order: object
/// navigation, then review-cursor text reading, then time and tray list.
const DESKTOP_BINDINGS: &[RawBinding] = &[
    ("kb:verbatim+numpad5", ScriptAction::ReportCurrentObject),
    ("kb:verbatim+numpad8", ScriptAction::MoveToParent),
    ("kb:verbatim+numpad6", ScriptAction::MoveToNextSibling),
    ("kb:verbatim+numpad4", ScriptAction::MoveToPreviousSibling),
    ("kb:verbatim+numpad2", ScriptAction::MoveToFirstChild),
    (
        "kb:verbatim+numpadminus",
        ScriptAction::MoveReviewCursorToFocus,
    ),
    (
        "kb:verbatim+numpadenter",
        ScriptAction::ActivateCurrentObject,
    ),
    ("kb:shift+numpad7", ScriptAction::ReviewTop),
    ("kb:numpad7", ScriptAction::ReviewPreviousLine),
    ("kb:numpad8", ScriptAction::ReviewCurrentLine),
    ("kb:numpad9", ScriptAction::ReviewNextLine),
    ("kb:numpad4", ScriptAction::ReviewPreviousWord),
    ("kb:numpad5", ScriptAction::ReviewCurrentWord),
    ("kb:numpad6", ScriptAction::ReviewNextWord),
    ("kb:shift+numpad1", ScriptAction::ReviewStartOfLine),
    ("kb:numpad1", ScriptAction::ReviewPreviousCharacter),
    ("kb:numpad2", ScriptAction::ReviewCurrentCharacter),
    ("kb:numpad3", ScriptAction::ReviewNextCharacter),
    ("kb:shift+numpad3", ScriptAction::ReviewEndOfLine),
    ("kb:shift+numpad9", ScriptAction::ReviewBottom),
    ("kb:verbatim+f12", ScriptAction::SpeakTime),
    ("kb:verbatim+f11", ScriptAction::ShowTrayList),
];

/// The laptop layout's M3 bindings, in the same order as
/// [`DESKTOP_BINDINGS`].
const LAPTOP_BINDINGS: &[RawBinding] = &[
    ("kb:verbatim+shift+o", ScriptAction::ReportCurrentObject),
    ("kb:verbatim+shift+uparrow", ScriptAction::MoveToParent),
    (
        "kb:verbatim+shift+rightarrow",
        ScriptAction::MoveToNextSibling,
    ),
    (
        "kb:verbatim+shift+leftarrow",
        ScriptAction::MoveToPreviousSibling,
    ),
    (
        "kb:verbatim+shift+downarrow",
        ScriptAction::MoveToFirstChild,
    ),
    (
        "kb:verbatim+backspace",
        ScriptAction::MoveReviewCursorToFocus,
    ),
    ("kb:verbatim+enter", ScriptAction::ActivateCurrentObject),
    ("kb:verbatim+control+home", ScriptAction::ReviewTop),
    ("kb:verbatim+uparrow", ScriptAction::ReviewPreviousLine),
    ("kb:verbatim+shift+period", ScriptAction::ReviewCurrentLine),
    ("kb:verbatim+downarrow", ScriptAction::ReviewNextLine),
    (
        "kb:verbatim+control+leftarrow",
        ScriptAction::ReviewPreviousWord,
    ),
    (
        "kb:verbatim+control+period",
        ScriptAction::ReviewCurrentWord,
    ),
    (
        "kb:verbatim+control+rightarrow",
        ScriptAction::ReviewNextWord,
    ),
    ("kb:verbatim+home", ScriptAction::ReviewStartOfLine),
    (
        "kb:verbatim+leftarrow",
        ScriptAction::ReviewPreviousCharacter,
    ),
    ("kb:verbatim+period", ScriptAction::ReviewCurrentCharacter),
    ("kb:verbatim+rightarrow", ScriptAction::ReviewNextCharacter),
    ("kb:verbatim+end", ScriptAction::ReviewEndOfLine),
    ("kb:verbatim+control+end", ScriptAction::ReviewBottom),
    ("kb:verbatim+f12", ScriptAction::SpeakTime),
    ("kb:verbatim+f11", ScriptAction::ShowTrayList),
];

/// The complete M3 gesture table for the chosen layout, as bound in
/// `docs/roadmap.md`'s M3 object-navigation bullet.
///
/// # Panics
///
/// Never panics in practice: every identifier in the desktop and laptop
/// binding tables is a fixed literal pinned well-formed by this module's own
/// tests, not user input.
#[must_use]
pub fn bindings_for(layout: KeyboardLayout) -> Vec<(GestureId, ScriptAction)> {
    let raw = match layout {
        KeyboardLayout::Desktop => DESKTOP_BINDINGS,
        KeyboardLayout::Laptop => LAPTOP_BINDINGS,
    };
    raw.iter()
        .map(|&(identifier, action)| {
            (
                GestureId::parse(identifier).expect("M3 binding table entries are well-formed"),
                action,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn desktop_table_has_the_documented_count() {
        // 7 object-navigation + 13 review-cursor + 2 (time, tray list).
        assert_eq!(bindings_for(KeyboardLayout::Desktop).len(), 22);
    }

    #[test]
    fn laptop_table_has_the_documented_count() {
        assert_eq!(bindings_for(KeyboardLayout::Laptop).len(), 22);
    }

    #[test]
    fn desktop_table_has_no_duplicate_gesture() {
        let bindings = bindings_for(KeyboardLayout::Desktop);
        let unique: HashSet<&GestureId> = bindings.iter().map(|(gesture, _)| gesture).collect();
        assert_eq!(unique.len(), bindings.len());
    }

    #[test]
    fn laptop_table_has_no_duplicate_gesture() {
        let bindings = bindings_for(KeyboardLayout::Laptop);
        let unique: HashSet<&GestureId> = bindings.iter().map(|(gesture, _)| gesture).collect();
        assert_eq!(unique.len(), bindings.len());
    }

    #[test]
    fn every_gesture_in_both_tables_parses_and_is_keyboard_sourced() {
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            for (gesture, _) in bindings_for(layout) {
                // Re-parsing the normalized identifier must round-trip,
                // pinning that every table entry is a valid, keyboard-
                // sourced gesture identifier.
                let reparsed = GestureId::parse(gesture.as_str()).expect("re-parses");
                assert_eq!(reparsed, gesture);
                assert_eq!(gesture.source(), "kb");
            }
        }
    }

    #[test]
    fn both_layouts_bind_time_and_tray_list_identically() {
        let identical = |action: ScriptAction, expected: &str| {
            for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
                let gesture = bindings_for(layout)
                    .into_iter()
                    .find(|(_, a)| *a == action)
                    .map(|(gesture, _)| gesture)
                    .expect("action bound");
                assert_eq!(gesture.as_str(), expected);
            }
        };
        identical(ScriptAction::SpeakTime, "kb:f12+verbatim");
        identical(ScriptAction::ShowTrayList, "kb:f11+verbatim");
    }

    #[test]
    fn verbatim_v_is_not_in_either_table() {
        // The menu gesture stays bound where it already is today; it is not
        // part of the M3 object-navigation vocabulary.
        let menu = GestureId::parse("kb:v+verbatim").expect("valid identifier");
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            assert!(
                bindings_for(layout)
                    .iter()
                    .all(|(gesture, _)| *gesture != menu)
            );
        }
    }
}
