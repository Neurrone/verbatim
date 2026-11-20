//! The one gesture table: what each gesture means, from which the hook's
//! gesture map is built ([`Tables::gesture_map`]) and on which the hook
//! thread looks up what a gesture or an observed key is to the reducer.
//! It is made when the hook starts and never changed.

use std::collections::{BTreeMap, BTreeSet};

use verbatim_model::{CaretKey, CaretMotion, GestureId, LockKey};

use crate::map::GestureMap;
use crate::scripts::{
    KeyboardLayout, ScriptAction, bindings_for, caret_bindings, clearing_keys, editing_chords,
    selection_bindings,
};
use crate::toggle::ToggleKey;

/// What a gesture or an observed key is to the reducer, looked up on the
/// hook thread: fixed when the hook starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tables {
    /// The caret keys, moving and selecting, by gesture.
    pub caret_keys: BTreeMap<GestureId, CaretKey>,
    /// The keys that end or clear the command line being typed.
    pub clearing_keys: BTreeSet<GestureId>,
    /// The editing chords.
    pub editing_chords: BTreeSet<GestureId>,
    /// The gesture that shows Verbatim's menu.
    pub show_menu: GestureId,
    /// The layout's bound scripts, by gesture.
    pub scripts: BTreeMap<GestureId, ScriptAction>,
    /// The lock keys whose new state is announced, by gesture.
    pub toggles: BTreeMap<GestureId, LockKey>,
}

impl Tables {
    /// The tables for keyboard layout `layout`, the menu shown by
    /// `show_menu`.
    #[must_use]
    pub fn for_layout(layout: KeyboardLayout, show_menu: GestureId) -> Self {
        Self {
            caret_keys: caret_bindings()
                .into_iter()
                .chain(selection_bindings())
                .collect(),
            clearing_keys: clearing_keys().into_iter().collect(),
            editing_chords: editing_chords().into_iter().collect(),
            show_menu,
            scripts: bindings_for(layout).into_iter().collect(),
            toggles: [
                ToggleKey::CapsLock,
                ToggleKey::NumLock,
                ToggleKey::ScrollLock,
            ]
            .into_iter()
            .map(|toggle| (toggle.gesture(), toggle.lock_key()))
            .collect(),
        }
    }

    /// The gesture map the hook's decision machine owns: the menu gesture
    /// and every script bound, the clearing keys, editing chords and
    /// selection keys observed, and the other caret keys held with their
    /// caret key. They reach the application, held back while the focus
    /// has text until its caret is read; Enter is held only, in a text
    /// field other than a terminal, and otherwise typed text. The selection
    /// keys pass at once.
    #[must_use]
    pub fn gesture_map(&self) -> GestureMap {
        let caret = |select: bool, new_line: bool| {
            self.caret_keys
                .iter()
                .filter(move |(_, key)| {
                    key.select == select && (key.motion == CaretMotion::NewLine) == new_line
                })
                .map(|(gesture, key)| (gesture.clone(), *key))
        };
        GestureMap::new(std::iter::once(self.show_menu.clone()).chain(self.scripts.keys().cloned()))
            .with_observed(self.clearing_keys.iter().cloned())
            .with_observed(self.editing_chords.iter().cloned())
            .with_observed(
                caret(true, false)
                    .chain(caret(true, true))
                    .map(|(gesture, _)| gesture),
            )
            .with_held(caret(false, false))
            .with_held_only(caret(false, true))
    }
}
