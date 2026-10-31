//! The gesture map: the set of bound gesture identifiers the hook consults.
//!
//! In milestone M1 the map is exactly a set of bound [`GestureId`]s; resolving
//! a gesture to an action lives in the application's gesture router, not here.
//! The decision machine owns its map, given when it is made: nothing changes
//! the bindings while Verbatim runs.

use std::collections::BTreeSet;

use verbatim_model::GestureId;

/// The set of gesture identifiers currently bound to some action, and the
/// set of gestures observed on their way to the application.
///
/// Membership is all the hook needs: it swallows and emits a gesture when the
/// map contains it, and otherwise leaves the keys alone, apart from the
/// Verbatim modifier's own transitions, which the decision machine swallows.
/// An observed gesture (a caret key, [`crate::caret_bindings`]) is passed to
/// the application as usual and reported as well, so the reducer can speak
/// what the application did with it. Identifiers are already normalized by
/// [`GestureId`], so lookup is order- and case-insensitive.
#[derive(Clone, Debug, Default)]
pub struct GestureMap {
    bound: BTreeSet<GestureId>,
    observed: BTreeSet<GestureId>,
}

impl GestureMap {
    /// Builds a map from an iterator of bound gesture identifiers.
    #[must_use]
    pub fn new(gestures: impl IntoIterator<Item = GestureId>) -> Self {
        Self {
            bound: gestures.into_iter().collect(),
            observed: BTreeSet::new(),
        }
    }

    /// This map, also observing `gestures`: passing them to the application
    /// and reporting them. A gesture both bound and observed is bound.
    #[must_use]
    pub fn with_observed(mut self, gestures: impl IntoIterator<Item = GestureId>) -> Self {
        self.observed.extend(gestures);
        self
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
