//! The gesture map: the gestures the hook consults, bound, observed and
//! held, each held one with its caret key. It is built from the one
//! gesture table ([`crate::Tables::gesture_map`]); resolving a bound
//! gesture to its action is the table's. The decision machine owns its
//! map, given when it is made: nothing changes the bindings while
//! Verbatim runs.

use std::collections::{BTreeMap, BTreeSet};

use verbatim_model::{CaretKey, GestureId};

/// The set of gesture identifiers currently bound to some action, and the
/// set of gestures observed on their way to the application.
///
/// Membership is all the hook needs: it swallows and emits a gesture when the
/// map contains it, and otherwise leaves the keys alone, apart from the
/// Verbatim modifier's own transitions, which the decision machine swallows.
/// An observed gesture (a caret key, [`crate::caret_bindings`]) is passed to
/// the application as usual and reported as well, so the reducer can speak
/// what the application did with it. Some observed gestures are held
/// ([`GestureMap::holds`]): while Core's view lets keys be held, their key
/// is held back for its caret to be read and sent again
/// (`books/dev/src/explanation/design/key-holding.md`). Identifiers are already normalized by
/// [`GestureId`], so lookup is order- and case-insensitive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GestureMap {
    bound: BTreeSet<GestureId>,
    observed: BTreeSet<GestureId>,
    /// The held gestures, each with its caret key.
    held: BTreeMap<GestureId, CaretKey>,
    /// The held gestures observed only when held: Enter, which passed is
    /// typed text, reported as the text it types.
    held_only: BTreeSet<GestureId>,
}

impl GestureMap {
    /// Builds a map from an iterator of bound gesture identifiers.
    #[must_use]
    pub fn new(gestures: impl IntoIterator<Item = GestureId>) -> Self {
        Self {
            bound: gestures.into_iter().collect(),
            observed: BTreeSet::new(),
            held: BTreeMap::new(),
            held_only: BTreeSet::new(),
        }
    }

    /// This map, also observing `gestures`: passing them to the application
    /// and reporting them. A gesture both bound and observed is bound.
    #[must_use]
    pub fn with_observed(mut self, gestures: impl IntoIterator<Item = GestureId>) -> Self {
        self.observed.extend(gestures);
        self
    }

    /// This map, also observing and holding `gestures`, each with its
    /// caret key: the caret keys, held back while Core's view lets keys be
    /// held.
    #[must_use]
    pub fn with_held(mut self, gestures: impl IntoIterator<Item = (GestureId, CaretKey)>) -> Self {
        for (gesture, key) in gestures {
            self.observed.insert(gesture.clone());
            self.held.insert(gesture, key);
        }
        self
    }

    /// This map, also holding `gestures` while Core's view lets them be
    /// held, and neither observing nor reporting them as gestures when it
    /// does not: Enter, which then passes as typed text
    /// (`books/dev/src/explanation/design/key-holding.md`).
    #[must_use]
    pub fn with_held_only(
        mut self,
        gestures: impl IntoIterator<Item = (GestureId, CaretKey)>,
    ) -> Self {
        for (gesture, key) in gestures {
            self.held.insert(gesture.clone(), key);
            self.held_only.insert(gesture);
        }
        self
    }

    /// The caret key of the given gesture when it is held: observed, or
    /// held only, and its key held back while Core's view lets keys be
    /// held.
    #[must_use]
    pub fn holds(&self, gesture: &GestureId) -> Option<CaretKey> {
        self.held.get(gesture).copied()
    }

    /// Whether the given gesture is observed only when held
    /// ([`GestureMap::with_held_only`]).
    #[must_use]
    pub fn holds_only(&self, gesture: &GestureId) -> bool {
        self.held_only.contains(gesture)
    }

    /// Whether the given gesture is bound.
    #[must_use]
    pub fn contains(&self, gesture: &GestureId) -> bool {
        self.bound.contains(gesture)
    }

    /// Whether the given gesture is observed.
    #[must_use]
    pub fn observes(&self, gesture: &GestureId) -> bool {
        self.observed.contains(gesture)
    }

    /// The number of bound gestures.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bound.len()
    }

    /// Whether no gestures are bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> GestureId {
        GestureId::parse(raw).expect("valid identifier")
    }

    #[test]
    fn membership_is_normalized() {
        let map = GestureMap::new([id("kb:Verbatim+V")]);
        // A differently ordered, differently cased identifier still matches.
        assert!(map.contains(&id("kb:v+verbatim")));
        assert!(!map.contains(&id("kb:v")));
        assert_eq!(map.len(), 1);
        assert!(!map.is_empty());
    }

    #[test]
    fn empty_map_binds_nothing() {
        let map = GestureMap::default();
        assert!(map.is_empty());
        assert!(!map.contains(&id("kb:v+verbatim")));
    }

    #[test]
    fn observed_gestures_are_not_bound() {
        let map = GestureMap::new([id("kb:f6")]).with_observed([id("kb:leftarrow")]);
        assert!(map.observes(&id("kb:leftarrow")));
        assert!(!map.contains(&id("kb:leftarrow")));
        assert!(!map.observes(&id("kb:f6")));
        assert_eq!(map.len(), 1);
    }
}
