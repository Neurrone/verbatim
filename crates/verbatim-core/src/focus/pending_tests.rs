//! The overrides and the pending judgement, each from its rule.

use verbatim_model::property::{PropertyAnswer, RuntimeId};
use verbatim_model::request::{ObjectClass, Probe};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Payload, Relation, WindowFacet,
};
use verbatim_model::{Pid, WindowHandle};

use super::super::versions::{LastFocus, Place, ReadBy};
use super::*;

fn facet(handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: Pid(40),
        thread: 41,
    }
}

/// A window under the foreground window, so acceptance passes.
fn facts(class: &str) -> AcceptanceFacts {
    AcceptanceFacts {
        window: facet(5, class),
        parent_class: String::new(),
        root: facet(4, "Frame"),
        root_owner: facet(4, "Frame"),
        window_topmost: false,
        root_topmost: false,
        foreground: WindowHandle(4),
        foreground_root_owner: WindowHandle(4),
        window_to_foreground: Relation::Under,
        root_owner_to_foreground: Relation::Equal,
        root_owner_is_foregrounds: true,
        active_window: WindowHandle(4),
        window_to_active: Relation::Under,
        desktop: false,
        hidden_frame: false,
        hidden_frame_owned: false,
    }
}

fn value(id: PropertyId, value: PropertyValue) -> PropertyAnswer {
    PropertyAnswer {
        id,
        value: Some(value),
    }
}

fn uia_focus(cached: Vec<PropertyAnswer>, window_class: &str, checked: Checked) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(7),
        kind: EventKind::UiaFocus,
        process: Pid(40),
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Uia {
            token: ElementToken(9),
            runtime_id: RuntimeId(vec![42, 1]),
        },
        cached: PropertyValues(cached),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts(window_class))),
        checked: Some(checked),
    }
}

fn focused(keyboard_focus: Read<bool>) -> Checked {
    Checked {
        probe: None,
        keyboard_focus: Some(keyboard_focus),
        kept: Vec::new(),
        previous_sibling: None,
    }
}

fn msaa_focus(kind: EventKind, object: i32) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(7),
        kind,
        process: Pid(40),
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Msaa {
            window: WindowHandle(5),
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts("Button"))),
        checked: None,
    }
}

fn last_msaa(object: i32) -> LastFocus {
    LastFocus {
        read_by: ReadBy::Msaa {
            event: Some(MsaaIds {
                window: WindowHandle(5),
                object,
                child: 0,
            }),
            held_child: 0,
        },
        place: Place {
            window: WindowHandle(5),
            top_level: WindowHandle(4),
            root_owner: WindowHandle(4),
            ancestor_windows: Vec::new(),
        },
        class: ObjectClass::Plain,
    }
}

#[test]
fn the_console_windows_own_element_is_refused() {
    let tell = uia_focus(
        vec![value(
            PropertyId::AutomationId,
            PropertyValue::Text("Console Window".to_owned()),
        )],
        "ConsoleWindowClass",
        focused(Read::Answered(true)),
    );
    assert_eq!(focus_override(&tell), Override::Refused);
    let elsewhere = uia_focus(
        tell.cached.0.clone(),
        "Other",
        focused(Read::Answered(true)),
    );
    assert_eq!(focus_override(&elsewhere), Override::None);
}

#[test]
fn an_excel6_pane_is_refused() {
    let tell = uia_focus(
        vec![value(PropertyId::Role, PropertyValue::Role(Role::Pane))],
        "EXCEL6",
        focused(Read::Answered(true)),
    );
    assert_eq!(focus_override(&tell), Override::Refused);
}

#[test]
fn an_unnamed_first_netui_menu_item_is_let_through_and_redirected() {
    let cached = vec![
        value(
            PropertyId::ClassName,
            PropertyValue::Text("NetUITWMenuItem".to_owned()),
        ),
        value(PropertyId::Role, PropertyValue::Role(Role::MenuItem)),
    ];
    let mut checked = focused(Read::Answered(false));
    checked.previous_sibling = Some(Read::Answered(false));
    let tell = uia_focus(cached.clone(), "NetUIHWND", checked.clone());
    assert_eq!(focus_override(&tell), Override::NetUiMenuItem);
    assert!(counts_as_pending(
        &tell,
        None,
        &BackendCache::default(),
        2_000
    ));

    checked.previous_sibling = Some(Read::Answered(true));
    let second = uia_focus(cached, "NetUIHWND", checked);
    assert_eq!(focus_override(&second), Override::None);
    assert!(!counts_as_pending(
        &second,
        None,
        &BackendCache::default(),
        2_000
    ));
}

#[test]
fn a_uia_focus_counts_as_pending_unless_it_lost_the_keyboard_focus() {
    let cache = BackendCache::default();
    let kept = uia_focus(Vec::new(), "UiaHost", focused(Read::Answered(true)));
    assert!(counts_as_pending(&kept, None, &cache, 2_000));
    let failed = uia_focus(Vec::new(), "UiaHost", focused(Read::Failed));
    assert!(counts_as_pending(&failed, None, &cache, 2_000));
    let lost = uia_focus(Vec::new(), "UiaHost", focused(Read::Answered(false)));
    assert!(!counts_as_pending(&lost, None, &cache, 2_000));
}

#[test]
fn a_uia_focus_whose_walk_found_no_window_does_not_count() {
    let mut tell = uia_focus(Vec::new(), "UiaHost", focused(Read::Answered(true)));
    tell.window = TellWindow::NoWindow;
    assert!(!counts_as_pending(
        &tell,
        None,
        &BackendCache::default(),
        2_000
    ));
    tell.window = TellWindow::Failed;
    assert!(!counts_as_pending(
        &tell,
        None,
        &BackendCache::default(),
        2_000
    ));
}

#[test]
fn a_uia_focus_in_a_window_read_through_msaa_does_not_count() {
    let mut cache = BackendCache::default();
    cache.record(&Probe {
        window: WindowHandle(5),
        thread: 41,
        provider: false,
        began_us: 900,
    });
    let tell = uia_focus(Vec::new(), "UiaHost", focused(Read::Answered(true)));
    assert!(!counts_as_pending(&tell, None, &cache, 1_100));
}

#[test]
fn a_uia_duplicate_of_the_last_focus_does_not_count() {
    let last = LastFocus {
        read_by: ReadBy::Uia {
            runtime_id: RuntimeId(vec![42, 1]),
            token: Some(ElementToken(3)),
        },
        ..last_msaa(0)
    };
    let mut checked = focused(Read::Answered(true));
    checked.kept = vec![(ElementToken(3), Read::Answered(true))];
    let tell = uia_focus(Vec::new(), "UiaHost", checked);
    assert!(!counts_as_pending(
        &tell,
        Some(&last),
        &BackendCache::default(),
        2_000
    ));
}

#[test]
fn an_msaa_focus_counts_unless_a_duplicate_or_refused() {
    let cache = BackendCache::default();
    let tell = msaa_focus(EventKind::MsaaFocus, -4);
    assert!(counts_as_pending(&tell, None, &cache, 2_000));
    assert!(!counts_as_pending(
        &tell,
        Some(&last_msaa(-4)),
        &cache,
        2_000
    ));
    assert!(counts_as_pending(
        &tell,
        Some(&last_msaa(-5)),
        &cache,
        2_000
    ));

    let mut unrelated = tell;
    let TellWindow::Found(facts) = &mut unrelated.window else {
        unreachable!()
    };
    facts.window_to_foreground = Relation::Outside;
    facts.root_owner_to_foreground = Relation::Outside;
    facts.root_owner_is_foregrounds = false;
    assert!(!counts_as_pending(&unrelated, None, &cache, 2_000));
}

#[test]
fn a_foreground_tell_counts_and_other_kinds_do_not() {
    let cache = BackendCache::default();
    assert!(counts_as_pending(
        &msaa_focus(EventKind::Foreground, 0),
        Some(&last_msaa(0)),
        &cache,
        2_000
    ));
    assert!(!counts_as_pending(
        &msaa_focus(EventKind::NameChange, -4),
        None,
        &cache,
        2_000
    ));
}

#[test]
fn file_explorers_switcher_frames_are_refused_and_only_there() {
    for (class, role) in [
        ("MultitaskingViewFrame", Role::Window),
        ("Windows.UI.Input.InputSite.WindowClass", Role::Pane),
    ] {
        let cached = vec![
            value(PropertyId::ClassName, PropertyValue::Text(class.to_owned())),
            value(PropertyId::Role, PropertyValue::Role(role)),
        ];
        let mut tell = uia_focus(cached, "UiaHost", focused(Read::Answered(true)));
        assert_eq!(focus_override(&tell), Override::None);
        tell.application = "explorer".to_owned();
        assert_eq!(focus_override(&tell), Override::Refused);
        assert!(!counts_as_pending(
            &tell,
            None,
            &BackendCache::default(),
            2_000
        ));
    }
    let cached = vec![
        value(
            PropertyId::ClassName,
            PropertyValue::Text("Windows.UI.Input.InputSite.WindowClass".to_owned()),
        ),
        value(PropertyId::Role, PropertyValue::Role(Role::Window)),
    ];
    let mut other_role = uia_focus(cached, "UiaHost", focused(Read::Answered(true)));
    other_role.application = "explorer".to_owned();
    assert_eq!(focus_override(&other_role), Override::None);
}
