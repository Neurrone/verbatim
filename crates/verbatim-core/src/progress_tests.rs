//! Progress bars as Core's asks, through the event queue with real tells
//! and answers: one test per cell of the bar rule (a classic bar's value
//! change and a UI Automation bar's change, in the foreground or not, with
//! the setting on or off), the read's endings, and the queue's handling of
//! a bar's tells.

use crate::entries::tests::found_at;
use crate::readers::Found;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{MsaaIds, NodeRead, Probe, QueueEffect, Reply, RequestNumber};
use verbatim_model::tell::{Checked, Event, EventObject, Payload, Relation, TellWindow};
use verbatim_model::{
    Earcon, Effect, Percent, Pid, Rect, Role, SegmentContent, State, StateSet, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, LISTENER, answer, answer_outcome, facts, feed_at, ready, win,
};
use crate::event_queue::{Foreground, Input, Timed};
use crate::focus::versions::{LastFocus, ReadBy};

/// What a step did, as these tests read it: each request by its reader
/// and what it asks, each indication, and each utterance.
fn step(held: &mut Held, at_us: u64, input: Input<Entry>) -> Vec<String> {
    crate::entries::feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { to, ask }) => Some(match &ask.request {
                verbatim_model::request::Question::Batch(batch)
                    if matches!(
                        batch.steps.as_slice(),
                        [verbatim_model::batch::Step {
                            op: verbatim_model::batch::Op::Read {
                                subject: verbatim_model::batch::Subject::Msaa { .. },
                                ..
                            },
                            ..
                        }]
                    ) =>
                {
                    let verbatim_model::batch::Op::Read { subject, set } = &batch.steps[0].op
                    else {
                        unreachable!("matched as a read");
                    };
                    format!(
                        "send {:?} {} ReadObject {subject:?} {set:?}",
                        to.reader, ask.number.0
                    )
                }
                verbatim_model::request::Question::Batch(batch) => format!(
                    "send {:?} {} {}",
                    to.reader,
                    ask.number.0,
                    crate::entries::tests::batch_kind(batch)
                ),
                verbatim_model::request::Question::Terminal(other) => {
                    format!("send {:?} {} {other:?}", to.reader, ask.number.0)
                }
            }),
            Effect::PlayEarcon(earcon) => Some(format!("{earcon:?}")),
            Effect::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect()
}

/// The classic bar's object: window 5, its client, child 0.
const IDS: MsaaIds = MsaaIds {
    window: WindowHandle(5),
    object: -4,
    child: 0,
};

/// The read a classic bar's value change asks, as `step` shows it.
fn bar_read(number: u64) -> String {
    format!(
        "send Listener {number} ReadObject {:?} [Role, Value, States, Rect]",
        verbatim_model::batch::Subject::Msaa {
            ids: IDS,
            redirect: false,
        }
    )
}

/// The facts of window 5 of `class`, its relation to the foreground
/// window `relation`; a window outside the foreground is topmost, so that
/// acceptance takes it whatever the setting and the bar rule alone
/// decides.
fn window(class: &str, relation: Relation) -> TellWindow {
    let mut facts = facts(class);
    facts.window_to_foreground = relation;
    if relation == Relation::Outside {
        facts.root_owner_to_foreground = Relation::Outside;
        facts.root_owner_is_foregrounds = false;
        facts.window_topmost = true;
    }
    TellWindow::Found(Box::new(facts))
}

/// A classic bar's value change, its window related to the foreground as
/// `relation` says.
fn value_change(relation: Relation) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(8),
        kind: EventKind::ValueChange,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Msaa {
            window: IDS.window,
            object: IDS.object,
            child: IDS.child,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: window("msctls_progress32", relation),
        checked: None,
    }
}

/// A UI Automation bar's change at `percent` of the element `[42, 7]` of
/// `process`, its nearest window related to the foreground as `relation`
/// says, its check's probe answered `provider`.
fn uia_bar(percent: &str, relation: Relation, (process, provider): (Pid, bool)) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(9),
        kind: EventKind::UiaProgress,
        process,
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Followed {
            runtime_id: RuntimeId(vec![42, 7]),
        },
        cached: PropertyValues::default(),
        payload: Payload::Progress {
            percent: Percent::parse(percent).unwrap(),
            place: (50, 60),
        },
        window: window("AppPane", relation),
        checked: Some(Checked {
            probe: Some(Probe {
                window: WindowHandle(5),
                thread: 41,
                provider,
                began_us: 900,
            }),
            keyboard_focus: None,
            kept: Vec::new(),
            previous_sibling: None,
        }),
    }
}

/// `tell` as the queue takes it, under the setting as it stands.
fn told(held: &Held, tell: Event) -> Input<Entry> {
    Input::Entry(
        Entry::of_tell(
            LISTENER,
            tell,
            held.state.settings.report_background_progress_bars,
        )
        .unwrap(),
    )
}

/// The listener's answer to a bar read: `role`, `value`, `states`, and a
/// rectangle centred on (50, 20).
fn bar_answer(role: Role, value: &str, states: StateSet) -> Reply {
    let answer = |id, value| PropertyAnswer {
        id,
        value: Some(value),
    };
    crate::entries::tests::one(verbatim_model::batch::Done::Read(NodeRead {
        node: None,
        window: win(5),
        values: PropertyValues(vec![
            answer(PropertyId::Role, PropertyValue::Role(role)),
            answer(PropertyId::Value, PropertyValue::Text(value.to_owned())),
            answer(PropertyId::States, PropertyValue::States(states)),
            answer(
                PropertyId::Rect,
                PropertyValue::Rect(Rect {
                    left: 0,
                    top: 10,
                    width: 100,
                    height: 20,
                }),
            ),
        ]),
    }))
}

/// The tone at `percent`.
fn tone(percent: u8) -> String {
    format!("{:?}", Earcon::Progress(percent))
}

/// Ready, with background progress bars reported as `on` says.
fn ready_with(on: bool) -> Held {
    let mut held = ready();
    held.state.settings.report_background_progress_bars = on;
    held
}

/// A classic bar in the foreground is read with the setting off or on,
/// and its answer plays its tone.
#[test]
fn a_foreground_classic_bar_is_read_and_heard_either_way() {
    for on in [false, true] {
        let mut held = ready_with(on);
        let tell = told(&held, value_change(Relation::Under));
        assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)], "setting {on}");
        assert_eq!(
            step(
                &mut held,
                2_000,
                answer(
                    LISTENER,
                    1,
                    bar_answer(Role::ProgressBar, "40%", StateSet::new())
                )
            ),
            [tone(40)]
        );
        assert!(held.state.bar_reads.is_empty());
    }
}

/// A classic bar outside the foreground, accepted as topmost, is read only
/// with the setting on.
#[test]
fn a_background_classic_bar_is_read_only_with_the_setting_on() {
    let mut held = ready_with(false);
    let tell = told(&held, value_change(Relation::Outside));
    assert_eq!(step(&mut held, 1_000, tell), Vec::<String>::new());
    let mut held = ready_with(true);
    let tell = told(&held, value_change(Relation::Outside));
    assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
    assert_eq!(
        step(
            &mut held,
            2_000,
            answer(
                LISTENER,
                1,
                bar_answer(Role::ProgressBar, "60", StateSet::new())
            )
        ),
        [tone(60)]
    );
}

/// The readers' last focus on the change's ids, `role` its role.
fn last_focus_on_the_bar(held: &mut Held, role: Role) {
    held.state.versions.last = Some(LastFocus {
        read_by: ReadBy::Msaa {
            event: Some(IDS),
            held_child: 0,
        },
        place: crate::focus::versions::Place {
            window: WindowHandle(5),
            top_level: WindowHandle(4),
            root_owner: WindowHandle(4),
            ancestor_windows: Vec::new().into(),
        },
        role,
        since: None,
    });
}

/// A value change on the readers' last focus whose role is known and no
/// bar, a field being typed in, is not read; one whose role is unknown
/// is, and a bar beeps.
#[test]
fn a_value_change_on_the_last_focus_is_read_only_when_it_may_be_a_bar() {
    let mut held = ready_with(false);
    last_focus_on_the_bar(&mut held, Role::EditableText);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_000, tell), Vec::<String>::new());
    let mut held = ready_with(false);
    last_focus_on_the_bar(&mut held, Role::Unknown);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
    assert_eq!(
        step(
            &mut held,
            2_000,
            answer(
                LISTENER,
                1,
                bar_answer(Role::ProgressBar, "10", StateSet::new())
            )
        ),
        [tone(10)]
    );
}

/// An MSAA value change in a window read through UI Automation is
/// dropped, as NVDA drops a native window's `WinEvent`.
#[test]
fn a_classic_value_change_of_a_uia_window_is_dropped() {
    let mut held = ready_with(true);
    let mut tell = value_change(Relation::Under);
    tell.window = window("RAIL_WINDOW", Relation::Under);
    let tell = told(&held, tell);
    assert_eq!(step(&mut held, 1_000, tell), Vec::<String>::new());
}

/// A read's answer plays only for a progress bar on screen whose value is
/// a number.
#[test]
fn a_bar_read_plays_only_for_an_on_screen_numeric_bar() {
    for (reply, why) in [
        (bar_answer(Role::Slider, "40", StateSet::new()), "a slider"),
        (
            bar_answer(
                Role::ProgressBar,
                "40",
                StateSet::from_iter([State::Invisible]),
            ),
            "invisible",
        ),
        (
            bar_answer(
                Role::ProgressBar,
                "40",
                StateSet::from_iter([State::Offscreen]),
            ),
            "off screen",
        ),
        (
            bar_answer(Role::ProgressBar, "busy", StateSet::new()),
            "no number",
        ),
    ] {
        let mut held = ready_with(false);
        let tell = told(&held, value_change(Relation::Under));
        assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
        assert_eq!(
            step(&mut held, 2_000, answer(LISTENER, 1, reply)),
            Vec::<String>::new(),
            "{why}"
        );
        assert!(held.state.bar_reads.is_empty(), "{why}");
    }
}

/// A key pressed while a bar read is out runs at once: the read is
/// outside the queue, and its answer plays after.
#[test]
fn no_key_waits_behind_a_bar_read() {
    use verbatim_model::request::{Press, PressAction};
    let mut held = ready_with(false);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
    let key = Input::Entry(Entry::Press(Box::new(crate::keys::entry::PressEntry::new(
        verbatim_model::TraceId::from_raw(70),
        Press {
            key_seq: verbatim_model::KeySeq::new(1),
            pressed_at_us: 1_100,
            action: PressAction::Command {
                command: verbatim_model::ReviewCommand::ReportFocus,
                repeat: 0,
            },
        },
    ))));
    let said = step(&mut held, 1_100, key);
    assert!(!said.is_empty(), "the key spoke: {said:?}");
    assert!(held.state.queue.front_for_tests().is_none(), "and ended");
    assert_eq!(
        step(
            &mut held,
            2_000,
            answer(
                LISTENER,
                1,
                bar_answer(Role::ProgressBar, "40", StateSet::new())
            )
        ),
        [tone(40)]
    );
}

/// Window 4 of [`APP`] in front, or window 70 of another `process`.
fn in_front(process: Pid) -> Input<Entry> {
    let window = WindowHandle(if process == APP { 4 } else { 70 });
    Input::Foreground(Foreground {
        window,
        root_owner: window,
        process,
        thread: 41,
        by_class: None,
        ..Foreground::BLANK
    })
}

/// Core's first foreground record, window 4 of [`APP`], with the read of
/// the focus at startup answered with nothing there, so the front is free;
/// then `process` in front, when it is another.
fn started(held: &mut Held, process: Pid) {
    assert_eq!(step(held, 400, in_front(APP)).len(), 1, "the startup read");
    let _ = step(
        held,
        450,
        answer_outcome(
            crate::entries::tests::OUTPOST,
            1,
            verbatim_model::request::Outcome::NotThere,
        ),
    );
    if process != APP {
        let _ = step(held, 500, in_front(process));
    }
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A bar read refused because its window is hung plays "not responding"
/// only while its application holds attention, and the read ends.
#[test]
fn a_hung_bar_read_plays_not_responding_only_when_attended() {
    for (front, heard) in [
        (APP, vec![format!("{:?}", Earcon::AppNotResponding)]),
        (Pid(77), Vec::new()),
    ] {
        let mut held = ready_with(true);
        started(&mut held, front);
        let tell = told(&held, value_change(Relation::Under));
        assert_eq!(step(&mut held, 1_000, tell), [bar_read(2)]);
        assert_eq!(
            step(&mut held, 1_100, Input::RefusedHung(RequestNumber(2))),
            heard,
            "{front:?} in front"
        );
        assert!(held.state.bar_reads.is_empty());
    }
}

/// A bar read past Core's ordinary due time ends with nothing said, and
/// one out when the listener ends too.
#[test]
fn a_bar_read_past_its_due_time_or_its_listener_ends_silently() {
    let mut held = ready_with(false);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
    assert_eq!(
        step(
            &mut held,
            1_000 + crate::event_queue::OTHER_DUE_US,
            Input::Tick
        ),
        Vec::<String>::new()
    );
    assert!(held.state.bar_reads.is_empty());
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 20_000_000, tell), [bar_read(2)]);
    let _ = step(&mut held, 20_000_100, Input::ReaderEnded(LISTENER));
    assert!(held.state.bar_reads.is_empty());
    assert_eq!(
        step(
            &mut held,
            20_000_200,
            answer_outcome(
                LISTENER,
                2,
                verbatim_model::request::Outcome::Result(bar_answer(
                    Role::ProgressBar,
                    "40",
                    StateSet::new()
                ))
            )
        ),
        Vec::<String>::new(),
        "a late answer plays nothing"
    );
}

/// A native UI Automation bar in the foreground plays from its tell with
/// no read, the setting off or on; one by its place moves by a percent.
#[test]
fn a_foreground_uia_bar_plays_from_its_tell() {
    for on in [false, true] {
        let mut held = ready_with(on);
        let tell = told(&held, uia_bar("30", Relation::Equal, (APP, true)));
        assert_eq!(step(&mut held, 1_000, tell), [tone(30)], "setting {on}");
        let tell = told(&held, uia_bar("30.5", Relation::Equal, (APP, true)));
        assert_eq!(
            step(&mut held, 2_000, tell),
            Vec::<String>::new(),
            "less than a percent from the last at its place"
        );
    }
}

/// A UI Automation bar outside the foreground is accepted and plays with
/// the setting on, and is refused at its arrival with it off.
#[test]
fn a_background_uia_bar_is_heard_only_with_the_setting_on() {
    let mut background = uia_bar("30", Relation::Outside, (APP, true));
    let TellWindow::Found(facts) = &mut background.window else {
        unreachable!()
    };
    facts.window_topmost = false;
    let mut held = ready_with(false);
    let entry = Entry::of_tell(LISTENER, background.clone(), false).unwrap();
    let Entry::Progress(bar) = &entry else {
        panic!("a progress entry: {entry:?}");
    };
    assert!(bar.accepted.is_err(), "refused at its arrival");
    assert_eq!(
        step(&mut held, 1_000, Input::Entry(entry)),
        Vec::<String>::new()
    );
    let mut held = ready_with(true);
    let tell = told(&held, background);
    assert_eq!(step(&mut held, 1_000, tell), [tone(30)]);
}

/// A UI Automation bar outside the foreground that acceptance takes as
/// topmost plays only with the setting on: the bar rule decides.
#[test]
fn a_topmost_background_uia_bar_plays_only_with_the_setting_on() {
    let mut held = ready_with(false);
    let tell = told(&held, uia_bar("30", Relation::Outside, (APP, true)));
    assert_eq!(step(&mut held, 1_000, tell), Vec::<String>::new());
    let mut held = ready_with(true);
    let tell = told(&held, uia_bar("30", Relation::Outside, (APP, true)));
    assert_eq!(step(&mut held, 1_000, tell), [tone(30)]);
}

/// A UI Automation bar whose window is read through MSAA, by its probe or
/// by NVDA's class lists, is dropped: the MSAA path hears it.
#[test]
fn a_uia_bar_not_native_is_dropped() {
    let mut held = ready_with(true);
    let tell = told(&held, uia_bar("30", Relation::Equal, (APP, false)));
    assert_eq!(
        step(&mut held, 1_000, tell),
        Vec::<String>::new(),
        "no provider"
    );
    let mut listed = uia_bar("30", Relation::Equal, (APP, true));
    listed.window = window("msctls_progress32", Relation::Equal);
    let tell = told(&held, listed);
    assert_eq!(
        step(&mut held, 2_000, tell),
        Vec::<String>::new(),
        "a class NVDA reads through MSAA, with a provider"
    );
}

/// While Core's foreground record still names another application's
/// window, a bar
/// whose own tell says its window is in front is heard with the setting
/// off: the foreground hold's 20 ms leaves the record behind, and the
/// tell's relation was read with the window in front, as NVDA reads it.
#[test]
fn a_bar_in_front_by_its_tell_is_heard_before_the_foreground_report() {
    let mut held = ready_with(false);
    started(&mut held, Pid(77));
    let tell = told(&held, uia_bar("30", Relation::Equal, (APP, true)));
    assert_eq!(step(&mut held, 1_000, tell), [tone(30)]);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_100, tell), [bar_read(2)]);
}

/// The UI Automation focus waiting for the listener's read holds the
/// front.
fn front_held() -> Held {
    let mut held = ready_with(false);
    assert_eq!(
        feed_at(
            &mut held,
            900,
            crate::entries::tests::entry(crate::entries::tests::uia_tell(1, Vec::new()))
        ),
        ["send Listener ReadObject"]
    );
    held
}

/// A flood of one bar's changes while the front is busy leaves one entry
/// waiting, the newest, which plays its percentage at its turn.
#[test]
fn a_flood_of_one_uia_bars_changes_waits_as_one_entry() {
    let mut held = front_held();
    for percent in 0..100_u8 {
        let tell = told(
            &held,
            uia_bar(&percent.to_string(), Relation::Equal, (APP, true)),
        );
        assert_eq!(
            step(&mut held, 1_000 + u64::from(percent), tell),
            Vec::<String>::new()
        );
    }
    assert_eq!(held.state.queue.waiting_for_tests(), 1);
    assert_eq!(
        step(
            &mut held,
            2_000,
            answer_outcome(LISTENER, 1, verbatim_model::request::Outcome::NotThere)
        ),
        [tone(99)],
        "the front ends and the newest percentage plays"
    );
}

/// Two processes' bars with one runtime id are two bars: neither replaces
/// the other as they wait.
#[test]
fn bars_of_two_processes_sharing_a_runtime_id_are_kept_apart() {
    let mut held = front_held();
    for process in [APP, Pid(41)] {
        let tell = told(&held, uia_bar("30", Relation::Equal, (process, true)));
        let _ = step(&mut held, 1_000, tell);
    }
    assert_eq!(held.state.queue.waiting_for_tests(), 2);
}

/// A bar's tell naming the committed focus's runtime id reaches neither
/// the caret path nor the property-change path: it plays, and asks and
/// says nothing else.
#[test]
fn a_uia_bars_tell_is_neither_a_caret_tell_nor_a_property_change() {
    let mut held = crate::entries::tests::asked_completion();
    let _ = found_at(
        &mut held,
        3_000,
        (crate::entries::tests::OUTPOST, 2),
        Found::Read(crate::entries::tests::focus_read(None)),
    );
    let mut tell = uia_bar("30", Relation::Equal, (APP, true));
    tell.object = EventObject::Followed {
        runtime_id: RuntimeId(vec![42, 1]),
    };
    let entry = Entry::of_tell(LISTENER, tell, false).unwrap();
    assert!(matches!(entry, Entry::Progress(_)), "{entry:?}");
    assert_eq!(step(&mut held, 4_000, Input::Entry(entry)), [tone(30)]);
}

/// The bar's read out, by its token, and whether a newer change marked
/// it.
fn bar_out(held: &Held) -> Option<(u64, bool)> {
    let out = held
        .state
        .bar_reads
        .get(&(IDS.window.0, IDS.object, IDS.child))?;
    Some(match *out {
        crate::progress::BarRead::Reading { token } => (token, false),
        crate::progress::BarRead::ReadingNewer { token, .. } => (token, true),
    })
}

/// A flood of one classic bar's changes keeps one read out: each change
/// after the first only marks it, so the work each step does over the
/// requests out stays at one request however long the flood. The read's
/// answer reads the bar once more, and that answer plays the newest
/// value.
#[test]
fn a_flood_of_one_classic_bars_changes_keeps_one_read_out() {
    let mut held = ready_with(false);
    let tell = told(&held, value_change(Relation::Under));
    assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
    for change in 1..=1_000_u64 {
        let tell = told(&held, value_change(Relation::Under));
        assert_eq!(
            step(&mut held, 1_000 + change, tell),
            Vec::<String>::new(),
            "change {change}"
        );
        assert_eq!(
            held.state.queue.batches_for_tests().len(),
            1,
            "change {change}"
        );
        assert_eq!(held.state.bar_reads.len(), 1, "change {change}");
    }
    assert_eq!(bar_out(&held).map(|(_, newer)| newer), Some(true));
    assert_eq!(
        step(
            &mut held,
            5_000,
            answer(
                LISTENER,
                1,
                bar_answer(Role::ProgressBar, "40", StateSet::new())
            )
        ),
        [tone(40), bar_read(2)]
    );
    assert_eq!(
        step(
            &mut held,
            6_000,
            answer(
                LISTENER,
                2,
                bar_answer(Role::ProgressBar, "100", StateSet::new())
            )
        ),
        [tone(100)],
        "the last change is heard"
    );
    assert!(held.state.bar_reads.is_empty());
    assert_eq!(held.state.queue.batches_for_tests(), Vec::new());
}

/// An ending of a bar's read, by its name, fed to the state.
type Ending = (&'static str, fn(&mut Held) -> Vec<String>);

/// Every ending of a bar's read out ends it when no change came since it
/// was sent, and reads the bar again when one did: its answer, a refusal
/// as hung, its due time, and its listener's end, whose read again waits
/// for the next listener.
#[test]
fn each_ending_of_a_bar_read_ends_it_or_reads_the_bar_again() {
    let endings: [Ending; 4] = [
        ("answered", |held| {
            step(
                held,
                2_000,
                answer(LISTENER, 1, bar_answer(Role::Slider, "40", StateSet::new())),
            )
        }),
        ("hung", |held| {
            step(held, 2_000, Input::RefusedHung(RequestNumber(1)))
        }),
        ("due", |held| {
            step(held, 1_000 + crate::event_queue::OTHER_DUE_US, Input::Tick)
        }),
        ("listener ended", |held| {
            step(held, 2_000, Input::ReaderEnded(LISTENER))
        }),
    ];
    for (why, end) in endings {
        for newer in [false, true] {
            let mut held = ready_with(false);
            let tell = told(&held, value_change(Relation::Under));
            assert_eq!(step(&mut held, 1_000, tell), [bar_read(1)]);
            if newer {
                let tell = told(&held, value_change(Relation::Under));
                assert_eq!(step(&mut held, 1_100, tell), Vec::<String>::new());
            }
            let said = end(&mut held);
            let read_again = said.contains(&bar_read(2));
            match (newer, why) {
                (false, _) => {
                    assert!(!read_again, "{why}: {said:?}");
                    assert_eq!(bar_out(&held), None, "{why}");
                }
                (true, "listener ended") => {
                    assert!(!read_again, "{why}: no listener to send to");
                    assert_eq!(bar_out(&held), Some((2, false)), "{why}");
                }
                (true, _) => {
                    assert!(read_again, "{why}: {said:?}");
                    assert_eq!(bar_out(&held), Some((2, false)), "{why}");
                }
            }
        }
    }
}
