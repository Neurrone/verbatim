//! Core's foreground record (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the
//! window title"), driven through Core's event queue
//! (`docs/design/readers-tell-core-asks-build1.md`, "Focus and
//! foreground"): the listener's foreground report, whose marker empties
//! the readers' last focus at an unrelated window; the whole-window focus,
//! an MSAA foreground tell read by its application's outpost; acceptance
//! on the facts a tell carries; and the title rule. Each test drives the
//! reducer with one ordering of the tells, reports and answers the readers
//! produce and asserts every effect.

mod desk;

use desk::{App, Desk, Heard, LISTENER, follow, queued, values_of};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyValues, RuntimeId};
use verbatim_model::request::{
    CandidateAnswer, FocusAncestor, FocusCaret, FocusNode, FocusRead, ForegroundReport, HeldIds,
    MsaaIds, NodeRead, NodeWindow, ObjectClass, ObjectRead, Outcome, QueueInput, Reader, Reply,
    Request as Wire, Route, RouteEnd, RouteStep,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Event, EventObject, Payload, Read, Relation,
    TellWindow, WindowFacet,
};
use verbatim_model::{
    Backend, FocusNow, NodeDetails, NodeSnapshot, ObjectKey, Pid, Role, SegmentContent, StateSet,
    TraceId, UtteranceSegment, WindowHandle,
};

/// The application the user starts in, M, and its window.
const M: App = App {
    pid: Pid(1),
    generation: 11,
    top: 1_000,
};
/// An application that holds its calls, H, and its window.
const H: App = App {
    pid: Pid(2),
    generation: 12,
    top: 2_000,
};
/// An application whose splash window reports no focus.
const SPLASH: App = App {
    pid: Pid(3),
    generation: 13,
    top: 3_000,
};
/// A Windows Forms application, F: its form's window.
const F: App = App {
    pid: Pid(4),
    generation: 14,
    top: 4_000,
};
/// Another application.
const OTHER: App = App {
    pid: Pid(5),
    generation: 15,
    top: 5_000,
};
/// Explorer, whose Program Manager window holds the desktop.
const EXPLORER: App = App {
    pid: Pid(7),
    generation: 17,
    top: 7_000,
};
/// Verbatim, whose hidden frame owns its menu.
const VERBATIM: App = App {
    pid: Pid(9),
    generation: 19,
    top: 90_000,
};

/// F's dialog, a top-level window F's form owns.
const DIALOG: u64 = 4_100;
/// F's context menu, a popup F's form owns.
const MENU: u64 = 4_200;
/// F's combo box list, a popup F's form owns.
const LIST: u64 = 4_300;
/// F's named drop-down, a popup F's form owns.
const DROP_DOWN: u64 = 4_400;
/// A popup M's window owns.
const POPUP: u64 = 1_500;
/// A window M's window owns.
const OWNED: u64 = 1_300;
/// Verbatim's menu, which its hidden frame owns.
const VERBATIM_MENU: u64 = 91_000;

/// Where a focus is: its top-level window and that window's root owner,
/// and the window in front, with its root owner, when the reader checked
/// it.
#[derive(Clone, Copy, Debug)]
struct Win {
    top: u64,
    owner: u64,
    front: u64,
    front_owner: u64,
    /// Whether the window is owned by Verbatim's hidden frame, as its menu
    /// is.
    frame_owned: bool,
}

/// Top-level window `top`, its own root owner, in front.
const fn in_front(top: u64) -> Win {
    Win {
        top,
        owner: top,
        front: top,
        front_owner: top,
        frame_owned: false,
    }
}

/// Top-level window `top`, owned by `owner`, read while `front`, its own
/// root owner, was in front.
const fn owned(top: u64, owner: u64, front: u64) -> Win {
    Win {
        top,
        owner,
        front,
        front_owner: front,
        frame_owned: false,
    }
}

/// The window of `app`, of `class`.
fn facet(app: App, handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: app.pid,
        thread: 41,
    }
}

/// The acceptance facts of a focus in `app` at `win`, its control in the
/// window after its top-level window.
fn facts(app: App, win: Win) -> AcceptanceFacts {
    let to_front = if win.top == win.front {
        Relation::Under
    } else {
        Relation::Outside
    };
    AcceptanceFacts {
        window: facet(app, win.top + 1, "AppPane"),
        parent_class: String::new(),
        root: facet(app, win.top, "Frame"),
        root_owner: facet(app, win.owner, "Frame"),
        window_topmost: false,
        root_topmost: false,
        foreground: WindowHandle(win.front),
        foreground_root_owner: WindowHandle(win.front_owner),
        window_to_foreground: to_front,
        root_owner_to_foreground: if win.owner == win.front {
            Relation::Equal
        } else {
            Relation::Outside
        },
        root_owner_is_foregrounds: win.owner == win.front_owner,
        active_window: WindowHandle(win.front),
        window_to_active: to_front,
        desktop: false,
        hidden_frame: false,
        hidden_frame_owned: win.frame_owned,
    }
}

/// The control window of a focus at `win` in `app`, as a read names it.
fn node_window(app: App, win: Win) -> NodeWindow {
    NodeWindow {
        handle: WindowHandle(win.top + 1),
        top_level: WindowHandle(win.top),
        root_owner: WindowHandle(win.owner),
        process: app.pid,
        thread: 41,
        desktop: false,
    }
}

/// How a reader tells a focus.
#[derive(Clone, Copy, Debug)]
enum Via {
    /// UI Automation's focus event, which the check thread found kept and
    /// focused when `repeat`: a repeat of the reader's last focus.
    Uia { repeat: bool },
    /// An MSAA focus event on the client object of the focus's window,
    /// child id the focus's number.
    Msaa,
    /// An MSAA focus event on the client object of the top-level window
    /// itself, with no focused child.
    Client,
    /// The MSAA foreground event of the top-level window: a whole-window
    /// focus.
    Whole,
}

/// The MSAA ids a focus via `via` on node `number` at `win` is named by.
fn msaa_ids(via: Via, number: u64, win: Win) -> MsaaIds {
    match via {
        Via::Whole => MsaaIds {
            window: WindowHandle(win.top),
            object: 0,
            child: 0,
        },
        Via::Client => MsaaIds {
            window: WindowHandle(win.top),
            object: -4,
            child: 0,
        },
        Via::Msaa | Via::Uia { .. } => MsaaIds {
            window: WindowHandle(win.top + 1),
            object: -4,
            child: i32::try_from(number).expect("a small node number"),
        },
    }
}

/// The runtime id of node `number`.
fn runtime_of(number: u64) -> RuntimeId {
    RuntimeId(vec![
        42,
        i32::try_from(number).expect("a small node number"),
    ])
}

/// The listener's tell of a focus via `via` on node `number` of `app` at
/// `win`.
fn focus_tell(app: App, win: Win, via: Via, number: u64, at_us: u64) -> Event {
    let (kind, object, checked) = match via {
        Via::Uia { repeat } => (
            EventKind::UiaFocus,
            EventObject::Uia {
                token: ElementToken(number),
                runtime_id: runtime_of(number),
            },
            Some(Checked {
                probe: Some(verbatim_model::request::Probe {
                    window: WindowHandle(win.top + 1),
                    thread: 41,
                    provider: true,
                    began_us: at_us,
                }),
                keyboard_focus: Some(Read::Answered(true)),
                kept: if repeat {
                    vec![(ElementToken(number), Read::Answered(true))]
                } else {
                    Vec::new()
                },
                previous_sibling: None,
            }),
        ),
        Via::Msaa | Via::Client | Via::Whole => {
            let ids = msaa_ids(via, number, win);
            (
                if matches!(via, Via::Whole) {
                    EventKind::Foreground
                } else {
                    EventKind::MsaaFocus
                },
                EventObject::Msaa {
                    window: ids.window,
                    object: ids.object,
                    child: ids.child,
                },
                None,
            )
        }
    };
    let mut facts = facts(app, win);
    if matches!(via, Via::Whole | Via::Client) {
        facts.window = facts.root.clone();
        facts.window_to_foreground = if win.top == win.front {
            Relation::Equal
        } else {
            Relation::Outside
        };
        facts.window_to_active = facts.window_to_foreground;
    }
    Event {
        trace_id: TraceId::from_raw(7),
        kind,
        process: app.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us,
        object,
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts)),
        checked,
    }
}

/// `snapshot` as a read's node of `app` at `win`, with its runtime id when
/// read through UI Automation.
fn read_node(app: App, win: Win, via: Via, snapshot: &NodeSnapshot) -> FocusNode {
    FocusNode {
        read: NodeRead {
            node: Some(snapshot.id),
            window: node_window(app, win),
            values: values_of(snapshot),
        },
        runtime_id: matches!(via, Via::Uia { .. }).then(|| runtime_of(snapshot.id.number())),
    }
}

/// The outpost's read of a focus via `via` on `focus` of `app` at `win`,
/// under `ancestors`, outermost first, its foreground object `foreground`.
fn focus_read(
    (app, win, via): (App, Win, Via),
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
    foreground: Option<&NodeSnapshot>,
) -> FocusRead {
    FocusRead {
        focus: read_node(app, win, via, focus),
        msaa: match via {
            Via::Uia { .. } => None,
            Via::Msaa | Via::Client | Via::Whole => {
                let event = msaa_ids(via, focus.id.number(), win);
                Some(HeldIds {
                    event,
                    held_child: event.child,
                })
            }
        },
        ancestors: ancestors
            .iter()
            .rev()
            .map(|ancestor| FocusAncestor {
                node: read_node(app, win, via, ancestor),
                simple_child: false,
                dialog_text: None,
            })
            .collect(),
        met: None,
        foreground: foreground.map(|foreground| read_node(app, win, via, foreground)),
        value_from_selection: false,
        class: ObjectClass::Plain,
        runtime_id_reused: false,
        caret: FocusCaret::NoText,
    }
}

/// The listener's read of a UI Automation focus element of node `number`
/// in `app` at `win`: its route to its window.
fn listener_read(app: App, win: Win, number: u64) -> ObjectRead {
    ObjectRead {
        object: NodeRead {
            node: None,
            window: node_window(app, win),
            values: PropertyValues::default(),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: Some(Route {
            window: WindowHandle(win.top + 1),
            steps: vec![RouteStep {
                runtime_id: runtime_of(number),
                window: None,
                content: true,
            }],
            end: RouteEnd::ContinueFrom(WindowHandle(win.top + 1)),
        }),
    }
}

/// A focus via `via` on `focus` of `app` at `win`, under `ancestors`,
/// outermost first, its foreground object `foreground`, told by the
/// listener and read by the readers as Core asks: every effect of the
/// tell and the answers, the requests left out. A tell Core ends with no
/// request ends there.
fn focus(
    desk: &mut Desk,
    (app, win, via): (App, Win, Via),
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
    foreground: Option<&NodeSnapshot>,
) -> Vec<Heard> {
    desk.app(app);
    let number = focus.id.number();
    let tell = focus_tell(app, win, via, number, desk.at_us + 1_000);
    let read = focus_read((app, win, via), focus, ancestors, foreground);
    let heard = desk.tell(LISTENER, tell);
    answered(desk, heard, &|request| match request {
        Wire::ReadObject(_) => Reply::Object(Box::new(listener_read(app, win, number))),
        Wire::CompleteUia(_) => Reply::Focus(Box::new(read.clone())),
        Wire::MsaaRun(_) => Reply::Candidate(CandidateAnswer::focused(read.clone())),
        other => panic!("a focus read asked: {other:?}"),
    })
}

/// `heard`, then every request the step sent and those their answers
/// send, answered in order: a `Follow` followed, any other by `reply`.
/// Every effect but the requests, which the answers stand for.
fn answered(desk: &mut Desk, heard: Vec<Heard>, reply: &dyn Fn(&Wire) -> Reply) -> Vec<Heard> {
    let mut heard = heard;
    let mut pending = std::mem::take(&mut desk.sent);
    while !pending.is_empty() {
        let (number, to, request) = pending.remove(0);
        let answer = match &request {
            Wire::Follow(_) => Reply::Followed,
            other => reply(other),
        };
        heard.extend(desk.answer(to, number, Outcome::Result(answer)));
        pending.append(&mut desk.sent);
    }
    heard
        .into_iter()
        .filter(|heard| !matches!(heard, Heard::Sent(_, kind) if *kind != "Follow"))
        .collect()
}

/// The listener's report that window `window`, of root owner `root_owner`,
/// of `app`, came to the foreground: every effect of it.
fn change(desk: &mut Desk, app: App, (window, root_owner): (u64, u64)) -> Vec<Heard> {
    desk.app(app);
    desk.step(QueueInput::Foreground(ForegroundReport {
        window: WindowHandle(window),
        root_owner: WindowHandle(root_owner),
        process: app.pid,
        thread: 41,
        class: "Frame".to_owned(),
    }))
}

/// [`change`] for `app`'s window `window`, its own root owner.
fn change_to(desk: &mut Desk, app: App, window: u64) -> Vec<Heard> {
    change(desk, app, (window, window))
}

/// Where the focus now is: node `focus` of `app` under `ancestors`, with
/// the foreground object `foreground`, a node of `app`.
fn now(app: App, focus: u64, ancestors: &[u64], foreground: Option<u64>) -> Heard {
    let key = |number| ObjectKey::of_id(app.node(number));
    Heard::Expire(FocusNow {
        focus: key(focus),
        ancestors: ancestors.iter().map(|&number| key(number)).collect(),
        foreground: foreground.map(key),
        menu_item_of_focus: None,
    })
}

/// A queued utterance of `name` and, when given, `role`.
fn said(name: &str, role: Option<Role>) -> Heard {
    let mut segments = vec![UtteranceSegment::label(name)];
    segments.extend(role.map(|role| UtteranceSegment::new(SegmentContent::Role(role))));
    queued(segments)
}

fn node(app: App, id: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(id),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

fn button(app: App, id: u64, name: &str) -> NodeSnapshot {
    node(app, id, Role::Button, name)
}

/// A window element of `app` for window `handle`, as a chain carries it.
fn window_entry(app: App, id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut window = node(app, id, Role::Window, name);
    window.details.window = Some(WindowHandle(handle));
    window
}

/// A window's client area, a pane with the window's handle, not
/// presentable as a container, as a Windows Forms edit's walk reaches its
/// form.
fn quiet_pane(app: App, id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut pane = node(app, id, Role::Pane, name);
    pane.details.window = Some(WindowHandle(handle));
    pane
}

/// M's window element and the edit control in it, the focus the user is in.
fn main_window() -> NodeSnapshot {
    window_entry(M, 5, "Notes", M.top)
}

fn edit() -> NodeSnapshot {
    button(M, 1, "Edit")
}

/// A desk with the user in M's edit control: M's window came to the
/// foreground, the listener's first report, Core's read of M's focus at
/// startup answered with the edit, and UI Automation's focus event for the
/// edit told and read.
fn in_the_edit() -> Desk {
    let mut desk = Desk::new();
    let heard = change_to(&mut desk, M, M.top);
    assert_eq!(heard, [Heard::Sent(Reader::Outpost(M.pid), "CurrentFocus")]);
    let read = focus_read(
        (M, in_front(M.top), Via::Uia { repeat: false }),
        &edit(),
        &[main_window()],
        Some(&main_window()),
    );
    let _ = answered(&mut desk, Vec::new(), &|_| {
        Reply::Candidate(verbatim_model::request::CandidateAnswer::focused(
            read.clone(),
        ))
    });
    // UI Automation's focus event for the edit, which the listener keeps,
    // so that a repeat of it is a duplicate.
    let _ = focus(
        &mut desk,
        (M, in_front(M.top), Via::Uia { repeat: false }),
        &edit(),
        &[main_window()],
        Some(&main_window()),
    );
    desk
}

/// The edit focused again in M's window, as UI Automation tells it, its
/// element still kept.
fn the_edit_again(desk: &mut Desk) -> Vec<Heard> {
    focus(
        desk,
        (M, in_front(M.top), Via::Uia { repeat: true }),
        &edit(),
        &[main_window()],
        Some(&main_window()),
    )
}

/// What the edit's focus says when it enters M's window from another.
fn the_edit_entered() -> Vec<Heard> {
    vec![
        now(M, 1, &[5], Some(5)),
        Heard::Stop,
        said("Notes", Some(Role::Window)),
        said("Edit", Some(Role::Button)),
        follow(M),
    ]
}

// ---- The foreground change ----

/// A popup's focus, whose top-level window is not the foreground window,
/// leaves the entered window as it was; a foreground change of that window,
/// reported again, does not leave the popup's focus.
#[test]
fn a_foreground_change_of_the_entered_window_leaves_a_popup_focus_in_front() {
    let mut desk = in_the_edit();
    let item = node(M, 3, Role::ListItem, "Copy");
    let popup = owned(POPUP, M.top, M.top);
    let _ = focus(
        &mut desk,
        (M, popup, Via::Uia { repeat: false }),
        &item,
        &[],
        None,
    );
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(M.node(3))
    );
    assert_eq!(change_to(&mut desk, M, M.top), []);
    assert_eq!(
        focus(
            &mut desk,
            (M, popup, Via::Uia { repeat: true }),
            &item,
            &[],
            None
        ),
        []
    );
}

/// A foreground change into a window M owns does not leave M's focus.
#[test]
fn a_foreground_change_into_an_owned_window_leaves_the_focus_in_front() {
    let mut desk = in_the_edit();
    assert_eq!(change(&mut desk, M, (OWNED, M.top)), []);
    assert_eq!(the_edit_again(&mut desk), []);
}

/// A focus left, the user having gone to H and back, is no duplicate when
/// it is reported again: it is spoken as a focus entering M from another
/// window.
#[test]
fn a_left_focus_reported_again_is_no_duplicate() {
    let mut desk = in_the_edit();
    assert_eq!(change_to(&mut desk, H, H.top), []);
    assert_eq!(change_to(&mut desk, M, M.top), []);
    assert_eq!(the_edit_again(&mut desk), the_edit_entered());
}

/// A left focus reported again by a replacement outpost, its own having
/// ended, is spoken, not taken silently as the same focus.
#[test]
fn a_left_focus_from_a_replacement_outpost_is_spoken() {
    let mut desk = in_the_edit();
    let replaced = M.replaced();
    desk.app(replaced);
    let _ = desk.step(QueueInput::ReaderEnded(M.instance()));
    let _ = change_to(&mut desk, H, H.top);
    let _ = change_to(&mut desk, replaced, M.top);
    let window = window_entry(replaced, 5, "Notes", M.top);
    assert_eq!(
        focus(
            &mut desk,
            (replaced, in_front(M.top), Via::Uia { repeat: false }),
            &button(replaced, 1, "Edit"),
            std::slice::from_ref(&window),
            Some(&window),
        ),
        [
            now(replaced, 1, &[5], Some(5)),
            Heard::Stop,
            said("Notes", Some(Role::Window)),
            said("Edit", Some(Role::Button)),
            follow(replaced),
        ]
    );
}

/// A whole-window focus of the window holding Core's focus, after that focus
/// was left, is taken and spoken.
#[test]
fn a_whole_window_focus_of_the_left_focus_window_is_taken() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, H, H.top);
    let _ = change_to(&mut desk, M, M.top);
    let client = quiet_pane(M, 7, "Notes", M.top);
    assert_eq!(
        focus(
            &mut desk,
            (M, in_front(M.top), Via::Whole),
            &client,
            &[],
            Some(&client)
        ),
        [
            now(M, 7, &[], Some(7)),
            Heard::Stop,
            said("Notes", None),
            follow(M),
        ]
    );
}

/// A splash window that takes the foreground and reports no focus leaves
/// Core's focus as any window does, so the return after it is spoken.
#[test]
fn the_return_after_a_uia_splash_window_is_spoken() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, SPLASH, SPLASH.top);
    let _ = change_to(&mut desk, M, M.top);
    assert_eq!(the_edit_again(&mut desk), the_edit_entered());
}

/// The switch to H and back, by whether H answered: with a stall, H's focus
/// never comes; without, Core takes H's focus before the return. Returns
/// the return's effects.
fn return_to(stall: bool) -> Vec<Heard> {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, H, H.top);
    if !stall {
        let window = window_entry(H, 9, "Busy app", H.top);
        let _ = focus(
            &mut desk,
            (H, in_front(H.top), Via::Uia { repeat: false }),
            &button(H, 2, "Busy"),
            std::slice::from_ref(&window),
            Some(&window),
        );
    }
    let _ = change_to(&mut desk, M, M.top);
    the_edit_again(&mut desk)
}

/// The return from an application that never answered says what the same
/// switch says when the application's focus was taken.
#[test]
fn a_return_after_a_foreground_change_says_what_the_switch_says_without_a_stall() {
    assert_eq!(return_to(true), the_edit_entered());
    let mut answered = the_edit_entered();
    answered.insert(answered.len() - 1, follow(H));
    assert_eq!(return_to(false), answered);
}

// ---- Acceptance ----

/// A held application's focus, checked while its window was in front and
/// told after the user returned to M, is dropped: the window in front at
/// its check, M, is unrelated to it, so the edit stays the focus.
#[test]
fn a_held_applications_late_focus_after_the_return_is_dropped() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, H, H.top);
    let _ = change_to(&mut desk, M, M.top);
    assert_eq!(the_edit_again(&mut desk), the_edit_entered());
    assert_eq!(
        focus(
            &mut desk,
            (H, owned(H.top, H.top, M.top), Via::Uia { repeat: false }),
            &button(H, 2, "Late"),
            &[],
            None
        ),
        []
    );
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(M.node(1))
    );
}

/// A focus entering the window in front makes its chain's outermost entry
/// the foreground object, as NVDA's `doPreGainFocus` moves its foreground
/// object from the focus's ancestry; a later focus in the same window
/// leaves it there.
#[test]
fn a_focus_entering_a_uia_window_moves_the_foreground_object() {
    let mut desk = in_the_edit();
    assert_eq!(
        focus(
            &mut desk,
            (M, in_front(M.top), Via::Uia { repeat: false }),
            &button(M, 2, "Save"),
            &[main_window()],
            Some(&main_window())
        ),
        [
            now(M, 2, &[5], Some(5)),
            said("Save", Some(Role::Button)),
            follow(M),
        ]
    );
}

// ---- The whole-window focus ----

/// H's window's client pane, as a whole-window focus reads it.
fn calculator() -> NodeSnapshot {
    quiet_pane(H, 7, "Calculator", H.top)
}

/// A whole-window focus checked while its window was in front, told after
/// a switch to an unrelated window reached Core, is dropped.
#[test]
fn a_whole_window_focus_of_a_window_left_since_is_dropped() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, SPLASH, SPLASH.top);
    let window = calculator();
    assert_eq!(
        focus(
            &mut desk,
            (H, in_front(H.top), Via::Whole),
            &window,
            &[],
            Some(&window)
        ),
        []
    );
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(M.node(1))
    );
}

/// A whole-window focus of the window holding Core's focus, not left, is
/// refused.
#[test]
fn a_whole_window_focus_in_the_window_holding_the_focus_is_refused() {
    let mut desk = in_the_edit();
    let client = quiet_pane(M, 7, "Notes", M.top);
    assert_eq!(
        focus(
            &mut desk,
            (M, in_front(M.top), Via::Whole),
            &client,
            &[],
            Some(&client)
        ),
        []
    );
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(M.node(1))
    );
}

/// An MSAA focus on a window's client object, with no focused child, reads
/// as the same node as a whole-window focus of the window, but is a focus:
/// it is taken in the window holding Core's focus, not refused.
#[test]
fn a_focus_on_a_windows_client_is_not_refused_as_a_whole_window_focus() {
    let mut desk = in_the_edit();
    let client = quiet_pane(M, 7, "Notes", M.top);
    assert_eq!(
        focus(
            &mut desk,
            (M, in_front(M.top), Via::Client),
            &client,
            &[main_window()],
            Some(&main_window())
        ),
        [now(M, 7, &[5], Some(5)), said("Notes", None), follow(M)]
    );
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(M.node(7))
    );
}

// ---- The title rule ----

/// F's form, its client reached with no identity string, titled `title`.
fn form(title: &str) -> NodeSnapshot {
    quiet_pane(F, 400, title, F.top)
}

/// The text box in F's form.
fn text_box() -> NodeSnapshot {
    node(F, 401, Role::EditableText, "Note")
}

/// A focus in F's form via MSAA on `focused`, under the form titled
/// "second-application".
fn in_form(desk: &mut Desk, focused: &NodeSnapshot) -> Vec<Heard> {
    let form = form("second-application");
    focus(
        desk,
        (F, in_front(F.top), Via::Msaa),
        focused,
        std::slice::from_ref(&form),
        Some(&form),
    )
}

/// F's form whole, as its MSAA foreground event reads it.
fn whole_form(desk: &mut Desk) -> Vec<Heard> {
    let form = form("second-application");
    focus(
        desk,
        (F, in_front(F.top), Via::Whole),
        &form,
        &[],
        Some(&form),
    )
}

/// The title the title rule says for a form's quiet client pane: its name
/// alone, a pane's role being silent on focus, as a whole-window focus of
/// the window's client says it.
fn title(name: &str) -> Heard {
    said(name, None)
}

/// What the text box's focus says entering the form from another window.
fn the_box_entered() -> Vec<Heard> {
    vec![
        now(F, 401, &[400], Some(400)),
        Heard::Stop,
        title("second-application"),
        said("Note", Some(Role::EditableText)),
    ]
}

/// A focus entering a form, after its foreground change, says the form's
/// title, which its line does not speak.
#[test]
fn a_forms_focus_with_its_foreground_change_left_untried_says_the_title() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let mut entered = the_box_entered();
    entered.push(follow(M));
    assert_eq!(in_form(&mut desk, &text_box()), entered);
}

/// A button b of form F has the focus; Enter opens dialog D, and b's
/// focus, served by F's busy thread, is told after D is in front: Core
/// takes it with the entered window unchanged. D's foreground change and
/// its first focus follow: D's title is said with its focus.
#[test]
fn a_dialogs_title_is_said_with_its_focus_after_its_forms_late_focus() {
    let mut desk = in_the_edit();
    let form = form("Form");
    let _ = change_to(&mut desk, F, F.top);
    let _ = focus(
        &mut desk,
        (F, in_front(F.top), Via::Msaa),
        &button(F, 401, "Open"),
        std::slice::from_ref(&form),
        Some(&form),
    );
    assert_eq!(
        focus(
            &mut desk,
            (
                F,
                Win {
                    top: F.top,
                    owner: F.top,
                    front: DIALOG,
                    front_owner: F.top,
                    frame_owned: false,
                },
                Via::Msaa,
            ),
            &button(F, 402, "Open again"),
            std::slice::from_ref(&form),
            Some(&form),
        ),
        [
            now(F, 402, &[400], Some(400)),
            said("Open again", Some(Role::Button)),
        ]
    );
    let _ = change(&mut desk, F, (DIALOG, F.top));
    let dialog = quiet_pane(F, 410, "Find", DIALOG);
    assert_eq!(
        focus(
            &mut desk,
            (F, dialog_window(), Via::Msaa),
            &button(F, 411, "OK"),
            std::slice::from_ref(&dialog),
            Some(&dialog),
        ),
        [
            now(F, 411, &[410], Some(410)),
            Heard::Stop,
            title("Find"),
            said("OK", Some(Role::Button)),
        ]
    );
}

/// F's dialog in front, its root owner F's form.
const fn dialog_window() -> Win {
    Win {
        top: DIALOG,
        owner: F.top,
        front: DIALOG,
        front_owner: F.top,
        frame_owned: false,
    }
}

/// A whole-window focus taken first enters its window: the focus after it
/// says no title, so the title is said once.
#[test]
fn a_whole_window_focus_taken_first_says_the_title_once() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    assert_eq!(
        whole_form(&mut desk),
        [
            now(F, 400, &[], Some(400)),
            Heard::Stop,
            said("second-application", None),
            follow(M),
        ]
    );
    assert_eq!(
        in_form(&mut desk, &text_box()),
        [
            now(F, 401, &[400], Some(400)),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The focus before the whole-window focus: the title is said with the
/// focus, and the whole-window focus is then refused.
#[test]
fn a_focus_before_its_whole_window_focus_says_the_title_and_the_whole_window_focus_is_refused() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let mut entered = the_box_entered();
    entered.push(follow(M));
    assert_eq!(in_form(&mut desk, &text_box()), entered);
    assert_eq!(whole_form(&mut desk), []);
}

/// The context menu of a form's control, Shift+F10 then Escape: the menu is
/// a popup in the form's foreground, so the return says no title.
#[test]
fn a_context_menu_opened_and_closed_says_no_title_on_the_return() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &text_box());
    let _ = focus(
        &mut desk,
        (F, owned(MENU, F.top, F.top), Via::Msaa),
        &node(F, 421, Role::MenuItem, "Cut"),
        &[],
        None,
    );
    assert_eq!(
        in_form(&mut desk, &text_box()),
        [
            now(F, 401, &[400], Some(400)),
            Heard::Stop,
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// A combo box's list opening, a popup owned by the form, and closing: no
/// title either way.
#[test]
fn a_combo_box_list_opening_and_closing_says_no_title() {
    let mut desk = in_the_edit();
    let combo = node(F, 401, Role::ComboBox, "Size");
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &combo);
    assert_eq!(
        focus(
            &mut desk,
            (F, owned(LIST, F.top, F.top), Via::Msaa),
            &node(F, 431, Role::ListItem, "Large"),
            &[],
            None
        ),
        [
            now(F, 431, &[], Some(400)),
            Heard::Stop,
            said("Large", None),
        ]
    );
    assert_eq!(
        in_form(&mut desk, &combo),
        [
            now(F, 401, &[400], Some(400)),
            Heard::Stop,
            said("Size", Some(Role::ComboBox)),
        ]
    );
}

/// A named Windows Forms drop-down, a top-level window of its own that is
/// not the foreground window, is spoken only as the presentability rule
/// says: as an entered container, with no title of the form's.
#[test]
fn a_named_drop_down_entered_is_spoken_only_as_the_presentability_rule_says() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &button(F, 401, "Pick"));
    let drop_down = window_entry(F, 440, "Choices", DROP_DOWN);
    assert_eq!(
        focus(
            &mut desk,
            (F, owned(DROP_DOWN, F.top, F.top), Via::Msaa),
            &node(F, 441, Role::ListItem, "First"),
            std::slice::from_ref(&drop_down),
            None,
        ),
        [
            now(F, 441, &[440], Some(400)),
            Heard::Stop,
            said("Choices", Some(Role::Window)),
            said("First", None),
        ]
    );
}

/// An owned dialog opening and closing: its title, then the main window's.
#[test]
fn an_owned_dialog_opening_and_closing_says_its_title_then_the_main_windows() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &text_box());
    let _ = change(&mut desk, F, (DIALOG, F.top));
    let dialog = quiet_pane(F, 410, "Find", DIALOG);
    assert_eq!(
        focus(
            &mut desk,
            (F, dialog_window(), Via::Msaa),
            &button(F, 411, "OK"),
            std::slice::from_ref(&dialog),
            Some(&dialog),
        ),
        [
            now(F, 411, &[410], Some(410)),
            Heard::Stop,
            title("Find"),
            said("OK", Some(Role::Button)),
        ]
    );
    let _ = change_to(&mut desk, F, F.top);
    assert_eq!(in_form(&mut desk, &text_box()), the_box_entered());
}

/// Verbatim's menu opened over F's form: Verbatim's window in front, and
/// the focus on its menu's item, in a window its hidden frame owns.
fn verbatims_menu(desk: &mut Desk) {
    let _ = change_to(desk, VERBATIM, VERBATIM.top);
    let frame = window_entry(VERBATIM, 900, "Verbatim", VERBATIM.top);
    let menu = Win {
        top: VERBATIM_MENU,
        owner: VERBATIM.top,
        front: VERBATIM.top,
        front_owner: VERBATIM.top,
        frame_owned: true,
    };
    let _ = focus(
        desk,
        (VERBATIM, menu, Via::Msaa),
        &node(VERBATIM, 911, Role::MenuItem, "Settings..."),
        std::slice::from_ref(&frame),
        Some(&frame),
    );
}

/// The return from Verbatim's menu: the form's whole-window focus on the
/// return is taken, Core's focus being the menu's item, and says the
/// title, once.
#[test]
fn a_return_from_verbatims_menu_says_the_title() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &text_box());
    verbatims_menu(&mut desk);
    let _ = change_to(&mut desk, F, F.top);
    assert_eq!(
        whole_form(&mut desk),
        [
            now(F, 400, &[], Some(400)),
            Heard::Stop,
            said("second-application", None),
        ]
    );
    assert_eq!(
        in_form(&mut desk, &text_box()),
        [
            now(F, 401, &[400], Some(400)),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The return from Verbatim's menu whose only report is the form's focus:
/// the menu took the user out of the entered window, so the focus says the
/// title.
#[test]
fn a_return_from_verbatims_menu_with_its_focus_alone_says_the_title() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &text_box());
    verbatims_menu(&mut desk);
    let _ = change_to(&mut desk, F, F.top);
    assert_eq!(in_form(&mut desk, &text_box()), the_box_entered());
}

/// A focus on the desktop, in Program Manager's window in front, says no
/// title: NVDA takes no foreground event for Program Manager or the taskbar.
#[test]
fn a_focus_in_program_manager_says_no_title() {
    let mut desk = in_the_edit();
    desk.app(EXPLORER);
    let _ = desk.step(QueueInput::Foreground(ForegroundReport {
        window: WindowHandle(EXPLORER.top),
        root_owner: WindowHandle(EXPLORER.top),
        process: EXPLORER.pid,
        thread: 41,
        class: "Progman".to_owned(),
    }));
    let program_manager = quiet_pane(EXPLORER, 700, "Program Manager", EXPLORER.top);
    let list = node(EXPLORER, 710, Role::List, "Desktop");
    assert_eq!(
        focus(
            &mut desk,
            (EXPLORER, in_front(EXPLORER.top), Via::Msaa),
            &node(EXPLORER, 711, Role::ListItem, "NVDA"),
            &[program_manager.clone(), list],
            Some(&program_manager),
        ),
        [
            now(EXPLORER, 711, &[700, 710], Some(700)),
            Heard::Stop,
            said("Desktop", Some(Role::List)),
            said("NVDA", None),
            follow(M),
        ]
    );
}

/// A nameless foreground window's entry, and a chain with no entry for the
/// window, add nothing.
#[test]
fn a_nameless_window_and_a_chain_without_the_window_add_no_title() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let mut nameless = quiet_pane(F, 400, "", F.top);
    nameless.name = None;
    assert_eq!(
        focus(
            &mut desk,
            (F, in_front(F.top), Via::Msaa),
            &button(F, 401, "One"),
            std::slice::from_ref(&nameless),
            Some(&nameless),
        ),
        [
            now(F, 401, &[400], Some(400)),
            Heard::Stop,
            said("One", Some(Role::Button)),
            follow(M),
        ]
    );
    let _ = change_to(&mut desk, OTHER, OTHER.top);
    assert_eq!(
        focus(
            &mut desk,
            (OTHER, in_front(OTHER.top), Via::Msaa),
            &button(OTHER, 501, "Two"),
            &[],
            None
        ),
        [
            now(OTHER, 501, &[], None),
            Heard::Stop,
            said("Two", Some(Role::Button)),
        ]
    );
}

/// The title's words are those of a whole-window focus of the same window
/// read by the same backend: the client pane through MSAA.
#[test]
fn the_titles_words_are_a_whole_window_focus_words() {
    let mut entering = in_the_edit();
    let _ = change_to(&mut entering, F, F.top);
    let titled = in_form(&mut entering, &button(F, 401, "One"));
    let mut whole = in_the_edit();
    let _ = change_to(&mut whole, F, F.top);
    let window = whole_form(&mut whole);
    assert_eq!(titled.get(2), Some(&title("second-application")));
    assert_eq!(window.get(2), titled.get(2));
}

/// A foreground change into another window, then a focus in the window
/// left: its title is said, Core's focus having been left.
#[test]
fn a_focus_in_the_window_left_after_a_foreground_change_says_the_title() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, F, F.top);
    let _ = in_form(&mut desk, &text_box());
    let _ = change_to(&mut desk, H, H.top);
    let _ = change_to(&mut desk, F, F.top);
    assert_eq!(in_form(&mut desk, &text_box()), the_box_entered());
}

// ---- A window's object through two backends ----

/// A UIA focus after the whole-window focus of its window's MSAA object, a
/// window read from local state when its UIA element could not be read,
/// says the window again: the window's UIA element in its chain is another
/// object, as NVDA never equals objects of two types (`NVDAObject.__eq__`;
/// `docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_uia_focus_after_its_windows_msaa_object_says_the_window_again() {
    let mut desk = in_the_edit();
    let _ = change_to(&mut desk, H, H.top);
    let plain = window_entry(H, 8, "held-application", H.top);
    assert_eq!(
        focus(
            &mut desk,
            (H, in_front(H.top), Via::Whole),
            &plain,
            &[],
            Some(&plain)
        ),
        [
            now(H, 8, &[], Some(8)),
            Heard::Stop,
            said("held-application", Some(Role::Window)),
            follow(M),
        ]
    );
    let window = window_entry(H, 9, "held-application", H.top);
    assert_eq!(
        focus(
            &mut desk,
            (H, in_front(H.top), Via::Uia { repeat: false }),
            &button(H, 3, "OK"),
            std::slice::from_ref(&window),
            Some(&window),
        ),
        [
            now(H, 3, &[9], Some(8)),
            said("held-application", Some(Role::Window)),
            said("OK", Some(Role::Button)),
            follow(H),
        ]
    );
}
