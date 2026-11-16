//! Regression test: a flight-recorder dump captured from a scripted reducer
//! session, replayed and checked against the exact effects that session
//! must produce, which the test states input by input, and against the
//! effect counts the dump recorded alongside each input (architecture
//! section 9, milestone M2 — "reducer replay tests built from live dumps").
//! The committed fixture pins this sequence forever; [`regenerate_fixture`]
//! below is how it was produced and how a future intentional change to the
//! scripted shapes regenerates it.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "reads its fixture file, and writes it when regenerated: listed under the determinism rule's exceptions"
)]

use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

mod desk;

use desk::{LISTENER, ONE, focus_read_of, focus_tell, listener_read, runtime_of};
use verbatim_core::{RecordedInput, SrState, dump, reduce, replay};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    Answer, Instance, Outcome, ProcessFact, QueueEffect, QueueInput, Reader, Reply,
    Request as Wire, RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, Effect, FocusNow, FocusValidity, Input, NodeDetails, NodeSnapshot, ObjectKey, Pid,
    Role, SegmentContent, SpeechPriority, State, StateSet, TraceId, Utterance, UtteranceSegment,
    UtteranceSource,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/focus_value_states.jsonl")
}

/// The slider "Rate" at 50, node 1 of the application [`ONE`].
fn slider() -> NodeSnapshot {
    NodeSnapshot {
        id: ONE.node(1),
        backend: Backend::Uia,
        role: Role::Slider,
        name: Some("Rate".to_owned()),
        value: Some("50".to_owned()),
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// `input` to Core's event queue at `at_us`, traced as `trace`.
fn queued(trace: u64, at_us: u64, input: QueueInput) -> Input {
    Input::Queue {
        trace_id: TraceId::from_raw(trace),
        at_us,
        input,
    }
}

/// The answer to request `number` from `from`, `reply`.
fn answer(from: Instance, number: u64, reply: Reply) -> QueueInput {
    QueueInput::Answer {
        from,
        answer: Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(reply),
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// The outpost's tell of a change of `property` to `value` on the slider,
/// which it follows as the committed focus.
fn followed(trace: u64, property: PropertyId, value: PropertyValue) -> QueueInput {
    QueueInput::Tell {
        from: ONE.instance(),
        tell: Box::new(Event {
            trace_id: TraceId::from_raw(trace),
            kind: EventKind::UiaPropertyChanged,
            process: ONE.pid,
            application: "app".to_owned(),
            thread: 41,
            at_us: trace * 1_000,
            object: EventObject::Followed {
                runtime_id: runtime_of(1),
            },
            cached: PropertyValues::default(),
            payload: Payload::PropertyChanged {
                property,
                value: Some(value),
            },
            window: TellWindow::NoWindow,
            checked: None,
        }),
    }
}

/// A focus, a value change, and a states change on the same node, a
/// slider tabbed to at 50, dragged to 55, then disabled, as Core's event
/// queue takes them from a start: the listener starting and ready, its
/// tell of the focus and its read of the element, the application's
/// outpost started on Core's request and ready, its completion of the
/// read, and its tells of the two changes. This is the session the
/// committed fixture pins.
fn scripted_inputs() -> Vec<Input> {
    vec![
        queued(
            1,
            1_000,
            QueueInput::Process(ProcessFact::ListenerStarted(1)),
        ),
        queued(2, 2_000, QueueInput::ReaderReady(LISTENER)),
        queued(
            3,
            3_000,
            QueueInput::Tell {
                from: LISTENER,
                tell: Box::new(focus_tell(ONE, 1, 3_000)),
            },
        ),
        queued(
            4,
            4_000,
            answer(LISTENER, 1, Reply::Object(Box::new(listener_read(ONE, 1)))),
        ),
        queued(
            5,
            5_000,
            QueueInput::Process(ProcessFact::OutpostStarted {
                process: ONE.pid,
                generation: ONE.generation,
            }),
        ),
        queued(6, 6_000, QueueInput::ReaderReady(ONE.instance())),
        queued(
            7,
            7_000,
            answer(
                ONE.instance(),
                2,
                Reply::Focus(Box::new(focus_read_of(ONE, &slider(), &[]))),
            ),
        ),
        queued(
            8,
            8_000,
            followed(8, PropertyId::Value, PropertyValue::Text("55".to_owned())),
        ),
        queued(
            9,
            9_000,
            followed(
                9,
                PropertyId::States,
                PropertyValue::States(StateSet::new().with(State::Disabled)),
            ),
        ),
    ]
}

/// One effect as this test compares it: a wake asked, an outpost asked
/// to start, a request sent by its reader, number and kind, an event given
/// to the control plane's stream by its trace, and expired speech dropped
/// or an utterance spoken whole. Every effect becomes exactly one of these,
/// so comparing the lists compares every effect.
#[derive(Debug, PartialEq)]
enum Seen {
    Wake(Option<u64>),
    Start(Pid),
    Sent(Reader, u64, &'static str),
    Observed(TraceId),
    Effect(Effect),
}

fn seen(effect: Effect) -> Seen {
    match effect {
        Effect::Queue(QueueEffect::WakeAt(wake)) => Seen::Wake(wake),
        Effect::Queue(QueueEffect::StartOutpost(process)) => Seen::Start(process),
        Effect::Queue(QueueEffect::Send { to, ask }) => Seen::Sent(
            to.reader,
            ask.number.0,
            match ask.request {
                Wire::ReadObject(_) => "ReadObject",
                Wire::CompleteUia(_) => "CompleteUia",
                Wire::Follow(_) => "Follow",
                other => panic!("a request this session does not send: {other:?}"),
            },
        ),
        Effect::Observed(observed) => Seen::Observed(observed.trace_id),
        effect @ (Effect::DropExpiredSpeech(_) | Effect::Speak(..)) => Seen::Effect(effect),
        other => panic!("an effect this session does not have: {other:?}"),
    }
}

/// The exact effects each scripted input must produce: the listener's
/// start and readiness ask nothing; the focus's tell asks the listener's
/// read, whose answer starts the outpost, whose readiness sends the
/// completion; the completion's answer drops expired speech, announces
/// the slider with its value and follows it; the drag speaks the new
/// value alone, and disabling it says "unavailable". Only the focus
/// announcement carries a focus validity.
fn expected_effects() -> Vec<Vec<Seen>> {
    let slider = ONE.node(1);
    let speech = |trace, segments, validity| {
        Seen::Effect(Effect::Speak(
            Utterance {
                trace_id: TraceId::from_raw(trace),
                priority: SpeechPriority::Queued,
                segments,
                source: Some(UtteranceSource {
                    role: Role::Slider,
                    rect: None,
                }),
                validity,
                say_all: false,
            },
            None,
        ))
    };
    let outpost = Reader::Outpost(ONE.pid);
    vec![
        vec![Seen::Wake(None)],
        vec![Seen::Wake(None)],
        vec![
            Seen::Sent(Reader::Listener, 1, "ReadObject"),
            Seen::Wake(Some(503_000)),
        ],
        vec![Seen::Start(ONE.pid), Seen::Wake(Some(13_003_000))],
        vec![Seen::Wake(Some(9_005_000))],
        vec![
            Seen::Sent(outpost, 2, "CompleteUia"),
            Seen::Wake(Some(506_000)),
        ],
        vec![
            Seen::Observed(TraceId::from_raw(7)),
            Seen::Effect(Effect::DropExpiredSpeech(FocusNow {
                focus: ObjectKey::of_id(slider),
                ancestors: Vec::new(),
                foreground: None,
                menu_item_of_focus: None,
            })),
            speech(
                7,
                vec![
                    UtteranceSegment::label("Rate"),
                    UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
                    UtteranceSegment::value("50"),
                ],
                Some(FocusValidity {
                    node: slider,
                    had_focus: true,
                    window: None,
                }),
            ),
            Seen::Sent(outpost, 3, "Follow"),
            Seen::Wake(Some(506_000)),
        ],
        vec![
            speech(8, vec![UtteranceSegment::value("55")], None),
            Seen::Wake(Some(506_000)),
        ],
        vec![
            speech(
                9,
                vec![UtteranceSegment::new(SegmentContent::State(
                    State::Disabled,
                ))],
                None,
            ),
            Seen::Wake(Some(506_000)),
        ],
    ]
}

#[test]
fn replays_the_committed_fixture_deterministically_to_the_exact_effects() {
    let file = fs::File::open(fixture_path()).expect("fixture file exists");
    let mut reader = BufReader::new(file);
    let contents = dump::read_dump(&mut reader).expect("fixture parses");

    assert!(!contents.truncated, "the committed fixture is complete");

    let inputs: Vec<Input> = contents.inputs.iter().map(|r| r.input.clone()).collect();
    assert_eq!(
        inputs,
        scripted_inputs(),
        "the fixture holds the scripted inputs"
    );

    let first = replay(&contents.base, &inputs);
    let second = replay(&contents.base, &inputs);
    assert_eq!(first, second, "replay is deterministic");
    let replayed: Vec<Vec<Seen>> = first
        .iter()
        .map(|effects| effects.iter().cloned().map(seen).collect())
        .collect();
    assert_eq!(replayed, expected_effects());

    let recorded_counts: Vec<usize> = contents.inputs.iter().map(|r| r.effect_count).collect();
    let replayed_counts: Vec<usize> = first.iter().map(Vec::len).collect();
    assert_eq!(
        replayed_counts, recorded_counts,
        "replayed effect counts must match what the live session recorded"
    );
}

/// Regenerates the committed fixture from [`scripted_inputs`] by actually
/// running each input through [`reduce`], exactly like a live reducer
/// thread would. Not part of the default test run: only an intentional
/// change to the scripted shapes above should touch the fixture, and that
/// change is reviewed as a diff to the committed file, not silently
/// regenerated by CI. Run explicitly after changing `scripted_inputs`:
/// `cargo test -p verbatim-core --test replay_fixture -- --ignored regenerate_fixture`.
#[test]
#[ignore = "regenerates the committed fixture; run explicitly, not part of the default suite"]
fn regenerate_fixture() {
    let mut state = SrState::new();
    let mut runtime = verbatim_core::Runtime::default();
    let mut recorded = Vec::new();
    for input in scripted_inputs() {
        let effects = reduce(&mut state, &mut runtime, &input);
        recorded.push(RecordedInput {
            input,
            effect_count: effects.len(),
        });
    }

    let mut buffer = Vec::new();
    dump::write_dump(
        &mut buffer,
        env!("CARGO_PKG_VERSION"),
        dump::UNKNOWN_COMMIT,
        false,
        "2026-07-14T00:00:00Z",
        &SrState::new(),
        &recorded,
    )
    .expect("writes");

    let path = fixture_path();
    fs::create_dir_all(path.parent().expect("fixtures dir")).expect("creates fixtures dir");
    fs::write(path, buffer).expect("writes fixture");
}
