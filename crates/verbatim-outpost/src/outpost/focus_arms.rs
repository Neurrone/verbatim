//! The dispatcher's `FocusRead` through the executors' arms
//! (`verbatim_ia2::batch::focus_read` and
//! `verbatim_uia_rops::batch::focus`), chosen by
//! [`super::Outpost::set_focus_arms`] until the switch: the subject's
//! backend picks the executor, and the outpost's resolvers are the one
//! door from the arm to its state.

#![forbid(unsafe_code)]

use verbatim_ia2::batch::{FocusFound, FocusSubject};
use verbatim_model::TextDetails;
use verbatim_model::property::PropertyId;
use verbatim_model::request::{FocusRead, KnownAncestor};

use super::RequestContext;
use super::asked::FocusNow;
use super::read::{self, Client};

/// The application's focus in `hwnd`, the window with the keyboard focus
/// taken already, through that window's backend's arm, as
/// [`super::asked::focus_now`] reads it.
pub(super) fn focus_now(
    context: &RequestContext<'_>,
    client: &mut Client,
    hwnd: isize,
    set: &[PropertyId],
    (known, details, depth): (&[KnownAncestor], TextDetails, read::FocusDepth),
) -> FocusNow {
    if read::window_uses_uia(context, hwnd) {
        return super::uia_focus::focus_now(context, client, hwnd, set, (known, details));
    }
    let subject = FocusSubject::Focus {
        hwnd,
        if_focused: depth == read::FocusDepth::FullIfFocused,
    };
    match super::msaa_object::focus_read(context, client, subject, (known, set, details)) {
        FocusFound::Read(focused, read) => FocusNow::Read(focused, read),
        FocusFound::Unfocused => FocusNow::Unfocused,
        FocusFound::NotThere => FocusNow::NotThere,
    }
}

/// An acquired MSAA candidate's full read through the MSAA arm, as
/// [`super::asked::candidate_read`] reads it.
pub(super) fn candidate(
    context: &RequestContext<'_>,
    client: &mut Client,
    candidate: verbatim_ia2::acquire::FocusCandidate,
    (known, set, details): (&[KnownAncestor], &[PropertyId], TextDetails),
) -> Option<FocusRead> {
    match super::msaa_object::focus_read(
        context,
        client,
        FocusSubject::Candidate(candidate),
        (known, set, details),
    ) {
        FocusFound::Read(_, read) => Some(*read),
        FocusFound::Unfocused | FocusFound::NotThere => None,
    }
}
