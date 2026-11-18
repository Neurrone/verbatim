//! Replay fixtures for the one definition of focus validity: flight
//! recorder dumps of Core's inputs, in the listener's order and with the
//! gaps of real recordings, replayed by `cargo xtask replay` and here, and
//! checked against their exact speech.
//!
//! Each fixture's order comes from a recording, named on its session:
//! Verbatim's settings dialog raising its list's own focus twice (rec
//! `settings_dialog_keys`, passes 311 and 313); mockapp's list on a mouse
//! return (`validity-list-button`, run 1); five Tabs in a Properties sheet
//! (NVDA capture C7, run 1); a UI Automation focus raised while Windows
//! still names the old window (change B, the order of requirements fact
//! 4); Notepad's Open dialog, its folder tree's stale MSAA focus behind a
//! Left Arrow in the empty "File name" field (survey `np_open`); and the
//! Save As dialog, the tree's stale focus before Shift+Right's caret event
//! (`validity-saveas-control`, run 1). Where a recording cannot place an
//! event, such as a key inside a 100 ms watch, the placement is code-level
//! and said so on the session. The readers' answers are written by hand.
//! [`regenerate`] writes the fixtures from the sessions.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "reads its fixture files, and writes them when regenerated: listed under the determinism rule's exceptions"
)]

use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

mod desk;

use desk::{App, Desk, LISTENER, ONE, TWO, focus_read_of, focus_tell};
use verbatim_core::{SrState, dump, replay};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, CaretBinding, FocusCaret, FocusCaretRead, FocusRead,
    ForegroundReport, HeldIds, MsaaIds, Outcome, Press, PressAction, QueueInput, Redirect, Reply,
};
use verbatim_model::stroke::{HeldStroke, Stroke};
use verbatim_model::tell::{Event, EventObject, Relation, TellWindow};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReport, CaretSelection, Effect, KeySeq, NodeDetails,
    NodeSnapshot, Role, SegmentContent, Selection, SelectionChange, SelectionDifference,
    SelectionReadReply, Started, StateSet, TextAnchor, TextChunk, TextPosition, TextReply,
    TextUnit, Watched, WatchedRead, WindowHandle,
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jsonl"))
}

/// An MSAA node of `app`, its node `number`, of `role`, named `name`.
fn msaa_node(app: App, number: u64, role: Role, name: Option<&str>) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(number),
        backend: Backend::Msaa,
        role,
        name: name.map(str::to_owned),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// `node` as item `position` of `size`.
fn item(mut node: NodeSnapshot, position: u32, size: u32) -> NodeSnapshot {
    node.details.position_in_set = Some(position);
    node.details.set_size = Some(size);
    node
}

/// The listener's MSAA tell of `kind` on (`window`, `object`, `child`) of
/// `app`, raised at `at_us`.
fn msaa_tell(
    app: App,
    kind: EventKind,
    (window, object, child): (u64, i32, i32),
    at_us: u64,
) -> Event {
    let mut facts = app.facts();
    facts.window.handle = WindowHandle(window);
    Event {
        kind,
        object: EventObject::Msaa {
            window: WindowHandle(window),
            object,
            child,
        },
        cached: verbatim_model::property::PropertyValues::default(),
        checked: None,
        window: TellWindow::Found(Box::new(facts)),
        trace_id: verbatim_model::TraceId::from_raw(at_us),
        ..focus_tell(app, 1, at_us)
    }
}

/// The ids of (`window`, `object`, `child`).
fn ids(window: u64, object: i32, child: i32) -> MsaaIds {
    MsaaIds {
        window: WindowHandle(window),
        object,
        child,
    }
}

/// `read` as an MSAA focus read by `event`, held by its child id.
fn by_ids(mut read: FocusRead, event: MsaaIds) -> FocusRead {
    read.msaa = Some(HeldIds {
        event,
        held_child: event.child,
    });
    read
}

/// The numbers of the requests the desk answered.
fn answered(desk: &Desk) -> Vec<u64> {
    desk.recorded
        .iter()
        .filter_map(|recorded| match &recorded.input {
            verbatim_model::Input::Queue {
                input: QueueInput::Answer { answer, .. },
                ..
            } => Some(answer.number.0),
            _ => None,
        })
        .collect()
}

/// The one request of `kind` out, not yet answered, with its reader.
fn out_of(desk: &Desk, kind: &str) -> (u64, verbatim_model::request::Instance) {
    let done = answered(desk);
    let out: Vec<_> = desk
        .asked
        .iter()
        .filter(|(number, _, request)| {
            desk::kind(request) == kind
                && !done.contains(number)
                && !desk.cancelled.contains(number)
        })
        .map(|(number, to, _)| (*number, *to))
        .collect();
    let [one] = out[..] else {
        panic!("one {kind} out: {out:?}");
    };
    one
}

/// The one candidate out, with its reader.
fn run_sent(desk: &Desk) -> (u64, verbatim_model::request::Instance) {
    out_of(desk, "MsaaRun")
}

/// The one request of `kind` out.
fn sent_of(desk: &Desk, kind: &str) -> u64 {
    out_of(desk, kind).0
}

/// Answers the one candidate the last step asked with `answer` at `at_us`.
fn answer_run(desk: &mut Desk, at_us: u64, answer: CandidateAnswer) {
    let (number, to) = run_sent(desk);
    desk.step_at(
        at_us,
        QueueInput::Answer {
            from: to,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(number),
                outcome: Outcome::Result(Reply::Candidate(answer)),
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
}

/// Answers request `number` of `app`'s outpost with `reply` at `at_us`.
fn answer_at(desk: &mut Desk, at_us: u64, app: App, number: u64, reply: Reply) {
    desk.step_at(
        at_us,
        QueueInput::Answer {
            from: app.instance(),
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(number),
                outcome: Outcome::Result(reply),
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
}

/// Facts of a candidate found, focused or not, with no redirect.
fn found(focused: bool) -> CandidateFacts {
    CandidateFacts::Found {
        focused: Some(focused),
        role: None,
        window_class: String::new(),
        redirect: None,
    }
}

/// A candidate found focused, read in full, redirected by NVDA's list rule
/// to `child`.
fn listed(read: FocusRead, child: MsaaIds) -> CandidateAnswer {
    CandidateAnswer {
        facts: CandidateFacts::Found {
            focused: Some(true),
            role: None,
            window_class: "SysListView32".to_owned(),
            redirect: Some((Redirect::List, child)),
        },
        read: Some(Box::new(read)),
    }
}

/// A candidate found without the focused state, not read further.
fn unfocused() -> CandidateAnswer {
    CandidateAnswer {
        facts: found(false),
        read: None,
    }
}

/// A tell from the listener at `at_us`.
fn tell_at(desk: &mut Desk, at_us: u64, tell: Event) {
    desk.step_at(
        at_us,
        QueueInput::Tell {
            from: LISTENER,
            tell: Box::new(tell),
        },
    );
}

/// The settings dialog's list, rec `settings_dialog_keys` passes 309 to
/// 313: the dialog's foreground (refused with no read, no report of it in
/// front yet), the list's own focus at 2 798.517 ms, read alone with Core
/// idle and redirected to "Speech", and its second own focus 1.489 ms
/// later, arriving while that read is out, redirected to the same item:
/// "Speech 1 of 3" once.
fn settings_list(desk: &mut Desk) {
    let list = ONE.top + 1;
    tell_at(
        desk,
        2_797_075,
        msaa_tell(ONE, EventKind::Foreground, (ONE.top, 0, 0), 2_797_075),
    );
    tell_at(
        desk,
        2_798_517,
        msaa_tell(ONE, EventKind::MsaaFocus, (list, -4, 0), 2_798_517),
    );
    tell_at(
        desk,
        2_800_006,
        msaa_tell(ONE, EventKind::MsaaFocus, (list, -4, 0), 2_800_006),
    );
    let speech = item(msaa_node(ONE, 11, Role::ListItem, Some("Speech")), 1, 3);
    let categories = msaa_node(ONE, 10, Role::List, Some("Categories:"));
    let child = ids(list, -4, 1);
    let read = by_ids(focus_read_of(ONE, &speech, &[categories]), child);
    answer_run(desk, 2_800_300, listed(read.clone(), child));
    answer_run(desk, 2_800_700, listed(read, child));
}

/// mockapp's list on a mouse return (`validity-list-button`, run 1): a
/// click on the Apply button, its focus at 30 ms; the click back on the
/// list, its own focus at 1 532 ms, then its item's 0.1 ms later, NVDA
/// saying "list", "Alpha 1 of 3": the item said once.
fn mouse_return(desk: &mut Desk) {
    let button = ONE.top + 2;
    let list = ONE.top + 1;
    tell_at(
        desk,
        30_000,
        msaa_tell(ONE, EventKind::MsaaFocus, (button, -4, 0), 30_000),
    );
    let apply = msaa_node(ONE, 20, Role::Button, Some("Apply"));
    let read = by_ids(focus_read_of(ONE, &apply, &[]), ids(button, -4, 0));
    answer_run(desk, 31_000, CandidateAnswer::focused(read));
    tell_at(
        desk,
        1_532_000,
        msaa_tell(ONE, EventKind::MsaaFocus, (list, -4, 0), 1_532_000),
    );
    tell_at(
        desk,
        1_532_100,
        msaa_tell(ONE, EventKind::MsaaFocus, (list, -4, 1), 1_532_100),
    );
    let alpha = item(msaa_node(ONE, 31, Role::ListItem, Some("Alpha")), 1, 3);
    let child = ids(list, -4, 1);
    let read = by_ids(
        focus_read_of(ONE, &alpha, &[msaa_node(ONE, 30, Role::List, None)]),
        child,
    );
    answer_run(desk, 1_533_000, listed(read, child));
}

/// Five Tabs in a Properties sheet (capture C7, run 1): the flush 23 ms
/// after the last Tab, a Button, then a second Button, the tab control's
/// own focus and its item's. The first Button, split off by a state change
/// between the two, is read alone and found without the focused state;
/// the rest wait as one run, its newest, the tab item, read and spoken:
/// "tab control", "General tab selected". The Tab keys themselves are not
/// in the fixture: they reach Core from the hook, apart from the listener.
fn five_tabs(desk: &mut Desk) {
    let tab = ONE.top + 13;
    tell_at(
        desk,
        23_000,
        msaa_tell(ONE, EventKind::MsaaFocus, (ONE.top + 11, -4, 0), 23_000),
    );
    tell_at(
        desk,
        23_010,
        msaa_tell(ONE, EventKind::MsaaFocus, (ONE.top + 12, -4, 0), 23_010),
    );
    tell_at(
        desk,
        23_020,
        msaa_tell(ONE, EventKind::MsaaFocus, (tab, -4, 0), 23_020),
    );
    tell_at(
        desk,
        23_030,
        msaa_tell(ONE, EventKind::MsaaFocus, (tab, -4, 1), 23_030),
    );
    answer_run(desk, 31_000, unfocused());
    let mut general = msaa_node(ONE, 41, Role::Tab, Some("General"));
    general.states = StateSet::new().with(verbatim_model::State::Selected);
    let control = msaa_node(ONE, 40, Role::TabControl, None);
    let read = by_ids(focus_read_of(ONE, &general, &[control]), ids(tab, -4, 1));
    answer_run(desk, 42_000, CandidateAnswer::focused(read));
}

/// Change B's order: a second application's UI Automation focus raised
/// while Windows still names the first's window, sent at once past the
/// listener's hold, reaching Core before the report of the new window in
/// front and its foreground tell. It is refused as unrelated; the title
/// comes from the foreground once in front.
fn uia_past_the_hold(desk: &mut Desk) {
    // The first application in front, its focus read at startup.
    desk.step_at(
        10_000,
        QueueInput::Foreground(ForegroundReport {
            window: ONE.top_level(),
            root_owner: ONE.top_level(),
            process: ONE.pid,
            thread: 41,
            class: "Frame".to_owned(),
        }),
    );
    let ok = msaa_node(ONE, 1, Role::Button, Some("OK"));
    let number = sent_of(desk, "CurrentFocus");
    answer_at(
        desk,
        11_000,
        ONE,
        number,
        Reply::Candidate(CandidateAnswer::focused(by_ids(
            focus_read_of(ONE, &ok, &[]),
            ids(ONE.top + 1, -4, 0),
        ))),
    );
    desk.app(TWO);
    let mut early = focus_tell(TWO, 2, desk.at_us + 1_000);
    if let TellWindow::Found(facts) = &mut early.window {
        facts.foreground = ONE.top_level();
        facts.foreground_root_owner = ONE.top_level();
        facts.window_to_foreground = Relation::Outside;
        facts.root_owner_to_foreground = Relation::Outside;
        facts.root_owner_is_foregrounds = false;
        facts.active_window = ONE.top_level();
        facts.window_to_active = Relation::Outside;
    }
    let at = desk.at_us + 1_000;
    tell_at(desk, at, early);
    desk.step_at(
        at + 50_900,
        QueueInput::Foreground(ForegroundReport {
            window: TWO.top_level(),
            root_owner: TWO.top_level(),
            process: TWO.pid,
            thread: 41,
            class: "Frame".to_owned(),
        }),
    );
    tell_at(
        desk,
        at + 51_000,
        msaa_tell(TWO, EventKind::Foreground, (TWO.top, 0, 0), at + 51_000),
    );
    let mut settings = msaa_node(TWO, 1, Role::Window, Some("Settings"));
    settings.details.window = Some(TWO.top_level());
    let read = by_ids(focus_read_of(TWO, &settings, &[]), ids(TWO.top, 0, 0));
    answer_run(desk, at + 52_000, CandidateAnswer::focused(read));
}

/// The "File name" edit of the Open dialog committed through MSAA, its
/// caret at the start of its empty text.
fn file_name(desk: &mut Desk) {
    let edit = ONE.top + 1;
    tell_at(
        desk,
        75_000,
        msaa_tell(ONE, EventKind::MsaaFocus, (edit, -4, 0), 75_000),
    );
    let field = msaa_node(ONE, 2, Role::EditableText, Some("File name:"));
    let mut read = by_ids(focus_read_of(ONE, &field, &[]), ids(edit, -4, 0));
    read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
        caret: caret_on(empty_line(), None),
        selected_text: None,
        baseline: TextAnchor(70),
    }));
    answer_run(desk, 76_000, CandidateAnswer::focused(read));
}

fn empty_line() -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: String::new(),
        start: TextAnchor(7),
        offset: 0,
        languages: Vec::new(),
        first: true,
        last: true,
        truncated: false,
        formats: Vec::new(),
    }
}

fn caret_on(line: TextChunk, selection: Option<CaretSelection>) -> CaretReport {
    CaretReport {
        line,
        selection,
        read_at_us: 0,
    }
}

/// Left Arrow, held, bound to the committed field, pressed at `at_us`.
fn left_arrow(desk: &mut Desk, at_us: u64) {
    let bound: Option<Box<CaretBinding>> = desk.state.caret_binding().map(Box::new);
    desk.keys += 1;
    let key = desk.keys;
    desk.step_at(
        at_us,
        QueueInput::Press(Box::new(Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: at_us,
            action: PressAction::CaretKey {
                key: CaretKey {
                    motion: CaretMotion::PreviousCharacter,
                    select: false,
                },
                held: Some(HeldStroke {
                    stroke: Stroke {
                        vk: 0x25,
                        scan_code: 0x4B,
                        extended: true,
                        sequence: u32::try_from(key).expect("a few keys"),
                    },
                    modifiers: Vec::new(),
                }),
                bound,
            },
        })),
    );
}

/// The key's starting read answered at `at_us`: the caret at the start of
/// the empty text.
fn started(desk: &mut Desk, at_us: u64) {
    let number = sent_of(desk, "Text");
    let mut unit = empty_line();
    unit.unit = TextUnit::Character;
    answer_at(
        desk,
        at_us,
        ONE,
        number,
        Reply::Text(TextReply::Started(Box::new(Started {
            point: TextAnchor(50),
            caret: caret_on(empty_line(), None),
            unit,
            word: None,
        }))),
    );
}

/// The folder tree's stale MSAA focus, child 16 of its tree view, at
/// `at_us`.
fn stale_tree(desk: &mut Desk, at_us: u64) {
    tell_at(
        desk,
        at_us,
        msaa_tell(ONE, EventKind::MsaaFocus, (ONE.top + 5, -4, 16), at_us),
    );
}

/// The Open dialog (survey `np_open`): "File name:" committed at 75 ms
/// after Control+O; the tree's stale focus 200 ms after the edit's, within
/// the recorded 124 to 432 ms. Left Arrow in the empty field is placed
/// code-level, its watch out when the tree's focus arrives. The tree's
/// confirmation answered `focused`: not focused, the watch runs out its
/// bound and the arrow says "blank", as NVDA in all 18 captured runs;
/// focused, the arrow is silent and the tree is read at its turn.
fn open_dialog(desk: &mut Desk, focused: bool) {
    file_name(desk);
    left_arrow(desk, 260_000);
    started(desk, 262_000);
    stale_tree(desk, 275_000);
    let (number, _) = run_sent(desk);
    answer_at(
        desk,
        277_000,
        ONE,
        number,
        Reply::Candidate(CandidateAnswer {
            facts: found(focused),
            read: None,
        }),
    );
    if focused {
        let tree_item = msaa_node(ONE, 60, Role::TreeItem, Some("This PC"));
        let read = by_ids(
            focus_read_of(ONE, &tree_item, &[msaa_node(ONE, 59, Role::Tree, None)]),
            ids(ONE.top + 5, -4, 16),
        );
        answer_run(desk, 280_000, CandidateAnswer::focused(read));
        return;
    }
    let watch = sent_of(desk, "Text");
    let mut unit = empty_line();
    unit.unit = TextUnit::Character;
    answer_at(
        desk,
        desk.at_us + 1_000 + 100_000,
        ONE,
        watch,
        Reply::Text(TextReply::Watched(Box::new(Watched::Bound(WatchedRead {
            caret: caret_on(empty_line(), None),
            unit,
        })))),
    );
    // At its turn the tree's stored refusal asks nothing.
}

/// The same, a second Left Arrow arriving while the tree's confirmation
/// is out: the confirmation is withdrawn and the first arrow is silent.
fn open_dialog_second_arrow(desk: &mut Desk) {
    file_name(desk);
    left_arrow(desk, 260_000);
    started(desk, 262_000);
    stale_tree(desk, 275_000);
    left_arrow(desk, 276_000);
}

/// The Save As dialog (`validity-saveas-control`, run 1): "File name:"
/// holding "np.txt" committed at 255 ms, the tree's stale focus at 389 ms,
/// read at its turn and found without the focused state, and Shift+Right
/// at 2 002 ms, its caret event at 2 004 ms: "n selected", as NVDA said.
fn save_as(desk: &mut Desk) {
    let edit = ONE.top + 1;
    tell_at(
        desk,
        255_000,
        msaa_tell(ONE, EventKind::MsaaFocus, (edit, -4, 0), 255_000),
    );
    let field = msaa_node(ONE, 2, Role::EditableText, Some("File name:"));
    let mut line = empty_line();
    "np.txt".clone_into(&mut line.text);
    let mut read = by_ids(focus_read_of(ONE, &field, &[]), ids(edit, -4, 0));
    read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
        caret: caret_on(line.clone(), None),
        selected_text: None,
        baseline: TextAnchor(70),
    }));
    answer_run(desk, 256_000, CandidateAnswer::focused(read));
    stale_tree(desk, 389_000);
    answer_run(desk, 390_000, unfocused());
    let mut caret = msaa_tell(ONE, EventKind::SystemCaret, (edit, -8, 0), 2_004_000);
    caret.kind = EventKind::SystemCaret;
    tell_at(desk, 2_004_000, caret);
    let number = sent_of(desk, "Text");
    let selected = SelectionChange {
        selected: true,
        text: "n".to_owned(),
        characters: 1,
    };
    answer_at(
        desk,
        2_005_000,
        ONE,
        number,
        Reply::Text(TextReply::SelectionRead(Box::new(SelectionReadReply {
            caret: caret_on(
                line,
                Some(CaretSelection {
                    range: Selection {
                        start: TextPosition {
                            anchor: TextAnchor(7),
                            offset: 0,
                        },
                        end: TextPosition {
                            anchor: TextAnchor(7),
                            offset: 1,
                        },
                    },
                }),
            ),
            baseline: TextAnchor(71),
            difference: Some(SelectionDifference {
                changes: vec![selected.clone()],
                new_selection: Some(selected),
            }),
            edit_selection: None,
        }))),
    );
}

/// Every fixture: its name and its session.
/// A session, which drives a desk through a recorded order.
type Session = fn(&mut Desk);

fn sessions() -> Vec<(&'static str, Session)> {
    vec![
        ("validity_settings_list", settings_list),
        ("validity_mouse_return", mouse_return),
        ("validity_five_tabs", five_tabs),
        ("validity_uia_past_the_hold", uia_past_the_hold),
        ("validity_open_dialog_unfocused", |desk| {
            open_dialog(desk, false);
        }),
        ("validity_open_dialog_focused", |desk| {
            open_dialog(desk, true);
        }),
        (
            "validity_open_dialog_second_arrow",
            open_dialog_second_arrow,
        ),
        ("validity_save_as", save_as),
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

#[test]
fn the_settings_lists_second_own_focus_says_the_item_once() {
    assert_eq!(
        replayed("validity_settings_list"),
        said(&[
            &["Label(\"Categories:\")", "Role(List)"],
            &[
                "Label(\"Speech\")",
                "Position { position: 1, set_size: Some(3) }"
            ],
        ])
    );
}

#[test]
fn the_mouse_return_says_the_lists_item_once() {
    assert_eq!(
        replayed("validity_mouse_return"),
        said(&[
            &["Label(\"Apply\")", "Role(Button)"],
            &["Role(List)"],
            &[
                "Label(\"Alpha\")",
                "Position { position: 1, set_size: Some(3) }"
            ],
        ])
    );
}

#[test]
fn five_tabs_say_the_tab_control_and_the_selected_tab() {
    assert_eq!(
        replayed("validity_five_tabs"),
        said(&[
            &["Role(TabControl)"],
            &["Label(\"General\")", "Role(Tab)", "State(Selected)"],
        ])
    );
}

#[test]
fn a_uia_focus_past_the_hold_says_nothing_and_the_foreground_its_title() {
    assert_eq!(
        replayed("validity_uia_past_the_hold"),
        said(&[
            &["Label(\"OK\")", "Role(Button)"],
            &["Label(\"Settings\")", "Role(Window)"],
        ])
    );
}

/// The field's focus says its empty text, "blank"; then the arrow's.
#[test]
fn the_open_dialogs_arrow_says_blank_when_the_tree_is_not_focused() {
    assert_eq!(
        replayed("validity_open_dialog_unfocused"),
        said(&[
            &["Label(\"File name:\")", "Role(EditableText)"],
            &["Message(Blank)"],
            &["Message(Blank)"],
        ])
    );
}

#[test]
fn the_open_dialogs_arrow_is_silent_when_the_tree_is_focused() {
    assert_eq!(
        replayed("validity_open_dialog_focused"),
        said(&[
            &["Label(\"File name:\")", "Role(EditableText)"],
            &["Message(Blank)"],
            &["Role(Tree)"],
            &["Label(\"This PC\")"],
        ])
    );
}

#[test]
fn a_second_arrow_while_the_tree_is_confirmed_leaves_the_first_silent() {
    assert_eq!(
        replayed("validity_open_dialog_second_arrow"),
        said(&[
            &["Label(\"File name:\")", "Role(EditableText)"],
            &["Message(Blank)"],
        ])
    );
}

#[test]
fn the_save_as_selection_says_what_was_selected() {
    assert_eq!(
        replayed("validity_save_as"),
        said(&[
            &["Label(\"File name:\")", "Role(EditableText)"],
            &["Text(\"np.txt\")"],
            &["Phrase(Selected(Character(\"n\")))"],
        ])
    );
}

/// Regenerates the committed fixtures from the sessions, as a live
/// reducer thread runs each input. Not part of the default run: only an
/// intentional change to a session touches its fixture, reviewed as a
/// diff. `cargo test -p verbatim-core --test replay_validity -- --ignored regenerate`.
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
            "2026-11-18T00:00:00Z",
            &SrState::new(),
            &desk.recorded,
        )
        .expect("writes");
        let path = fixture_path(name);
        fs::create_dir_all(path.parent().expect("fixtures dir")).expect("creates fixtures dir");
        fs::write(path, buffer).expect("writes fixture");
    }
}
