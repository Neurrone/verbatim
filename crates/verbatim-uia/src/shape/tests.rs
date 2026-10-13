use std::cell::RefCell;
use std::collections::BTreeSet;

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    UIA_IsDialogPropertyId, UIA_IsTogglePatternAvailablePropertyId, UIA_PROPERTY_ID,
    UIA_TextPatternId,
};

use super::{
    CachedReads, Kept, ListenerEvent, PropertyEvent, Reader, Shape, TextEvent, request_properties,
};
use crate::cache::{FOCUS_EVENT_PROPERTIES, NVDA_BASE_PROPERTIES, TEXT_EVENT_PROPERTIES};

/// A cache that answers every read, and records what was read.
#[derive(Default)]
struct Recorder(RefCell<Vec<UIA_PROPERTY_ID>>);

impl CachedReads for Recorder {
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        self.0.borrow_mut().push(property);
        Some(VARIANT::default())
    }

    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        self.value(property)
    }
}

fn ids(properties: &[UIA_PROPERTY_ID]) -> BTreeSet<i32> {
    properties.iter().map(|property| property.0).collect()
}

/// The properties a read of `S` reads, and those its request holds.
fn read_and_requested<S: Shape<Gates = ()>>() -> (BTreeSet<i32>, BTreeSet<i32>) {
    let recorder = Recorder::default();
    let _ = S::read_from(&Reader { source: &recorder }, &());
    (
        ids(&recorder.0.into_inner()),
        ids(&request_properties::<S>(&())),
    )
}

#[test]
fn every_shape_reads_exactly_what_its_request_holds() {
    for (shape, (read, requested)) in [
        ("Kept", read_and_requested::<Kept>()),
        ("TextEvent", read_and_requested::<TextEvent>()),
        ("PropertyEvent", read_and_requested::<PropertyEvent>()),
        ("ListenerEvent", read_and_requested::<ListenerEvent>()),
    ] {
        assert_eq!(read, requested, "{shape}");
    }
}

#[test]
fn a_shapes_properties_are_its_fields_union_without_repeats() {
    for (shape, properties, expected) in [
        ("Kept", Kept::PROPERTIES, 2),
        ("TextEvent", TextEvent::PROPERTIES, 3),
        ("PropertyEvent", PropertyEvent::PROPERTIES, 24),
        ("ListenerEvent", ListenerEvent::PROPERTIES, 15),
    ] {
        assert_eq!(properties.len(), expected, "{shape}");
        assert_eq!(ids(properties).len(), expected, "{shape} has no repeats");
    }
    // The event shapes' requests are today's lists, the listener's with the
    // two properties decision 6 adds.
    assert_eq!(ids(TextEvent::PROPERTIES), ids(TEXT_EVENT_PROPERTIES));
    assert_eq!(ids(PropertyEvent::PROPERTIES), ids(FOCUS_EVENT_PROPERTIES));
    let mut listener = ids(NVDA_BASE_PROPERTIES);
    listener.extend(ids(&[
        UIA_IsDialogPropertyId,
        UIA_IsTogglePatternAvailablePropertyId,
    ]));
    assert_eq!(ids(ListenerEvent::PROPERTIES), listener);
    assert_eq!(ListenerEvent::PATTERNS, [UIA_TextPatternId]);
    assert_eq!(PropertyEvent::PATTERNS, []);
}
