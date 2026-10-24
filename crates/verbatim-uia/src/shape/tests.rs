use std::cell::RefCell;
use std::collections::BTreeSet;

use windows::Win32::Foundation::RECT;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    UIA_HasKeyboardFocusPropertyId, UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_SelectionPatternId, UIA_TextPatternId,
    UiaGetReservedNotSupportedValue,
};
use windows::core::IUnknown;

use super::{
    Absent, Ancestor, CachedReads, Description, DialogText, FocusInput, HelpTextRefresh, Kept,
    ListenerEvent, Node, Place, Position, PropertyEvent, ProviderOrigin, Reader, RoleCore,
    SelectionContainer, Shape, ShellButton, Shortcut, States, SuggestionsRefresh, TextAreaPattern,
    TextEvent, ValueSet, WindowLookup, request_properties,
};

/// A cache that answers every read, and records what was read.
#[derive(Default)]
struct Recorder {
    properties: RefCell<Vec<UIA_PROPERTY_ID>>,
    patterns: RefCell<Vec<UIA_PATTERN_ID>>,
}

impl CachedReads for Recorder {
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        self.properties.borrow_mut().push(property);
        Some(VARIANT::default())
    }

    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        self.value(property)
    }

    fn rect(&self, property: UIA_PROPERTY_ID) -> Option<RECT> {
        self.properties.borrow_mut().push(property);
        Some(RECT::default())
    }

    fn pattern(&self, pattern: UIA_PATTERN_ID) -> Option<IUnknown> {
        self.patterns.borrow_mut().push(pattern);
        None
    }
}

fn ids(properties: &[UIA_PROPERTY_ID]) -> BTreeSet<i32> {
    properties.iter().map(|property| property.0).collect()
}

fn pattern_ids(patterns: &[UIA_PATTERN_ID]) -> BTreeSet<i32> {
    patterns.iter().map(|pattern| pattern.0).collect()
}

/// For every combination of `S`'s gates, the properties and patterns a
/// read of `S` reads, with the prefetched patterns, and those its request
/// for the same gates holds.
#[expect(
    clippy::type_complexity,
    reason = "the read's sets and the request's, side by side"
)]
fn reads_and_requests<S: Shape>() -> Vec<(
    (BTreeSet<i32>, BTreeSet<i32>),
    (BTreeSet<i32>, BTreeSet<i32>),
)> {
    S::combinations()
        .into_iter()
        .map(|gates| {
            let recorder = Recorder::default();
            let _ = S::read_from(
                &Reader {
                    source: &recorder,
                    absent: Absent::NONE,
                },
                &gates,
            );
            let mut patterns = pattern_ids(&recorder.patterns.into_inner());
            patterns.extend(pattern_ids(S::PREFETCHED));
            (
                (ids(&recorder.properties.into_inner()), patterns),
                (
                    ids(&request_properties::<S>(&gates)),
                    pattern_ids(S::PATTERNS),
                ),
            )
        })
        .collect()
}

/// Checks [`reads_and_requests`] for each shape named, by name.
macro_rules! check_every_combination {
    ($( $shape:ident ),* $(,)?) => {
        $(
            let combinations = reads_and_requests::<$shape>();
            assert!(!combinations.is_empty(), stringify!($shape));
            for (index, (read, requested)) in combinations.into_iter().enumerate() {
                assert_eq!(read, requested, "{} gates {index}", stringify!($shape));
            }
        )*
    };
}

#[test]
fn every_shape_reads_exactly_what_its_request_holds_for_every_combination_of_gates() {
    check_every_combination!(
        RoleCore,
        ValueSet,
        States,
        Place,
        Kept,
        TextEvent,
        PropertyEvent,
        ListenerEvent,
        FocusInput,
        Description,
        Shortcut,
        Position,
        Node,
        Ancestor,
        DialogText,
        SelectionContainer,
        HelpTextRefresh,
        SuggestionsRefresh,
        WindowLookup,
        ShellButton,
        TextAreaPattern,
        ProviderOrigin,
    );
    // Five gates each: the description, shortcut, position and place
    // groups and the level.
    assert_eq!(Node::combinations().len(), 32);
    assert_eq!(Ancestor::combinations().len(), 32);
}

#[test]
fn a_shapes_properties_are_its_fields_union_without_repeats() {
    for (shape, properties, expected) in [
        ("Kept", Kept::PROPERTIES, 2),
        ("TextEvent", TextEvent::PROPERTIES, 3),
        ("PropertyEvent", PropertyEvent::PROPERTIES, 23),
        ("ListenerEvent", ListenerEvent::PROPERTIES, 15),
        ("Node", Node::PROPERTIES, 31),
        // Neither the rectangle nor whether it is off the screen is read
        // unless the place's gate asks for them.
        ("Node, ungated", Node::UNGATED, 22),
        ("Ancestor", Ancestor::PROPERTIES, 33),
        ("Ancestor, ungated", Ancestor::UNGATED, 26),
        ("DialogText", DialogText::PROPERTIES, 24),
    ] {
        assert_eq!(properties.len(), expected, "{shape}");
        assert_eq!(ids(properties).len(), expected, "{shape} has no repeats");
    }
    assert_eq!(ListenerEvent::PATTERNS, [UIA_TextPatternId]);
    assert_eq!(PropertyEvent::PATTERNS, []);
    assert_eq!(SelectionContainer::PATTERNS, [UIA_SelectionPatternId]);
    // With every gate on, a node's request is all of its properties, and
    // an ancestor's is a node's with the content flags.
    let node = ids(&request_properties::<Node>(&Node::all_on()));
    assert_eq!(node, ids(Node::PROPERTIES));
    let mut ancestor = node;
    ancestor.extend(ids(&[
        UIA_IsContentElementPropertyId,
        UIA_IsControlElementPropertyId,
    ]));
    assert_eq!(
        ids(&request_properties::<Ancestor>(&Ancestor::all_on())),
        ancestor
    );
}

/// A focus event's cache whose `HasKeyboardFocus` is the given boolean, or
/// UIA's default for it (`None`), as its provider's empty answer or failure
/// leaves it (mockapp's events test
/// `uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one`),
/// and which holds every other property empty.
struct KeyboardFocus(Option<bool>);

impl CachedReads for KeyboardFocus {
    fn value(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        if property == UIA_HasKeyboardFocusPropertyId {
            Some(VARIANT::from(self.0.unwrap_or(false)))
        } else {
            Some(VARIANT::default())
        }
    }

    fn value_ignoring_default(&self, property: UIA_PROPERTY_ID) -> Option<VARIANT> {
        if property == UIA_HasKeyboardFocusPropertyId && self.0.is_none() {
            // SAFETY: a local call that returns UIA's process-wide sentinel.
            let sentinel = unsafe { UiaGetReservedNotSupportedValue() }.ok()?;
            Some(VARIANT::from(sentinel))
        } else {
            self.value(property)
        }
    }

    fn rect(&self, _: UIA_PROPERTY_ID) -> Option<RECT> {
        None
    }

    fn pattern(&self, _: UIA_PATTERN_ID) -> Option<IUnknown> {
        None
    }
}

/// A focus event says its element lacked the keyboard focus, which the
/// focus listener takes from the cache once its read's time is up, unless
/// its cache holds `true`: a cached `false` says so, and so does UIA's
/// default, which a provider that answered nothing or failed leaves, as
/// NVDA's live read of such a provider answers `false`.
#[test]
fn a_focus_event_lacks_the_keyboard_focus_unless_its_cache_holds_true() {
    let lacked = |focus: Option<bool>| {
        ListenerEvent::read_from(
            &Reader {
                source: &KeyboardFocus(focus),
                absent: Absent::NONE,
            },
            &ListenerEvent::all_on(),
        )
        .lacked_keyboard_focus()
    };
    assert_eq!(
        [None, Some(false), Some(true)].map(lacked),
        [true, true, false]
    );
}
