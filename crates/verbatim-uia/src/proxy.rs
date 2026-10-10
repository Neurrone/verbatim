//! UIA's MSAA proxy told never to turn `WinEvent`s into the UIA events
//! Verbatim handles, as NVDA tells it (`ignoreWinEventsMap`, applied where
//! `source/UIAHandler/__init__.py` creates its client; NVDA issue 7345).
//!
//! For a window with no UIA provider of its own, UIA's proxies listen to the
//! application's `WinEvent`s and raise UIA events from them, in the client's
//! process, with calls into the application to build them. The outposts
//! handle such a window through MSAA and drop its UIA events, and NVDA found
//! that the mapping can leave the UIA client library unresponsive when an
//! application with a slow message pump raises `WinEvent`s. Focus changes
//! keep their mapping, as NVDA keeps it.

use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationProxyFactoryEntry, UIA_ActiveTextPositionChangedEventId,
    UIA_AutomationPropertyChangedEventId, UIA_EVENT_ID, UIA_MenuOpenedEventId,
    UIA_NotificationEventId, UIA_PROPERTY_ID, UIA_SelectionItem_ElementSelectedEventId,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};

use crate::com::take_i32_safearray;
use crate::subscribe::FOCUS_PROPERTIES;

/// The events Verbatim subscribes to that UIA's proxies are told not to
/// raise from `WinEvent`s, each with its property: every property the
/// focus-following subscription follows, for property changes, and 0 for
/// the automation events.
fn ignored_events() -> Vec<(UIA_EVENT_ID, UIA_PROPERTY_ID)> {
    let automation_events = [
        UIA_SelectionItem_ElementSelectedEventId,
        UIA_MenuOpenedEventId,
        UIA_NotificationEventId,
        UIA_Text_TextSelectionChangedEventId,
        UIA_Text_TextChangedEventId,
        UIA_ActiveTextPositionChangedEventId,
    ];
    FOCUS_PROPERTIES
        .iter()
        .map(|&property| (UIA_AutomationPropertyChangedEventId, property))
        .chain(
            automation_events
                .into_iter()
                .map(|event| (event, UIA_PROPERTY_ID(0))),
        )
        .collect()
}

/// The `WinEvent`s `entry` maps to UIA event `event` with `property`.
fn mapped(
    entry: &IUIAutomationProxyFactoryEntry,
    (event, property): (UIA_EVENT_ID, UIA_PROPERTY_ID),
) -> windows::core::Result<Vec<i32>> {
    // SAFETY: a local call on a live entry; the array returned is owned
    // here and handed to `take_i32_safearray`, which destroys it.
    let array = unsafe { entry.GetWinEventsForAutomationEvent(event, property) }?;
    // SAFETY: as above.
    Ok(unsafe { take_i32_safearray(array) })
}

/// Clears, in `client`'s proxy factory mapping, every `WinEvent` mapped to
/// one of the events Verbatim handles. An entry's changes take effect only
/// once it is put back in the mapping, so a changed entry is removed and
/// inserted again at its place, as NVDA does. A mapping is cleared with no
/// array at all: UIA refuses an empty one as an invalid argument.
pub(crate) fn ignore_win_events(client: &IUIAutomation) -> windows::core::Result<()> {
    // SAFETY: local calls on a live client and the mapping it returns.
    let mapping = unsafe { client.ProxyFactoryMapping() }?;
    // SAFETY: as above.
    let count = unsafe { mapping.Count() }?;
    for index in 0..count {
        // SAFETY: as above, with an index below the count.
        let entry = unsafe { mapping.GetEntry(index) }?;
        let mut changed = false;
        for (event, property) in ignored_events() {
            if mapped(&entry, (event, property))?.is_empty() {
                continue;
            }
            // SAFETY: a local call on a live entry; a null array maps no
            // `WinEvent`.
            unsafe { entry.SetWinEventsForAutomationEvent(event, property, std::ptr::null()) }?;
            changed = true;
        }
        if changed {
            // SAFETY: local calls on the live mapping, putting the entry
            // back where it was.
            unsafe { mapping.RemoveEntry(index) }?;
            // SAFETY: as above.
            unsafe { mapping.InsertEntry(index, &entry) }?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::Accessibility::{UIA_AutomationFocusChangedEventId, UIA_PROPERTY_ID};

    use super::*;

    /// Every entry of a new client's mapping, with each event's mapped
    /// `WinEvent`s.
    fn mappings(
        events: &[(UIA_EVENT_ID, UIA_PROPERTY_ID)],
    ) -> Vec<Vec<(UIA_EVENT_ID, UIA_PROPERTY_ID, Vec<i32>)>> {
        let uia = crate::Uia::new().expect("a client");
        // SAFETY: local calls on a live client and its mapping.
        let mapping = unsafe { uia.client().ProxyFactoryMapping() }.expect("the mapping");
        // SAFETY: as above.
        let count = unsafe { mapping.Count() }.expect("the entry count");
        (0..count)
            .map(|index| {
                // SAFETY: as above, with an index below the count.
                let entry = unsafe { mapping.GetEntry(index) }.expect("an entry");
                events
                    .iter()
                    .map(|&(event, property)| {
                        let winevents = mapped(&entry, (event, property)).expect("its mapping");
                        (event, property, winevents)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_client_maps_no_win_event_to_the_events_verbatim_handles() {
        for entry in mappings(&ignored_events()) {
            for (event, property, winevents) in entry {
                assert_eq!(winevents, [], "{event:?} with {property:?}");
            }
        }
        // The mapping was read, and focus changes keep theirs, as NVDA
        // keeps them.
        let focus = mappings(&[(UIA_AutomationFocusChangedEventId, UIA_PROPERTY_ID(0))]);
        assert!(
            focus
                .iter()
                .flatten()
                .any(|(_, _, winevents)| !winevents.is_empty()),
            "some proxy still raises focus changes from WinEvents"
        );
    }
}
