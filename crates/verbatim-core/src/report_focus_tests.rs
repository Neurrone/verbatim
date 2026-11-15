//! Report focus as an event-queue entry: one read of the committed
//! focus's node at its turn, "No focus" with nothing committed, silence
//! for any outcome but a result, and the not-responding sound for a hung
//! window.

use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, TextSummary,
};
use verbatim_model::request::{Press, PressAction, Reply, Request as Wire};
use verbatim_model::{Effect as Said, KeySeq, ReviewCommand, Role, TraceId, WindowHandle};

use crate::entries::tests::{
    APP, Held, OUTPOST, SPOKEN_FOLLOWED, answer, feed_at, focus_read, node, ready,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Effect, Foreground, Input, Timed};
use crate::keys::entry::PressEntry;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
};

/// The startup read answered: the button "OK" committed, its `Follow`
/// request 2, so the next request is 3.
fn committed() -> Held {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 500, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            600,
            answer(OUTPOST, 1, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    state
}

/// Report focus pressed as key `key`, `repeat` extra times.
fn press(key: u64, repeat: u8) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action: PressAction::Command {
                command: ReviewCommand::ReportFocus,
                repeat,
            },
        },
    ))))
}

/// An edit field "Body" whose text summary is `summary`.
fn field(summary: TextSummary) -> Input<Entry> {
    answer(
        OUTPOST,
        3,
        Reply::Properties(PropertyValues(vec![
            PropertyAnswer {
                id: PropertyId::Name,
                value: Some(PropertyValue::Text("Body".to_owned())),
            },
            PropertyAnswer {
                id: PropertyId::Role,
                value: Some(PropertyValue::Role(Role::EditableText)),
            },
            PropertyAnswer {
                id: PropertyId::Value,
                value: Some(PropertyValue::Text("one two".to_owned())),
            },
            PropertyAnswer {
                id: PropertyId::TextSummary,
                value: Some(PropertyValue::TextSummary(summary)),
            },
        ])),
    )
}

#[test]
fn with_nothing_committed_it_says_no_focus_and_asks_nothing() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, press(1, 0)),
        ["Message(NoFocus)"]
    );
    assert_eq!(state.queue.front_for_tests(), None);
}

#[test]
fn it_reads_the_committed_node_live_and_speaks_it_with_its_line() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 1_000, press(1, 0)),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Properties {
            node: node(2),
            set: super::REPORT_SET.to_vec(),
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            field(TextSummary::Line("one two\n".to_owned()))
        ),
        ["Label(\"Body\") Role(EditableText)", "Text(\"one two\")"]
    );
    // The answer is only spoken: the committed focus stays the button.
    assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
}

#[test]
fn a_selection_is_spoken_as_selected_text() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            field(TextSummary::Selected("two".to_owned()))
        ),
        [
            "Label(\"Body\") Role(EditableText)",
            "Phrase(Preselected(Text(\"two\")))"
        ]
    );
}

#[test]
fn a_repeat_spells_the_name() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, 1));
    assert_eq!(
        feed_at(&mut state, 2_000, field(TextSummary::Line(String::new()))),
        ["SpelledCapital(\"B\") Text(\"o\") Text(\"d\") Text(\"y\")"]
    );
}

#[test]
fn each_press_waits_its_turn_and_is_spoken() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, 0));
    assert_eq!(
        feed_at(&mut state, 1_100, press(2, 0)),
        Vec::<String>::new()
    );
    assert_eq!(
        feed_at(&mut state, 2_000, field(TextSummary::Line("a".to_owned()))),
        [
            "Label(\"Body\") Role(EditableText)",
            "Text(\"a\")",
            "send Outpost(Pid(40)) other"
        ]
    );
}

#[test]
fn a_hung_window_plays_the_sound_and_says_nothing() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, 0));
    let effects = feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 2_000,
            input: Input::RefusedHung(verbatim_model::request::RequestNumber(3)),
        },
    )
    .unwrap();
    assert_eq!(effects, [Effect::NotRespondingSound]);
}

#[test]
fn a_node_not_there_says_nothing() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            crate::entries::tests::answer_outcome(
                OUTPOST,
                3,
                verbatim_model::request::Outcome::NotThere
            )
        ),
        Vec::<String>::new()
    );
}

/// The speech of an entry carries the newest key number at its arrival:
/// a key that joins later does not move it.
#[test]
fn its_speech_carries_its_own_key_number_not_a_later_keys() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(5, 0));
    let _ = feed_at(&mut state, 1_100, press(6, 0));
    let effects = feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 2_000,
            input: field(TextSummary::Line("a".to_owned())),
        },
    )
    .unwrap();
    let keys: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Said(Said::Speak(_, key)) => Some(*key),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [Some(KeySeq::new(5)), Some(KeySeq::new(5))]);
    assert_eq!(state.arrivals.fence().key(), Some(KeySeq::new(6)));
}

/// A press reaches the queue through the reducer, and the reducer leaves
/// the fence its speech carries.
#[test]
fn the_reducer_routes_a_press_into_the_queue_and_keeps_its_fence() {
    use verbatim_model::request::{QueueEffect, QueueInput};
    let mut held = Held::default();
    let effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &verbatim_model::Input::Queue {
            trace_id: TraceId::from_raw(9),
            at_us: 1_000,
            input: QueueInput::Press(Box::new(Press {
                key_seq: KeySeq::new(9),
                pressed_at_us: 900,
                action: PressAction::Command {
                    command: ReviewCommand::ReportFocus,
                    repeat: 0,
                },
            })),
        },
    );
    let [
        Said::Speak(utterance, key),
        Said::Queue(QueueEffect::WakeAt(None)),
    ] = effects.as_slice()
    else {
        panic!("the message and the wake: {effects:?}");
    };
    assert_eq!(*key, Some(KeySeq::new(9)));
    assert_eq!(
        utterance.segments,
        crate::review_text::message(verbatim_model::Message::NoFocus)
    );
}
