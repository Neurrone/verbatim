//! The narrow ordering rule (design, "The narrow ordering rule"; ADR
//! 0003): when the answer that completes a focus read arrives and its
//! ancestors include the committed focus by runtime id, Core first judges
//! the committed focus's waiting UI Automation state tells, in their order,
//! then takes the focus. Name and value changes are not moved.
//! Verbatim-only: the listener's and an outpost's events come from two
//! processes with no order between them, so a state change the outpost
//! told, such as a combo box's "expanded", can arrive behind the focus on
//! the list item the listener told.

use verbatim_model::property::PropertyId;
use verbatim_model::request::FocusRead;

use super::versions::ReadBy;
use crate::entries::{Entries, Entry};
use crate::event_queue::Cx;

/// Judges the committed focus's waiting UI Automation state tells ahead of
/// the focus `read`, when its ancestors include the committed focus by
/// runtime id.
pub(crate) fn judge_ahead(cx: &mut Cx<Entries>, read: &FocusRead) {
    let committed = match cx
        .facts()
        .versions
        .committed_read()
        .map(|kept| &kept.read_by)
    {
        Some(ReadBy::Uia { runtime_id, .. }) => runtime_id.clone(),
        Some(ReadBy::Msaa { .. }) | None => return,
    };
    let under = read
        .ancestors
        .iter()
        .any(|ancestor| ancestor.node.runtime_id.as_ref() == Some(&committed));
    if !under {
        return;
    }
    let taken = cx.take_waiting(|entry| {
        matches!(entry, Entry::UiaChange(change)
            if change.property == PropertyId::States && change.runtime_id == committed)
    });
    for entry in taken {
        if let Entry::UiaChange(change) = entry {
            crate::changes::uia(cx, &change);
        }
    }
}
