//! The duplicate tests that judge a focus against the readers' last focus,
//! with no read (`docs/parity/focus-events.md`, "Duplicate focus
//! suppression"), private to the rows of `super::validity`. With the
//! readers' last focus empty every test finds nothing, as with no earlier
//! focus.

use verbatim_model::property::RuntimeId;
use verbatim_model::request::MsaaIds;
use verbatim_model::tell::{Checked, Read};

use super::versions::{LastFocus, ReadBy};

/// Whether an MSAA focus named by `ids` is a duplicate of `last`: both
/// MSAA, equal window, object and child ids, and the child id the earlier
/// object is held by not above 0, as NVDA tests its object's own child
/// id, not its event's (`docs/parity/focus-events.md`, "Duplicate focus
/// suppression"): an event on a child that resolved to a full object, held
/// by child 0, is a duplicate when repeated, and a list's focus the
/// outpost redirected to its focused item, which carries the item's ids,
/// never is. A focus read through `accFocus`
/// names no event ids, so nothing is its duplicate.
pub(super) fn msaa_duplicate(last: Option<&LastFocus>, ids: &MsaaIds) -> bool {
    let Some(last) = last else {
        return false;
    };
    match &last.read_by {
        ReadBy::Msaa {
            event: Some(earlier),
            held_child,
        } => earlier == ids && *held_child <= 0,
        ReadBy::Msaa { event: None, .. } | ReadBy::Uia { .. } | ReadBy::Plain => false,
    }
}

/// Whether a UI Automation focus on `runtime_id`, whose check found
/// `checked`, is a duplicate of `last`: a UI Automation object, equal
/// runtime ids, and the earlier element still
/// has the keyboard focus, by the answer the check carried for the token
/// that is the readers' last focus. A failed read, or the listener no
/// longer holding the earlier element, is not a duplicate.
pub(super) fn uia_duplicate(
    last: Option<&LastFocus>,
    runtime_id: &RuntimeId,
    checked: Option<&Checked>,
) -> bool {
    let Some(last) = last else {
        return false;
    };
    let ReadBy::Uia {
        runtime_id: earlier,
        token: Some(token),
    } = &last.read_by
    else {
        return false;
    };
    earlier == runtime_id
        && checked.is_some_and(|checked| {
            checked
                .kept
                .iter()
                .any(|(kept, focused)| kept == token && *focused == Read::Answered(true))
        })
}

#[cfg(test)]
mod tests {
    use verbatim_model::tell::ElementToken;

    use verbatim_model::WindowHandle;

    use super::super::versions::Place;
    use super::*;

    fn ids(window: u64, object: i32, child: i32) -> MsaaIds {
        MsaaIds {
            window: WindowHandle(window),
            object,
            child,
        }
    }

    fn place(window: u64) -> Place {
        Place {
            window: WindowHandle(window),
            top_level: WindowHandle(1),
            root_owner: WindowHandle(1),
            ancestor_windows: vec![WindowHandle(2), WindowHandle(1)].into(),
        }
    }

    fn msaa_last(event: Option<MsaaIds>) -> LastFocus {
        let held_child = event.map_or(0, |ids| ids.child);
        msaa_last_held(event, held_child)
    }

    fn msaa_last_held(event: Option<MsaaIds>, held_child: i32) -> LastFocus {
        LastFocus {
            read_by: ReadBy::Msaa { event, held_child },
            place: place(3),
            role: verbatim_model::Role::Button,
            since: None,
        }
    }

    fn uia_last(token: Option<u64>) -> LastFocus {
        LastFocus {
            read_by: ReadBy::Uia {
                runtime_id: RuntimeId(vec![42, 1]),
                token: token.map(ElementToken),
            },
            place: place(3),
            role: verbatim_model::Role::Button,
            since: None,
        }
    }

    fn checked(kept: Vec<(u64, Read<bool>)>) -> Checked {
        Checked {
            probe: None,
            keyboard_focus: Some(Read::Answered(true)),
            kept: kept
                .into_iter()
                .map(|(token, read)| (ElementToken(token), read))
                .collect(),
            previous_sibling: None,
        }
    }

    #[test]
    fn an_msaa_focus_with_the_last_focuss_ids_and_no_child_is_a_duplicate() {
        let last = msaa_last(Some(ids(3, -4, 0)));
        assert!(msaa_duplicate(Some(&last), &ids(3, -4, 0)));
    }

    #[test]
    fn an_msaa_focus_on_a_child_above_zero_is_never_a_duplicate() {
        let last = msaa_last(Some(ids(3, -4, 2)));
        assert!(!msaa_duplicate(Some(&last), &ids(3, -4, 2)));
    }

    /// An object held by child 3 is no duplicate whatever ids its event
    /// named, as NVDA's object's own child id is above 0
    /// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
    #[test]
    fn a_repeat_of_a_focus_held_by_a_child_above_zero_is_not_a_duplicate() {
        let last = msaa_last_held(Some(ids(3, -4, 0)), 3);
        assert!(!msaa_duplicate(Some(&last), &ids(3, -4, 0)));
    }

    /// An event on a child above 0 that resolved to a full object, held
    /// by child 0: its repeat is a duplicate, as NVDA's object's own child
    /// id is then 0 (`docs/parity/focus-events.md`, "Duplicate focus
    /// suppression").
    #[test]
    fn a_repeat_of_a_child_event_held_as_a_full_object_is_a_duplicate() {
        let last = msaa_last_held(Some(ids(3, -4, 5)), 0);
        assert!(msaa_duplicate(Some(&last), &ids(3, -4, 5)));
    }

    #[test]
    fn an_msaa_focus_with_other_ids_is_not_a_duplicate() {
        let last = msaa_last(Some(ids(3, -4, 0)));
        assert!(!msaa_duplicate(Some(&last), &ids(3, -4, 1)));
        assert!(!msaa_duplicate(Some(&last), &ids(9, -4, 0)));
    }

    #[test]
    fn nothing_is_a_duplicate_of_a_focus_read_through_acc_focus() {
        let last = msaa_last(None);
        assert!(!msaa_duplicate(Some(&last), &ids(3, -4, 0)));
    }

    #[test]
    fn nothing_is_a_duplicate_with_the_last_focus_empty() {
        assert!(!msaa_duplicate(None, &ids(3, -4, 0)));
        assert!(!uia_duplicate(
            None,
            &RuntimeId(vec![42, 1]),
            Some(&checked(vec![(5, Read::Answered(true))]))
        ));
    }

    #[test]
    fn an_msaa_focus_is_no_duplicate_of_a_uia_last_focus() {
        assert!(!msaa_duplicate(Some(&uia_last(Some(5))), &ids(3, -4, 0)));
    }

    #[test]
    fn a_uia_focus_on_the_last_focuss_element_still_focused_is_a_duplicate() {
        let last = uia_last(Some(5));
        let found = checked(vec![(4, Read::Answered(false)), (5, Read::Answered(true))]);
        assert!(uia_duplicate(
            Some(&last),
            &RuntimeId(vec![42, 1]),
            Some(&found)
        ));
    }

    #[test]
    fn a_uia_focus_whose_earlier_element_lost_the_focus_is_not_a_duplicate() {
        let last = uia_last(Some(5));
        let found = checked(vec![(5, Read::Answered(false))]);
        assert!(!uia_duplicate(
            Some(&last),
            &RuntimeId(vec![42, 1]),
            Some(&found)
        ));
    }

    #[test]
    fn a_failed_read_of_the_earlier_element_is_not_a_duplicate() {
        let last = uia_last(Some(5));
        let found = checked(vec![(5, Read::Failed)]);
        assert!(!uia_duplicate(
            Some(&last),
            &RuntimeId(vec![42, 1]),
            Some(&found)
        ));
    }

    #[test]
    fn a_uia_focus_on_another_runtime_id_is_not_a_duplicate() {
        let last = uia_last(Some(5));
        let found = checked(vec![(5, Read::Answered(true))]);
        assert!(!uia_duplicate(
            Some(&last),
            &RuntimeId(vec![42, 2]),
            Some(&found)
        ));
    }

    #[test]
    fn a_last_focus_the_listener_no_longer_holds_has_no_duplicate() {
        let last = uia_last(None);
        let found = checked(vec![(5, Read::Answered(true))]);
        assert!(!uia_duplicate(
            Some(&last),
            &RuntimeId(vec![42, 1]),
            Some(&found)
        ));
    }
}
