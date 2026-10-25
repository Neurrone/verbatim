//! The system tray icons and taskbar buttons the systrayList replica
//! dialog lists (roadmap, milestone M3), and which button an item chosen
//! from an earlier list is when it is clicked ([`refind`]).
//!
//! The GUI makes no UIA connection into the shell's windows: it asks its
//! [`ShellItemsHost`], which the app implements by sending File Explorer's
//! outpost a query; the outpost reads the items, or the positions of the
//! buttons that may be the one item clicked, which no list carries, within
//! its own application
//! (`docs/design/listener-without-connections.md`, decision 9) and the
//! answer comes back through the host's delivery callback, on another
//! thread, which the GUI uses to send it down the same channel
//! [`GuiHandle`](crate::GuiHandle) posts commands through. The GUI thread
//! never waits for the shell. Which of those buttons is the item, if any,
//! the GUI decides, so a click never lands on a button that merely took the
//! item's place or name.

use verbatim_model::Rect;
pub use verbatim_model::{ShellCandidate, ShellCandidates, ShellItem, ShellItemKind};

/// Where an enumeration's outcome goes: `Some(items)` on success, possibly
/// empty, or `None` when it failed or passed its deadline (already logged).
/// Called once, on a thread that is not the GUI's.
pub type DeliverShellItems = Box<dyn FnOnce(Option<Vec<ShellItem>>) + Send>;

/// Where a position's outcome goes: `Some(candidates)`, the buttons that
/// may be the chosen item now, or `None` when the request failed or passed
/// its deadline (already logged). Called once, on a thread that is not the
/// GUI's.
pub type DeliverShellItemPosition = Box<dyn FnOnce(Option<ShellCandidates>) + Send>;

/// What the GUI asks its [`ShellItemsHost`] about one shell surface.
pub enum ShellAsk {
    /// The surface's items, for the list.
    Items(DeliverShellItems),
    /// Where an item chosen from an earlier list of the surface is now, for
    /// a click on it: the outpost answers the buttons with its runtime id or
    /// its name, and [`refind`] picks the item from them.
    Position(ShellItem, DeliverShellItemPosition),
}

impl ShellAsk {
    /// Hands the asker the outcome of a request that got no answer.
    pub fn answer_nothing(self) {
        match self {
            Self::Items(deliver) => deliver(None),
            Self::Position(_, deliver) => deliver(None),
        }
    }
}

/// What reads the shell's items for the GUI: the app, which asks the
/// outpost of the process that owns the taskbar.
pub trait ShellItemsHost: Send + Sync {
    /// Asks `ask` of `kind`'s surface and returns at once. Its delivery
    /// callback is called exactly once with the outcome when this returns
    /// true; false means the request could not be made (logged), and the
    /// callback is never called.
    fn request(&self, kind: ShellItemKind, ask: ShellAsk) -> bool;
}

/// The button among `found` that is `chosen`, an item the user picked from
/// an earlier list, wherever it is now: the candidate with the same runtime
/// id, or else the only candidate with the same name, taken only when
/// `name_unique` (no other item of the list `chosen` was picked from had
/// its name) and the walk was complete. `None` when it is gone, or when its
/// name alone cannot tell it from another button's, so a click never lands
/// on one that merely took its place: when one of two "Volume" icons has
/// gone, the other is not clicked in its stead.
#[must_use]
pub fn refind<'a>(
    chosen: &ShellItem,
    name_unique: bool,
    found: &'a ShellCandidates,
) -> Option<&'a ShellCandidate> {
    if !chosen.runtime_id.is_empty()
        && let Some(candidate) = found
            .candidates
            .iter()
            .find(|candidate| candidate.item.runtime_id == chosen.runtime_id)
    {
        return Some(candidate);
    }
    if !name_unique || !found.complete {
        return None;
    }
    let mut named = found
        .candidates
        .iter()
        .filter(|candidate| candidate.item.name == chosen.name);
    match (named.next(), named.next()) {
        (Some(candidate), None) => Some(candidate),
        _ => None,
    }
}

/// The rectangle a click on `candidate` targets: none when it is off the
/// screen or has no rectangle or an empty one, since a click targets its
/// center.
#[must_use]
pub fn usable_position(candidate: &ShellCandidate) -> Option<Rect> {
    if candidate.offscreen {
        return None;
    }
    candidate
        .rect
        .filter(|rect| rect.width > 0 && rect.height > 0)
}

/// The screen point click actions target: the center of `rect`.
#[must_use]
pub fn center_of(rect: Rect) -> (i32, i32) {
    // Saturating, since the rectangle came from another process.
    (
        rect.left.saturating_add(rect.width / 2),
        rect.top.saturating_add(rect.height / 2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: i32, height: i32) -> Rect {
        Rect {
            left: 100,
            top: 200,
            width,
            height,
        }
    }

    fn item(name: &str, runtime_id: &[i32]) -> ShellItem {
        ShellItem {
            name: name.into(),
            runtime_id: runtime_id.to_vec(),
        }
    }

    fn candidate(name: &str, runtime_id: &[i32]) -> ShellCandidate {
        ShellCandidate {
            item: item(name, runtime_id),
            offscreen: false,
            rect: Some(rect(24, 24)),
        }
    }

    fn found(candidates: Vec<ShellCandidate>, complete: bool) -> ShellCandidates {
        ShellCandidates {
            candidates,
            complete,
        }
    }

    #[test]
    fn a_moved_item_is_found_by_its_runtime_id() {
        let chosen = item("Volume", &[42, 1]);
        let found = found(
            vec![candidate("Volume", &[42, 3]), candidate("Volume", &[42, 1])],
            false,
        );
        assert_eq!(
            refind(&chosen, false, &found),
            Some(&found.candidates[1]),
            "the runtime id decides even with a twin, a shared name and an incomplete walk"
        );
    }

    #[test]
    fn an_item_with_a_new_runtime_id_is_found_by_its_unique_name() {
        let chosen = item("Volume", &[42, 1]);
        let found = found(vec![candidate("Volume", &[42, 7])], true);
        assert_eq!(refind(&chosen, true, &found), Some(&found.candidates[0]));
    }

    #[test]
    fn a_name_shared_when_the_list_was_made_finds_nothing() {
        let chosen = item("Volume", &[42, 1]);
        let found = found(vec![candidate("Volume", &[42, 4])], true);
        assert_eq!(
            refind(&chosen, false, &found),
            None,
            "the other of two Volume icons is not clicked when the chosen one has gone"
        );
    }

    #[test]
    fn two_candidates_of_the_name_find_nothing() {
        let chosen = item("Volume", &[42, 1]);
        let found = found(
            vec![candidate("Volume", &[42, 3]), candidate("Volume", &[42, 4])],
            true,
        );
        assert_eq!(refind(&chosen, true, &found), None);
    }

    #[test]
    fn an_incomplete_walk_finds_nothing_by_name() {
        let chosen = item("Volume", &[42, 1]);
        let found = found(vec![candidate("Volume", &[42, 7])], false);
        assert_eq!(refind(&chosen, true, &found), None);
    }

    #[test]
    fn an_onscreen_rectangle_is_the_position() {
        assert_eq!(
            usable_position(&candidate("Clock", &[1])),
            Some(rect(24, 24))
        );
    }

    #[test]
    fn an_offscreen_button_has_no_position() {
        let offscreen = ShellCandidate {
            offscreen: true,
            ..candidate("Clock", &[1])
        };
        assert_eq!(usable_position(&offscreen), None);
    }

    #[test]
    fn a_missing_or_empty_rectangle_is_no_position() {
        let with = |rect| ShellCandidate {
            rect,
            ..candidate("Clock", &[1])
        };
        assert_eq!(usable_position(&with(None)), None);
        assert_eq!(
            usable_position(&with(Some(rect(0, 24)))),
            None,
            "a zero-width rectangle has no clickable center"
        );
        assert_eq!(
            usable_position(&with(Some(rect(24, 0)))),
            None,
            "a zero-height rectangle has no clickable center"
        );
    }

    #[test]
    fn answering_nothing_hands_a_position_ask_none() {
        let outcomes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = std::sync::Arc::clone(&outcomes);
        ShellAsk::Position(
            item("Clock", &[1]),
            Box::new(move |position| kept.lock().expect("outcomes").push(position)),
        )
        .answer_nothing();
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }

    #[test]
    fn center_is_the_rectangle_midpoint() {
        assert_eq!(center_of(rect(24, 32)), (112, 216));
        assert_eq!(
            center_of(Rect {
                left: -10,
                top: 5,
                width: 4,
                height: 3,
            }),
            (-8, 6),
            "integer division truncates toward zero, still inside the rectangle"
        );
    }
}
