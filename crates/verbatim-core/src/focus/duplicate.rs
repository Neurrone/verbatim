//! The tests that judge a focus against the readers' last focus, with no
//! read (design, "Focus and foreground"; `docs/parity/focus-events.md`,
//! "Duplicate focus suppression"): the two duplicate tests, the
//! whole-window refusal and a run's refusals. With the readers' last focus
//! empty every test finds nothing, as with no earlier focus.

use verbatim_model::WindowHandle;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{MsaaIds, ObjectClass};
use verbatim_model::tell::{Checked, Read};

use super::versions::{LastFocus, ReadBy};

/// Whether an MSAA focus named by `ids` is a duplicate of `last`: both
/// MSAA, equal window, object and child ids, the earlier child id not
/// above 0. A focus read through `accFocus` names no event ids, so nothing
/// is its duplicate. The class verdict may change the test; no class with
/// rules of its own is read yet.
pub(crate) fn msaa_duplicate(last: Option<&LastFocus>, ids: &MsaaIds) -> bool {
    let Some(last) = last else {
        return false;
    };
    let ReadBy::Msaa {
        event: Some(earlier),
        ..
    } = &last.read_by
    else {
        return false;
    };
    match last.class {
        ObjectClass::Plain => earlier == ids && earlier.child <= 0,
    }
}

/// Whether a UI Automation focus on `runtime_id`, whose check found
/// `checked`, is a duplicate of `last`: a UI Automation object whose class
/// allows no duplicate, equal runtime ids, and the earlier element still
/// has the keyboard focus, by the answer the check carried for the token
/// that is the readers' last focus. A failed read, or the listener no
/// longer holding the earlier element, is not a duplicate.
pub(crate) fn uia_duplicate(
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
    let allows_duplicate = match last.class {
        ObjectClass::Plain => false,
    };
    !allows_duplicate
        && earlier == runtime_id
        && checked.is_some_and(|checked| {
            checked
                .kept
                .iter()
                .any(|(kept, focused)| kept == token && *focused == Read::Answered(true))
        })
}

/// Whether a foreground tell for the window `top_level` is refused as one
/// of the window the readers' last focus is in.
pub(crate) fn whole_window_refused(last: Option<&LastFocus>, top_level: WindowHandle) -> bool {
    last.is_some_and(|last| last.place.top_level == top_level)
}

/// Why a run's candidate is refused with no read, judged against the
/// readers' last focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunRefusal {
    /// A foreground tell whose window is not the foreground record's.
    NotInFront,
    /// A foreground tell of the last focus's window or an ancestor's.
    LastFocusWindow,
    /// A foreground tell whose ids equal the last focus's.
    SameIds,
}

/// What a run makes of one candidate before asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunJudgement {
    /// Refused, with no read.
    Refused(RunRefusal),
    /// An MSAA duplicate of the last focus: it ends the run as its valid
    /// focus, with no read.
    Duplicate,
    /// Tried by its application's outpost.
    Try,
}

/// How a run judges the candidate named by `ids`, a foreground tell's when
/// `foreground`, while `in_front` is the foreground record's window.
pub(crate) fn judge_candidate(
    last: Option<&LastFocus>,
    ids: &MsaaIds,
    foreground: bool,
    in_front: Option<WindowHandle>,
) -> RunJudgement {
    if foreground {
        if in_front != Some(ids.window) {
            return RunJudgement::Refused(RunRefusal::NotInFront);
        }
        if let Some(last) = last {
            if last.place.holds_window(ids.window) {
                return RunJudgement::Refused(RunRefusal::LastFocusWindow);
            }
            if matches!(&last.read_by, ReadBy::Msaa { event: Some(earlier), .. } if earlier == ids)
            {
                return RunJudgement::Refused(RunRefusal::SameIds);
            }
        }
        return RunJudgement::Try;
    }
    if msaa_duplicate(last, ids) {
        RunJudgement::Duplicate
    } else {
        RunJudgement::Try
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::tell::ElementToken;

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
            ancestor_windows: vec![WindowHandle(2), WindowHandle(1)],
        }
    }

    fn msaa_last(event: Option<MsaaIds>) -> LastFocus {
        LastFocus {
            read_by: ReadBy::Msaa {
                event,
                held_child: 0,
            },
            place: place(3),
            class: ObjectClass::Plain,
        }
    }

    fn uia_last(token: Option<u64>) -> LastFocus {
        LastFocus {
            read_by: ReadBy::Uia {
                runtime_id: RuntimeId(vec![42, 1]),
                token: token.map(ElementToken),
            },
            place: place(3),
            class: ObjectClass::Plain,
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

    #[test]
    fn a_foreground_tell_of_the_last_focuss_top_level_window_is_refused() {
        let last = msaa_last(None);
        assert!(whole_window_refused(Some(&last), WindowHandle(1)));
        assert!(!whole_window_refused(Some(&last), WindowHandle(7)));
        assert!(!whole_window_refused(None, WindowHandle(1)));
    }

    #[test]
    fn a_foreground_candidate_not_in_front_is_refused() {
        assert_eq!(
            judge_candidate(None, &ids(7, 0, 0), true, Some(WindowHandle(8))),
            RunJudgement::Refused(RunRefusal::NotInFront)
        );
    }

    #[test]
    fn a_foreground_candidate_of_the_last_focuss_window_or_an_ancestors_is_refused() {
        let last = uia_last(Some(5));
        for window in [3, 2, 1] {
            assert_eq!(
                judge_candidate(
                    Some(&last),
                    &ids(window, 0, 0),
                    true,
                    Some(WindowHandle(window))
                ),
                RunJudgement::Refused(RunRefusal::LastFocusWindow)
            );
        }
    }

    #[test]
    fn a_foreground_candidate_with_the_last_focuss_ids_is_refused() {
        let mut last = msaa_last(Some(ids(9, 0, 0)));
        last.place = Place {
            window: WindowHandle(8),
            top_level: WindowHandle(8),
            root_owner: WindowHandle(8),
            ancestor_windows: Vec::new(),
        };
        assert_eq!(
            judge_candidate(Some(&last), &ids(9, 0, 0), true, Some(WindowHandle(9))),
            RunJudgement::Refused(RunRefusal::SameIds)
        );
    }

    #[test]
    fn a_foreground_candidate_in_front_and_elsewhere_is_tried() {
        let last = msaa_last(Some(ids(3, -4, 0)));
        assert_eq!(
            judge_candidate(Some(&last), &ids(7, 0, 0), true, Some(WindowHandle(7))),
            RunJudgement::Try
        );
    }

    #[test]
    fn a_focus_candidate_duplicating_the_last_focus_ends_the_run() {
        let last = msaa_last(Some(ids(3, -4, 0)));
        assert_eq!(
            judge_candidate(Some(&last), &ids(3, -4, 0), false, None),
            RunJudgement::Duplicate
        );
        assert_eq!(
            judge_candidate(Some(&last), &ids(3, -4, 1), false, None),
            RunJudgement::Try
        );
    }
}
