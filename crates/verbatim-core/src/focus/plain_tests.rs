//! Core's plain reader, and the reads of the focus Core makes itself
//! answered hung: the decision table of each entry, the plain object's
//! commit through the ordinary path, the plain reader's answers, and the
//! commands on the plain object through their own entries.

use crate::readers::Found;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusNode, FocusRead, FollowGroup, Front, GhostFront, NodeRead, NodeWindow,
    Outcome as WireOutcome, Press, PressAction, QueueEffect, QueueInput, Refusal, Reply,
};
use verbatim_model::{
    Effect as Said, KeySeq, NodeId, OutpostId, Pid, ReviewCommand, Role, SegmentContent, State,
    StateSet, TraceId, WindowHandle,
};

use crate::entries::tests::{
    APP, Held, LISTENER, OUTPOST, SPOKEN, entry, focus_read, held, msaa_tell, ready,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Asker, Foreground, Input, Instance, Reader, Timed};
use crate::focus::versions::{Committed, ReadBy};
use crate::keys::entry::PressEntry;
use verbatim_model::Effect;

/// hungwin's window, which the shell's foreground report names in place
/// of the ghost in front of it.
const HUNGWIN: WindowHandle = WindowHandle(4);

/// The ghost window in front of hungwin.
const GHOST: WindowHandle = WindowHandle(90);

/// The foreground record: hungwin, as the shell maps the ghost's report.
const IN_FRONT: Foreground = Foreground {
    window: HUNGWIN,
    root_owner: HUNGWIN,
    process: APP,
    thread: 41,
    by_class: None,
    ..Foreground::BLANK
};

/// The ghost as the shell reads it in front.
fn ghost(name: &str) -> Front {
    Front::Ghost(GhostFront {
        ghost: GHOST,
        stands_for: HUNGWIN,
        process: APP,
        thread: 41,
        name: name.to_owned(),
        unavailable: false,
    })
}

/// What the plain object of the ghost "hungwin (Not Responding)" says
/// when its focus is executed.
const PLAIN_SPOKEN: &str = "Label(\"hungwin (Not Responding)\") Role(Window)";

/// Every effect of `input` fed at `at_us`, as a test reads it: each
/// request sent by its reader and kind, each utterance by its segments,
/// the sound, a stop of speech, and the hang check by its windows.
fn effects(held: &mut Held, at_us: u64, input: Input<Entry>) -> Vec<String> {
    feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { to, ask }) => Some(format!(
                "send {:?} {}",
                to.reader,
                crate::entries::tests::kind(&ask)
            )),
            Said::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding) => {
                Some("sound".to_owned())
            }
            Said::StopSpeech => Some("stop".to_owned()),
            Effect::Queue(QueueEffect::Undecodable(number)) => {
                Some(format!("undecodable {}", number.0))
            }
            Effect::Queue(QueueEffect::CheckHung(windows)) => Some(format!("check {windows:?}")),
            _ => None,
        })
        .collect()
}

/// What `from` answering its request `number`, having found `found`, did
/// at `at_us`.
fn found_effects(
    held: &mut Held,
    at_us: u64,
    (from, number): (crate::event_queue::Instance, u64),
    reader_found: crate::readers::Found,
) -> Vec<String> {
    let input = crate::entries::tests::found(held, from, number, reader_found);
    effects(held, at_us, input)
}

/// The shell refused request `number`, a read of the current focus, as
/// hung, with `front` in front.
fn hung_focus(number: u64, front: Front) -> Input<Entry> {
    Input::InFront {
        number: verbatim_model::request::RequestNumber(number),
        front,
    }
}

/// hungwin in front, its startup read answered with its MSAA button "OK"
/// in the frame "Editor", so the next request is 2.
fn in_front() -> Held {
    let mut state = ready();
    assert_eq!(
        effects(&mut state, 100, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        found_effects(
            &mut state,
            200,
            (OUTPOST, 1),
            Found::Read(focus_read(Some(held(-4))))
        ),
        SPOKEN
    );
    state
}

/// An end of `kind` told at `at_us`.
fn end(kind: EventKind) -> Input<Entry> {
    let mut tell = msaa_tell(-4);
    tell.kind = kind;
    entry(tell)
}

/// The check after an end of `kind` at 1 ms, asked at 51 ms as request 2.
fn checked(kind: EventKind) -> Held {
    let mut state = in_front();
    assert_eq!(effects(&mut state, 1_000, end(kind)), Vec::<String>::new());
    crate::entries::tests::in_front_at(&mut state, 51_000, &IN_FRONT);
    assert_eq!(
        effects(&mut state, 51_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    state
}

/// The menu end's check answered with the ghost "hungwin (Not
/// Responding)" in front, at 52 ms.
fn plain_committed() -> Held {
    let mut state = checked(EventKind::MenuEnd);
    assert_eq!(
        effects(
            &mut state,
            52_000,
            hung_focus(2, ghost("hungwin (Not Responding)"))
        ),
        ["stop", PLAIN_SPOKEN]
    );
    state
}

/// Whether hungwin is watched: the half-second round, once due, checks
/// it.
fn watched(state: &mut Held) -> bool {
    let now = state.queue.now_us();
    effects(state, now + 600_000, Input::Tick) == [format!("check [{HUNGWIN:?}]")]
}

/// The plain object's node of `generation`.
fn plain_node(generation: u64) -> NodeId {
    NodeId::plain(generation, 1)
}

#[test]
fn a_menu_end_over_a_ghost_speaks_the_plain_window_with_no_sound_and_watches_hungwin() {
    let mut state = plain_committed();
    let Committed::Read(focus) = &state.versions.committed else {
        panic!("the plain object is committed");
    };
    assert_eq!(focus.context.snapshot.id, plain_node(1));
    assert_eq!(focus.kept.read_by, ReadBy::Plain);
    // It names hungwin's process, never the ghost's own.
    assert_eq!(focus.context.source, APP);
    assert_eq!(focus.kept.asked.process, APP);
    assert_eq!(focus.kept.place.top_level, GHOST);
    assert_eq!(focus.kept.place.root_owner, GHOST);
    assert_eq!(
        state.versions.last.as_ref().map(|last| &last.read_by),
        Some(&ReadBy::Plain)
    );
    assert_eq!(
        state
            .versions
            .focus()
            .map(|focus| focus.context.snapshot.id),
        Some(plain_node(1))
    );
    assert_eq!(
        state
            .navigator
            .as_ref()
            .map(|navigator| navigator.object.id),
        Some(plain_node(1))
    );
    assert_eq!(
        state.foreground.object().map(|object| object.id),
        Some(plain_node(1))
    );
    assert_eq!(state.foreground.entered(), Some(GHOST));
    assert_eq!(state.follows.named(APP), None);
    assert!(watched(&mut state));
}

/// The menu or popup end's check answered hung with no ghost: the sound
/// plays and hungwin is watched, as before.
#[test]
fn a_menu_end_over_a_hung_window_with_no_ghost_plays_the_sound_and_watches() {
    let mut state = checked(EventKind::MenuEnd);
    assert_eq!(
        effects(&mut state, 52_000, hung_focus(2, Front::Hung)),
        ["sound"]
    );
    assert!(watched(&mut state));
}

/// The menu end's check answered with a responsive window, or nothing, in
/// front: nothing is said and nothing watched; the window in front's own
/// tells speak it.
#[test]
fn a_menu_end_over_a_responsive_window_or_nothing_says_nothing_and_watches_nothing() {
    for front in [Front::Responsive, Front::Gone] {
        let mut state = checked(EventKind::MenuEnd);
        assert_eq!(
            effects(&mut state, 52_000, hung_focus(2, front)),
            Vec::<String>::new()
        );
        assert!(!watched(&mut state));
    }
}

/// The popup end's check is NVDA's fake focus as the menu end's is.
#[test]
fn a_popup_end_over_a_ghost_speaks_the_plain_window() {
    let mut state = checked(EventKind::PopupEnd);
    assert_eq!(
        effects(
            &mut state,
            52_000,
            hung_focus(2, ghost("hungwin (Not Responding)"))
        ),
        ["stop", PLAIN_SPOKEN]
    );
    assert!(watched(&mut state));
}

/// The task switcher's check over a ghost is left as it was: the sound
/// plays and hungwin is watched, ghost or not (a recorded difference).
#[test]
fn the_switcher_check_over_a_ghost_or_a_hung_window_plays_the_sound_and_watches() {
    for front in [ghost("hungwin (Not Responding)"), Front::Hung] {
        let mut state = checked(EventKind::SwitchEnd);
        assert_eq!(effects(&mut state, 52_000, hung_focus(2, front)), ["sound"]);
        assert!(watched(&mut state));
        assert!(state.state.plain.object.is_none());
    }
}

/// The startup read answered with the ghost in front speaks the plain
/// window and watches hungwin; answered hung with no ghost, the sound.
#[test]
fn the_startup_read_over_a_ghost_speaks_the_plain_window_and_over_a_hung_window_plays_the_sound() {
    let mut state = ready();
    assert_eq!(
        effects(&mut state, 100, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        effects(
            &mut state,
            200,
            hung_focus(1, ghost("hungwin (Not Responding)"))
        ),
        [PLAIN_SPOKEN]
    );
    assert!(watched(&mut state));

    let mut state = ready();
    let _ = effects(&mut state, 100, Input::Foreground(IN_FRONT));
    assert_eq!(
        effects(&mut state, 200, hung_focus(1, Front::Hung)),
        ["sound"]
    );
    assert!(watched(&mut state));
}

/// The listener's tell of a switch of the input desktop, `open` when the
/// new one can be opened.
fn switch(open: bool) -> QueueInput {
    QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(verbatim_model::tell::Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::DesktopSwitch,
            process: APP,
            application: "app".to_owned(),
            thread: 41,
            at_us: 1_000,
            object: verbatim_model::tell::EventObject::Msaa {
                window: WindowHandle(0),
                object: 0,
                child: 0,
            },
            cached: PropertyValues::default(),
            payload: verbatim_model::tell::Payload::DesktopSwitch {
                input_desktop_open: open,
            },
            window: verbatim_model::tell::TellWindow::NoWindow,
            checked: None,
        }),
    }
}

/// Routes `input` from the shell at `at_us`: the shell's effects.
fn route(state: &mut Held, at_us: u64, input: &QueueInput) -> Vec<Said> {
    crate::entries::route(
        &mut state.state,
        &mut state.runtime,
        (TraceId::from_raw(1), at_us),
        input,
    )
    .said()
}

/// The secure desktop up and back, the read after the return asked as
/// request 2, 200 ms after the return.
fn returned() -> Held {
    let mut state = in_front();
    let _ = route(&mut state, 1_000, &switch(false));
    let _ = route(&mut state, 10_000, &switch(true));
    let asked: Vec<String> = route(
        &mut state,
        10_000 + crate::input_desktop::RETURN_DELAY_US,
        &QueueInput::Due { in_front: None },
    )
    .into_iter()
    .filter_map(|said| match said {
        Said::Queue(QueueEffect::Send { to, ask }) => Some(format!(
            "send {:?} {}",
            to.reader,
            crate::entries::tests::kind(&ask)
        )),
        _ => None,
    })
    .collect();
    assert_eq!(asked, ["send Outpost(Pid(40)) CurrentFocus"]);
    state
}

/// The read after the return from the secure desktop answered with the
/// ghost in front speaks the plain window, as NVDA's `gainFocus` of the
/// window in front does (`IAccessibleHandler/__init__.py` 809 to 813;
/// Dickson, 2026-11-19), and watches hungwin; hung with no ghost, the
/// sound.
#[test]
fn the_return_from_the_secure_desktop_over_a_ghost_speaks_the_plain_window() {
    let mut state = returned();
    assert_eq!(
        effects(
            &mut state,
            300_000,
            hung_focus(2, ghost("hungwin (Not Responding)"))
        ),
        [PLAIN_SPOKEN]
    );
    assert!(watched(&mut state));

    let mut state = returned();
    assert_eq!(
        effects(&mut state, 300_000, hung_focus(2, Front::Hung)),
        ["sound"]
    );
    assert!(watched(&mut state));
}

/// The re-take after the focus's outpost is replaced, answered with a
/// ghost or a hung window in front: NVDA has no such read, so the sound
/// plays and hungwin is watched, as before.
#[test]
fn the_retake_over_a_ghost_or_a_hung_window_plays_the_sound_and_watches() {
    for front in [ghost("hungwin (Not Responding)"), Front::Hung] {
        let mut state = in_front();
        let _ = effects(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
        let next = Instance {
            reader: Reader::Outpost(APP),
            generation: 4,
        };
        assert_eq!(
            crate::entries::tests::ready_at(&mut state, 2_000, next),
            ["send Outpost(Pid(40)) CurrentFocus"]
        );
        assert_eq!(effects(&mut state, 3_000, hung_focus(2, front)), ["sound"]);
        assert!(watched(&mut state));
    }
}

/// The read after hungwin answers again, answered with a ghost or a hung
/// window in front once more: the sound plays and it is watched again.
#[test]
fn the_recovery_read_over_a_ghost_or_a_hung_window_plays_the_sound_and_watches() {
    for front in [ghost("hungwin (Not Responding)"), Front::Hung] {
        let mut state = checked(EventKind::MenuEnd);
        assert_eq!(
            effects(&mut state, 52_000, hung_focus(2, Front::Hung)),
            ["sound"]
        );
        assert_eq!(
            effects(
                &mut state,
                60_000,
                Input::HangCheck {
                    window: HUNGWIN,
                    hung: false
                }
            ),
            ["send Outpost(Pid(40)) CurrentFocus"]
        );
        assert_eq!(effects(&mut state, 61_000, hung_focus(3, front)), ["sound"]);
        assert!(watched(&mut state));
    }
}

/// The plain object, disabled, as the plain reader holds it.
fn disabled_plain() -> Held {
    let mut state = plain_committed();
    if let Some(super::PlainObject {
        kind: super::PlainKind::Ghost { unavailable, .. },
        ..
    }) = state.state.plain.object.as_mut()
    {
        *unavailable = true;
    }
    state
}

/// The properties the tests ask.
fn asked_set() -> Vec<PropertyId> {
    vec![
        PropertyId::Name,
        PropertyId::Role,
        PropertyId::States,
        PropertyId::Value,
    ]
}

/// The plain reader's answer to each operation not run by the batch
/// tests below, on the plain node, as NVDA's plain `Window` gives it: no
/// object one step away in any direction, no descendant found or child
/// walked, a `Follow` applied with nothing to follow, and a terminal's
/// text operation finding no text. A batch naming a node of an outpost,
/// or no node, is not the plain reader's.
#[test]
fn the_plain_reader_answers_each_operation_as_a_plain_window() {
    use verbatim_model::batch::{
        Descended, Done, FollowTarget, Op, Relation, StepOutcome, Subject, View, Walk,
    };
    use verbatim_model::matcher::Matcher;
    let state = disabled_plain();
    let answer = |request| super::answer(&state.state.plain, &request);
    let node = plain_node(1);
    let related = |relation| Op::Related {
        subject: Subject::Node(node),
        relation,
        set: asked_set(),
    };
    let walk = |depth, matcher, limit| Op::Descendants {
        subject: Subject::Node(node),
        walk: Box::new(Walk {
            depth,
            view: View::Raw,
            matcher,
            prune: None,
            include_subject: false,
            limit,
            nodes: 1000,
            set: asked_set(),
        }),
    };
    let none_walked = || {
        StepOutcome::Done(Done::Descendants(Descended {
            nodes: Vec::new(),
            complete: true,
        }))
    };
    assert_eq!(
        answer(plain_batch(vec![
            related(Relation::Parent),
            related(Relation::NextSibling),
            related(Relation::PreviousSibling),
            related(Relation::FirstChild),
        ])),
        Some(WireOutcome::Result(Reply::Steps(vec![
            StepOutcome::Done(
                Done::Related(Vec::new())
            );
            4
        ])))
    );
    let find = Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::Button));
    assert_eq!(
        answer(plain_batch(vec![
            walk(64, Some(find), 1),
            walk(1, None, 1000),
            Op::Follow(Some(FollowTarget {
                subject: Subject::Node(node),
                group: FollowGroup::Base,
                extra: None,
            })),
        ])),
        Some(WireOutcome::Result(Reply::Steps(vec![
            none_walked(),
            none_walked(),
            StepOutcome::Done(Done::Follow),
        ])))
    );
    assert_eq!(
        answer(verbatim_model::request::Question::Terminal(
            verbatim_model::request::TerminalAsk {
                node,
                op: verbatim_model::TextOp::TerminalFind {
                    needle: "ready>".to_owned(),
                    from: 0,
                    rows: 1,
                    start: verbatim_model::TextAnchor(1),
                },
            }
        )),
        Some(WireOutcome::Result(Reply::Text(
            verbatim_model::TextReply::NoText
        )))
    );
    assert_eq!(
        answer(plain_batch(vec![Op::Activate {
            subject: Subject::Node(NodeId::in_outpost(OutpostId(3), 1)),
        }])),
        None
    );
    assert_eq!(
        answer(plain_batch(vec![Op::Read {
            subject: Subject::Window {
                which: verbatim_model::batch::WindowOf::Handle(GHOST),
                backend: verbatim_model::Backend::Uia,
                visible: false,
            },
            set: asked_set(),
        }])),
        None
    );
}

/// A request on an older plain object's node finds it not there. A popup
/// end with no focus in its run checks again first: the same ghost is
/// spoken again, as a new generation, since a plain focus is never a
/// duplicate (`IAccessibleHandler/__init__.py` 772 to 778).
#[test]
fn a_request_on_an_older_plain_node_is_not_there() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 100_000, end(EventKind::PopupEnd)),
        Vec::<String>::new()
    );
    crate::entries::tests::in_front_at(&mut state, 150_000, &IN_FRONT);
    assert_eq!(
        effects(&mut state, 150_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(
        effects(
            &mut state,
            151_000,
            hung_focus(3, ghost("hungwin (Not Responding)"))
        ),
        [PLAIN_SPOKEN]
    );
    let older = plain_batch(vec![verbatim_model::batch::Op::Activate {
        subject: verbatim_model::batch::Subject::Node(plain_node(1)),
    }]);
    assert_eq!(
        super::answer(&state.state.plain, &older),
        Some(WireOutcome::NotThere)
    );
}

/// The listener's destroy of `window`, raised on a thread it could not
/// open, as the desktop window manager's thread destroys a ghost.
fn destroyed(window: WindowHandle) -> QueueInput {
    QueueInput::WindowDestroyed {
        from: LISTENER,
        window,
        process: None,
    }
}

/// The plain object's answer to [`asked_set`] once its ghost is
/// destroyed: no name, the role window and the invisible state alone, as
/// NVDA's plain `Window` of the dead handle answers
/// (`NVDAObjects/window/__init__.py` 190 to 194, 299 to 306).
fn destroyed_values() -> PropertyValues {
    PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::Name,
            value: None,
        },
        PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(Role::Window)),
        },
        PropertyAnswer {
            id: PropertyId::States,
            value: Some(PropertyValue::States(StateSet::from_iter([
                State::Invisible,
            ]))),
        },
        PropertyAnswer {
            id: PropertyId::Value,
            value: None,
        },
    ])
}

/// The ghost committed as the plain focus is destroyed by the desktop
/// window manager's thread, which the listener cannot open: the destroy
/// arrives with no process, sends nothing, and the plain object stays the
/// focus, now NVDA's plain `Window` of a dead handle. Its node answers no
/// name, the role window and the invisible state, and Insert+Tab, a
/// query, says "window invisible", as NVDA's `processAndLabelStates` keeps
/// the invisible state for a query (`controlTypes/processAndLabelStates.py`
/// 62 to 68; Dickson, 2026-11-19).
#[test]
fn the_ghost_destroyed_under_the_plain_focus_says_window_invisible() {
    let mut state = plain_committed();
    // hungwin stays watched: only the ghost's own window was destroyed.
    assert_eq!(
        route(&mut state, 160_000, &destroyed(GHOST)),
        [Said::Queue(QueueEffect::WakeAt(Some(551_000)))]
    );
    let read = plain_batch(vec![verbatim_model::batch::Op::Read {
        subject: verbatim_model::batch::Subject::Node(plain_node(1)),
        set: asked_set(),
    }]);
    let Some(WireOutcome::Result(Reply::Steps(steps))) = super::answer(&state.state.plain, &read)
    else {
        panic!("the plain reader answers its node's read");
    };
    let [verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Read(read))] =
        steps.as_slice()
    else {
        panic!("one read: {steps:?}");
    };
    assert_eq!(read.values, destroyed_values());
    assert_eq!(
        effects(&mut state, 200_000, press(1, ReviewCommand::ReportFocus, 0)),
        ["Role(Window) State(Invisible)"]
    );
    assert_eq!(
        effects(
            &mut state,
            201_000,
            press(2, ReviewCommand::ReportObject, 0)
        ),
        ["Role(Window) State(Invisible)"]
    );
}

/// Another window's destruction leaves the plain ghost as it was, and the
/// next plain commit replaces the destroyed one with a new generation.
#[test]
fn another_windows_destroy_leaves_the_ghost_and_the_next_commit_replaces_the_dead_one() {
    let mut state = plain_committed();
    assert_eq!(
        route(&mut state, 60_000, &destroyed(WindowHandle(91))),
        [Said::Queue(QueueEffect::WakeAt(Some(551_000)))]
    );
    assert_eq!(
        effects(&mut state, 61_000, press(1, ReviewCommand::ReportFocus, 0)),
        [PLAIN_SPOKEN]
    );
    assert_eq!(
        route(&mut state, 62_000, &destroyed(GHOST)),
        [Said::Queue(QueueEffect::WakeAt(Some(551_000)))]
    );
    assert_eq!(
        effects(&mut state, 100_000, end(EventKind::PopupEnd)),
        Vec::<String>::new()
    );
    crate::entries::tests::in_front_at(&mut state, 150_000, &IN_FRONT);
    assert_eq!(
        effects(&mut state, 150_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    // Request 3 was report focus's, answered by the plain reader.
    assert_eq!(
        effects(
            &mut state,
            151_000,
            hung_focus(4, ghost("hungwin (Not Responding)"))
        ),
        [PLAIN_SPOKEN]
    );
    let older = plain_batch(vec![verbatim_model::batch::Op::Activate {
        subject: verbatim_model::batch::Subject::Node(plain_node(1)),
    }]);
    assert_eq!(
        super::answer(&state.state.plain, &older),
        Some(WireOutcome::NotThere)
    );
    assert_eq!(
        effects(&mut state, 152_000, press(2, ReviewCommand::ReportFocus, 0)),
        [PLAIN_SPOKEN]
    );
}

/// `command` pressed as key `key`, `repeat` extra times.
fn press(key: u64, command: ReviewCommand, repeat: u8) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action: PressAction::Command { command, repeat },
        },
    ))))
}

/// Report focus on the plain object speaks it, read from the plain reader
/// with nothing sent; pressed twice it spells its name (`globalCommands.py`
/// 2956 to 2979).
#[test]
fn report_focus_on_the_plain_window_speaks_it_and_spells_it_when_pressed_twice() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 200_000, press(1, ReviewCommand::ReportFocus, 0)),
        [PLAIN_SPOKEN]
    );
    assert_eq!(
        effects(&mut state, 201_000, press(2, ReviewCommand::ReportFocus, 1)),
        [concat!(
            "Text(\"h\") Text(\"u\") Text(\"n\") Text(\"g\") Text(\"w\") Text(\"i\") ",
            "Text(\"n\") Message(Space) Character(\"(\") SpelledCapital(\"N\") Text(\"o\") ",
            "Text(\"t\") Message(Space) SpelledCapital(\"R\") Text(\"e\") Text(\"s\") ",
            "Text(\"p\") Text(\"o\") Text(\"n\") Text(\"d\") Text(\"i\") Text(\"n\") ",
            "Text(\"g\") Character(\")\")"
        )]
    );
}

/// Report the navigator object on the plain object speaks it, with nothing
/// sent (`globalCommands.py` 1515 onward).
#[test]
fn report_navigator_object_on_the_plain_window_speaks_it() {
    let mut state = plain_committed();
    assert_eq!(
        effects(
            &mut state,
            200_000,
            press(1, ReviewCommand::ReportObject, 0)
        ),
        [PLAIN_SPOKEN]
    );
}

/// Review the current line on the plain object reads its flat text, its
/// name, as NVDA's `NVDAObjectTextInfo` reads `basicText`
/// (`globalCommands.py` 1450 to 1463).
#[test]
fn review_current_line_on_the_plain_window_reads_its_name() {
    let mut state = plain_committed();
    assert_eq!(
        effects(
            &mut state,
            200_000,
            press(1, ReviewCommand::ReviewCurrentLine, 0)
        ),
        ["Text(\"hungwin (Not Responding)\")"]
    );
}

/// Say-all on the plain object says nothing, as NVDA's, which can make no
/// caret position there (`speech/sayAll.py` 419 to 423, 137 to 139).
#[test]
fn say_all_on_the_plain_window_says_nothing() {
    let mut state = plain_committed();
    assert_eq!(
        effects(
            &mut state,
            200_000,
            press(1, ReviewCommand::SayAllFromCaret, 0)
        ),
        Vec::<String>::new()
    );
    assert!(state.say_all.is_none());
}

/// A move to the first child, or the parent, of the plain object meets the
/// edge, with nothing sent (the parent's a recorded limit: NVDA reaches the
/// desktop).
#[test]
fn the_first_child_and_the_parent_of_the_plain_window_are_the_edge() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 200_000, press(1, ReviewCommand::FirstChild, 0)),
        ["Message(NoObjectsInside)"]
    );
    assert_eq!(
        effects(&mut state, 201_000, press(2, ReviewCommand::Parent, 0)),
        ["Message(NoContainingObject)"]
    );
}

/// Activate on the plain object says "No action", as NVDA's script does
/// where `doAction` raises (`globalCommands.py` 1862 to 1903).
#[test]
fn activate_on_the_plain_window_says_no_action() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 200_000, press(1, ReviewCommand::Activate, 0)),
        ["Message(NoAction)"]
    );
}

/// A request on the plain node from outside the event queue is answered
/// by the plain reader at once, with nothing sent to any reader.
#[test]
fn a_request_from_outside_the_queue_on_the_plain_node_is_answered_with_nothing_sent() {
    let mut state = plain_committed();
    let window = state.versions.focus().unwrap().kept.asked;
    let effects = feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 200_000,
            input: Input::AskOutside {
                asker: Asker(5),
                reader: Reader::Outpost(APP),
                window,
                request: plain_batch(vec![verbatim_model::batch::Op::Read {
                    subject: verbatim_model::batch::Subject::Node(plain_node(1)),
                    set: vec![PropertyId::Role],
                }]),
                due_after_us: 11_000_000,
            },
        },
    )
    .unwrap();
    assert_eq!(
        effects,
        [Effect::Queue(QueueEffect::Answered {
            asker: 5,
            outcome: WireOutcome::Result(crate::entries::tests::one(
                verbatim_model::batch::Done::Read(NodeRead {
                    node: Some(plain_node(1)),
                    window: NodeWindow {
                        handle: GHOST,
                        top_level: GHOST,
                        root_owner: GHOST,
                        process: APP,
                        thread: 41,
                        desktop: false,
                    },
                    values: PropertyValues(vec![PropertyAnswer {
                        id: PropertyId::Role,
                        value: Some(PropertyValue::Role(Role::Window)),
                    }]),
                })
            )),
        })]
    );
}

/// The listener ending while the plain object is committed asks nothing
/// again: the plain object has no reader that ended, and hungwin would only
/// be found hung once more.
#[test]
fn the_listener_ending_under_the_plain_window_retakes_nothing() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 200_000, Input::ReaderEnded(LISTENER)),
        Vec::<String>::new()
    );
}

/// A UI Automation menu opening while the plain object is committed is no
/// opening on it, which has no runtime id: it is judged and read as a new
/// focus.
#[test]
fn a_uia_menu_opening_under_the_plain_window_is_read_as_a_new_focus() {
    let mut state = plain_committed();
    let mut opening = crate::entries::tests::uia_tell(1, Vec::new());
    opening.kind = EventKind::UiaMenuOpened;
    assert_eq!(
        effects(&mut state, 200_000, entry(opening)),
        ["send Listener ReadObject"]
    );
}

/// An MSAA focus on the object the button "OK" was read by, after the
/// plain object: nothing is a duplicate of the plain object, so it is read
/// and spoken, as a second menu's items are.
#[test]
fn a_focus_after_the_plain_window_is_no_duplicate_of_it() {
    let mut state = plain_committed();
    assert_eq!(
        effects(&mut state, 200_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) Candidate"]
    );
}

/// hungwin answers again: the recovery read is spoken, the window it
/// reads, and mockapp's own focus tell of that window that follows is its
/// duplicate, unspoken.
#[test]
fn after_recovery_the_recovered_read_is_spoken_once() {
    let mut state = plain_committed();
    assert_eq!(
        effects(
            &mut state,
            500_000,
            Input::HangCheck {
                window: HUNGWIN,
                hung: false
            }
        ),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    let window = FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: Some(crate::entries::tests::node(1)),
                window: crate::entries::tests::win(4),
                values: crate::entries::tests::values("hungwin", Role::Window),
            },
            runtime_id: None,
        },
        ancestors: Vec::new(),
        msaa: Some(held(-4)),
        ..focus_read(None)
    };
    assert_eq!(
        found_effects(&mut state, 501_000, (OUTPOST, 3), Found::Read(window)),
        ["stop", "Label(\"hungwin\") Role(Window)"]
    );
    assert_eq!(
        effects(&mut state, 502_000, entry(msaa_tell(-4))),
        Vec::<String>::new()
    );
}

/// The shell's hung answer for a request that is no read of the current
/// focus does not decode: it is logged, and nothing is said.
#[test]
fn a_hung_focus_answer_to_a_properties_request_does_not_decode() {
    let mut state = in_front();
    assert_eq!(
        effects(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0)),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        effects(
            &mut state,
            2_000,
            hung_focus(2, ghost("hungwin (Not Responding)"))
        ),
        ["undecodable 2"]
    );
}

/// A ghost with no window text is spoken by its role alone.
#[test]
fn a_ghost_with_no_name_says_window() {
    let mut state = checked(EventKind::MenuEnd);
    assert_eq!(
        effects(&mut state, 52_000, hung_focus(2, ghost(""))),
        ["stop", "Role(Window)"]
    );
}

/// The ghost of another hung application in front when the check of the
/// asked one is refused, as a click can bring it before its foreground
/// report reaches Core: NVDA speaks the ghost in front, and so does
/// Verbatim, naming the plain object by the application of the window the
/// ghost stands for, never the asked one, which is not watched for it.
#[test]
fn another_applications_ghost_in_front_is_named_by_its_own_application() {
    let mut state = checked(EventKind::MenuEnd);
    let other = Front::Ghost(GhostFront {
        ghost: GHOST,
        stands_for: WindowHandle(50),
        process: Pid(60),
        thread: 61,
        name: "otherwin (Not Responding)".to_owned(),
        unavailable: false,
    });
    assert_eq!(
        effects(&mut state, 52_000, hung_focus(2, other)),
        ["stop", "Label(\"otherwin (Not Responding)\") Role(Window)"]
    );
    let Committed::Read(focus) = &state.versions.committed else {
        panic!("the plain object is committed");
    };
    assert_eq!(focus.context.source, Pid(60));
    assert_eq!(focus.kept.asked.process, Pid(60));
    assert_eq!(focus.kept.asked.thread, 61);
    assert!(!watched(&mut state));
}

/// A UI Automation focus's read afresh, of its application's current
/// focus, answered with a ghost or a hung window in front: NVDA reads the
/// event's own element there, never the window in front
/// (`UIAHandler/__init__.py` 874 onward), so the sound plays and the
/// window is watched, as before.
#[test]
fn a_uia_focus_read_afresh_over_a_ghost_or_a_hung_window_plays_the_sound_and_watches() {
    for front in [ghost("hungwin (Not Responding)"), Front::Hung] {
        let mut state = in_front();
        crate::entries::tests::committed_window_known(&mut state, 4);
        assert_eq!(
            effects(
                &mut state,
                1_000,
                entry(crate::entries::tests::uia_tell(1, Vec::new()))
            ),
            ["send Listener ReadObject"]
        );
        let input = crate::entries::tests::walk_meeting_a_changed_line(&mut state, 2);
        assert_eq!(
            effects(&mut state, 2_000, input),
            ["send Outpost(Pid(40)) CurrentFocus"]
        );
        assert_eq!(effects(&mut state, 3_000, hung_focus(3, front)), ["sound"]);
        assert!(state.state.plain.object.is_none());
        let now = state.queue.now_us();
        assert_eq!(
            effects(&mut state, now + 600_000, Input::Tick),
            ["check [WindowHandle(5)]"]
        );
    }
}

/// The desktop window, in front when no window is.
fn desktop() -> Front {
    Front::Desktop(verbatim_model::request::DesktopFront {
        desktop: WindowHandle(65_552),
        process: Pid(600),
        thread: 601,
    })
}

/// What the plain desktop says when its focus is executed: NVDA's
/// `Desktop`, "Desktop  window".
const DESKTOP_SPOKEN: &str = "Message(Desktop) Role(Window)";

/// A menu, popup or switcher end whose check finds no window in front
/// focuses the desktop, as NVDA's fake focus does
/// (`IAccessibleHandler/__init__.py` 989 to 1002; `NVDAObjects/window/
/// __init__.py` 162 to 170, 123 to 127, 409 to 413; Dickson, 2026-11-19),
/// with no sound and nothing watched.
#[test]
fn an_end_whose_check_finds_no_window_in_front_focuses_the_desktop() {
    for kind in [
        EventKind::MenuEnd,
        EventKind::PopupEnd,
        EventKind::SwitchEnd,
    ] {
        let mut state = checked(kind);
        assert_eq!(
            effects(&mut state, 52_000, hung_focus(2, desktop())),
            ["stop", DESKTOP_SPOKEN]
        );
        let Committed::Read(focus) = &state.versions.committed else {
            panic!("the plain desktop is committed");
        };
        assert_eq!(focus.kept.read_by, ReadBy::Plain);
        assert_eq!(focus.kept.place.top_level, WindowHandle(65_552));
        assert!(!watched(&mut state));
    }
}

/// The desktop in front answering a read NVDA does not make itself, the
/// re-take, plays the sound and watches the read's window, as before.
#[test]
fn the_retake_with_no_window_in_front_plays_the_sound_and_watches() {
    let mut state = in_front();
    let _ = effects(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    let next = Instance {
        reader: Reader::Outpost(APP),
        generation: 4,
    };
    assert_eq!(
        crate::entries::tests::ready_at(&mut state, 2_000, next),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        effects(&mut state, 3_000, hung_focus(2, desktop())),
        ["sound"]
    );
    assert!(watched(&mut state));
}

/// The plain reader holding the ghost of hungwin, generation 1.
fn plain_reader() -> super::PlainReader {
    let mut reader = super::PlainReader::default();
    reader.hold(
        GHOST,
        super::PlainKind::Ghost {
            stands_for: HUNGWIN,
            name: "hungwin (Not Responding)".to_owned(),
            unavailable: false,
        },
        (APP, 41),
    );
    reader
}

/// A batch of `ops`, each with no condition.
fn plain_batch(ops: Vec<verbatim_model::batch::Op>) -> verbatim_model::request::Question {
    verbatim_model::request::Question::Batch(verbatim_model::batch::Batch {
        steps: ops
            .into_iter()
            .map(|op| verbatim_model::batch::Step { op, when: None })
            .collect(),
        placement: verbatim_model::batch::Placement::Queue,
        spare: None,
    })
}

/// The plain reader answers a batch on its node at once from the
/// ghost's facts, as NVDA's plain `Window` answers: the read and a test
/// on its result from its name and role, no text, no action, and its
/// place among the nodes Core names by its node.
#[test]
fn the_plain_reader_answers_each_operation_from_the_ghosts_facts() {
    use verbatim_model::batch::{
        Done, Op, Place, Point, StepOutcome, StepRefusal, Subject, Tested,
    };
    use verbatim_model::matcher::Matcher;
    let node = plain_node(1);
    let reader = plain_reader();
    let window = NodeWindow {
        handle: GHOST,
        top_level: GHOST,
        root_owner: GHOST,
        process: APP,
        thread: 41,
        desktop: false,
    };
    let answered = super::answer(
        &reader,
        &plain_batch(vec![
            Op::Read {
                subject: Subject::Node(node),
                set: vec![PropertyId::Name],
            },
            Op::Test {
                subject: Subject::Result(Place(0)),
                matcher: Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::Window)),
            },
            Op::ReadText {
                trim: false,
                whole: false,
                subject: Subject::Node(node),
                at: Point::At(verbatim_model::TextPoint::Caret),
                movement: None,
                unit: verbatim_model::TextUnit::Line,
                count: 1,
                details: verbatim_model::TextDetails::NONE,
            },
            Op::IsOneOf {
                subject: Subject::Node(node),
                equal: Some(plain_node(7)),
                at_address: vec![node],
            },
        ]),
    );
    let name = PropertyValues(vec![PropertyAnswer {
        id: PropertyId::Name,
        value: Some(PropertyValue::Text("hungwin (Not Responding)".to_owned())),
    }]);
    let role = PropertyValues(vec![PropertyAnswer {
        id: PropertyId::Role,
        value: Some(PropertyValue::Role(Role::Window)),
    }]);
    assert_eq!(
        answered,
        Some(WireOutcome::Result(Reply::Steps(vec![
            StepOutcome::Done(Done::Read(NodeRead {
                node: Some(node),
                window,
                values: name,
            })),
            StepOutcome::Done(Done::Test(Tested {
                matched: true,
                node: NodeRead {
                    node: Some(node),
                    window,
                    values: role,
                },
            })),
            StepOutcome::Refused(StepRefusal::NoText),
            StepOutcome::Done(Done::IsOneOf(Some(1))),
        ])))
    );
    let acted = super::answer(
        &reader,
        &plain_batch(vec![
            Op::Activate {
                subject: Subject::Node(node),
            },
            Op::Related {
                subject: Subject::Node(node),
                relation: verbatim_model::batch::Relation::FirstChild,
                set: Vec::new(),
            },
        ]),
    );
    assert_eq!(
        acted,
        Some(WireOutcome::Result(Reply::Steps(vec![
            StepOutcome::Refused(StepRefusal::Unsupported),
            StepOutcome::Done(Done::Related(Vec::new())),
        ])))
    );
}

/// A batch on a plain node of a generation no longer held is not there;
/// one asking an operation the plain reader does not run is refused whole;
/// one naming no plain node is a reader process's.
#[test]
fn the_plain_reader_refuses_an_old_generation_and_an_operation_it_does_not_run() {
    use verbatim_model::batch::{Malformed, Op, OpKind, Place, Subject};
    let reader = plain_reader();
    let read = |node| Op::Read {
        subject: Subject::Node(node),
        set: vec![PropertyId::Name],
    };
    assert_eq!(
        super::answer(&reader, &plain_batch(vec![read(plain_node(0))])),
        Some(WireOutcome::NotThere)
    );
    assert_eq!(
        super::answer(
            &reader,
            &plain_batch(vec![
                read(plain_node(1)),
                Op::Ancestors {
                    subject: Subject::Result(Place(0)),
                    until: Vec::new(),
                    depth: 4,
                    set: Vec::new(),
                },
            ])
        ),
        Some(WireOutcome::Refused(Refusal::Malformed(
            Malformed::NotRun {
                step: Place(1),
                op: OpKind::Ancestors,
            }
        )))
    );
    assert_eq!(
        super::answer(&reader, &plain_batch(vec![read(NodeId::new(5))])),
        None
    );
}
