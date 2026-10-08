//! The script vocabulary and binding tables: M3's object navigation and
//! review (`docs/roadmap-done.md`'s M3 object-navigation bullet,
//! transcribed exactly), and M4's review, say-all, select-then-copy, and
//! typing commands (`phase6-design.md`, "The autonomous run", M4 item 5,
//! with NVDA's desktop and laptop keys), plus the caret keys the hook
//! observes on their way to the application ([`caret_bindings`]).
//!
//! [`ScriptAction`] names every command abstractly, independent of any
//! particular key; [`bindings_for`] returns the complete gesture table for a
//! chosen [`KeyboardLayout`]. Verbatim+V (the menu) is not part of this
//! table — it stays bound the way it already is today, outside M3's
//! object-navigation vocabulary.
//!
//! [`crate::GestureMap`] is a plain membership set, not generic over the
//! bound action, so this module hands the application data it can adapt: a
//! list of gesture and action pairs, rather than a ready-built router.

use verbatim_model::{CaretKey, CaretMotion, GestureId, ReviewCommand};

use crate::map::GestureMap;

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
    /// Move the review cursor to the previous page.
    ReviewPreviousPage,
    /// Move the review cursor to the next page.
    ReviewNextPage,
    /// Move the review cursor to the start of the selection.
    ReviewSelectionStart,
    /// Move the review cursor to the end of the selection.
    ReviewSelectionEnd,
    /// Say all from the review cursor.
    SayAllFromReview,
    /// Say all from the caret.
    SayAllFromCaret,
    /// Mark the start of a select then copy at the review cursor.
    SetStartMarker,
    /// Move the review cursor to the start marker.
    MoveToStartMarker,
    /// Select from the start marker to the review cursor; pressed twice,
    /// copy it.
    SelectThenCopy,
    /// Toggle whether the review cursor follows the caret.
    ToggleFollowCaret,
    /// Cycle "Speak typed characters".
    ToggleTypedCharacters,
    /// Cycle "Speak typed words".
    ToggleTypedWords,
    /// Toggle "Report new output" in terminals (NVDA's key for its "report
    /// dynamic content changes" toggle).
    ToggleReportNewOutput,
    /// Report the caret's location.
    ReportCaretLocation,
    /// Report the review cursor's location.
    ReportReviewLocation,
    /// Report the focus object; repeated presses spell its name, then
    /// spell it with character descriptions.
    ReportFocus,
}

impl ScriptAction {
    /// The reducer command this action runs, or `None` for the actions the
    /// shell's router handles itself (speak time, the tray list). The two
    /// vocabularies stay separate, the input crate owning key scripts and
    /// the model owning reducer commands; this is where they meet.
    #[must_use]
    pub const fn review_command(self) -> Option<ReviewCommand> {
        Some(match self {
            Self::ReportCurrentObject => ReviewCommand::ReportObject,
            Self::MoveToParent => ReviewCommand::Parent,
            Self::MoveToNextSibling => ReviewCommand::NextSibling,
            Self::MoveToPreviousSibling => ReviewCommand::PreviousSibling,
            Self::MoveToFirstChild => ReviewCommand::FirstChild,
            Self::MoveReviewCursorToFocus => ReviewCommand::ToFocus,
            Self::ActivateCurrentObject => ReviewCommand::Activate,
            Self::ReviewTop => ReviewCommand::ReviewTop,
            Self::ReviewPreviousLine => ReviewCommand::ReviewPreviousLine,
            Self::ReviewCurrentLine => ReviewCommand::ReviewCurrentLine,
            Self::ReviewNextLine => ReviewCommand::ReviewNextLine,
            Self::ReviewPreviousWord => ReviewCommand::ReviewPreviousWord,
            Self::ReviewCurrentWord => ReviewCommand::ReviewCurrentWord,
            Self::ReviewNextWord => ReviewCommand::ReviewNextWord,
            Self::ReviewStartOfLine => ReviewCommand::ReviewStartOfLine,
            Self::ReviewPreviousCharacter => ReviewCommand::ReviewPreviousCharacter,
            Self::ReviewCurrentCharacter => ReviewCommand::ReviewCurrentCharacter,
            Self::ReviewNextCharacter => ReviewCommand::ReviewNextCharacter,
            Self::ReviewEndOfLine => ReviewCommand::ReviewEndOfLine,
            Self::ReviewBottom => ReviewCommand::ReviewBottom,
            Self::ReviewPreviousPage => ReviewCommand::ReviewPreviousPage,
            Self::ReviewNextPage => ReviewCommand::ReviewNextPage,
            Self::ReviewSelectionStart => ReviewCommand::ReviewSelectionStart,
            Self::ReviewSelectionEnd => ReviewCommand::ReviewSelectionEnd,
            Self::SayAllFromReview => ReviewCommand::SayAllFromReview,
            Self::SayAllFromCaret => ReviewCommand::SayAllFromCaret,
            Self::SetStartMarker => ReviewCommand::SetStartMarker,
            Self::MoveToStartMarker => ReviewCommand::MoveToStartMarker,
            Self::SelectThenCopy => ReviewCommand::SelectThenCopy,
            Self::ToggleFollowCaret => ReviewCommand::ToggleFollowCaret,
            Self::ToggleTypedCharacters => ReviewCommand::ToggleTypedCharacters,
            Self::ToggleTypedWords => ReviewCommand::ToggleTypedWords,
            Self::ToggleReportNewOutput => ReviewCommand::ToggleReportNewOutput,
            Self::ReportCaretLocation => ReviewCommand::ReportCaretLocation,
            Self::ReportReviewLocation => ReviewCommand::ReportReviewLocation,
            Self::ReportFocus => ReviewCommand::ReportFocus,
            Self::SpeakTime | Self::ShowTrayList => return None,
        })
    }
}

/// One raw `(gesture identifier, action)` pair before parsing, so the tables
/// below read as plain data.
type RawBinding = (&'static str, ScriptAction);

/// The bindings of every layout: M3's in `docs/roadmap-done.md` order
/// (object navigation, then review-cursor text reading, then time and tray
/// list), then M4's, then report focus. NVDA binds each of these for all layouts (`kb:` rather
/// than `kb(desktop):`), so a laptop-layout user with a numpad keeps them.
const COMMON_BINDINGS: &[RawBinding] = &[
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
    ("kb:verbatim+alt+home", ScriptAction::ReviewSelectionStart),
    ("kb:verbatim+alt+end", ScriptAction::ReviewSelectionEnd),
    ("kb:numpadplus", ScriptAction::SayAllFromReview),
    ("kb:verbatim+f9", ScriptAction::SetStartMarker),
    ("kb:verbatim+shift+f9", ScriptAction::MoveToStartMarker),
    ("kb:verbatim+f10", ScriptAction::SelectThenCopy),
    ("kb:verbatim+6", ScriptAction::ToggleFollowCaret),
    ("kb:verbatim+2", ScriptAction::ToggleTypedCharacters),
    ("kb:verbatim+3", ScriptAction::ToggleTypedWords),
    ("kb:verbatim+5", ScriptAction::ToggleReportNewOutput),
    ("kb:verbatim+tab", ScriptAction::ReportFocus),
];

/// The desktop layout's own M4 bindings, NVDA's `kb(desktop):` ones: keys
/// the laptop layout gives other meanings (Verbatim+Down Arrow reads the
/// next review line there).
const DESKTOP_BINDINGS: &[RawBinding] = &[
    ("kb:verbatim+pageup", ScriptAction::ReviewPreviousPage),
    ("kb:verbatim+pagedown", ScriptAction::ReviewNextPage),
    ("kb:verbatim+downarrow", ScriptAction::SayAllFromCaret),
    (
        "kb:verbatim+numpaddelete",
        ScriptAction::ReportCaretLocation,
    ),
    (
        "kb:verbatim+shift+numpaddelete",
        ScriptAction::ReportReviewLocation,
    ),
];

/// The laptop layout's own bindings, M3's in the same order as
/// [`COMMON_BINDINGS`], then M4's.
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
    ("kb:verbatim+shift+pageup", ScriptAction::ReviewPreviousPage),
    ("kb:verbatim+shift+pagedown", ScriptAction::ReviewNextPage),
    ("kb:verbatim+a", ScriptAction::SayAllFromCaret),
    ("kb:verbatim+shift+a", ScriptAction::SayAllFromReview),
    ("kb:verbatim+delete", ScriptAction::ReportCaretLocation),
    (
        "kb:verbatim+shift+delete",
        ScriptAction::ReportReviewLocation,
    ),
];

/// The complete gesture table for the chosen layout: the layout's own
/// bindings, then those of every layout.
///
/// # Panics
///
/// Never panics in practice: every identifier in the desktop and laptop
/// binding tables is a fixed literal pinned well-formed by this module's own
/// tests, not user input.
#[must_use]
pub fn bindings_for(layout: KeyboardLayout) -> Vec<(GestureId, ScriptAction)> {
    let own: &[RawBinding] = match layout {
        KeyboardLayout::Desktop => DESKTOP_BINDINGS,
        KeyboardLayout::Laptop => LAPTOP_BINDINGS,
    };
    own.iter()
        .chain(COMMON_BINDINGS)
        .map(|&(identifier, action)| {
            (
                GestureId::parse(identifier).expect("binding table entries are well-formed"),
                action,
            )
        })
        .collect()
}

/// The caret keys of NVDA's editable-text commands, every layout alike:
/// moving by character, word, line, paragraph, page, to the line's ends and
/// the document's, each with Shift to select, plus Backspace, Delete, their
/// Control forms, and Control+A. They are observed, never bound: the hook
/// passes each to the application, which moves the caret, and reports it
/// so the reducer can speak the result (`docs/nvda/editable-text-and-terminals.md`).
/// Build the hook's map with [`GestureMap::with_observed`] over these
/// gestures; the shell turns a reported gesture into `Input::CaretKey`
/// through the same table.
///
/// # Panics
///
/// Never panics in practice: every identifier is a fixed literal pinned
/// well-formed by this module's tests.
#[must_use]
pub fn caret_bindings() -> Vec<(GestureId, CaretKey)> {
    const MOVES: &[(&str, CaretMotion)] = &[
        ("leftarrow", CaretMotion::PreviousCharacter),
        ("rightarrow", CaretMotion::NextCharacter),
        ("control+leftarrow", CaretMotion::PreviousWord),
        ("control+rightarrow", CaretMotion::NextWord),
        ("uparrow", CaretMotion::PreviousLine),
        ("downarrow", CaretMotion::NextLine),
        ("control+uparrow", CaretMotion::PreviousParagraph),
        ("control+downarrow", CaretMotion::NextParagraph),
        ("home", CaretMotion::StartOfLine),
        ("end", CaretMotion::EndOfLine),
        ("pageup", CaretMotion::PreviousPage),
        ("pagedown", CaretMotion::NextPage),
        ("control+home", CaretMotion::Top),
        ("control+end", CaretMotion::Bottom),
    ];
    const EDITS: &[(&str, CaretMotion)] = &[
        ("backspace", CaretMotion::Backspace),
        ("control+backspace", CaretMotion::BackspaceWord),
        ("delete", CaretMotion::Delete),
        ("control+delete", CaretMotion::DeleteWord),
    ];
    let parse = |identifier: String| {
        GestureId::parse(&identifier).expect("caret table entries are well-formed")
    };
    let mut bindings = Vec::new();
    for &(keys, motion) in MOVES {
        bindings.push((
            parse(format!("kb:{keys}")),
            CaretKey {
                motion,
                select: false,
            },
        ));
        bindings.push((
            parse(format!("kb:shift+{keys}")),
            CaretKey {
                motion,
                select: true,
            },
        ));
    }
    for &(keys, motion) in EDITS {
        bindings.push((
            parse(format!("kb:{keys}")),
            CaretKey {
                motion,
                select: false,
            },
        ));
    }
    bindings.push((
        parse("kb:control+a".to_owned()),
        CaretKey {
            motion: CaretMotion::SelectAll,
            select: true,
        },
    ));
    bindings
}

/// The keys that end or clear the command line being typed: Escape,
/// Control+C, Control+D, and Control+Break, both as NVDA names it
/// (Control+Pause) and as Windows reports it (its own key, `break`). Observed like the caret keys, never bound: the hook
/// passes each to the application and reports it, and the shell turns it
/// into `Input::ClearingKey`.
///
/// # Panics
///
/// Never panics in practice: every identifier is a fixed literal pinned
/// well-formed by this module's tests.
#[must_use]
pub fn clearing_keys() -> Vec<GestureId> {
    [
        "kb:escape",
        "kb:control+c",
        "kb:control+d",
        "kb:control+pause",
        "kb:control+break",
    ]
    .into_iter()
    .map(|identifier| GestureId::parse(identifier).expect("clearing keys are well-formed"))
    .collect()
}

/// Builds the hook's bound-gesture set from a binding table, ready to wrap
/// in a [`crate::SharedGestureMap`] (or store into an existing one).
///
/// [`GestureMap`] is a plain membership set, not generic over the bound
/// action, so the action half of each pair lives with the consumer, which
/// keeps the same `Vec<(GestureId, ScriptAction)>` (or a `HashMap` built
/// from it) to resolve an emitted gesture to its action. `verbatim-app`
/// does not call this: it reads `Settings.keyboard.layout` once at
/// startup, builds its own map from [`bindings_for`] plus the menu
/// gesture, and its router builds the lookup. Layouts are not rebound at
/// runtime; a layout change takes effect on the next start.
#[must_use]
pub fn gesture_map_for(bindings: &[(GestureId, ScriptAction)]) -> GestureMap {
    GestureMap::new(bindings.iter().map(|(gesture, _)| gesture.clone()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Every layout's 33 bindings, NVDA's gestures for these commands: M3's
    /// 7 object-navigation, 13 review-cursor, and 2 for time and the tray
    /// list; M4's 10; and report focus.
    const EVERY_LAYOUT: [(&str, ScriptAction); 33] = [
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
        ("kb:verbatim+alt+home", ScriptAction::ReviewSelectionStart),
        ("kb:verbatim+alt+end", ScriptAction::ReviewSelectionEnd),
        ("kb:numpadplus", ScriptAction::SayAllFromReview),
        ("kb:verbatim+f9", ScriptAction::SetStartMarker),
        ("kb:verbatim+shift+f9", ScriptAction::MoveToStartMarker),
        ("kb:verbatim+f10", ScriptAction::SelectThenCopy),
        ("kb:verbatim+6", ScriptAction::ToggleFollowCaret),
        ("kb:verbatim+2", ScriptAction::ToggleTypedCharacters),
        ("kb:verbatim+3", ScriptAction::ToggleTypedWords),
        ("kb:verbatim+5", ScriptAction::ToggleReportNewOutput),
        ("kb:verbatim+tab", ScriptAction::ReportFocus),
    ];

    /// `own` then [`EVERY_LAYOUT`], parsed.
    fn expected_table(own: &[(&str, ScriptAction)]) -> Vec<(GestureId, ScriptAction)> {
        own.iter()
            .chain(&EVERY_LAYOUT)
            .map(|&(gesture, action)| (GestureId::parse(gesture).unwrap(), action))
            .collect()
    }

    #[test]
    fn desktop_table_binds_exactly_the_documented_gestures() {
        // The desktop's own 5, then the 33 of every layout.
        assert_eq!(
            bindings_for(KeyboardLayout::Desktop),
            expected_table(&[
                ("kb:verbatim+pageup", ScriptAction::ReviewPreviousPage),
                ("kb:verbatim+pagedown", ScriptAction::ReviewNextPage),
                ("kb:verbatim+downarrow", ScriptAction::SayAllFromCaret),
                (
                    "kb:verbatim+numpaddelete",
                    ScriptAction::ReportCaretLocation
                ),
                (
                    "kb:verbatim+shift+numpaddelete",
                    ScriptAction::ReportReviewLocation
                ),
            ])
        );
    }

    #[test]
    fn laptop_table_binds_exactly_the_documented_gestures() {
        // Its own 26 (M3's 7 object-navigation and 13 review-cursor, M4's
        // 6), then the 33 of every layout.
        assert_eq!(
            bindings_for(KeyboardLayout::Laptop),
            expected_table(&[
                ("kb:verbatim+shift+o", ScriptAction::ReportCurrentObject),
                ("kb:verbatim+shift+uparrow", ScriptAction::MoveToParent),
                (
                    "kb:verbatim+shift+rightarrow",
                    ScriptAction::MoveToNextSibling
                ),
                (
                    "kb:verbatim+shift+leftarrow",
                    ScriptAction::MoveToPreviousSibling
                ),
                (
                    "kb:verbatim+shift+downarrow",
                    ScriptAction::MoveToFirstChild
                ),
                (
                    "kb:verbatim+backspace",
                    ScriptAction::MoveReviewCursorToFocus
                ),
                ("kb:verbatim+enter", ScriptAction::ActivateCurrentObject),
                ("kb:verbatim+control+home", ScriptAction::ReviewTop),
                ("kb:verbatim+uparrow", ScriptAction::ReviewPreviousLine),
                ("kb:verbatim+shift+period", ScriptAction::ReviewCurrentLine),
                ("kb:verbatim+downarrow", ScriptAction::ReviewNextLine),
                (
                    "kb:verbatim+control+leftarrow",
                    ScriptAction::ReviewPreviousWord
                ),
                (
                    "kb:verbatim+control+period",
                    ScriptAction::ReviewCurrentWord
                ),
                (
                    "kb:verbatim+control+rightarrow",
                    ScriptAction::ReviewNextWord
                ),
                ("kb:verbatim+home", ScriptAction::ReviewStartOfLine),
                (
                    "kb:verbatim+leftarrow",
                    ScriptAction::ReviewPreviousCharacter
                ),
                ("kb:verbatim+period", ScriptAction::ReviewCurrentCharacter),
                ("kb:verbatim+rightarrow", ScriptAction::ReviewNextCharacter),
                ("kb:verbatim+end", ScriptAction::ReviewEndOfLine),
                ("kb:verbatim+control+end", ScriptAction::ReviewBottom),
                ("kb:verbatim+shift+pageup", ScriptAction::ReviewPreviousPage),
                ("kb:verbatim+shift+pagedown", ScriptAction::ReviewNextPage),
                ("kb:verbatim+a", ScriptAction::SayAllFromCaret),
                ("kb:verbatim+shift+a", ScriptAction::SayAllFromReview),
                ("kb:verbatim+delete", ScriptAction::ReportCaretLocation),
                (
                    "kb:verbatim+shift+delete",
                    ScriptAction::ReportReviewLocation
                ),
            ])
        );
    }

    #[test]
    fn every_m4_command_is_bound_in_both_layouts() {
        let m4 = [
            ScriptAction::ReviewPreviousPage,
            ScriptAction::ReviewNextPage,
            ScriptAction::ReviewSelectionStart,
            ScriptAction::ReviewSelectionEnd,
            ScriptAction::SayAllFromReview,
            ScriptAction::SayAllFromCaret,
            ScriptAction::SetStartMarker,
            ScriptAction::MoveToStartMarker,
            ScriptAction::SelectThenCopy,
            ScriptAction::ToggleFollowCaret,
            ScriptAction::ToggleTypedCharacters,
            ScriptAction::ToggleTypedWords,
            ScriptAction::ToggleReportNewOutput,
            ScriptAction::ReportCaretLocation,
            ScriptAction::ReportReviewLocation,
        ];
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            let bindings = bindings_for(layout);
            for action in m4 {
                assert!(
                    bindings.iter().any(|(_, bound)| *bound == action),
                    "{action:?} is bound in {layout:?}"
                );
                assert!(action.review_command().is_some());
            }
        }
        let gesture_of = |layout, action| {
            bindings_for(layout)
                .into_iter()
                .find(|(_, bound)| *bound == action)
                .map(|(gesture, _)| gesture.as_str().to_owned())
        };
        assert_eq!(
            gesture_of(KeyboardLayout::Desktop, ScriptAction::SayAllFromCaret).as_deref(),
            Some("kb:downarrow+verbatim")
        );
        assert_eq!(
            gesture_of(KeyboardLayout::Laptop, ScriptAction::SayAllFromCaret).as_deref(),
            Some("kb:a+verbatim")
        );
        assert_eq!(
            gesture_of(KeyboardLayout::Laptop, ScriptAction::ReviewNextLine).as_deref(),
            Some("kb:downarrow+verbatim")
        );
    }

    #[test]
    fn clearing_keys_are_never_bound_nor_caret_keys() {
        let clearing = clearing_keys();
        assert_eq!(clearing.len(), 5);
        let caret = caret_bindings();
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            let bindings = bindings_for(layout);
            for gesture in &clearing {
                assert!(bindings.iter().all(|(bound, _)| bound != gesture));
                assert!(caret.iter().all(|(observed, _)| observed != gesture));
            }
        }
    }

    #[test]
    fn caret_keys_are_distinct_and_never_bound() {
        let caret = caret_bindings();
        // 14 motions with and without Shift, 4 deletions, and Control+A.
        assert_eq!(caret.len(), 33);
        let unique: HashSet<&GestureId> = caret.iter().map(|(gesture, _)| gesture).collect();
        assert_eq!(unique.len(), caret.len());
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            let bindings = bindings_for(layout);
            for (gesture, _) in &caret {
                assert!(
                    bindings.iter().all(|(bound, _)| bound != gesture),
                    "{} is a caret key, not a command",
                    gesture.as_str()
                );
            }
        }
        let shift_end = GestureId::parse("kb:shift+end").expect("valid");
        assert!(caret.contains(&(
            shift_end,
            CaretKey {
                motion: CaretMotion::EndOfLine,
                select: true
            }
        )));
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
    fn both_layouts_bind_time_tray_list_and_report_focus_identically() {
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
        identical(ScriptAction::ReportFocus, "kb:tab+verbatim");
        assert_eq!(
            ScriptAction::ReportFocus.review_command(),
            Some(ReviewCommand::ReportFocus)
        );
    }

    #[test]
    fn the_laptop_layout_keeps_the_numpad_bindings() {
        let laptop = bindings_for(KeyboardLayout::Laptop);
        let desktop_own: Vec<GestureId> = DESKTOP_BINDINGS
            .iter()
            .map(|(identifier, _)| GestureId::parse(identifier).expect("valid"))
            .collect();
        for (gesture, action) in bindings_for(KeyboardLayout::Desktop)
            .into_iter()
            .filter(|(gesture, _)| !desktop_own.contains(gesture))
        {
            assert!(
                laptop.contains(&(gesture.clone(), action)),
                "{} is bound on every layout",
                gesture.as_str()
            );
        }
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

    #[test]
    fn gesture_map_for_contains_exactly_the_bound_gestures() {
        for layout in [KeyboardLayout::Desktop, KeyboardLayout::Laptop] {
            let bindings = bindings_for(layout);
            let map = gesture_map_for(&bindings);
            assert_eq!(map.len(), bindings.len());
            for (gesture, _) in &bindings {
                assert!(map.contains(gesture));
            }
        }
    }
}
