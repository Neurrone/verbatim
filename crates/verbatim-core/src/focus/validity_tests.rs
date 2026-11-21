//! The one definition, row by row: each row crossed with the facts an
//! outpost read (none, focused, not focused, not there, skipped),
//! asserting the exact answer and the depth with it; the identity check;
//! the overrides; and the agreement of the turn's walk with the pending
//! judgement.

use std::collections::VecDeque;

use verbatim_model::property::{PropertyAnswer, RuntimeId};
use verbatim_model::request::{FocusNode, HeldIds, NodeRead, NodeWindow, Probe};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, EventObject, Payload, Relation, WindowFacet,
};
use verbatim_model::{KeySeq, Pid, WindowHandle};

use super::super::msaa_run::TurnDepth;
use super::super::versions::{Committed, LastFocus, Place, ReadBy};
use super::*;
use crate::keys::entry::Arrivals;
use crate::keys::entry::Fence;

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

fn uia_tell(cached: Vec<PropertyAnswer>, window_class: &str, checked: Checked) -> Event {
    Event {
        trace_id: TraceId::from_raw(7),
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

fn uia(tell: Event) -> UiaFocus {
    UiaFocus {
        tell,
        token: ElementToken(9),
        runtime_id: RuntimeId(vec![42, 1]),
        listener: 1,
        alt_down: false,
        fence: Fence::default(),
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

/// A fence of key number `key`.
fn fence(key: u64) -> Fence {
    let mut arrivals = Arrivals::default();
    arrivals.key_joined(KeySeq::new(key));
    arrivals.fence()
}

/// An MSAA candidate of `kind` on object `object`, child `child` of
/// window 5, raised at `at_us`, with trace `trace`.
fn candidate(kind: EventKind, (object, child): (i32, i32), at_us: u64, trace: u64) -> Candidate {
    let tell = Event {
        trace_id: TraceId::from_raw(trace),
        kind,
        process: Pid(40),
        application: "app".to_owned(),
        thread: 41,
        at_us,
        object: EventObject::Msaa {
            window: WindowHandle(5),
            object,
            child,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts("Button"))),
        checked: None,
    };
    let window = crate::entries::request_window(&tell).expect("the tell has a window");
    Candidate {
        tell,
        ids: MsaaIds {
            window: WindowHandle(5),
            object,
            child,
        },
        window,
        fence: Fence::default(),
        confirmation: Confirmation::Unread,
    }
}

fn focus(object: i32) -> Candidate {
    candidate(EventKind::MsaaFocus, (object, 0), 1_000, 7)
}

fn last_msaa(object: i32, child: i32) -> LastFocus {
    LastFocus {
        read_by: ReadBy::Msaa {
            event: Some(MsaaIds {
                window: WindowHandle(5),
                object,
                child,
            }),
            held_child: child,
        },
        place: Place {
            window: WindowHandle(5),
            top_level: WindowHandle(4),
            root_owner: WindowHandle(4),
            ancestor_windows: Vec::new().into(),
        },
        role: Role::Button,
        since: None,
    }
}

fn versions(last: Option<LastFocus>) -> Versions {
    Versions {
        committed: Committed::Nothing,
        last,
        postponed: None,
    }
}

fn view<'a>(versions: &'a Versions, backend: &'a BackendCache) -> View<'a> {
    View {
        versions,
        in_front: Some(WindowHandle(5)),
        backend,
        now_us: 2_000,
        run_before_valid: false,
    }
}

fn found(focused: Option<bool>) -> CandidateFacts {
    CandidateFacts::Found {
        focused,
        role: None,
    }
}

fn msaa_answer(
    candidate: &Candidate,
    versions: &Versions,
    facts: Option<&CandidateFacts>,
) -> Validity {
    judge(
        Subject::Msaa(candidate),
        &view(versions, &BackendCache::default()),
        facts,
    )
}

// The overrides.

#[test]
fn the_console_windows_own_element_is_refused() {
    let tell = uia_tell(
        vec![value(
            PropertyId::AutomationId,
            PropertyValue::Text("Console Window".to_owned()),
        )],
        "ConsoleWindowClass",
        focused(Read::Answered(true)),
    );
    assert_eq!(focus_override(&tell), Override::Refused);
    let none = versions(None);
    assert_eq!(
        judge(
            Subject::Uia(&uia(tell.clone())),
            &view(&none, &BackendCache::default()),
            None
        ),
        Validity::Refused(Reason::Override)
    );
    let elsewhere = uia_tell(tell.cached.0, "Other", focused(Read::Answered(true)));
    assert_eq!(focus_override(&elsewhere), Override::None);
}

#[test]
fn an_excel6_pane_is_refused() {
    let tell = uia_tell(
        vec![value(PropertyId::Role, PropertyValue::Role(Role::Pane))],
        "EXCEL6",
        focused(Read::Answered(true)),
    );
    assert_eq!(focus_override(&tell), Override::Refused);
}

#[test]
fn an_unnamed_first_netui_menu_item_is_let_through_whatever_its_keyboard_focus() {
    let cached = vec![
        value(
            PropertyId::ClassName,
            PropertyValue::Text("NetUITWMenuItem".to_owned()),
        ),
        value(PropertyId::Role, PropertyValue::Role(Role::MenuItem)),
    ];
    let none = versions(None);
    let cache = BackendCache::default();
    let mut checked = focused(Read::Answered(false));
    checked.previous_sibling = Some(Read::Answered(false));
    let tell = uia_tell(cached.clone(), "NetUIHWND", checked.clone());
    assert_eq!(focus_override(&tell), Override::NetUiMenuItem);
    assert_eq!(
        judge(Subject::Uia(&uia(tell)), &view(&none, &cache), None),
        Validity::Valid(How::Read)
    );
    checked.previous_sibling = Some(Read::Answered(true));
    let second = uia_tell(cached, "NetUIHWND", checked);
    assert_eq!(focus_override(&second), Override::None);
    assert_eq!(
        judge(Subject::Uia(&uia(second)), &view(&none, &cache), None),
        Validity::Refused(Reason::KeyboardFocusLost)
    );
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
        let mut tell = uia_tell(cached, "UiaHost", focused(Read::Answered(true)));
        assert_eq!(focus_override(&tell), Override::None);
        tell.application = "explorer".to_owned();
        assert_eq!(focus_override(&tell), Override::Refused);
    }
    let cached = vec![
        value(
            PropertyId::ClassName,
            PropertyValue::Text("Windows.UI.Input.InputSite.WindowClass".to_owned()),
        ),
        value(PropertyId::Role, PropertyValue::Role(Role::Window)),
    ];
    let mut other_role = uia_tell(cached, "UiaHost", focused(Read::Answered(true)));
    other_role.application = "explorer".to_owned();
    assert_eq!(focus_override(&other_role), Override::None);
}

// The UI Automation rows.

#[test]
fn a_uia_focus_is_valid_with_no_read_unless_a_row_refuses_it() {
    let none = versions(None);
    let cache = BackendCache::default();
    let answer = |tell: Event| judge(Subject::Uia(&uia(tell)), &view(&none, &cache), None);
    assert_eq!(
        answer(uia_tell(
            Vec::new(),
            "UiaHost",
            focused(Read::Answered(true))
        )),
        Validity::Valid(How::Read)
    );
    assert_eq!(
        answer(uia_tell(Vec::new(), "UiaHost", focused(Read::Failed))),
        Validity::Valid(How::Read)
    );
    assert_eq!(
        answer(uia_tell(
            Vec::new(),
            "UiaHost",
            focused(Read::Answered(false))
        )),
        Validity::Refused(Reason::KeyboardFocusLost)
    );
    let mut no_window = uia_tell(Vec::new(), "UiaHost", focused(Read::Answered(true)));
    no_window.window = TellWindow::NoWindow;
    assert_eq!(answer(no_window), Validity::Refused(Reason::NoWindow));
    let none = versions(None);
    assert_eq!(
        depth(
            Subject::Uia(&uia(uia_tell(
                Vec::new(),
                "UiaHost",
                focused(Read::Answered(true))
            ))),
            &view(&none, &BackendCache::default()),
            None
        ),
        TurnDepth::Full
    );
}

#[test]
fn a_uia_focus_in_a_window_read_through_msaa_is_refused_by_the_verdict() {
    let mut cache = BackendCache::default();
    cache.record(&Probe {
        window: WindowHandle(5),
        thread: 41,
        provider: false,
        began_us: 900,
    });
    let none = versions(None);
    let tell = uia_tell(Vec::new(), "UiaHost", focused(Read::Answered(true)));
    let mut at = view(&none, &cache);
    at.now_us = 1_100;
    assert_eq!(
        judge(Subject::Uia(&uia(tell)), &at, None),
        Validity::Refused(Reason::Verdict)
    );
}

#[test]
fn a_uia_focus_on_the_last_focuss_element_still_focused_is_a_duplicate() {
    let last = LastFocus {
        read_by: ReadBy::Uia {
            runtime_id: RuntimeId(vec![42, 1]),
            token: Some(ElementToken(3)),
        },
        ..last_msaa(0, 0)
    };
    let mut checked = focused(Read::Answered(true));
    checked.kept = vec![(ElementToken(3), Read::Answered(true))];
    let tell = uia_tell(Vec::new(), "UiaHost", checked);
    let with = versions(Some(last));
    {
        assert_eq!(
            judge(
                Subject::Uia(&uia(tell.clone())),
                &view(&with, &BackendCache::default()),
                None
            ),
            Validity::Duplicate
        );
    }
}

#[test]
fn the_menu_opening_on_the_committed_element_takes_acceptance_then_executes_it_again() {
    let committed = crate::focus::versions::tests::focus(4);
    let both = Versions {
        last: Some(committed.last(Fence::default())),
        committed: Committed::Read(Box::new(committed)),
        postponed: None,
    };
    let cache = BackendCache::default();
    let opening = |checked, related: bool| {
        let mut tell = uia_tell(Vec::new(), "UiaHost", checked);
        tell.kind = EventKind::UiaMenuOpened;
        if !related && let TellWindow::Found(facts) = &mut tell.window {
            facts.window_to_foreground = Relation::Outside;
            facts.root_owner_to_foreground = Relation::Outside;
            facts.root_owner_is_foregrounds = false;
        }
        judge(Subject::Uia(&uia(tell)), &view(&both, &cache), None)
    };
    assert_eq!(
        opening(focused(Read::Answered(true)), true),
        Validity::Valid(How::Again)
    );
    assert_eq!(
        opening(focused(Read::Answered(false)), true),
        Validity::Refused(Reason::KeyboardFocusLost)
    );
    assert_eq!(
        opening(focused(Read::Answered(true)), false),
        Validity::Refused(Reason::Acceptance)
    );
}

// The MSAA focus rows, crossed with the purpose and the facts.

#[test]
fn an_msaa_focus_needs_its_focused_state_and_the_facts_decide() {
    let none = versions(None);
    let candidate = focus(-4);
    assert_eq!(
        depth(
            Subject::Msaa(&candidate),
            &view(&none, &BackendCache::default()),
            None
        ),
        TurnDepth::FullIfFocused
    );
    {
        let answer = |facts: Option<&CandidateFacts>| msaa_answer(&candidate, &none, facts);
        assert_eq!(answer(None), Validity::NeedsRead);
        assert_eq!(answer(Some(&found(Some(true)))), Validity::Valid(How::Read));
        assert_eq!(
            answer(Some(&found(Some(false)))),
            Validity::Refused(Reason::NotFocused)
        );
        assert_eq!(
            answer(Some(&CandidateFacts::NotThere)),
            Validity::Refused(Reason::NotThere)
        );
        assert_eq!(
            answer(Some(&CandidateFacts::Skipped)),
            Validity::Refused(Reason::Skipped)
        );
    }
}

#[test]
fn an_msaa_duplicate_and_the_repeated_item_are_judged_with_no_read() {
    let list = focus(-4);
    let duplicate = versions(Some(last_msaa(-4, 0)));
    let other = versions(Some(last_msaa(-5, 0)));
    {
        assert_eq!(msaa_answer(&list, &duplicate, None), Validity::Duplicate);
        assert_eq!(msaa_answer(&list, &other, None), Validity::NeedsRead);
    }
    // A child above 0 with equal ids is never a duplicate by the test:
    // File Explorer's second focus on an item has no rule of its own.
    let item = versions(Some(last_msaa(-4, 3)));
    assert_eq!(
        msaa_answer(
            &candidate(EventKind::MsaaFocus, (-4, 3), 1_000, 7),
            &item,
            None
        ),
        Validity::NeedsRead
    );
}

#[test]
fn an_msaa_focus_refused_by_acceptance_the_verdict_or_the_hidden_frame_needs_no_read() {
    let none = versions(None);
    let mut unrelated = focus(-4);
    if let TellWindow::Found(facts) = &mut unrelated.tell.window {
        facts.window_to_foreground = Relation::Outside;
        facts.root_owner_to_foreground = Relation::Outside;
        facts.root_owner_is_foregrounds = false;
    }
    assert_eq!(
        msaa_answer(&unrelated, &none, None),
        Validity::Refused(Reason::Acceptance)
    );
    let mut hidden = focus(-4);
    if let TellWindow::Found(facts) = &mut hidden.tell.window {
        facts.hidden_frame = true;
    }
    assert_eq!(
        msaa_answer(&hidden, &none, None),
        Validity::Refused(Reason::HiddenFrame)
    );
}

/// A focus on the desktop window is refused with no read: no reader reads
/// it, since Core starts no outpost for the desktop window's process (a
/// limit).
#[test]
fn a_candidate_on_the_desktop_window_is_refused_as_having_no_reader() {
    let none = versions(None);
    let mut desktop = focus(-4);
    if let TellWindow::Found(facts) = &mut desktop.tell.window {
        facts.desktop = true;
    }
    {
        assert_eq!(
            msaa_answer(&desktop, &none, None),
            Validity::Refused(Reason::NoReader)
        );
    }
}

// The identity check in the duplicate test (section 11).

/// The item "Copy", 2 of 3, at child 2 of window 5's client, committed and
/// the readers' last focus, executed for an entry of fence 3, with its
/// reading.
fn committed_item() -> (Versions, NodeSnapshot) {
    let mut focus = super::super::versions::tests::focus(4);
    focus.kept.read_by = ReadBy::Msaa {
        event: Some(MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 2,
        }),
        held_child: 2,
    };
    let snapshot = NodeSnapshot {
        id: focus.context.snapshot.id,
        backend: verbatim_model::Backend::Msaa,
        role: Role::ListItem,
        name: Some("Copy".to_owned()),
        value: None,
        states: verbatim_model::StateSet::default(),
        details: verbatim_model::NodeDetails {
            position_in_set: Some(2),
            set_size: Some(3),
            ..verbatim_model::NodeDetails::default()
        },
    };
    focus.context.snapshot = snapshot.clone();
    let mut versions = Versions::default();
    versions.execute(focus, fence(3));
    (versions, snapshot)
}

/// An MSAA focus with no key since the readers' last focus executed, a
/// child above 0 that is committed, is read unless it reaches that focus by
/// its ids and its reading, named from Core's snapshot; a key since, a
/// failed try since, or a foreground or menu start is read as before.
#[test]
fn a_focus_with_no_key_since_names_the_committed_items_ids_and_identity() {
    let (state, snapshot) = committed_item();
    let cache = BackendCache::default();
    let at = view(&state, &cache);
    let mut list = focus(-4);
    list.fence = fence(3);
    let named = TurnDepth::UnlessSame(super::super::msaa_run::UnlessSame {
        ids: MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 2,
        },
        held_child: 2,
        identity: super::super::msaa_run::Identity {
            states: verbatim_model::StateSet::default(),
            role: Role::ListItem,
            position_in_set: Some(2),
            set_size: Some(3),
            name: Some("Copy".to_owned()),
            value: None,
        },
    });
    assert_eq!(depth(Subject::Msaa(&list), &at, Some(&snapshot)), named);
    // A key since.
    list.fence = fence(4);
    assert_eq!(
        depth(Subject::Msaa(&list), &at, Some(&snapshot)),
        TurnDepth::FullIfFocused
    );
    // A failed try since.
    list.fence = fence(3);
    let mut tried = state.clone();
    tried.tried_and_refused();
    assert_eq!(
        depth(Subject::Msaa(&list), &view(&tried, &cache), Some(&snapshot)),
        TurnDepth::FullIfFocused
    );
    // Core's own focus committed over it.
    let mut own = state.clone();
    own.commit_own();
    assert_eq!(
        depth(Subject::Msaa(&list), &view(&own, &cache), Some(&snapshot)),
        TurnDepth::FullIfFocused
    );
    // A foreground and a menu start are judged by their own rows.
    let mut foreground = candidate(EventKind::Foreground, (0, 0), 1_000, 7);
    foreground.fence = fence(3);
    assert_eq!(
        depth(Subject::Msaa(&foreground), &at, Some(&snapshot)),
        TurnDepth::Full
    );
}

/// A last focus held by child id 0 keeps NVDA's test, equal ids a
/// duplicate with no read, and names no identity to other ids.
#[test]
fn a_last_focus_held_by_child_0_names_no_identity() {
    let mut last = last_msaa(-4, 0);
    last.since = Some(fence(3));
    let state = versions(Some(last));
    let cache = BackendCache::default();
    let mut other = focus(-5);
    other.fence = fence(3);
    assert_eq!(
        depth(Subject::Msaa(&other), &view(&state, &cache), None),
        TurnDepth::FullIfFocused
    );
    let mut same = focus(-4);
    same.fence = fence(3);
    assert_eq!(msaa_answer(&same, &state, None), Validity::Duplicate);
}

/// A pending judgement has no identity row: a focus at the committed
/// item's ids with no key since counts, as NVDA's duplicate test never
/// holds for a child id above 0 (decided).
#[test]
fn a_pending_focus_at_the_committed_items_ids_counts() {
    let (state, _) = committed_item();
    let mut item = candidate(EventKind::MsaaFocus, (-4, 2), 1_000, 7);
    item.fence = fence(3);
    item.confirmation = Confirmation::Read(found(Some(true)));
    let waiting: VecDeque<Item<Entry>> =
        std::iter::once(Item::One(Entry::Candidate(Box::new(item)))).collect();
    assert_eq!(
        pending_behind(
            &waiting,
            &view(&state, &BackendCache::default()),
            Within::ALL
        ),
        Pending::Counts
    );
}

/// The identity row at an answer that the object is the one named: a
/// duplicate, unless a row before the duplicate test refuses the candidate
/// now.
#[test]
fn the_object_named_is_a_duplicate_unless_refused_before_the_duplicate_test() {
    let (state, _) = committed_item();
    let cache = BackendCache::default();
    let list = focus(-4);
    assert_eq!(before_duplicate(&list, &view(&state, &cache)), None);
    let mut hidden = focus(-4);
    if let TellWindow::Found(facts) = &mut hidden.tell.window {
        facts.hidden_frame = true;
    }
    assert_eq!(
        before_duplicate(&hidden, &view(&state, &cache)),
        Some(Reason::HiddenFrame)
    );
}

// The foreground rows.

#[test]
fn a_foreground_is_valid_with_no_read_unless_a_foreground_row_refuses_it() {
    let foreground = candidate(EventKind::Foreground, (0, 0), 1_000, 7);
    let none = versions(None);
    assert_eq!(
        depth(
            Subject::Msaa(&foreground),
            &view(&none, &BackendCache::default()),
            None
        ),
        TurnDepth::Full
    );
    assert_eq!(
        msaa_answer(&foreground, &none, None),
        Validity::Valid(How::Read)
    );
    let cache = BackendCache::default();
    let mut elsewhere = view(&none, &cache);
    elsewhere.in_front = Some(WindowHandle(8));
    assert_eq!(
        judge(Subject::Msaa(&foreground), &elsewhere, None),
        Validity::Refused(Reason::NotInFront)
    );
    let own = versions(Some(last_msaa(-4, 0)));
    assert_eq!(
        msaa_answer(&foreground, &own, None),
        Validity::Refused(Reason::LastFocusWindow)
    );
    let mut same = last_msaa(0, 0);
    same.place.window = WindowHandle(8);
    let same = versions(Some(same));
    assert_eq!(
        msaa_answer(&foreground, &same, None),
        Validity::Refused(Reason::SameIds)
    );
    assert_eq!(
        msaa_answer(&foreground, &none, Some(&CandidateFacts::NotThere)),
        Validity::Refused(Reason::NotThere)
    );
}

// The menu start rows.

#[test]
fn a_menu_start_needs_its_role_and_only_a_menu_popup_is_valid() {
    let start = candidate(EventKind::PopupStart, (-4, 0), 1_000, 7);
    let none = versions(None);
    assert_eq!(
        depth(
            Subject::Msaa(&start),
            &view(&none, &BackendCache::default()),
            None
        ),
        TurnDepth::Full
    );
    let role = |role| CandidateFacts::Found {
        focused: None,
        role: Some(role),
    };
    {
        assert_eq!(msaa_answer(&start, &none, None), Validity::NeedsRead);
        assert_eq!(
            msaa_answer(&start, &none, Some(&role(Role::Menu))),
            Validity::Valid(How::Read)
        );
        assert_eq!(
            msaa_answer(&start, &none, Some(&role(Role::Window))),
            Validity::Refused(Reason::NotMenuPopup)
        );
    }
    let mut menu = last_msaa(-4, 0);
    menu.role = Role::MenuItem;
    let on_menu = versions(Some(menu));
    let cache = BackendCache::default();
    let mut after_valid = view(&on_menu, &cache);
    after_valid.run_before_valid = true;
    assert_eq!(
        judge(Subject::Msaa(&start), &after_valid, None),
        Validity::Refused(Reason::OnMenu)
    );
}

// The menu check's read and the recovered read.

#[test]
fn the_menu_checks_read_is_judged_by_its_focused_fact_and_the_duplicate_test() {
    let none = versions(None);
    let cache = BackendCache::default();
    let check = |ids: Option<&MsaaIds>, facts: &CandidateFacts, versions: &Versions| {
        judge(
            Subject::MenuCheck(ids),
            &view(versions, &cache),
            Some(facts),
        )
    };
    assert_eq!(
        depth(Subject::MenuCheck(None), &view(&none, &cache), None),
        TurnDepth::FullIfFocused
    );
    // A UI Automation answer carries no focused fact, and passes.
    assert_eq!(check(None, &found(None), &none), Validity::Valid(How::Read));
    let ids = MsaaIds {
        window: WindowHandle(5),
        object: -4,
        child: 0,
    };
    assert_eq!(
        check(Some(&ids), &found(Some(false)), &none),
        Validity::Refused(Reason::NotFocused)
    );
    assert_eq!(
        check(Some(&ids), &found(Some(true)), &none),
        Validity::Valid(How::Read)
    );
    let duplicate = versions(Some(last_msaa(-4, 0)));
    assert_eq!(
        check(Some(&ids), &found(Some(true)), &duplicate),
        Validity::Duplicate
    );
}

fn read_of(msaa: Option<HeldIds>, runtime_id: Option<RuntimeId>) -> FocusRead {
    FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: None,
                window: NodeWindow {
                    handle: WindowHandle(5),
                    top_level: WindowHandle(4),
                    root_owner: WindowHandle(4),
                    process: Pid(40),
                    thread: 41,
                    desktop: false,
                },
                values: PropertyValues::default(),
            },
            runtime_id,
        },
        msaa,
        ancestors: Vec::new(),
        met: None,
        foreground: None,
        value_from_selection: false,
        runtime_id_reused: false,
        caret: verbatim_model::request::FocusCaret::NoText,
    }
}

#[test]
fn a_recovered_read_of_the_last_focuss_element_is_a_duplicate_and_any_other_is_valid() {
    let last = LastFocus {
        read_by: ReadBy::Uia {
            runtime_id: RuntimeId(vec![42, 1]),
            token: None,
        },
        ..last_msaa(0, 0)
    };
    let state = versions(Some(last));
    let cache = BackendCache::default();
    let recovered = |read: &FocusRead| judge(Subject::Recovered(read), &view(&state, &cache), None);
    assert_eq!(
        recovered(&read_of(None, Some(RuntimeId(vec![42, 1])))),
        Validity::Duplicate
    );
    assert_eq!(
        recovered(&read_of(None, Some(RuntimeId(vec![42, 2])))),
        Validity::Valid(How::Read)
    );
    let msaa_last = versions(Some(last_msaa(-4, 0)));
    let held = HeldIds {
        event: MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 0,
        },
        held_child: 0,
    };
    assert_eq!(
        judge(
            Subject::Recovered(&read_of(Some(held), None)),
            &view(&msaa_last, &cache),
            None
        ),
        Validity::Valid(How::Read)
    );
}

// The pending judgement, and its agreement with the turn.

#[test]
fn valid_counts_a_needed_read_is_unconfirmed_and_the_rest_do_not_count() {
    let trace = TraceId::from_raw(7);
    assert_eq!(pending(Validity::Valid(How::Read), trace), Pending::Counts);
    assert_eq!(pending(Validity::Valid(How::Again), trace), Pending::Counts);
    assert_eq!(
        pending(Validity::NeedsRead, trace),
        Pending::Unconfirmed(trace)
    );
    assert_eq!(pending(Validity::Duplicate, trace), Pending::No);
    assert_eq!(
        pending(Validity::Refused(Reason::NotThere), trace),
        Pending::No
    );
}

fn run_of(candidates: Vec<Candidate>) -> VecDeque<Item<Entry>> {
    VecDeque::from([Item::Run(
        candidates
            .into_iter()
            .map(|candidate| Entry::Candidate(Box::new(candidate)))
            .collect(),
    )])
}

/// The turn's walk over `candidates`, oldest first, as the run's turn
/// takes its first step (`super::super::msaa_run::run` walks with the same
/// [`judge_run`]).
fn walked(candidates: &[Candidate], versions: &Versions) -> Option<(Validity, TraceId)> {
    let cache = BackendCache::default();
    let newest_first: Vec<Candidate> = candidates.iter().rev().cloned().collect();
    judge_run(&newest_first, &view(versions, &cache))
        .found
        .map(|found| (found.answer, found.candidate.tell.trace_id))
}

fn behind(candidates: Vec<Candidate>, versions: &Versions, bound_us: Option<u64>) -> Pending {
    let cache = BackendCache::default();
    let within = bound_us.map_or(Within::ALL, Within::key);
    pending_behind(&run_of(candidates), &view(versions, &cache), within)
}

/// A run whose newest candidate is a duplicate, behind an unconfirmed one:
/// the turn stops at the duplicate, and nothing is pending, as NVDA's
/// single pump stops at it.
#[test]
fn a_newest_duplicate_hides_an_older_unconfirmed_candidate_from_both() {
    let state = versions(Some(last_msaa(-4, 0)));
    let older = candidate(EventKind::MsaaFocus, (-9, 0), 1_000, 1);
    let newest = candidate(EventKind::MsaaFocus, (-4, 0), 1_100, 2);
    let run = vec![older, newest];
    assert_eq!(
        walked(&run, &state),
        Some((Validity::Duplicate, TraceId::from_raw(2)))
    );
    assert_eq!(behind(run, &state, None), Pending::No);
}

/// A duplicate behind a newer unconfirmed candidate: the newer one's read
/// decides, so the run is unconfirmed by it.
#[test]
fn a_duplicate_behind_a_newer_unconfirmed_candidate_leaves_the_run_unconfirmed() {
    let state = versions(Some(last_msaa(-4, 0)));
    let older = candidate(EventKind::MsaaFocus, (-4, 0), 1_000, 1);
    let newest = candidate(EventKind::MsaaFocus, (-9, 0), 1_100, 2);
    let run = vec![older, newest];
    assert_eq!(
        walked(&run, &state),
        Some((Validity::NeedsRead, TraceId::from_raw(2)))
    );
    assert_eq!(
        behind(run, &state, None),
        Pending::Unconfirmed(TraceId::from_raw(2))
    );
}

/// A valid candidate counts even behind a newer one needing a read, whose
/// read could only make it count or be refused down to the older one.
#[test]
fn a_valid_candidate_counts_behind_a_newer_unconfirmed_one() {
    let state = versions(None);
    let foreground = candidate(EventKind::Foreground, (0, 0), 1_000, 1);
    let newest = candidate(EventKind::MsaaFocus, (-9, 0), 1_100, 2);
    assert_eq!(
        behind(vec![foreground, newest], &state, None),
        Pending::Counts
    );
}

#[test]
fn stored_facts_decide_a_waiting_candidate_and_the_turn_alike() {
    let state = versions(None);
    let mut confirmed = focus(-4);
    confirmed.confirmation = Confirmation::Read(found(Some(true)));
    assert_eq!(
        walked(std::slice::from_ref(&confirmed), &state),
        Some((Validity::Valid(How::Read), TraceId::from_raw(7)))
    );
    assert_eq!(behind(vec![confirmed], &state, None), Pending::Counts);
    let mut refused = focus(-4);
    refused.confirmation = Confirmation::Read(found(Some(false)));
    assert_eq!(walked(std::slice::from_ref(&refused), &state), None);
    assert_eq!(behind(vec![refused], &state, None), Pending::No);
}

#[test]
fn an_unconfirmed_candidate_raised_at_or_after_the_bound_is_not_pending() {
    let state = versions(None);
    let raised = |at_us| candidate(EventKind::MsaaFocus, (-4, 0), at_us, 7);
    assert_eq!(
        behind(vec![raised(1_999)], &state, Some(2_000)),
        Pending::Unconfirmed(TraceId::from_raw(7))
    );
    assert_eq!(
        behind(vec![raised(2_000)], &state, Some(2_000)),
        Pending::No
    );
}

#[test]
fn over_the_queue_a_counting_item_wins_and_otherwise_the_newest_unconfirmed() {
    let state = versions(None);
    let cache = BackendCache::default();
    let at = view(&state, &cache);
    let one = |candidate: Candidate| Item::One(Entry::Candidate(Box::new(candidate)));
    let queue = VecDeque::from([
        one(candidate(EventKind::MsaaFocus, (-4, 0), 1_000, 1)),
        one(candidate(EventKind::MsaaFocus, (-5, 0), 1_100, 2)),
    ]);
    assert_eq!(
        pending_behind(&queue, &at, Within::ALL),
        Pending::Unconfirmed(TraceId::from_raw(2))
    );
    let mut queue = queue;
    queue.push_front(Item::One(Entry::UiaFocus(Box::new(uia(uia_tell(
        Vec::new(),
        "UiaHost",
        focused(Read::Answered(true)),
    ))))));
    assert_eq!(pending_behind(&queue, &at, Within::ALL), Pending::Counts);
}
