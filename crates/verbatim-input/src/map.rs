//! The gesture map: the set of bound gesture identifiers the hook consults.
//!
//! In milestone M1 the map is exactly a set of bound [`GestureId`]s; resolving
//! a gesture to an action lives in the application's gesture router, not here.
//! The hook reads the map through an [`ArcSwap`] snapshot so rebinding is a
//! single atomic store that never blocks the never-blocking hook thread.

use std::collections::HashSet;
use std::sync::Arc;

use arc_swap::ArcSwap;
use verbatim_model::GestureId;

/// A lock-free, cheaply cloneable handle to the current gesture map.
///
/// The hook thread holds one of these and loads a snapshot per keystroke;
/// whoever owns bindings replaces the map with
/// [`ArcSwap::store`](arc_swap::ArcSwapAny::store). The snapshot the hook is
/// mid-read on stays valid until it is dropped, so a rebind never tears a
/// read.
pub type SharedGestureMap = Arc<ArcSwap<GestureMap>>;

/// The set of gesture identifiers currently bound to some action.
///
/// Membership is all the hook needs: it swallows and emits a gesture when the
/// map contains it, and otherwise leaves the keys alone (modulo the
/// modifier-companion trapping rule). Identifiers are already normalized by
/// [`GestureId`], so lookup is order- and case-insensitive.
#[derive(Clone, Debug, Default)]
pub struct GestureMap {
    bound: HashSet<GestureId>,
}

impl GestureMap {
    /// Builds a map from an iterator of bound gesture identifiers.
    #[must_use]
    pub fn new(gestures: impl IntoIterator<Item = GestureId>) -> Self {
        Self {
            bound: gestures.into_iter().collect(),
        }
    }

    /// Whether the given gesture is bound.
    #[must_use]
    pub fn contains(&self, gesture: &GestureId) -> bool {
        self.bound.contains(gesture)
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

    /// Wraps this map in a fresh [`SharedGestureMap`] handle.
    #[must_use]
    pub fn into_shared(self) -> SharedGestureMap {
        Arc::new(ArcSwap::from_pointee(self))
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
    fn into_shared_round_trips() {
        let shared = GestureMap::new([id("kb:f6")]).into_shared();
        assert!(shared.load().contains(&id("kb:f6")));
    }
}
