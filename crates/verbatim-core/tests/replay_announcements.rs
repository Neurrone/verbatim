//! Replay fixtures for state changes and the focus line
//! (`docs/parity/changes-and-selection.md`, "State changes after the focus
//! has left"): flight recorder dumps of Core's inputs, in the listener's
//! and the outpost's order and with the gaps of real recordings, replayed
//! by `cargo xtask replay` and here, and checked against the speech NVDA
//! gave in the captures of the same applications (2026-11-19, five runs
//! alike).
//!
//! The sessions load their events' order and gaps from committed
//! recording excerpts (`tests/fixtures/recordings/`, each naming its
//! source on its first line): File Explorer's Properties sheet, whose
//! Apply button, read through MSAA, disables itself and moves the focus to
//! Read-only; and a WPF window, read through UI Automation, whose Apply
//! disables itself after focusing OK. What no recording gives is
//! code-level and said so on its session: every reader's answer, each
//! given 400 µs after its request. [`regenerate`] writes the fixtures from
//! the sessions.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "reads its fixture files, and writes them when regenerated: listed under the determinism rule's exceptions"
)]

use readers::Found;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;
use verbatim_model::request::Question;

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{App, Desk, LISTENER, ONE, TWO, focus_read_of, focus_tell, listener_read, runtime_of};
use verbatim_core::{SrState, dump, replay};
use verbatim_model::batch::{Done, StepOutcome};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    Answer, FocusRead, HeldIds, Instance, MsaaIds, NodeRead, Outcome, QueueInput, Reply,
    RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, Effect, NodeDetails, NodeSnapshot, Role, SegmentContent, State, StateSet, TraceId,
    WindowHandle,
};

/// How long after its request each reader's answer comes, code-level.
const ANSWER_US: u64 = 400;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jsonl"))
}

/// One event of a committed recording excerpt: `verbatim-event-record`'s
/// line, cut to what the session feeds.
#[derive(Clone, Debug)]
struct Recorded {
    /// When it was raised, in microseconds on the recording's clock.
    t_us: u64,
    /// Its kind, as the recording names it.
    kind: String,
    /// Its window, as the recording names it; `None` for an event raised
    /// with no window.
    window: Option<String>,
    /// Its MSAA object id; `None` for a UI Automation event.
    object: Option<i32>,
}

/// The recording excerpt `name`, in its order; its first line, naming its
/// source, left out.
fn recording(name: &str) -> Vec<Recorded> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recordings")
        .join(format!("{name}.jsonl"));
    let text = fs::read_to_string(path).expect("the recording excerpt");
    text.lines()
        .skip(1)
        .map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).expect("a recorded event");
            Recorded {
                t_us: event["t_us"].as_u64().expect("a time"),
                kind: event["kind"].as_str().expect("a kind").to_owned(),
                window: event["window"].as_str().map(str::to_owned),
                object: event["object"]
                    .as_i64()
                    .map(|object| i32::try_from(object).expect("an object id")),
            }
        })
        .collect()
}

/// A request out and the time its answer comes.
struct Due {
    at_us: u64,
    number: u64,
    to: Instance,
    request: Question,
}

/// A session's run: the desk, the requests whose answers are due, and the
/// answer each gets.
struct Run<'a> {
    desk: &'a mut Desk,
    due: Vec<Due>,
    answer: Box<dyn FnMut(&Question) -> Outcome + 'a>,
}

impl Run<'_> {
    /// Feeds `input` at `at_us`, the answers due before it first; each
    /// request it sends is answered [`ANSWER_US`] later, but a `Follow`,
    /// whose answer Core does not wait for.
    fn feed(&mut self, at_us: u64, input: QueueInput) {
        self.answer_until(at_us);
        self.step(at_us, input);
    }

    fn step(&mut self, at_us: u64, input: QueueInput) {
        let _ = self.desk.step_at(at_us, input);
        let now = self.desk.at_us;
        for (number, to, request) in self.desk.sent.clone() {
            if desk::kind(&request) == "Follow" {
                continue;
            }
            self.due.push(Due {
                at_us: now + ANSWER_US,
                number,
                to,
                request,
            });
        }
    }

    /// Answers every request due by `at_us`, in the order they are due.
    fn answer_until(&mut self, at_us: u64) {
        loop {
            let next = self
                .due
                .iter()
                .enumerate()
                .filter(|(_, due)| due.at_us <= at_us)
                .min_by_key(|(_, due)| due.at_us)
                .map(|(place, _)| place);
            let Some(place) = next else {
                return;
            };
            let due = self.due.remove(place);
            if self.desk.cancelled.contains(&due.number) {
                continue;
            }
            let outcome = (self.answer)(&due.request);
            self.step(
                due.at_us,
                QueueInput::Answer {
                    from: due.to,
                    answer: Answer {
                        number: RequestNumber(due.number),
                        outcome,
                        probes: Vec::new(),
                        detail: String::new(),
                    },
                },
            );
        }
    }
}

/// Node `number` of `app`, read through `backend`.
fn node(
    app: App,
    number: u64,
    (backend, role, name): (Backend, Role, &str),
    states: StateSet,
) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(number),
        backend,
        role,
        name: Some(name.to_owned()),
        value: None,
        states,
        details: NodeDetails::default(),
    }
}

fn focused() -> StateSet {
    StateSet::new().with(State::Focusable).with(State::Focused)
}

/// The listener's MSAA tell of `kind` on object `object` of window
/// `window` of `app`, raised at `at_us`.
fn msaa_tell(app: App, kind: EventKind, (window, object): (u64, i32), at_us: u64) -> Event {
    let mut facts = app.facts();
    facts.window.handle = WindowHandle(window);
    Event {
        kind,
        object: EventObject::Msaa {
            window: WindowHandle(window),
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        checked: None,
        window: TellWindow::Found(Box::new(facts)),
        trace_id: TraceId::from_raw(at_us),
        ..focus_tell(app, 1, at_us)
    }
}

/// `read` as an MSAA focus read by the event of `object` in `window`, the
/// control's own window, which its read names.
fn by_ids(mut read: FocusRead, window: u64) -> FocusRead {
    read.focus.read.window.handle = WindowHandle(window);
    read.msaa = Some(HeldIds {
        event: MsaaIds {
            window: WindowHandle(window),
            object: -4,
            child: 0,
        },
        held_child: 0,
    });
    read
}

/// The handle the desk gives the recording's window `window`: each in the
/// order first seen, after the application's top-level window and its
/// pane.
fn handle_of(seen: &mut Vec<String>, window: &str) -> u64 {
    let place = if let Some(place) = seen.iter().position(|known| known == window) {
        place
    } else {
        seen.push(window.to_owned());
        seen.len() - 1
    };
    ONE.top + 2 + u64::try_from(place).expect("a few windows")
}

/// File Explorer's Properties sheet (`ann_button_win32_run1`), read
/// through MSAA: Tab to Apply, its focus read and spoken, a state change
/// of the button the default left and one of Apply's; then Space, Apply's
/// state changes as it is pressed and as it disables itself, a focus with
/// no window, which the listener tells on the desktop window (served by
/// the desktop's process, `TWO` standing for it), a state change of
/// Apply's window object, which the hook takes as its client's, one of OK,
/// and the focus on Read-only. Code-level: each read of Apply's state
/// answers its states unchanged until the change that disables it, whose
/// read and the next answer it disabled without the focused state, and the
/// run's read answers Read-only focused, in the "General" page. NVDA, in
/// all five captured runs: "unavailable", the page, then Read-only.
fn win32_apply(desk: &mut Desk) {
    let events = recording("ann_button_win32_run1");
    let mut seen = Vec::new();
    let sheet = node(
        ONE,
        50,
        (Backend::Msaa, Role::Dialog, "apply.txt Properties"),
        StateSet::new(),
    );
    let page = node(
        ONE,
        51,
        (Backend::Msaa, Role::PropertyPage, "General"),
        StateSet::new(),
    );
    let apply = node(ONE, 52, (Backend::Msaa, Role::Button, "Apply"), focused());
    let read_only = node(
        ONE,
        53,
        (Backend::Msaa, Role::CheckBox, "Read-only"),
        focused(),
    );
    let apply_window = handle_of(&mut seen, events[0].window.as_deref().expect("a window"));
    let read_only_window = handle_of(
        &mut seen,
        events
            .iter()
            .rev()
            .find(|event| event.kind == "msaa_focus")
            .and_then(|event| event.window.as_deref())
            .expect("Read-only's window"),
    );
    // The runs read, and the reads of Apply's state changes: the fourth,
    // of the change that disables it, and the next find it disabled.
    let mut runs = 0_u32;
    let mut reads = 0_u32;
    let apply_states = |reads: u32| {
        if reads < 4 {
            focused()
        } else {
            StateSet::new().with(State::Focusable).with(State::Disabled)
        }
    };
    let mut run = Run {
        desk,
        due: Vec::new(),
        answer: Box::new(|request| match request {
            Question::Batch(batch) if runs == 0 => {
                runs += 1;
                readers::outcome(
                    batch,
                    &Found::Read(by_ids(
                        focus_read_of(ONE, &apply, std::slice::from_ref(&sheet)),
                        apply_window,
                    )),
                )
            }
            Question::Batch(batch) if desk::kind(request) == "ReadObject" => {
                reads += 1;
                Outcome::Result(Reply::Steps(vec![
                    StepOutcome::Done(Done::IsOneOf(Some(0))),
                    StepOutcome::Done(Done::Read(NodeRead {
                        node: None,
                        window: ONE.node_window(),
                        values: PropertyValues(vec![PropertyAnswer {
                            id: PropertyId::States,
                            value: Some(PropertyValue::States(apply_states(reads))),
                        }]),
                    })),
                ]))
            }
            Question::Batch(batch) => readers::outcome(
                batch,
                &Found::Read(by_ids(
                    focus_read_of(ONE, &read_only, &[sheet.clone(), page.clone()]),
                    read_only_window,
                )),
            ),
        }),
    };
    let first = events[0].t_us;
    for event in &events {
        let at_us = 10_000 + event.t_us - first;
        let tell = win32_tell(&mut seen, event, at_us);
        run.feed(
            at_us,
            QueueInput::Tell {
                from: LISTENER,
                tell: Box::new(tell),
            },
        );
    }
    run.answer_until(u64::MAX);
}

/// The listener's tell of `event`, an MSAA focus or state change of the
/// recording, raised at `at_us`: the hook takes a window object's event as
/// its client's, and the listener tells an event with no window on the
/// desktop window, served by the desktop's process (`TWO`).
fn win32_tell(seen: &mut Vec<String>, event: &Recorded, at_us: u64) -> Event {
    let kind = match event.kind.as_str() {
        "msaa_focus" => EventKind::MsaaFocus,
        "msaa_state" => EventKind::StateChange,
        other => panic!("a kind the session does not feed: {other}"),
    };
    let object = match event.object {
        Some(0) => -4,
        Some(object) => object,
        None => panic!("an MSAA event's object"),
    };
    if let Some(window) = &event.window {
        return msaa_tell(ONE, kind, (handle_of(seen, window), object), at_us);
    }
    let mut tell = msaa_tell(TWO, kind, (10, object), at_us);
    if let TellWindow::Found(facts) = &mut tell.window {
        facts.desktop = true;
    }
    tell
}

/// A WPF window (`ann_button_wpf_run1`), read through UI Automation: Tab
/// to Apply, its focus read and spoken; then Space, OK's focus told by the
/// listener and Apply's `IsEnabled` change, which the outpost following
/// Apply tells 1.9 ms later, as recorded. Code-level: the reads of each
/// focus, and the change's states, disabled without the focused state, as
/// the outpost maps them from the event's cache. NVDA, in all five
/// captured runs: "OK, button" alone, the change handled once OK is its
/// focus.
fn wpf_apply(desk: &mut Desk) {
    let events = recording("ann_button_wpf_run1");
    let apply = node(ONE, 1, (Backend::Uia, Role::Button, "Apply"), focused());
    let ok = node(ONE, 2, (Backend::Uia, Role::Button, "OK"), focused());
    let number_of = |runtime_id: &str| -> u64 {
        match runtime_id {
            "7.1444.57846722" => 1,
            "7.1444.45850844" => 2,
            other => panic!("an element the session does not know: {other}"),
        }
    };
    let ids: Vec<String> = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/recordings/ann_button_wpf_run1.jsonl"),
    )
    .expect("the recording excerpt")
    .lines()
    .skip(1)
    .map(|line| {
        let event: serde_json::Value = serde_json::from_str(line).expect("a recorded event");
        event["runtime_id"]
            .as_str()
            .expect("a runtime id")
            .to_owned()
    })
    .collect();
    let focus = std::cell::Cell::new(0);
    let mut run = Run {
        desk,
        due: Vec::new(),
        answer: Box::new(|request| match (request, desk::kind(request)) {
            (Question::Batch(batch), "ReadObject") => {
                readers::outcome(batch, &listener_read(ONE, focus.get()))
            }
            (Question::Batch(batch), "Completion") => readers::outcome(
                batch,
                &Found::Read(focus_read_of(
                    ONE,
                    if focus.get() == 1 { &apply } else { &ok },
                    &[],
                )),
            ),
            (other, _) => panic!("a request the session does not answer: {other:?}"),
        }),
    };
    let first = events[0].t_us;
    for (event, runtime_id) in events.iter().zip(&ids) {
        let at_us = 10_000 + event.t_us - first;
        let number = number_of(runtime_id);
        match event.kind.as_str() {
            "uia_focus" => {
                run.answer_until(at_us);
                focus.set(number);
                run.feed(
                    at_us,
                    QueueInput::Tell {
                        from: LISTENER,
                        tell: Box::new(Event {
                            trace_id: TraceId::from_raw(at_us),
                            ..focus_tell(ONE, number, at_us)
                        }),
                    },
                );
            }
            "uia_property" => {
                let disabled = StateSet::new().with(State::Focusable).with(State::Disabled);
                let tell = Event {
                    trace_id: TraceId::from_raw(at_us),
                    kind: EventKind::UiaPropertyChanged,
                    process: ONE.pid,
                    application: "app".to_owned(),
                    thread: 41,
                    at_us,
                    object: EventObject::Followed {
                        runtime_id: runtime_of(number),
                    },
                    cached: PropertyValues(vec![PropertyAnswer {
                        id: PropertyId::HasTextPattern,
                        value: Some(PropertyValue::Flag(false)),
                    }]),
                    payload: Payload::PropertyChanged {
                        property: PropertyId::States,
                        value: Some(PropertyValue::States(disabled)),
                    },
                    window: TellWindow::NoWindow,
                    checked: None,
                };
                run.feed(
                    at_us,
                    QueueInput::Tell {
                        from: ONE.instance(),
                        tell: Box::new(tell),
                    },
                );
            }
            other => panic!("a kind the session does not feed: {other}"),
        }
    }
    run.answer_until(u64::MAX);
}

type Session = fn(&mut Desk);

fn sessions() -> Vec<(&'static str, Session)> {
    vec![
        ("announcements_button_win32", win32_apply),
        ("announcements_button_wpf", wpf_apply),
    ]
}

/// The speech of a replay's effects, utterance by utterance, each its
/// segments.
fn speech(effects: &[Vec<Effect>]) -> Vec<Vec<String>> {
    effects
        .iter()
        .flatten()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

/// The committed fixture `name`, replayed: its speech, after checking that
/// replay is deterministic and has the effect counts recorded.
fn replayed(name: &str) -> Vec<Vec<String>> {
    let file = fs::File::open(fixture_path(name)).expect("fixture file exists");
    let contents = dump::read_dump(&mut BufReader::new(file)).expect("fixture parses");
    assert!(!contents.truncated, "the committed fixture is complete");
    let inputs: Vec<_> = contents.inputs.iter().map(|r| r.input.clone()).collect();
    let first = replay(&contents.base, &inputs);
    assert_eq!(
        first,
        replay(&contents.base, &inputs),
        "replay is deterministic"
    );
    let counts: Vec<usize> = first.iter().map(Vec::len).collect();
    let recorded: Vec<usize> = contents.inputs.iter().map(|r| r.effect_count).collect();
    assert_eq!(counts, recorded, "the effect counts the session recorded");
    speech(&first)
}

fn said(lines: &[&[&str]]) -> Vec<Vec<String>> {
    lines
        .iter()
        .map(|line| line.iter().map(|segment| (*segment).to_owned()).collect())
        .collect()
}

/// Space on Explorer's Apply says "unavailable", then the page and
/// Read-only, as NVDA says them.
#[test]
fn explorers_apply_disabling_itself_says_unavailable_then_read_only() {
    assert_eq!(
        replayed("announcements_button_win32"),
        said(&[
            &["Label(\"apply.txt Properties\")", "Role(Dialog)"],
            &["Label(\"Apply\")", "Role(Button)"],
            &["State(Disabled)"],
            &["Label(\"General\")", "Role(PropertyPage)"],
            &[
                "Label(\"Read-only\")",
                "Role(CheckBox)",
                "NegatedState(Checked)"
            ],
        ])
    );
}

/// Space on the WPF window's Apply says "OK button" alone, as NVDA does.
#[test]
fn a_wpf_apply_disabling_itself_after_focusing_ok_says_only_ok() {
    assert_eq!(
        replayed("announcements_button_wpf"),
        said(&[
            &["Label(\"Apply\")", "Role(Button)"],
            &["Label(\"OK\")", "Role(Button)"],
        ])
    );
}

/// Regenerates the committed fixtures from the sessions, as a live
/// reducer thread runs each input. Not part of the default run: only an
/// intentional change to a session touches its fixture, reviewed as a
/// diff. `cargo test -p verbatim-core --test replay_announcements -- --ignored regenerate`.
#[test]
#[ignore = "regenerates the committed fixtures; run explicitly"]
fn regenerate() {
    for (name, session) in sessions() {
        let mut desk = Desk::new();
        session(&mut desk);
        let mut buffer = Vec::new();
        dump::write_dump(
            &mut buffer,
            env!("CARGO_PKG_VERSION"),
            dump::UNKNOWN_COMMIT,
            false,
            "2026-11-19T00:00:00Z",
            &SrState::new(),
            &desk.recorded,
        )
        .expect("writes");
        let path = fixture_path(name);
        fs::create_dir_all(path.parent().expect("fixtures dir")).expect("creates fixtures dir");
        fs::write(path, buffer).expect("writes fixture");
    }
}
