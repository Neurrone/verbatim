//! Core's check before speaking and its live re-reads, as reducer tests
//! asserting every effect (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The ancestry cache"): a report that would speak a cached ancestor, as
//! entered or as the title, is set aside with nothing written but its focus
//! group's rank, its focus reader asked for a live re-read, which is
//! counted and logged, and its answer applied as an ordinary report; each
//! other ending of a pending live re-read says nothing.

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

/// The live re-read the report of F2, `button(4, "Two")`, from A's
/// listener, traced 400, is set aside for, the `request`th.
fn reread_of_two(request: u64, lowest: Option<u64>, reason: LiveRereadReason) -> Heard {
    reread(request, (4, 400), (id(A, 4), Some("Two")), lowest, reason)
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

/// `after` with the three fields a report set aside may write put back as
/// they were in `before`: everything else must be unchanged.
fn but_the_set_aside(before: &SrState, after: &SrState) -> SrState {
    let mut after = after.clone();
    after.group_taken = before.group_taken;
    after.live_rereads = before.live_rereads.clone();
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
    assert_eq!(state.focus, before.focus);
    assert_eq!(state.foreground, before.foreground);
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
fn a_report_needing_a_live_re_read_writes_nothing_but_its_group_rank() {
    let mut state = in_x();
    let before = state.clone();
    let mut report = return_to_two();
    report.group = Some(FocusGroup { number: 7, rank: 2 });
    let _ = crate::reduce(&mut state, &report.input());
    assert_eq!(
        state.group_taken,
        Some((OutpostId(A), FocusGroup { number: 7, rank: 2 }))
    );
    assert_eq!(state.live_rereads.pending().len(), 1);
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
    let ancestors = &state.focus.as_ref().unwrap().ancestors;
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
    assert!(state.live_rereads.pending().is_empty());
    assert_eq!(state.live_re_reads(), 1);
}

#[test]
fn a_newer_focus_ends_the_pending_live_re_read() {
    let mut state = in_x();
    let _ = crate::reduce(&mut state, &return_to_two().input());
    let newer = Report::new(X, button(7, "Newer"), vec![window(8, "Elsewhere", X)], 500);
    assert_eq!(
        heard(&crate::reduce(&mut state, &newer.input())),
        [Heard::Expire(7), said("Newer", Role::Button)]
    );
    let before = state.clone();
    let mut late = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    late.answers = Some(0);
    assert_eq!(heard(&crate::reduce(&mut state, &late.input())), []);
    assert_eq!(state, before);
}

#[test]
fn a_newer_live_re_read_replaces_the_pending_one() {
    let mut state = in_x();
    let first = Report::new(A, button(4, "Two"), chain_of_a(), 400).cached(1);
    let second = Report::new(A, button(5, "Five"), chain_of_a(), 450).cached(1);
    assert_eq!(
        heard(&crate::reduce(&mut state, &first.input())),
        [reread_of_two(0, None, LiveRereadReason::Entered)]
    );
    assert_eq!(
        heard(&crate::reduce(&mut state, &second.input())),
        [reread(
            1,
            (5, 450),
            (id(A, 5), Some("Five")),
            None,
            LiveRereadReason::Entered
        )]
    );
    let mut old = Report::new(A, button(4, "Two"), chain_of_a(), 400);
    old.answers = Some(0);
    assert_eq!(heard(&crate::reduce(&mut state, &old.input())), []);
    let mut answer = Report::new(A, button(5, "Five"), chain_of_a(), 450);
    answer.answers = Some(1);
    assert_eq!(
        heard(&crate::reduce(&mut state, &answer.input())),
        [
            Heard::Expire(5),
            Heard::Stop,
            said("Notes", Role::Window),
            said("Tools", Role::Group),
            said("Five", Role::Button),
        ]
    );
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
    assert!(state.live_rereads.pending().is_empty());
}

#[test]
fn a_live_re_read_ends_at_its_outpost_generations_end() {
    let mut state = pending();
    let ended = Input::OutpostEnded {
        trace_id: TraceId::from_raw(1),
        outpost: OutpostId(A),
    };
    assert_eq!(heard(&crate::reduce(&mut state, &ended)), []);
    assert!(state.live_rereads.pending().is_empty());
}

#[test]
fn a_live_re_read_ends_when_the_listener_is_replaced() {
    let mut state = pending();
    assert_eq!(
        heard(&crate::reduce(&mut state, &Input::ListenerReplaced)),
        []
    );
    assert!(state.live_rereads.pending().is_empty());
}

#[test]
fn a_pending_live_re_read_counts_for_the_fake_focus_skip_and_the_barrier() {
    let mut state = pending();
    assert_eq!(state.outstanding_queries(), 1);
    let ended = Input::MenuOrSwitchEnded { ended_at_us: 350 };
    assert_eq!(heard(&crate::reduce(&mut state, &ended)), []);
    let gone = Input::LiveRereadGone {
        outpost: OutpostId(A),
        request: 0,
    };
    let _ = crate::reduce(&mut state, &gone);
    assert_eq!(state.outstanding_queries(), 0);
    assert_eq!(
        heard(&crate::reduce(&mut state, &ended)),
        [Heard::Other("ReadForegroundFocus".to_owned())]
    );
}

#[test]
fn each_live_re_read_is_counted_and_logged() {
    let mut state = in_x();
    let mut logged = Vec::new();
    for (request, focus, at_us) in [(0, 4, 400), (1, 5, 410), (2, 6, 420)] {
        let report = Report::new(A, button(focus, "Two"), chain_of_a(), at_us).cached(1);
        let effects = heard(&crate::reduce(&mut state, &report.input()));
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

#[test]
fn a_repeat_while_a_live_re_read_waits_is_a_duplicate() {
    let mut state = pending();
    let before = state.clone();
    let repeat = Report::new(A, button(4, "Two"), chain_of_a(), 410).cached(1);
    assert_eq!(heard(&crate::reduce(&mut state, &repeat.input())), []);
    assert_eq!(state.live_re_reads(), 1);
    assert_eq!(state.live_rereads, before.live_rereads);
}
