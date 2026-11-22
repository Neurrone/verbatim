//! The dispatcher's `FocusRead` and `Test` of the focus: the subject's
//! backend picks the executor's arm (`verbatim_ia2::batch::focus_read`
//! and `test_focus`, `verbatim_uia_rops::batch::focus`), and the
//! outpost's resolvers ([`super::msaa_object`], [`super::uia_focus`]) are
//! the one door from the arm to its state.

#![forbid(unsafe_code)]

use verbatim_ia2::batch::{FocusFound, FocusSubject};
use verbatim_model::TextDetails;
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyAnswer, PropertyId};
use verbatim_model::request::{FocusRead, KnownAncestor};

use super::RequestContext;
use super::read::{self, Client};

/// What a read of the application's focus found.
pub(super) enum FocusNow {
    /// The focus read.
    Read(Box<FocusRead>),
    /// No focus could be read, or a test of it found an MSAA focus without
    /// the focused state, and read nothing more.
    Unread,
}

/// The application's focus in `hwnd`, the window with the keyboard focus
/// taken already, through that window's backend's arm: its `FocusRead`,
/// or with `matcher` its `Test`, the focused state read first through
/// MSAA, with the test's result.
pub(super) fn focus_now(
    context: &RequestContext<'_>,
    client: &mut Client,
    (hwnd, matcher): (isize, Option<&Matcher>),
    read: (&[KnownAncestor], &[PropertyId], TextDetails),
) -> (FocusNow, Option<(bool, Vec<PropertyAnswer>)>) {
    if read::window_uses_uia(context, hwnd) {
        return super::uia_focus::focus_now(context, client, (hwnd, matcher), read);
    }
    let (found, tested) = match matcher {
        Some(matcher) => super::msaa_object::test_focus(context, client, (hwnd, matcher), read),
        None => (
            super::msaa_object::focus_read(
                context,
                client,
                FocusSubject::Focus {
                    hwnd,
                    if_focused: false,
                },
                read,
            ),
            None,
        ),
    };
    let now = match found {
        FocusFound::Read(_, read) => FocusNow::Read(read),
        FocusFound::Unfocused | FocusFound::NotThere => FocusNow::Unread,
    };
    (now, tested)
}

/// An acquired MSAA candidate's full read through the MSAA arm: `None`
/// when its ancestors cannot be read, which answers it not there.
pub(super) fn candidate(
    context: &RequestContext<'_>,
    client: &mut Client,
    candidate: verbatim_ia2::acquire::FocusCandidate,
    read: (&[KnownAncestor], &[PropertyId], TextDetails),
) -> Option<FocusRead> {
    match super::msaa_object::focus_read(context, client, FocusSubject::Candidate(candidate), read)
    {
        FocusFound::Read(_, read) => Some(*read),
        FocusFound::Unfocused | FocusFound::NotThere => None,
    }
}
