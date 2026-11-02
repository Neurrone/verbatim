//! Core's foreground record (`docs/design/focus-pipeline-rework-remaining.md`,
//! "The second build"): the focus listener's foreground change, the only
//! report that a window came to the foreground, which moves attention, sets
//! the record's front and marks Core's focus left; the front a focus is
//! judged against; the whole-window focus; and the title rule. Each test
//! drives the reducer with one ordering of the inputs the listener and the
//! outposts produce and asserts every effect.

use verbatim_core::SrState;
use verbatim_model::{
    Backend, Effect, ForegroundWindow, Input, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent,
    OutpostId, Pid, PropertyChange, Role, SegmentContent, SpeechPriority, StateSet, TraceId,
    WindowFacts, WindowHandle,
};

/// The window of the application the user starts in, M.
const MAIN: u64 = 10;
/// The window of an application that holds its UIA calls, H.
const HELD: u64 = 20;

/// The reducer, fed as Core feeds it: an event's node ids are stamped with
/// the outpost it came from, here the outpost numbered as its process.
struct Core {
    state: SrState,
}

/// One effect as these tests compare it: where the focus now is, by node
/// numbers, speech cut off, an utterance by its priority and segments, and
/// any other effect by its debug form, so that every effect is compared.
#[derive(Debug, PartialEq)]
enum Heard {
    Expire {
        focus: u64,
        ancestors: Vec<u64>,
        foreground: Option<u64>,
    },
    Stop,
    Say(SpeechPriority, Vec<SegmentContent>),
    Other(String),
}

impl Core {
    fn new() -> Self {
        Self {
            state: SrState::new(),
        }
    }

    /// Reduces `input`, an event from the outpost of its process.
    fn input(&mut self, input: Input) -> Vec<Heard> {
        let outpost = match &input {
            Input::Event { source, .. } => OutpostId(u64::from(source.0)),
            _ => OutpostId(0),
        };
        self.input_from(outpost, input)
    }

    /// Reduces `input`, an event from outpost generation `outpost`.
    fn input_from(&mut self, outpost: OutpostId, mut input: Input) -> Vec<Heard> {
        if let Input::Event { event, .. } = &mut input {
            event.assign_outpost(outpost);
        }
        heard(&verbatim_core::reduce(&mut self.state, &input))
    }

    /// The number of the node Core holds as its focus.
    fn focus(&self) -> Option<u64> {
        self.state.focused().map(|(_, node)| node.id.number())
    }
}

fn heard(effects: &[Effect]) -> Vec<Heard> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::DropExpiredSpeech(now) => Heard::Expire {
                focus: now.focus.id.number(),
                ancestors: now.ancestors.iter().map(|key| key.id.number()).collect(),
                foreground: now.foreground.map(|key| key.id.number()),
            },
            Effect::StopSpeech => Heard::Stop,
            Effect::Speak(utterance, _) => Heard::Say(
                utterance.priority,
                utterance
                    .segments
                    .iter()
                    .map(|segment| segment.content.clone())
                    .collect(),
            ),
            other => Heard::Other(format!("{other:?}")),
        })
        .collect()
}

/// A queued utterance of `name` and, when given, `role`.
fn said(name: &str, role: Option<Role>) -> Heard {
    let mut segments = vec![SegmentContent::Label(name.to_owned())];
    segments.extend(role.map(SegmentContent::Role));
    Heard::Say(SpeechPriority::Queued, segments)
}

fn node(id: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::new(id),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

fn button(id: u64, name: &str) -> NodeSnapshot {
    node(id, Role::Button, name)
}

/// A UIA window element for window `handle`, as a chain carries it.
fn window_entry(id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut window = node(id, Role::Window, name);
    window.details.window = Some(WindowHandle(handle));
    window
}

/// A window's client area reached by an MSAA walk with no identity string:
/// a pane with the window's handle, not presentable as a container, as a
/// Windows Forms edit's walk reaches its form.
fn quiet_pane(id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut pane = node(id, Role::Pane, name);
    pane.backend = Backend::Msaa;
    pane.details.window = Some(WindowHandle(handle));
    pane
}

/// A top-level window's client area read through MSAA, as the outpost
/// reports a whole-window focus of a window it reads through MSAA: a pane
/// standing for its window.
fn client(id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut pane = quiet_pane(id, name, handle);
    pane.details.top_level_client = true;
    pane
}

/// The facts of a window whose top-level window is `top`, its root owner
/// `root_owner`, read while `foreground` was the system's foreground window.
fn facts(top: u64, root_owner: u64, foreground: u64) -> WindowFacts {
    WindowFacts {
        top_level: WindowHandle(top),
        root_owner: WindowHandle(root_owner),
        topmost: false,
        under_active_window: None,
        in_foreground: foreground == top || foreground == root_owner,
        foreground_window: WindowHandle(foreground),
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
    }
}

/// The facts of top-level window `top`, read while it was in front.
fn in_front(top: u64) -> WindowFacts {
    facts(top, top, top)
}

/// A focus on `node`, with its chain `ancestors`, in the window `facts`
/// describe, from application `pid`, observed at `at_us`.
fn focus(
    pid: u32,
    facts: WindowFacts,
    node: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
    at_us: u64,
) -> Input {
    Input::Event {
        trace_id: TraceId::from_raw(at_us),
        observed_at_us: at_us,
        source: Pid(pid),
        backend: node.backend,
        window: Some(facts),
        event: NormalizedEvent::FocusChanged {
            node,
            whole_window: false,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    }
}

/// A whole-window focus of `node`, the top-level window `facts` describe,
/// from application `pid`, observed at `at_us`.
fn whole_window(pid: u32, facts: WindowFacts, node: NodeSnapshot, at_us: u64) -> Input {
    let mut input = focus(pid, facts, node, Vec::new(), at_us);
    if let Input::Event {
        event: NormalizedEvent::FocusChanged { whole_window, .. },
        ..
    } = &mut input
    {
        *whole_window = true;
    }
    input
}

/// The focus listener's report that window `window`, of root owner
/// `root_owner` and application `pid`, came to the foreground, observed at
/// `at_us`.
fn change(pid: u32, (window, root_owner): (u64, u64), at_us: u64) -> Input {
    Input::Foreground(ForegroundWindow {
        window: WindowHandle(window),
        root_owner: WindowHandle(root_owner),
        process: Pid(pid),
        observed_at_us: at_us,
        own: false,
        thread: 0,
        console: false,
        integrity: None,
        own_integrity: None,
    })
}

/// [`change`] for a window that is its own root owner.
fn change_to(pid: u32, window: u64, at_us: u64) -> Input {
    change(pid, (window, window), at_us)
}

/// M's window element and the edit control in it, the focus the user is in.
fn main_window() -> NodeSnapshot {
    window_entry(5, "Notes", MAIN)
}

fn edit() -> NodeSnapshot {
    button(1, "Edit")
}

/// Core with the user in M's edit control: M's foreground change and the
/// edit's focus, observed at 50 and 100.
fn in_the_edit() -> Core {
    let mut core = Core::new();
    assert_eq!(core.input(change_to(1, MAIN, 50)), []);
    core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 100));
    core
}

/// What the edit's focus says when it enters M's window from another.
fn the_edit_entered() -> Vec<Heard> {
    vec![
        Heard::Expire {
            focus: 1,
            ancestors: vec![5],
            foreground: Some(5),
        },
        Heard::Stop,
        said("Notes", Some(Role::Window)),
        said("Edit", Some(Role::Button)),
    ]
}

// ---- The foreground change ----

/// A foreground change observed before a focus from another outpost leaves
/// that focus judged by the focuses alone: it sets no ordering stamp, so the
/// focus observed after the edit's is not stale.
#[test]
fn a_foreground_change_sets_no_ordering_stamp() {
    let mut core = in_the_edit();
    assert_eq!(core.input(change_to(2, HELD, 500)), []);
    let effects = core.input(focus(2, in_front(HELD), button(2, "OK"), vec![], 300));
    assert_eq!(
        effects,
        [
            Heard::Expire {
                focus: 2,
                ancestors: vec![],
                foreground: None,
            },
            Heard::Stop,
            said("OK", Some(Role::Button)),
        ]
    );
    assert_eq!(core.focus(), Some(2));
}

/// A foreground change observed before Core's focus, reaching Core after
/// it, does not leave that focus: a repeat of the focus is a duplicate.
#[test]
fn a_foreground_change_observed_before_the_focus_leaves_it_in_front() {
    let mut core = Core::new();
    core.input(change_to(1, MAIN, 50));
    core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 500));
    assert_eq!(core.input(change_to(2, HELD, 400)), []);
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 600)),
        []
    );
}

/// A popup's focus, whose top-level window is not the foreground window,
/// leaves the entered window as it was; a foreground change of that window,
/// raised again, does not leave the popup's focus.
#[test]
fn a_foreground_change_of_the_entered_window_leaves_a_popup_focus_in_front() {
    let mut core = in_the_edit();
    let item = node(3, Role::ListItem, "Copy");
    let popup = facts(50, MAIN, MAIN);
    core.input(focus(1, popup.clone(), item.clone(), vec![], 200));
    assert_eq!(core.focus(), Some(3));
    assert_eq!(core.input(change_to(1, MAIN, 300)), []);
    assert_eq!(core.input(focus(1, popup, item, vec![], 400)), []);
}

/// A foreground change into a window M owns does not leave M's focus.
#[test]
fn a_foreground_change_into_an_owned_window_leaves_the_focus_in_front() {
    let mut core = in_the_edit();
    assert_eq!(core.input(change(1, (31, MAIN), 200)), []);
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 300)),
        []
    );
}

/// A focus left, the user having gone to H and back, is no duplicate when
/// it is reported again: it is spoken as a focus entering M from another
/// window.
#[test]
fn a_left_focus_reported_again_is_no_duplicate() {
    let mut core = in_the_edit();
    assert_eq!(core.input(change_to(2, HELD, 200)), []);
    assert_eq!(core.input(change_to(1, MAIN, 300)), []);
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 400)),
        the_edit_entered()
    );
}

/// A left focus reported again by a replacement outpost, its own having
/// ended, is spoken, not taken silently as the same focus.
#[test]
fn a_left_focus_from_a_replacement_outpost_is_spoken() {
    let mut core = in_the_edit();
    core.input(Input::OutpostEnded {
        trace_id: TraceId::from_raw(150),
        outpost: OutpostId(1),
    });
    core.input(change_to(2, HELD, 200));
    core.input(change_to(1, MAIN, 300));
    let again = focus(1, in_front(MAIN), edit(), vec![main_window()], 400);
    assert_eq!(core.input_from(OutpostId(11), again), the_edit_entered());
}

/// A whole-window focus of the window holding Core's focus, after that focus
/// was left, is taken and spoken.
#[test]
fn a_whole_window_focus_of_the_left_focus_window_is_taken() {
    let mut core = in_the_edit();
    core.input(change_to(2, HELD, 200));
    core.input(change_to(1, MAIN, 300));
    assert_eq!(
        core.input(whole_window(
            1,
            in_front(MAIN),
            client(7, "Notes", MAIN),
            400
        )),
        [
            Heard::Expire {
                focus: 7,
                ancestors: vec![],
                foreground: Some(7),
            },
            Heard::Stop,
            said("Notes", None),
        ]
    );
}

/// A foreground change observed before the newest report Core judged, a
/// duplicate of its focus that reached Core first, marks nothing: the
/// focus the user is back in stays a duplicate.
#[test]
fn a_foreground_change_observed_before_the_newest_judged_report_marks_nothing() {
    let mut core = in_the_edit();
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 350)),
        []
    );
    assert_eq!(core.input(change_to(2, HELD, 200)), []);
    assert_eq!(core.input(change_to(1, MAIN, 300)), []);
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 400)),
        []
    );
}

/// A splash window that takes the foreground and reports no focus leaves
/// Core's focus as any window does, so the return after it is spoken.
#[test]
fn the_return_after_a_uia_splash_window_is_spoken() {
    let mut core = in_the_edit();
    core.input(change_to(3, 60, 200));
    core.input(change_to(1, MAIN, 300));
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 400)),
        the_edit_entered()
    );
}

/// The switch to H and back, by whether H answered: with a stall, H's focus
/// never comes; without, Core takes H's focus before the return. Returns
/// the return's effects and Core's state after it.
fn return_to(
    (focused, chain): (&NodeSnapshot, &[NodeSnapshot]),
    stall: bool,
) -> (Vec<Heard>, SrState) {
    let mut core = Core::new();
    core.input(change_to(1, MAIN, 50));
    core.input(focus(
        1,
        in_front(MAIN),
        focused.clone(),
        chain.to_vec(),
        100,
    ));
    core.input(change_to(2, HELD, 200));
    if !stall {
        core.input(focus(
            2,
            in_front(HELD),
            button(2, "Busy"),
            vec![window_entry(9, "Busy app", HELD)],
            250,
        ));
    }
    core.input(change_to(1, MAIN, 300));
    let effects = core.input(focus(
        1,
        in_front(MAIN),
        focused.clone(),
        chain.to_vec(),
        400,
    ));
    (effects, core.state)
}

/// The return from an application that never answered says what the same
/// switch says when the application's focus was taken.
#[test]
fn a_return_after_a_foreground_change_says_what_the_switch_says_without_a_stall() {
    let chain = [main_window()];
    let (stalled, _) = return_to((&edit(), &chain), true);
    let (answered, _) = return_to((&edit(), &chain), false);
    assert_eq!(stalled, the_edit_entered());
    assert_eq!(stalled, answered);
}

/// Core's whole state after the return from an application that never
/// answered is the state the same switch leaves when the application's
/// focus was taken: for a button, and for a terminal, whose speech state
/// and model are keyed on its node.
#[test]
fn the_state_after_a_return_is_the_state_after_the_switch_without_a_stall() {
    let chain = [main_window()];
    let terminal = node(1, Role::Terminal, "Terminal");
    for focused in [edit(), terminal] {
        let (_, stalled) = return_to((&focused, &chain), true);
        let (_, answered) = return_to((&focused, &chain), false);
        assert_eq!(stalled, answered, "{:?}", focused.role);
    }
}

/// An event with no window facts from the application of Core's focus,
/// while that focus is in the attention window, is attended, as a Settings
/// page's content is hosted in `ApplicationFrameHost`'s frame, which holds
/// attention; one from a third application is not.
#[test]
fn an_event_with_no_window_facts_from_the_focus_hosted_application_is_attended() {
    const FRAME: u64 = 70;
    let mut core = Core::new();
    core.input(change_to(1, FRAME, 50));
    core.input(focus(2, in_front(FRAME), button(1, "Wi-Fi"), vec![], 100));
    let renamed = |pid: u32| Input::Event {
        trace_id: TraceId::from_raw(200),
        observed_at_us: 200,
        source: Pid(pid),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(1),
            change: PropertyChange::Name(Some("Wi-Fi off".to_owned())),
            child_count: None,
        },
    };
    assert_eq!(core.input(renamed(3)), []);
    assert_eq!(
        core.input(renamed(2)),
        [Heard::Say(
            SpeechPriority::Queued,
            vec![SegmentContent::Label("Wi-Fi off".to_owned())]
        )]
    );
}

// ---- The front ----

/// A held application's focus observed before the user returned to M and
/// released after the return is dropped: the front, M, was observed after it
/// and is unrelated to it, so the edit stays the focus.
#[test]
fn a_held_applications_late_focus_after_the_return_is_dropped() {
    let mut core = in_the_edit();
    core.input(change_to(2, HELD, 200));
    core.input(change_to(1, MAIN, 300));
    assert_eq!(
        core.input(focus(1, in_front(MAIN), edit(), vec![main_window()], 400)),
        the_edit_entered()
    );
    assert_eq!(
        core.input(focus(2, in_front(HELD), button(2, "Late"), vec![], 250)),
        []
    );
    assert_eq!(core.focus(), Some(1));
}

/// A focus entering the window in front makes its chain's outermost entry
/// the foreground object, as NVDA's `doPreGainFocus` moves its foreground
/// object from the focus's ancestry; a later focus in the same window
/// leaves it there.
#[test]
fn a_focus_entering_a_uia_window_moves_the_foreground_object() {
    let mut core = in_the_edit();
    let effects = core.input(focus(
        1,
        in_front(MAIN),
        button(2, "Save"),
        vec![main_window()],
        200,
    ));
    assert_eq!(
        effects,
        [
            Heard::Expire {
                focus: 2,
                ancestors: vec![5],
                foreground: Some(5),
            },
            said("Save", Some(Role::Button)),
        ]
    );
}

// ---- The whole-window focus ----

/// A whole-window focus reaching Core before the listener's change for the
/// same switch is taken, the front being the window before, observed
/// earlier; the change, observed before the focus, then leaves nothing, so
/// the same whole-window focus again is refused.
#[test]
fn a_whole_window_focus_arriving_before_its_foreground_change_is_taken() {
    let mut core = in_the_edit();
    let window = client(7, "Calculator", HELD);
    assert_eq!(
        core.input(whole_window(2, in_front(HELD), window.clone(), 300)),
        [
            Heard::Expire {
                focus: 7,
                ancestors: vec![],
                foreground: Some(7),
            },
            Heard::Stop,
            said("Calculator", None),
        ]
    );
    assert_eq!(core.input(change_to(2, HELD, 250)), []);
    assert_eq!(core.input(whole_window(2, in_front(HELD), window, 350)), []);
}

/// A whole-window focus observed before a switch to an unrelated window that
/// reached Core first is dropped.
#[test]
fn a_whole_window_focus_of_a_window_left_since_is_dropped() {
    let mut core = in_the_edit();
    core.input(change_to(3, 30, 400));
    assert_eq!(
        core.input(whole_window(
            2,
            in_front(HELD),
            client(7, "Calculator", HELD),
            300
        )),
        []
    );
    assert_eq!(core.focus(), Some(1));
}

/// A whole-window focus never moves attention: only the listener's
/// foreground change does.
#[test]
fn a_whole_window_focus_never_moves_attention() {
    let mut core = in_the_edit();
    core.input(whole_window(
        2,
        in_front(HELD),
        client(7, "Calculator", HELD),
        300,
    ));
    assert_eq!(core.focus(), Some(7));
    assert_eq!(core.state.attention(), Some(Pid(1)));
}

/// A whole-window focus of the window holding Core's focus, not left, is
/// refused.
#[test]
fn a_whole_window_focus_in_the_window_holding_the_focus_is_refused() {
    let mut core = in_the_edit();
    assert_eq!(
        core.input(whole_window(
            1,
            in_front(MAIN),
            client(7, "Notes", MAIN),
            200
        )),
        []
    );
    assert_eq!(core.focus(), Some(1));
}

/// An MSAA focus on a window's client object, with no focused child, reads
/// as the same node as a whole-window focus of the window, but is a focus:
/// it is taken in the window holding Core's focus, not refused.
#[test]
fn a_focus_on_a_windows_client_is_not_refused_as_a_whole_window_focus() {
    let mut core = in_the_edit();
    assert_eq!(
        core.input(focus(
            1,
            in_front(MAIN),
            client(7, "Notes", MAIN),
            vec![],
            200
        )),
        [
            Heard::Expire {
                focus: 7,
                ancestors: vec![],
                foreground: Some(5),
            },
            said("Notes", None),
        ]
    );
    assert_eq!(core.focus(), Some(7));
}

// ---- The title rule ----

/// A Windows Forms form M, the window of `form`, its client reached with no
/// identity string, and the text box in it.
fn form_box(form: u64, title: &str) -> (NodeSnapshot, Vec<NodeSnapshot>) {
    (
        node(form * 10 + 1, Role::EditableText, "Note"),
        vec![quiet_pane(form * 10, title, form)],
    )
}

/// The title the title rule says for a form's quiet client pane: its name
/// alone, a pane's role being silent on focus, as a whole-window focus of
/// the window's client says it.
fn title(name: &str) -> Heard {
    said(name, None)
}

/// A focus entering a form, its foreground change left untried in its
/// batch, says the form's title, which its line does not speak.
#[test]
fn a_forms_focus_with_its_foreground_change_left_untried_says_the_title() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 250)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            title("second-application"),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// A button b of form M has the focus; Enter opens dialog D, and b's read,
/// served by M's busy thread, is published after D is in front: Core takes
/// it with the entered window unchanged. D's foreground change and first
/// focus share a batch, the change left untried: D's title is said with its
/// focus.
#[test]
fn a_dialogs_title_is_said_with_its_focus_after_its_forms_late_focus() {
    let mut core = in_the_edit();
    let form = quiet_pane(400, "Form", 40);
    core.input(change_to(4, 40, 200));
    core.input(focus(
        4,
        in_front(40),
        button(401, "Open"),
        vec![form.clone()],
        210,
    ));
    let late = focus(
        4,
        facts(40, 40, 41),
        button(402, "Open again"),
        vec![form],
        300,
    );
    assert_eq!(
        core.input(late),
        [
            Heard::Expire {
                focus: 402,
                ancestors: vec![400],
                foreground: Some(400),
            },
            said("Open again", Some(Role::Button)),
        ]
    );
    core.input(change(4, (41, 40), 290));
    let dialog = quiet_pane(410, "Find", 41);
    assert_eq!(
        core.input(focus(
            4,
            facts(41, 40, 41),
            button(411, "OK"),
            vec![dialog],
            350
        )),
        [
            Heard::Expire {
                focus: 411,
                ancestors: vec![410],
                foreground: Some(410),
            },
            Heard::Stop,
            title("Find"),
            said("OK", Some(Role::Button)),
        ]
    );
}

/// A whole-window focus taken first enters its window: the focus after it
/// says no title, so the title is said once.
#[test]
fn a_whole_window_focus_taken_first_says_the_title_once() {
    let mut core = in_the_edit();
    let window = client(400, "second-application", 40);
    core.input(change_to(4, 40, 200));
    assert_eq!(
        core.input(whole_window(4, in_front(40), window.clone(), 210)),
        [
            Heard::Expire {
                focus: 400,
                ancestors: vec![],
                foreground: Some(400),
            },
            Heard::Stop,
            said("second-application", None),
        ]
    );
    assert_eq!(
        core.input(focus(
            4,
            in_front(40),
            node(401, Role::EditableText, "Note"),
            vec![window],
            220
        )),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The focus before the whole-window focus, in two batches: the title is
/// said with the focus, and the whole-window focus is then refused.
#[test]
fn a_focus_before_its_whole_window_focus_says_the_title_and_the_whole_window_focus_is_refused() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 210)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            title("second-application"),
            said("Note", Some(Role::EditableText)),
        ]
    );
    assert_eq!(
        core.input(whole_window(
            4,
            in_front(40),
            client(400, "second-application", 40),
            220
        )),
        []
    );
}

/// The context menu of a form's control, Shift+F10 then Escape: the menu is
/// a popup in the form's foreground, so the return says no title.
#[test]
fn a_context_menu_opened_and_closed_says_no_title_on_the_return() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), text_box.clone(), chain.clone(), 210));
    core.input(focus(
        4,
        facts(42, 40, 40),
        node(421, Role::MenuItem, "Cut"),
        vec![],
        220,
    ));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 230)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// A combo box's list opening, a popup owned by the form, and closing: no
/// title either way.
#[test]
fn a_combo_box_list_opening_and_closing_says_no_title() {
    let mut core = in_the_edit();
    let form = quiet_pane(400, "second-application", 40);
    let combo = node(401, Role::ComboBox, "Size");
    core.input(change_to(4, 40, 200));
    core.input(focus(
        4,
        in_front(40),
        combo.clone(),
        vec![form.clone()],
        210,
    ));
    assert_eq!(
        core.input(focus(
            4,
            facts(43, 40, 40),
            node(431, Role::ListItem, "Large"),
            vec![],
            220
        )),
        [
            Heard::Expire {
                focus: 431,
                ancestors: vec![],
                foreground: Some(400),
            },
            Heard::Stop,
            said("Large", None),
        ]
    );
    assert_eq!(
        core.input(focus(4, in_front(40), combo, vec![form], 230)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
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
    let mut core = in_the_edit();
    let form = quiet_pane(400, "second-application", 40);
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), button(401, "Pick"), vec![form], 210));
    let drop_down = window_entry(440, "Choices", 44);
    assert_eq!(
        core.input(focus(
            4,
            facts(44, 40, 40),
            node(441, Role::ListItem, "First"),
            vec![drop_down],
            220
        )),
        [
            Heard::Expire {
                focus: 441,
                ancestors: vec![440],
                foreground: Some(400),
            },
            Heard::Stop,
            said("Choices", Some(Role::Window)),
            said("First", None),
        ]
    );
}

/// An owned dialog opening and closing: its title, then the main window's.
#[test]
fn an_owned_dialog_opening_and_closing_says_its_title_then_the_main_windows() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), text_box.clone(), chain.clone(), 210));
    core.input(change(4, (41, 40), 220));
    assert_eq!(
        core.input(focus(
            4,
            facts(41, 40, 41),
            button(411, "OK"),
            vec![quiet_pane(410, "Find", 41)],
            230
        )),
        [
            Heard::Expire {
                focus: 411,
                ancestors: vec![410],
                foreground: Some(410),
            },
            Heard::Stop,
            title("Find"),
            said("OK", Some(Role::Button)),
        ]
    );
    core.input(change_to(4, 40, 240));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 250)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            title("second-application"),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The return from Verbatim's menu: the menu's foreground change, to
/// Verbatim's own window, is the keyboard hook's alone; the form's
/// whole-window focus on the return is taken, Core's focus being the menu's
/// item, and says the title, once.
#[test]
fn a_return_from_verbatims_menu_says_the_title() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), text_box.clone(), chain.clone(), 210));
    let menu = Input::Foreground(ForegroundWindow {
        own: true,
        ..match change_to(9, 90, 220) {
            Input::Foreground(change) => change,
            _ => unreachable!("a foreground change"),
        }
    });
    assert_eq!(core.input(menu), []);
    assert_eq!(
        core.state.attention(),
        Some(Pid(4)),
        "the menu's is the hook's"
    );
    let frame = node(900, Role::Window, "Verbatim");
    core.input(focus(
        9,
        facts(91, 91, 90),
        node(911, Role::MenuItem, "Settings..."),
        vec![frame],
        230,
    ));
    core.input(change_to(4, 40, 240));
    assert_eq!(
        core.input(whole_window(
            4,
            in_front(40),
            client(400, "second-application", 40),
            250
        )),
        [
            Heard::Expire {
                focus: 400,
                ancestors: vec![],
                foreground: Some(400),
            },
            Heard::Stop,
            said("second-application", None),
        ]
    );
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 260)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The return from Verbatim's menu whose only report is the form's focus,
/// its whole-window focus left untried in its batch: the menu took the user
/// out of the entered window, so the focus says the title.
#[test]
fn a_return_from_verbatims_menu_with_its_focus_alone_says_the_title() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), text_box.clone(), chain.clone(), 210));
    let menu = Input::Foreground(ForegroundWindow {
        own: true,
        ..match change_to(9, 90, 220) {
            Input::Foreground(change) => change,
            _ => unreachable!("a foreground change"),
        }
    });
    core.input(menu);
    core.input(focus(
        9,
        facts(91, 91, 90),
        node(911, Role::MenuItem, "Settings..."),
        vec![node(900, Role::Window, "Verbatim")],
        230,
    ));
    core.input(change_to(4, 40, 240));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 250)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            title("second-application"),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// A focus on the desktop, in Program Manager's window in front, says no
/// title: NVDA takes no foreground event for Program Manager or the taskbar.
#[test]
fn a_focus_in_program_manager_says_no_title() {
    let mut core = in_the_edit();
    core.input(change_to(7, 70, 200));
    let desktop = WindowFacts {
        foreground_is_shell: true,
        ..in_front(70)
    };
    let mut list = node(710, Role::List, "Desktop");
    list.backend = Backend::Msaa;
    let mut item = node(711, Role::ListItem, "NVDA");
    item.backend = Backend::Msaa;
    assert_eq!(
        core.input(focus(
            7,
            desktop,
            item,
            vec![quiet_pane(700, "Program Manager", 70), list],
            210
        )),
        [
            Heard::Expire {
                focus: 711,
                ancestors: vec![700, 710],
                foreground: Some(700),
            },
            Heard::Stop,
            said("Desktop", Some(Role::List)),
            said("NVDA", None),
        ]
    );
}

/// A nameless foreground window's entry, and a chain with no entry for the
/// window, add nothing.
#[test]
fn a_nameless_window_and_a_chain_without_the_window_add_no_title() {
    let mut core = in_the_edit();
    core.input(change_to(4, 40, 200));
    let mut nameless = quiet_pane(400, "", 40);
    nameless.name = None;
    assert_eq!(
        core.input(focus(
            4,
            in_front(40),
            button(401, "One"),
            vec![nameless],
            210
        )),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            said("One", Some(Role::Button)),
        ]
    );
    core.input(change_to(5, 50, 300));
    assert_eq!(
        core.input(focus(5, in_front(50), button(501, "Two"), vec![], 310)),
        [
            Heard::Expire {
                focus: 501,
                ancestors: vec![],
                foreground: None,
            },
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
    entering.input(change_to(4, 40, 200));
    let pane = client(400, "second-application", 40);
    let mut quiet = pane.clone();
    quiet.details.top_level_client = false;
    let titled = entering.input(focus(4, in_front(40), button(401, "One"), vec![quiet], 210));
    let mut whole = in_the_edit();
    whole.input(change_to(4, 40, 200));
    let mut quiet_whole = pane;
    quiet_whole.details.top_level_client = false;
    let window = whole.input(whole_window(4, in_front(40), quiet_whole, 210));
    assert_eq!(titled.get(2), window.get(2));
}

/// A foreground change into another window, then a focus in the window
/// left: its title is said, Core's focus having been left.
#[test]
fn a_focus_in_the_window_left_after_a_foreground_change_says_the_title() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(change_to(4, 40, 200));
    core.input(focus(4, in_front(40), text_box.clone(), chain.clone(), 210));
    core.input(change_to(2, HELD, 300));
    core.input(change_to(4, 40, 400));
    assert_eq!(
        core.input(focus(4, in_front(40), text_box, chain, 410)),
        [
            Heard::Expire {
                focus: 401,
                ancestors: vec![400],
                foreground: Some(400),
            },
            Heard::Stop,
            title("second-application"),
            said("Note", Some(Role::EditableText)),
        ]
    );
}

/// The title rule never moves attention.
#[test]
fn the_title_rule_leaves_attention() {
    let mut core = in_the_edit();
    let (text_box, chain) = form_box(40, "second-application");
    core.input(focus(4, in_front(40), text_box, chain, 210));
    assert_eq!(core.state.attention(), Some(Pid(1)));
}

// ---- The held UIA application's title ----

/// A UIA focus after its window's text, the whole-window focus of the
/// plain window the held application's title was read into, does not say
/// the window again, though its chain's window element is another node:
/// the two lines meet at the window, one window two readers read being one
/// entry (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_uia_focus_after_its_windows_text_does_not_say_the_window_again() {
    let mut core = in_the_edit();
    core.input(change_to(2, HELD, 200));
    let mut plain = node(8, Role::Window, "held-application");
    plain.backend = Backend::Msaa;
    plain.details.window = Some(WindowHandle(HELD));
    assert_eq!(
        core.input(whole_window(2, in_front(HELD), plain, 1_200)),
        [
            Heard::Expire {
                focus: 8,
                ancestors: vec![],
                foreground: Some(8),
            },
            Heard::Stop,
            said("held-application", Some(Role::Window)),
        ]
    );
    assert_eq!(
        core.input(focus(
            2,
            in_front(HELD),
            button(3, "OK"),
            vec![window_entry(9, "held-application", HELD)],
            1_300
        )),
        [
            Heard::Expire {
                focus: 3,
                ancestors: vec![8],
                foreground: Some(8),
            },
            said("OK", Some(Role::Button)),
        ]
    );
}
