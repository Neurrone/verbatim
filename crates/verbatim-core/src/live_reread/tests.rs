//! Core's check before speaking and its live re-reads, as reducer tests
//! asserting every effect (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The ancestry cache"): a report that would speak a cached ancestor, as
//! entered or as the title, is set aside with nothing written but its focus
//! group's rank, its focus reader asked for a live re-read, which is
//! counted and logged, and its answer executed in its turn, with the
//! reports queued behind it; each other ending of a waiting report says
//! nothing of it.

use verbatim_model::{
    Backend, Effect, FirstCachedAncestor, FocusGroup, FocusReaderKind, ForegroundWindow, Input,
    LiveReread, LiveRereadLog, LiveRereadReason, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Pid, Role, SegmentContent, SpeechPriority, StateSet, TraceId,
    WindowFacts, WindowHandle,
};

use crate::SrState;
use crate::focus_rules::first_cached_ancestor;

/// The window of application A, and its outpost generation.
const A: u64 = 10;
/// The window of application X, the one the user comes from.
const X: u64 = 30;

/// One effect, as these tests compare it.
#[derive(Debug, PartialEq)]
enum Heard {
    Expire(u64),
    Stop,
    Say(Vec<SegmentContent>),
    Reread(LiveReread),
    /// Effects carried under a key's trace.
    ForKey(u64, Vec<Heard>),
    Other(String),
}

fn heard(effects: &[Effect]) -> Vec<Heard> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::DropExpiredSpeech(now) => Heard::Expire(now.focus.id.number()),
            Effect::StopSpeech => Heard::Stop,
            Effect::Speak(utterance, _) => {
                assert_eq!(utterance.priority, SpeechPriority::Queued);
                Heard::Say(
                    utterance
                        .segments
                        .iter()
                        .map(|segment| segment.content.clone())
                        .collect(),
                )
            }
            Effect::LiveReread(reread) => Heard::Reread(reread.clone()),
            Effect::ForKey(trace, inner) => Heard::ForKey(
                trace.to_string().parse().expect("a numeric trace"),
                heard(std::slice::from_ref(inner)),
            ),
            other => Heard::Other(format!("{other:?}")),
        })
        .collect()
}

fn said(name: &str, role: Role) -> Heard {
    Heard::Say(vec![
        SegmentContent::Label(name.to_owned()),
        SegmentContent::Role(role),
    ])
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

fn window(id: u64, name: &str, handle: u64) -> NodeSnapshot {
    let mut window = node(id, Role::Window, name);
    window.details.window = Some(WindowHandle(handle));
    window
}

fn pane(id: u64, name: &str) -> NodeSnapshot {
    node(id, Role::Group, name)
}

fn button(id: u64, name: &str) -> NodeSnapshot {
    node(id, Role::Button, name)
}

fn facts(top: u64, foreground: u64) -> WindowFacts {
    WindowFacts {
        top_level: WindowHandle(top),
        root_owner: WindowHandle(top),
        topmost: false,
        under_active_window: None,
        in_foreground: foreground == top,
        foreground_window: WindowHandle(foreground),
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
    }
}

/// A report from the outpost of the window `top`'s application.
struct Report {
    top: u64,
    foreground: u64,
    node: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
    at_us: u64,
    cached: Option<(u32, FocusReaderKind)>,
    answers: Option<u64>,
    group: Option<FocusGroup>,
}

impl Report {
    fn new(top: u64, node: NodeSnapshot, ancestors: Vec<NodeSnapshot>, at_us: u64) -> Self {
        Self {
            top,
            foreground: top,
            node,
            ancestors,
            at_us,
            cached: None,
            answers: None,
            group: None,
        }
    }

    fn cached(mut self, index: u32) -> Self {
        self.cached = Some((index, FocusReaderKind::Listener));
        self
    }

    fn input(self) -> Input {
        let mut event = NormalizedEvent::FocusChanged {
            node: self.node,
            whole_window: false,
            ancestors: self.ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: self.group,
            first_cached_ancestor: self
                .cached
                .map(|(index, reader)| FirstCachedAncestor { index, reader }),
            answers: self.answers,
        };
        event.assign_outpost(OutpostId(self.top));
        Input::Event {
            trace_id: TraceId::from_raw(self.at_us),
            observed_at_us: self.at_us,
            source: Pid(u32::try_from(self.top).unwrap()),
            backend: Backend::Uia,
            window: Some(facts(self.top, self.foreground)),
            event,
            purpose: None,
        }
    }
}

fn change(top: u64, at_us: u64) -> Input {
    Input::Foreground(ForegroundWindow {
        window: WindowHandle(top),
        root_owner: WindowHandle(top),
        process: Pid(u32::try_from(top).unwrap()),
        observed_at_us: at_us,
        own: false,
        thread: 0,
        console: false,
        integrity: None,
        own_integrity: None,
    })
}

fn id(outpost: u64, number: u64) -> NodeId {
    NodeId::new(number).with_outpost(OutpostId(outpost))
}

/// A's chain: its window W, a group P, and focus `focus`.
fn chain_of_a() -> Vec<NodeSnapshot> {
    vec![window(1, "Notes", A), pane(2, "Tools")]
}

/// Core with the user in A, on F1 inside W and P.
fn in_a() -> SrState {
    let mut state = SrState::new();
    let _ = crate::reduce(&mut state, &change(A, 50));
    let _ = crate::reduce(
        &mut state,
        &Report::new(A, button(3, "One"), chain_of_a(), 100).input(),
    );
    state
}

/// Core with the user in X, on F0, after leaving A.
fn in_x() -> SrState {
    let mut state = in_a();
    let _ = crate::reduce(&mut state, &change(X, 200));
    let _ = crate::reduce(
        &mut state,
        &Report::new(X, button(9, "Other"), vec![window(8, "Elsewhere", X)], 300).input(),
    );
    state
}

fn reread(
    request: u64,
    (focus, at_us): (u64, u64),
    (join, join_name): (NodeId, Option<&str>),
    lowest: Option<u64>,
    reason: LiveRereadReason,
) -> Heard {
    Heard::Reread(LiveReread {
        request,
        outpost: OutpostId(A),
        reader: FocusReaderKind::Listener,
        trace_id: TraceId::from_raw(at_us),
        window: Some(WindowHandle(A)),
        logged: LiveRereadLog {
            focus: id(A, focus),
            cache_join: (join, join_name.map(str::to_owned)),
            lowest_common_ancestor: lowest.map(|number| id(A, number)),
            reason,
        },
    })
}

/// The return to A's F2 inside W and P, every ancestor cached.
fn return_to_two() -> Report {
    Report::new(A, button(4, "Two"), chain_of_a(), 400).cached(1)
}

/// `after` with the fields a report set aside may write put back as they
/// were in `before`: its group rank, the live re-reads' count, Core's
/// focus, now waiting, and the ordering stamp; everything else must be
/// unchanged.
fn but_the_set_aside(before: &SrState, after: &SrState) -> SrState {
    let mut after = after.clone();
    after.group_taken = before.group_taken;
    after.live_rereads = before.live_rereads.clone();
    after.focus = before.focus.clone();
    after.foreground = before.foreground.clone();
    after
}

#[test]
fn cached_ancestors_above_the_lowest_common_ancestor_are_not_spoken() {
    let mut state = in_a();
    let report = Report::new(A, button(4, "Two"), chain_of_a(), 400).cached(1);
    assert_eq!(
        heard(&crate::reduce(&mut state, &report.input())),
        [Heard::Expire(4), said("Two", Role::Button)]
    );
    assert_eq!(state.live_re_reads(), 0);
}

#[test]
fn a_cached_ancestor_below_the_lowest_common_ancestor_needs_a_live_re_read() {
    let mut state = in_x();
    let before = state.clone();
    let report = Report::new(A, button(4, "Two"), chain_of_a(), 400).cached(0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &report.input())),
        [reread(
            0,
            (4, 400),
            (id(A, 2), Some("Tools")),
            None,
            LiveRereadReason::Entered
        )]
    );
    assert_eq!(state.live_re_reads(), 1);
    assert_eq!(state.focus.object(), before.focus.object());
    assert_eq!(but_the_set_aside(&before, &state), before);
}

#[test]
fn a_cached_title_ancestor_needs_a_live_re_read() {
    // The title rule's entry, W at index 0, is cached, and nothing cached
    // is entered: the report is set aside for the title. Since the title
    // rule says a title only when the ancestries share no object, when
    // every ancestor is entered, the reducer meets this case only as an
    // entered one, which is checked first.
    assert_eq!(
        crate::focus_rules::needs_live_ancestry(Some(1), 2, Some(0)),
        Some(LiveRereadReason::Title)
    );
    assert_eq!(
        crate::focus_rules::needs_live_ancestry(Some(1), 0, Some(0)),
        Some(LiveRereadReason::Entered)
    );
    assert_eq!(
        crate::focus_rules::needs_live_ancestry(Some(1), 2, None),
        None
    );
    assert_eq!(
        crate::focus_rules::needs_live_ancestry(None, 0, Some(0)),
        None
    );
    // Through the reducer, the title entry W met at P says no title, and
    // no live re-read is asked for.
    let mut state = SrState::new();
    let _ = crate::reduce(&mut state, &change(X, 50));
    let mut first = Report::new(A, button(3, "One"), chain_of_a(), 100);
    first.foreground = X;
    let _ = crate::reduce(&mut state, &first.input());
    assert_eq!(
        heard(&crate::reduce(&mut state, &return_to_two().input())),
        [Heard::Expire(4), said("Two", Role::Button)]
    );
    assert_eq!(state.live_re_reads(), 0);
}

#[test]
fn a_filtered_cached_ancestor_rounds_first_cached_ancestor_outward() {
    // The reader took W and P from its cache, and read Q live below them;
    // its rules left P out, so the innermost cached entry is W.
    let cached = [id(A, 1), id(A, 2)];
    let emitted = [id(A, 1), id(A, 5)];
    assert_eq!(
        first_cached_ancestor(&emitted, |node| cached.contains(&node)),
        Some(0)
    );
    let unfiltered = [id(A, 1), id(A, 2), id(A, 5)];
    assert_eq!(
        first_cached_ancestor(&unfiltered, |node| cached.contains(&node)),
        Some(1)
    );
    assert_eq!(
        first_cached_ancestor(&[id(A, 5)], |node| cached.contains(&node)),
        None
    );
}

#[test]
fn drops_ancestry_voids_first_cached_ancestor() {
    // The task switcher's item with Alt held: its chain is dropped, so its
    // cached mark names nothing, and it is taken as today.
    let mut state = in_x();
    let mut item = node(4, Role::ListItem, "Notes");
    item.details.parent_automation_id = Some("SwitchItemListControl".to_owned());
    let mut report = Report::new(A, item, chain_of_a(), 400).cached(1);
    report.top = A;
    let mut input = report.input();
    if let Input::Event {
        window: Some(window),
        ..
    } = &mut input
    {
        window.application = "explorer".to_owned();
        window.alt_held = true;
    }
    assert_eq!(
        heard(&crate::reduce(&mut state, &input)),
        [
            Heard::Expire(4),
            Heard::Stop,
            Heard::Say(vec![SegmentContent::Label("Notes".to_owned())])
        ]
    );
    assert_eq!(state.live_re_reads(), 0);
}

#[test]
fn a_report_needing_a_live_re_read_writes_its_queue_entry_stamp_and_group_rank() {
    let mut state = in_x();
    let before = state.clone();
    let mut report = return_to_two();
    report.group = Some(FocusGroup { number: 7, rank: 2 });
    let _ = crate::reduce(&mut state, &report.input());
    assert_eq!(
        state.group_taken,
        Some((OutpostId(A), FocusGroup { number: 7, rank: 2 }))
    );
    assert_eq!(state.focus.rereading(), 1);
    assert_eq!(but_the_set_aside(&before, &state), before);
}

#[test]
fn a_live_re_read_ends_its_focus_group_for_older_entries() {
    let mut state = in_x();
    let mut report = return_to_two();
    report.group = Some(FocusGroup { number: 7, rank: 1 });
    let _ = crate::reduce(&mut state, &report.input());
    let before = state.clone();
    let mut older = Report::new(A, button(5, "Older"), chain_of_a(), 390);
    older.group = Some(FocusGroup { number: 7, rank: 2 });
    assert_eq!(heard(&crate::reduce(&mut state, &older.input())), []);
    assert_eq!(state, before);
}

#[test]
fn keep_newer_reading_skips_cached_ancestors() {
    // A repeat of F1 whose cached W was read under another name, and whose
    // live P was renamed: Core keeps its own W and takes the new P.
    let mut state = in_a();
    let repeat = Report::new(
        A,
        button(3, "One"),
        vec![window(1, "Old notes", A), pane(2, "Tools renamed")],
        400,
    )
    .cached(0);
    assert_eq!(heard(&crate::reduce(&mut state, &repeat.input())), []);
    let ancestors = &state.focus.object().unwrap().ancestors;
    assert_eq!(ancestors[0].name.as_deref(), Some("Notes"));
    assert_eq!(ancestors[1].name.as_deref(), Some("Tools renamed"));
}

#[test]
fn a_live_re_read_answer_never_needs_another() {
    let mut state = in_x();
    let _ = crate::reduce(&mut state, &return_to_two().input());
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400).cached(0);
    answer.answers = Some(0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        [
            Heard::Expire(4),
            Heard::Stop,
            said("Notes", Role::Window),
            said("Tools", Role::Group),
            said("Two", Role::Button),
        ]
    );
    assert_eq!(state.focus.rereading(), 0);
    assert_eq!(state.live_re_reads(), 1);
}

#[test]
fn a_newer_focus_waits_behind_a_pending_live_re_read() {
    // X's report of F7, read live, is queued behind F2's, and is executed
    // after F2's answer, against F2, as NVDA executes its queue in order.
    let mut state = in_x();
    let _ = crate::reduce(&mut state, &return_to_two().input());
    let newer = Report::new(X, button(7, "Newer"), vec![window(8, "Elsewhere", X)], 500);
    assert_eq!(heard(&crate::reduce(&mut state, &newer.input())), []);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    let mut expected = Vec::from(two_spoken());
    expected.extend([
        Heard::Expire(7),
        Heard::Stop,
        said("Elsewhere", Role::Window),
        said("Newer", Role::Button),
    ]);
    assert_eq!(heard(&crate::reduce(&mut state, &answer.input())), expected);
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_second_live_re_read_waits_behind_the_first() {
    // F2's report waits, then X's report of F7 waits behind it. F2's
    // answer is spoken, and F7, planned then against F2's line, which it
    // does not meet, asks its own live re-read with it; its answer is
    // spoken.
    let mut state = pending();
    let second = Report::new(X, button(7, "Newer"), vec![window(8, "Elsewhere", X)], 450).cached(0);
    assert_eq!(heard(&crate::reduce(&mut state, &second.input())), []);
    let mut first = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    first.answers = Some(0);
    let mut expected = Vec::from(two_spoken());
    expected.push(Heard::Reread(LiveReread {
        request: 1,
        outpost: OutpostId(X),
        reader: FocusReaderKind::Listener,
        trace_id: TraceId::from_raw(450),
        window: Some(WindowHandle(X)),
        logged: LiveRereadLog {
            focus: id(X, 7),
            cache_join: (id(X, 7), Some("Newer".to_owned())),
            lowest_common_ancestor: None,
            reason: LiveRereadReason::Entered,
        },
    }));
    assert_eq!(heard(&crate::reduce(&mut state, &first.input())), expected);
    assert_eq!(state.focus.rereading(), 1);
    let mut answer = Report::new(X, button(7, "Newer"), vec![window(8, "Elsewhere", X)], 450);
    answer.answers = Some(1);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        [
            Heard::Expire(7),
            Heard::Stop,
            said("Elsewhere", Role::Window),
            said("Newer", Role::Button),
        ]
    );
    assert_eq!(state.focus.rereading(), 0);
}

/// Core with a live re-read of F2 pending, and the state before its ending.
fn pending() -> SrState {
    let mut state = in_x();
    let _ = crate::reduce(&mut state, &return_to_two().input());
    state
}

#[test]
fn a_gone_live_re_read_says_nothing() {
    let mut state = pending();
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(heard(&crate::reduce(&mut state, &gone)), []);
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_live_re_read_ends_at_its_outpost_generations_end() {
    let mut state = pending();
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(1),
        outpost: OutpostId(A),
    };
    assert_eq!(heard(&crate::reduce(&mut state, &ended)), []);
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_live_re_read_ends_when_the_listener_is_replaced() {
    let mut state = pending();
    assert_eq!(
        heard(&crate::reduce(&mut state, &Input::ListenerReplaced)),
        []
    );
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_pending_live_re_read_counts_for_the_barrier() {
    let mut state = pending();
    assert_eq!(state.outstanding_queries(), 1);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    let _ = crate::reduce(&mut state, &gone);
    assert_eq!(state.outstanding_queries(), 0);
}

#[test]
fn a_menu_end_while_a_report_waits_asks_the_fake_focus_when_it_ends_unanswered() {
    // The end waits behind F2; F2 answered gone leaves no focus taken since
    // the end, so the fake focus is asked then, in its turn.
    let mut state = pending();
    let ended = Input::MenuOrSwitchEnded { ended_at_us: 350 };
    assert_eq!(heard(&crate::reduce(&mut state, &ended)), []);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(
        heard(&crate::reduce(&mut state, &gone)),
        [Heard::Other("ReadFocus(Fake)".to_owned())]
    );
}

#[test]
fn a_menu_end_while_a_report_waits_is_satisfied_by_its_answer() {
    // F2, observed after the end, is taken before the end is judged: no
    // fake focus.
    let mut state = pending();
    let ended = Input::MenuOrSwitchEnded { ended_at_us: 350 };
    assert_eq!(heard(&crate::reduce(&mut state, &ended)), []);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        two_spoken()
    );
}

/// What the move to the focus says of a button named `name`.
fn moved_to(name: &str) -> Heard {
    Heard::Say(vec![
        SegmentContent::Message(verbatim_model::Message::MoveToFocus),
        SegmentContent::Label(name.to_owned()),
        SegmentContent::Role(Role::Button),
    ])
}

/// Verbatim's move to the focus, pressed with trace `trace`.
fn to_focus(trace: u64) -> Input {
    Input::Command {
        trace_id: TraceId::from_raw(trace),
        key_seq: verbatim_model::KeySeq::none_yet(),
        command: verbatim_model::ReviewCommand::ToFocus,
        pressed_at_ms: 0,
        repeat: 0,
    }
}

#[test]
fn a_command_while_a_report_waits_acts_on_the_focus_executed_before_it() {
    // Pressed while F2 waits, the move to the focus runs after F2's answer,
    // on F2, under its own trace, as NVDA queues gestures with events.
    let mut state = pending();
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    let mut expected = Vec::from(two_spoken());
    expected.push(Heard::ForKey(450, vec![moved_to("Two")]));
    assert_eq!(heard(&crate::reduce(&mut state, &answer.input())), expected);
}

#[test]
fn a_command_with_nothing_waiting_runs_at_once() {
    let mut state = in_x();
    assert_eq!(
        heard(&crate::reduce(&mut state, &to_focus(450))),
        [moved_to("Other")]
    );
}

/// `input`, a focus report, carrying `earlier`, what its reader read of
/// the element it held before under its node.
fn with_earlier(mut input: Input, earlier: verbatim_model::EarlierElement) -> Input {
    if let Input::Event {
        event: NormalizedEvent::FocusChanged { earlier: held, .. },
        ..
    } = &mut input
    {
        *held = Some(earlier);
    }
    input
}

#[test]
fn a_repeat_whose_earlier_element_still_has_the_focus_is_a_duplicate() {
    let mut state = in_a();
    let repeat = Report::new(A, button(3, "One"), chain_of_a(), 400).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &with_earlier(repeat, verbatim_model::EarlierElement::Focused)
        )),
        []
    );
}

#[test]
fn a_repeat_whose_earlier_element_lost_the_focus_is_a_new_focus() {
    // The element its reader held under the node lost the focus: the same
    // object, so the lines meet at it, and only the focus is said.
    let mut state = in_a();
    let repeat = Report::new(A, button(3, "One"), chain_of_a(), 400).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &with_earlier(repeat, verbatim_model::EarlierElement::NotFocused)
        )),
        [Heard::Expire(3), said("One", Role::Button)]
    );
}

/// `input`, a focus report, as the answer to a focus query for `purpose`.
fn for_purpose(mut input: Input, for_purpose: verbatim_model::protocol::FocusPurpose) -> Input {
    if let Input::Event { purpose, .. } = &mut input {
        *purpose = Some(for_purpose);
    }
    input
}

#[test]
fn a_plain_focus_answer_is_judged_by_the_duplicate_rule() {
    let mut state = in_a();
    let answer = Report::new(A, button(3, "One"), chain_of_a(), 400).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(answer, verbatim_model::protocol::FocusPurpose::Plain)
        )),
        []
    );
}

#[test]
fn a_fake_focus_answer_read_through_ui_automation_waits_and_is_taken_in_its_turn() {
    // NVDA judges a fake focus for duplicates only for MSAA objects: the
    // UIA answer of F1, behind X's set-aside report, is taken after that
    // report ends unanswered, executed against the focus spoken before it,
    // F1, so the lines meet at F1.
    let mut state = set_aside_in_x();
    let answer = Report::new(A, button(3, "One"), chain_of_a(), 400).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(answer, verbatim_model::protocol::FocusPurpose::Fake)
        )),
        []
    );
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(X),
        request: 0,
    };
    assert_eq!(
        heard(&crate::reduce(&mut state, &gone)),
        [Heard::Expire(3), said("One", Role::Button)]
    );
}

#[test]
fn a_report_focus_answer_is_not_taken_into_the_queue() {
    // Verbatim+Tab's answer is only spoken, for a press; with none it says
    // nothing and leaves the queue as it was.
    let mut state = pending();
    let before = state.clone();
    let answer = Report::new(A, button(4, "Two"), chain_of_a(), 410).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(answer, verbatim_model::protocol::FocusPurpose::Report)
        )),
        []
    );
    assert_eq!(state.focus, before.focus);
}

#[test]
fn a_report_with_no_observation_time_takes_the_changes_before_it() {
    // A change that reached Core before a report of unknown time applies to
    // it: F1 again after the user went to X is spoken as the return.
    let mut state = in_a();
    let _ = crate::reduce(&mut state, &change(X, 200));
    let again = Report::new(A, button(3, "One"), chain_of_a(), 0).input();
    assert_eq!(
        heard(&crate::reduce(&mut state, &again)),
        [
            Heard::Expire(3),
            Heard::Stop,
            said("Notes", Role::Window),
            said("Tools", Role::Group),
            said("One", Role::Button)
        ]
    );
}

#[test]
fn a_focus_with_no_window_facts_is_never_left() {
    // F2 reported with no window facts: a change to X does not leave it,
    // so its repeat is a duplicate.
    let mut state = in_a();
    let mut two = Report::new(A, button(4, "Two"), chain_of_a(), 400).input();
    if let Input::Event { window, .. } = &mut two {
        *window = None;
    }
    let _ = crate::reduce(&mut state, &two);
    let _ = crate::reduce(&mut state, &change(X, 500));
    let mut again = Report::new(A, button(4, "Two"), chain_of_a(), 600).input();
    if let Input::Event { window, .. } = &mut again {
        *window = None;
    }
    assert_eq!(heard(&crate::reduce(&mut state, &again)), []);
}

#[test]
fn each_live_re_read_is_counted_and_logged() {
    let mut state = in_x();
    let mut logged = Vec::new();
    for (request, focus, at_us) in [(0, 4, 400), (1, 5, 410), (2, 6, 420)] {
        let report = Report::new(A, button(focus, "Two"), chain_of_a(), at_us).cached(1);
        let effects = heard(&crate::reduce(&mut state, &report.input()));
        // Each answered gone, so the next is planned against F0 again.
        let gone = Input::LiveRereadGone {
            outpost: OutpostId(A),
            request,
        };
        assert_eq!(heard(&crate::reduce(&mut state, &gone)), []);
        assert_eq!(
            effects,
            [reread(
                request,
                (focus, at_us),
                (id(A, focus), Some("Two")),
                None,
                LiveRereadReason::Entered
            )]
        );
        logged.extend(effects);
    }
    assert_eq!(state.live_re_reads(), 3);
    assert_eq!(logged.len(), 3);
}

/// The return to F2 spoken once, its whole entered ancestry with it.
fn two_spoken() -> [Heard; 5] {
    [
        Heard::Expire(4),
        Heard::Stop,
        said("Notes", Role::Window),
        said("Tools", Role::Group),
        said("Two", Role::Button),
    ]
}

#[test]
fn a_cached_repeat_while_a_live_re_read_waits_asks_a_new_one() {
    // The application raised two focus events for F2, both read from the
    // reader's cache: the repeat waits behind the first, and is decided when
    // the first request's answer of gone ends that wait: no duplicate of a
    // report that was still waiting when it came, it asks again under its
    // own trace, the read its reader holds last.
    let mut state = pending();
    let repeat = Report::new(A, button(4, "Two"), chain_of_a(), 410).cached(1);
    assert_eq!(heard(&crate::reduce(&mut state, &repeat.input())), []);
    assert_eq!(state.live_re_reads(), 1);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(
        heard(&crate::reduce(&mut state, &gone)),
        [reread(
            1,
            (4, 410),
            (id(A, 4), Some("Two")),
            None,
            LiveRereadReason::Entered
        )]
    );
    assert_eq!(state.live_re_reads(), 2);
    assert_eq!(state.focus.rereading(), 1);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 410);
    answer.answers = Some(1);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        two_spoken()
    );
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_live_repeat_while_a_live_re_read_waits_is_spoken_once_the_first_ends() {
    // The repeat was read live: it is no duplicate of the waiting report,
    // and waits behind it. Its reader answers the first gone, having read
    // again since, and the repeat is spoken.
    let mut state = pending();
    let repeat = Report::new(A, button(4, "Two"), chain_of_a(), 410);
    assert_eq!(heard(&crate::reduce(&mut state, &repeat.input())), []);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(heard(&crate::reduce(&mut state, &gone)), two_spoken());
    assert_eq!(state.focus.rereading(), 0);
}

/// The whole-window focus of the window `top`, `node`, observed at
/// `at_us`.
fn whole_window(top: u64, node: NodeSnapshot, at_us: u64) -> Input {
    let mut input = Report::new(top, node, Vec::new(), at_us).input();
    if let Input::Event {
        event: NormalizedEvent::FocusChanged { whole_window, .. },
        ..
    } = &mut input
    {
        *whole_window = true;
    }
    input
}

/// Core in A on F1, with X's report of F0 set aside for a live re-read,
/// request 0: NVDA's last queued focus is F0, its focus object F1.
fn set_aside_in_x() -> SrState {
    let mut state = in_a();
    let report = Report::new(X, button(9, "Other"), vec![window(8, "Elsewhere", X)], 300).cached(0);
    let effects = crate::reduce(&mut state, &report.input());
    assert!(matches!(heard(&effects).as_slice(), [Heard::Reread(_)]));
    state
}

#[test]
fn a_whole_window_focus_of_the_window_a_set_aside_report_is_in_is_refused() {
    // The foreground event waits behind F2; once F2's answer is executed,
    // NVDA drops it, as the last focus it queued, F2, is in that window, and
    // the answer says the whole return.
    let mut state = pending();
    let _ = crate::reduce(&mut state, &change(A, 410));
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &whole_window(A, window(1, "Notes", A), 420)
        )),
        []
    );
    assert_eq!(state.focus.rereading(), 1);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        two_spoken()
    );
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_whole_window_focus_of_the_focus_window_waits_behind_a_set_aside_report_elsewhere() {
    // The last queued focus is F0, in X, so A's whole-window focus is
    // NVDA's next focus: queued behind F0, and spoken after F0's answer.
    let mut state = set_aside_in_x();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &whole_window(A, window(1, "Notes", A), 400)
        )),
        []
    );
    assert_eq!(
        heard(&crate::reduce(&mut state, &other_answer())),
        [
            Heard::Expire(9),
            Heard::Stop,
            said("Elsewhere", Role::Window),
            said("Other", Role::Button),
            Heard::Expire(1),
            Heard::Stop,
            said("Notes", Role::Window),
        ]
    );
    assert_eq!(state.focus.rereading(), 0);
}

/// The answer to X's report of F0 set aside in [`set_aside_in_x`].
fn other_answer() -> Input {
    let mut answer = Report::new(X, button(9, "Other"), vec![window(8, "Elsewhere", X)], 300);
    answer.answers = Some(0);
    answer.input()
}

#[test]
fn a_repeat_of_the_focus_after_a_set_aside_report_elsewhere_is_no_duplicate() {
    // The last queued focus is F0, so F1 again is a new focus: queued
    // behind F0 and executed after F0's answer, against F0's line.
    let mut state = set_aside_in_x();
    let repeat = Report::new(A, button(3, "One"), chain_of_a(), 400);
    assert_eq!(heard(&crate::reduce(&mut state, &repeat.input())), []);
    assert_eq!(
        heard(&crate::reduce(&mut state, &other_answer())),
        [
            Heard::Expire(9),
            Heard::Stop,
            said("Elsewhere", Role::Window),
            said("Other", Role::Button),
            Heard::Expire(3),
            Heard::Stop,
            said("Notes", Role::Window),
            said("Tools", Role::Group),
            said("One", Role::Button)
        ]
    );
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_set_aside_report_left_for_another_window_lets_its_window_be_announced() {
    // F2's report waits, the user goes to X and back to A: F2 was left, so
    // A's whole-window focus is not refused. It waits behind F2 and is
    // spoken as a switch after F2's answer.
    let mut state = pending();
    let _ = crate::reduce(&mut state, &change(X, 410));
    let _ = crate::reduce(&mut state, &change(A, 420));
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &whole_window(A, window(1, "Notes", A), 430)
        )),
        []
    );
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    let mut expected = Vec::from(two_spoken());
    expected.extend([Heard::Expire(1), Heard::Stop, said("Notes", Role::Window)]);
    assert_eq!(heard(&crate::reduce(&mut state, &answer.input())), expected);
    assert_eq!(state.focus.rereading(), 0);
}

#[test]
fn a_command_while_a_report_waits_runs_when_it_is_answered_gone() {
    // F2 ends unanswered, so the move to the focus acts on F0, the focus
    // executed before it.
    let mut state = pending();
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(
        heard(&crate::reduce(&mut state, &gone)),
        [Heard::ForKey(450, vec![moved_to("Other")])]
    );
}

#[test]
fn a_command_while_a_report_waits_runs_when_its_outpost_generation_ends() {
    let mut state = pending();
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(460),
        outpost: OutpostId(A),
    };
    assert_eq!(
        heard(&crate::reduce(&mut state, &ended)),
        [Heard::ForKey(450, vec![moved_to("Other")])]
    );
}

#[test]
fn a_command_while_a_report_waits_runs_when_the_listener_is_replaced() {
    let mut state = pending();
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    assert_eq!(
        heard(&crate::reduce(&mut state, &Input::ListenerReplaced)),
        [Heard::ForKey(450, vec![moved_to("Other")])]
    );
}

#[test]
fn a_command_after_the_focus_generation_ended_says_nothing() {
    // F0's outpost generation ends while F2 waits: when F2 ends, the move
    // to the focus finds no live focus.
    let mut state = pending();
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(440),
        outpost: OutpostId(X),
    };
    let _ = crate::reduce(&mut state, &ended);
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    assert_eq!(heard(&crate::reduce(&mut state, &gone)), []);
}

/// `input`, a focus report, as read through MSAA.
fn through_msaa(mut input: Input) -> Input {
    if let Input::Event {
        backend,
        event: NormalizedEvent::FocusChanged { node, .. },
        ..
    } = &mut input
    {
        *backend = Backend::Msaa;
        node.backend = Backend::Msaa;
    }
    input
}

#[test]
fn a_fake_focus_through_msaa_repeating_the_last_queued_focus_is_dropped() {
    let mut state = SrState::new();
    let _ = crate::reduce(&mut state, &change(A, 50));
    let one = through_msaa(Report::new(A, button(3, "One"), chain_of_a(), 100).input());
    let _ = crate::reduce(&mut state, &one);
    let fake = through_msaa(Report::new(A, button(3, "One"), chain_of_a(), 400).input());
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(fake, verbatim_model::protocol::FocusPurpose::Fake)
        )),
        []
    );
}

#[test]
fn a_fake_focus_through_ui_automation_repeating_the_focus_is_spoken() {
    let mut state = in_a();
    let fake = Report::new(A, button(3, "One"), chain_of_a(), 400).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(fake, verbatim_model::protocol::FocusPurpose::Fake)
        )),
        [Heard::Expire(3), said("One", Role::Button)]
    );
}

#[test]
fn a_fake_answer_observed_before_another_applications_report_is_stale() {
    // X's report of F0, observed at 300, stamps the order: A's fake answer
    // read from 250 is dropped and changes nothing.
    let mut state = in_x();
    let before = state.clone();
    let fake = Report::new(A, button(3, "One"), chain_of_a(), 250).input();
    assert_eq!(
        heard(&crate::reduce(
            &mut state,
            &for_purpose(fake, verbatim_model::protocol::FocusPurpose::Fake)
        )),
        []
    );
    assert_eq!(state.focus, before.focus);
}

/// `input`, a focus report, from outpost generation `outpost`.
fn from_generation(mut input: Input, outpost: u64) -> Input {
    if let Input::Event { event, .. } = &mut input {
        event.assign_outpost(OutpostId(outpost));
    }
    input
}

#[test]
fn a_replacement_outposts_report_of_the_kept_focus_is_taken_silently() {
    let mut state = in_a();
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(200),
        outpost: OutpostId(A),
    };
    let _ = crate::reduce(&mut state, &ended);
    let again = from_generation(
        Report::new(A, button(3, "One"), chain_of_a(), 300).input(),
        A + 1,
    );
    assert_eq!(heard(&crate::reduce(&mut state, &again)), []);
    assert_eq!(state.focused().map(|(_, node)| node.id), Some(id(A + 1, 3)));
}

#[test]
fn a_replacement_outposts_report_of_a_left_focus_is_announced() {
    let mut state = in_a();
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(200),
        outpost: OutpostId(A),
    };
    let _ = crate::reduce(&mut state, &ended);
    let _ = crate::reduce(&mut state, &change(X, 250));
    let _ = crate::reduce(&mut state, &change(A, 260));
    let again = from_generation(
        Report::new(A, button(3, "One"), chain_of_a(), 300).input(),
        A + 1,
    );
    assert_eq!(
        heard(&crate::reduce(&mut state, &again)),
        [
            Heard::Expire(3),
            Heard::Stop,
            said("Notes", Role::Window),
            said("Tools", Role::Group),
            said("One", Role::Button)
        ]
    );
}

#[test]
fn a_repeat_from_the_other_reader_behind_an_answered_report_is_a_duplicate() {
    // F2 waits for the listener's live re-read; the outpost's own read of
    // F2 reaches Core behind it. The listener answers live and F2 is
    // spoken: the outpost's report is a duplicate of it, since only a
    // report that ended unanswered carries the repeat exception.
    let mut state = pending();
    let repeat = through_msaa(Report::new(A, button(4, "Two"), chain_of_a(), 410).input());
    assert_eq!(heard(&crate::reduce(&mut state, &repeat)), []);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        two_spoken()
    );
}

/// The caret key `motion` passed to the application, pressed with trace
/// `trace`.
fn caret_key(trace: u64) -> Input {
    Input::CaretKey {
        trace_id: TraceId::from_raw(trace),
        key_seq: verbatim_model::KeySeq::none_yet(),
        key: verbatim_model::CaretKey {
            motion: verbatim_model::CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 0,
    }
}

/// The caret key held by the hook, pressed with trace `trace`.
fn held_key(trace: u64) -> Input {
    Input::HeldKey {
        trace_id: TraceId::from_raw(trace),
        key_seq: verbatim_model::KeySeq::none_yet(),
        key: verbatim_model::CaretKey {
            motion: verbatim_model::CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 0,
    }
}

#[test]
fn a_caret_key_while_a_report_waits_runs_after_the_command_before_it() {
    // A command, then a caret key, both while the report of an edit field
    // waits: after its answer the command runs first, then the key, which
    // asks the field's caret watch, each under its own trace. Handled at
    // its arrival, the key would find F0, a button with no caret to watch.
    let field = || node(4, Role::EditableText, "Notes field");
    let mut state = in_x();
    let report = Report::new(A, field(), chain_of_a(), 400).cached(1);
    let _ = crate::reduce(&mut state, &report.input());
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    assert_eq!(heard(&crate::reduce(&mut state, &caret_key(460))), []);
    let mut answer = Report::new(A, field(), chain_of_a(), 400);
    answer.answers = Some(0);
    let effects = crate::reduce(&mut state, &answer.input());
    // Each key-tagged effect in order: its trace, and whether it asks the
    // field's caret watch.
    let keyed: Vec<(TraceId, bool)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ForKey(trace, inner) => Some((
                *trace,
                matches!(
                    &**inner,
                    Effect::Text(verbatim_model::TextRequest {
                        node_id,
                        op: verbatim_model::TextOp::Key(_),
                        ..
                    }) if *node_id == id(A, 4)
                ),
            )),
            _ => None,
        })
        .collect();
    // The move to the field says it, then reads its text.
    assert_eq!(
        keyed,
        [
            (TraceId::from_raw(450), false),
            (TraceId::from_raw(450), false),
            (TraceId::from_raw(460), false),
            (TraceId::from_raw(460), true)
        ]
    );
}

#[test]
fn a_held_key_while_a_report_waits_does_not_overtake_the_command_before_it() {
    // A command, then a key the hook holds, while F2 waits: after F2's
    // answer the command runs, then the held key is passed on, once, as
    // the focus has no caret to read.
    let mut state = pending();
    assert_eq!(heard(&crate::reduce(&mut state, &to_focus(450))), []);
    assert_eq!(heard(&crate::reduce(&mut state, &held_key(470))), []);
    let mut answer = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    answer.answers = Some(0);
    let mut expected = Vec::from(two_spoken());
    expected.push(Heard::ForKey(450, vec![moved_to("Two")]));
    expected.push(Heard::ForKey(
        470,
        vec![Heard::Other(format!(
            "{:?}",
            Effect::PassHeldKey(TraceId::from_raw(470))
        ))],
    ));
    assert_eq!(heard(&crate::reduce(&mut state, &answer.input())), expected);
}
