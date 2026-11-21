//! Replay fixtures for the one definition of focus validity: flight
//! recorder dumps of Core's inputs, in the listener's order and with the
//! gaps of real recordings, replayed by `cargo xtask replay` and here, and
//! checked against their exact speech.
//!
//! The sessions load their events' order and gaps from committed
//! recording excerpts (`tests/fixtures/recordings/`, each naming its
//! source on its first line): Verbatim's settings dialog raising its
//! list's own focus twice; mockapp's list on a mouse return; five Tabs in
//! a Properties sheet, from NVDA's log of capture C7; Notepad's Open
//! dialog, its folder tree's stale MSAA focus; and the Save As dialog's
//! events. What no recording gives is code-level and said so on its
//! session: a key placed inside a 100 ms watch, the Save As tree's focus
//! placed behind the caret event, the order of change B, and every
//! reader's answer, written by hand. [`regenerate`] writes the fixtures
//! from the sessions.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "reads its fixture files, and writes them when regenerated: listed under the determinism rule's exceptions"
)]

use readers::Found;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{App, Desk, LISTENER, ONE, TWO, focus_read_of, focus_tell};
use verbatim_core::{SrState, dump, replay};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{
    CaretBinding, FocusCaret, FocusCaretRead, FocusRead, ForegroundReport, HeldIds, MsaaIds,
    Outcome, Press, PressAction, QueueEffect, QueueInput, Reply,
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
    out_of(desk, "Candidate")
}

/// The one request of `kind` out.
fn sent_of(desk: &Desk, kind: &str) -> u64 {
    out_of(desk, kind).0
}

/// Answers the one candidate out at `at_us` as its outpost does, having
/// found `found`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "its callers build what was found in place"
)]
fn answer_run(desk: &mut Desk, at_us: u64, found: Found) {
    let (number, to) = run_sent(desk);
    desk.at_us = at_us.saturating_sub(1_000);
    let _ = desk.found(to, number, &found);
}

/// Answers batch `number` of `app`'s outpost at `at_us` as it does, having
/// found `found`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "its callers build what was found in place"
)]
fn found_at(desk: &mut Desk, at_us: u64, app: App, number: u64, found: Found) {
    desk.at_us = at_us.saturating_sub(1_000);
    let _ = desk.found(app.instance(), number, &found);
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

/// A candidate found without the focused state, not read further.
fn unfocused() -> Found {
    Found::Unfocused
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

/// One event of a committed recording excerpt
/// (`tests/fixtures/recordings/`): `verbatim-event-record`'s lines, or the
/// MSAA events NVDA's debug log emitted, cut to what the session feeds.
#[derive(Clone, Debug)]
struct Recorded {
    /// When it was raised, in microseconds on the recording's clock.
    t_us: u64,
    /// Its kind.
    kind: EventKind,
    /// Its window, as the recording names it.
    window: String,
    /// Its object id.
    object: i32,
    /// Its child id.
    child: i32,
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
            let number = |key: &str| event[key].as_i64().expect("a recorded number");
            Recorded {
                t_us: u64::try_from(number("t_us")).expect("a time"),
                kind: match event["kind"].as_str().expect("a kind") {
                    "msaa_focus" => EventKind::MsaaFocus,
                    "msaa_foreground" => EventKind::Foreground,
                    "msaa_state" => EventKind::StateChange,
                    "msaa_location" => EventKind::SystemCaret,
                    other => panic!("a kind the sessions do not feed: {other}"),
                },
                window: event["window"].as_str().expect("a window").to_owned(),
                object: i32::try_from(number("object")).expect("an object id"),
                child: i32::try_from(number("child")).expect("a child id"),
            }
        })
        .collect()
}

/// The recording's windows as `app`'s: its first window that `top` names
/// as the top-level window, every other window in the order first seen
/// after it.
struct Windows {
    app: App,
    seen: Vec<String>,
    top: Option<String>,
}

impl Windows {
    fn new(app: App, top: Option<&str>) -> Self {
        Self {
            app,
            seen: Vec::new(),
            top: top.map(str::to_owned),
        }
    }

    /// The desk's handle of the recording's window `window`.
    fn handle(&mut self, window: &str) -> u64 {
        if self.top.as_deref() == Some(window) {
            return self.app.top;
        }
        let place = if let Some(place) = self.seen.iter().position(|seen| seen == window) {
            place
        } else {
            self.seen.push(window.to_owned());
            self.seen.len() - 1
        };
        self.app.top + 1 + u64::try_from(place).expect("a few windows")
    }

    /// The listener's tell of `event`, at `at_us`.
    fn tell(&mut self, event: &Recorded, at_us: u64) -> Event {
        let handle = self.handle(&event.window);
        msaa_tell(
            self.app,
            event.kind,
            (handle, event.object, event.child),
            at_us,
        )
    }
}

/// Plays `events` from `at_us`, each at its recorded gap from the first,
/// one that shares its time with the one before 10 µs after it, the log's
/// order kept.
fn play(desk: &mut Desk, windows: &mut Windows, events: &[Recorded], at_us: u64) {
    let Some(first) = events.first() else {
        return;
    };
    let mut last = 0;
    for event in events {
        let at = (at_us + event.t_us - first.t_us).max(last + 10);
        last = at;
        let tell = windows.tell(event, at);
        tell_at(desk, at, tell);
    }
}

/// The settings dialog's list (`settings_dialog_keys_309_313`): the
/// dialog's foreground, refused with no read with no report of it in front
/// yet; the list's own focus, read alone with Core idle; and its second
/// own focus 1.489 ms later. Code-level: the first read's answer comes
/// after the second focus, redirected to "Speech"; the second is read
/// unless it is that item by its ids and reading, which its outpost
/// answers it is: "Speech 1 of 3" once.
fn settings_list(desk: &mut Desk) {
    let events = recording("settings_dialog_keys_309_313");
    let mut windows = Windows::new(ONE, Some(&events[0].window));
    play(desk, &mut windows, &events, 2_797_075);
    let list = windows.handle(&events[1].window);
    let speech = item(msaa_node(ONE, 11, Role::ListItem, Some("Speech")), 1, 3);
    let categories = msaa_node(ONE, 10, Role::List, Some("Categories:"));
    let child = ids(list, -4, 1);
    let read = by_ids(focus_read_of(ONE, &speech, &[categories]), child);
    answer_run(desk, 2_800_300, Found::Read(read));
    answer_run(desk, 2_800_700, Found::Same);
}

/// mockapp's list on a mouse return (`validity_list_button_run1`): the
/// Apply button's focus, read and spoken; then the list's own focus and
/// its item's 0.266 ms later, NVDA saying "list", "Alpha 1 of 3": the item
/// said once.
fn mouse_return(desk: &mut Desk) {
    let events = recording("validity_list_button_run1");
    let mut windows = Windows::new(ONE, None);
    play(desk, &mut windows, &events[..1], 30_000);
    let button = windows.handle(&events[0].window);
    let apply = msaa_node(ONE, 20, Role::Button, Some("Apply"));
    let read = by_ids(focus_read_of(ONE, &apply, &[]), ids(button, -4, 0));
    answer_run(desk, 31_000, Found::Read(read));
    play(desk, &mut windows, &events[1..], 1_532_000);
    let list = windows.handle(&events[1].window);
    let alpha = item(msaa_node(ONE, 31, Role::ListItem, Some("Alpha")), 1, 3);
    let child = ids(list, -4, 1);
    let read = by_ids(
        focus_read_of(ONE, &alpha, &[msaa_node(ONE, 30, Role::List, None)]),
        child,
    );
    answer_run(desk, 1_533_000, Found::Read(read));
}

/// Five Tabs in a Properties sheet (`c7_held_tab_run1`, the MSAA events
/// NVDA's log emitted in one flush): a Button's focus and a state change,
/// a second Button, the tab control's own focus and its item's, then two
/// state changes. The first Button is read alone and found without the
/// focused state; the focuses behind it wait as one run, its newest, the
/// tab item, read and spoken: "tab control", "General tab selected". The
/// Tab keys are not in the fixture: they reach Core from the hook, apart
/// from the listener.
fn five_tabs(desk: &mut Desk) {
    let events = recording("c7_held_tab_run1");
    let mut windows = Windows::new(ONE, None);
    play(desk, &mut windows, &events, 23_000);
    answer_run(desk, 31_000, unfocused());
    let tab = windows.handle(&events[3].window);
    let mut general = msaa_node(ONE, 41, Role::Tab, Some("General"));
    general.states = StateSet::new().with(verbatim_model::State::Selected);
    let control = msaa_node(ONE, 40, Role::TabControl, None);
    let read = by_ids(focus_read_of(ONE, &general, &[control]), ids(tab, -4, 1));
    answer_run(desk, 42_000, Found::Read(read));
}

/// Change B's order, code-level, as no recording gives it: a second application's UI Automation focus raised
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
            ghost: None,
            console: false,
            integrity: None,
            own_integrity: None,
        }),
    );
    let ok = msaa_node(ONE, 1, Role::Button, Some("OK"));
    let number = sent_of(desk, "CurrentFocus");
    found_at(
        desk,
        11_000,
        ONE,
        number,
        Found::Read(by_ids(
            focus_read_of(ONE, &ok, &[]),
            ids(ONE.top + 1, -4, 0),
        )),
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
            ghost: None,
            console: false,
            integrity: None,
            own_integrity: None,
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
    answer_run(desk, at + 52_000, Found::Read(read));
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
    CaretReport { line, selection }
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

/// The "File name" edit of the Open dialog committed through MSAA by its
/// recorded focus `event`, its caret at the start of `text`: the windows
/// of the dialog and its field.
fn file_name(desk: &mut Desk, event: &Recorded, text: &str) -> Windows {
    let mut windows = Windows::new(ONE, None);
    play(desk, &mut windows, std::slice::from_ref(event), 75_000);
    let edit = windows.handle(&event.window);
    let field = msaa_node(ONE, 2, Role::EditableText, Some("File name:"));
    let mut line = empty_line();
    text.clone_into(&mut line.text);
    let mut read = by_ids(focus_read_of(ONE, &field, &[]), ids(edit, -4, 0));
    read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
        caret: caret_on(line, None),
        selected_text: None,
        baseline: TextAnchor(70),
    }));
    answer_run(desk, 76_000, Found::Read(read));
    windows
}

/// The Open dialog (`np_open_run1`): "File name:" committed by its focus;
/// the folder tree's stale focus 432 ms after the edit's, as recorded.
/// Code-level: Left Arrow in the empty field placed 15 ms before the
/// tree's focus, so its watch is out when the focus arrives, which a real
/// dialog cannot place on demand. The tree's confirmation answered
/// `focused`: not focused, the watch runs out its bound and the arrow says
/// "blank", as NVDA in all 18 captured runs; focused, the arrow is silent
/// and the tree is read at its turn.
fn open_dialog(desk: &mut Desk, focused: bool) {
    let events = recording("np_open_run1");
    let mut windows = file_name(desk, &events[0], "");
    let tree_at = 75_000 + events[1].t_us - events[0].t_us;
    left_arrow(desk, tree_at - 15_000);
    started(desk, tree_at - 13_000);
    play(desk, &mut windows, &events[1..], tree_at);
    let (number, _) = run_sent(desk);
    found_at(
        desk,
        tree_at + 2_000,
        ONE,
        number,
        if focused {
            Found::Focused
        } else {
            Found::Unfocused
        },
    );
    let tree = windows.handle(&events[1].window);
    if focused {
        let tree_item = msaa_node(ONE, 60, Role::TreeItem, Some("This PC"));
        let read = by_ids(
            focus_read_of(ONE, &tree_item, &[msaa_node(ONE, 59, Role::Tree, None)]),
            ids(tree, -4, 16),
        );
        answer_run(desk, tree_at + 5_000, Found::Read(read));
        return;
    }
    // The bound: the watch's last check, the caret still at the start.
    blank_at_bound(desk, tree_at - 13_000 + 100_000);
    // At its turn the tree's stored refusal asks nothing.
}

/// The key's watch answered at its bound, at `at_us`: the caret at the
/// start of the empty text.
fn blank_at_bound(desk: &mut Desk, at_us: u64) {
    let watch = sent_of(desk, "Text");
    let mut unit = empty_line();
    unit.unit = TextUnit::Character;
    answer_at(
        desk,
        at_us,
        ONE,
        watch,
        Reply::Text(TextReply::Watched(Box::new(Watched::Bound(WatchedRead {
            caret: caret_on(empty_line(), None),
            unit,
        })))),
    );
}

/// The same, a second Left Arrow arriving while the tree's confirmation is
/// out (code-level): the confirmation is withdrawn and the first arrow is
/// silent; the tree, ahead of the second arrow, is read at its turn and
/// found without the focused state; then the second arrow reads, watches
/// to its bound and says "blank".
fn open_dialog_second_arrow(desk: &mut Desk) {
    let events = recording("np_open_run1");
    let mut windows = file_name(desk, &events[0], "");
    let tree_at = 75_000 + events[1].t_us - events[0].t_us;
    left_arrow(desk, tree_at - 15_000);
    started(desk, tree_at - 13_000);
    play(desk, &mut windows, &events[1..], tree_at);
    left_arrow(desk, tree_at + 1_000);
    answer_run(desk, tree_at + 3_000, unfocused());
    started(desk, tree_at + 5_000);
    blank_at_bound(desk, tree_at + 5_000 + 100_000);
}

/// The Save As dialog's decided question (`validity_saveas_control_run1`
/// gives the events' identities): "File name:" holding "np.txt" committed
/// by its recorded focus; Shift+Right's caret event, then the folder
/// tree's stale MSAA focus arriving behind it while the caret event's read
/// is out. Code-level: in all 19 recorded runs the tree's focus came 159
/// to 355 ms before the caret event, so no recording gives this order. The
/// unconfirmed focus behind the caret tell does not count and is not read
/// (Dickson, 2026-11-18): the selection says "n selected", as NVDA, and
/// only then is the tree read at its turn and refused.
fn save_as(desk: &mut Desk) {
    let events = recording("validity_saveas_control_run1");
    let mut windows = file_name(desk, &events[0], "np.txt");
    let caret_at = 2_004_000;
    play(desk, &mut windows, &events[2..], caret_at);
    let number = sent_of(desk, "Text");
    play(desk, &mut windows, &events[1..2], caret_at + 2_000);
    let mut line = empty_line();
    "np.txt".clone_into(&mut line.text);
    let selected = SelectionChange {
        selected: true,
        text: "n".to_owned(),
        characters: 1,
    };
    answer_at(
        desk,
        caret_at + 3_000,
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
    answer_run(desk, caret_at + 5_000, unfocused());
}

/// Every fixture: its name and its session.
/// A session, which drives a desk through a recorded order.
/// The desktop's list (`deldesk_run1`): its items a.txt, 14 of 16, and
/// b.txt, 15 of 16, focused as their names were typed, each read and
/// spoken; the Delete key 38.1 ms before the list's focus at child 15, the
/// deleted item's child id, where c.txt now is, 15 of 15. The key comes
/// between, so the focus is read in full and spoken, where NVDA's File
/// Explorer rule refuses it (Dickson, 2026-11-18: "I do want it to read").
/// The typed keys and every answer are code-level.
fn desktop_delete(desk: &mut Desk) {
    let events = recording("deldesk_run1");
    let mut windows = Windows::new(ONE, None);
    let start = 4_000_000;
    let at = |event: &Recorded| start + event.t_us - events[0].t_us;
    let list = windows.handle(&events[0].window);
    let desktop = msaa_node(ONE, 50, Role::List, Some("Desktop"));
    let file = |number, name, (position, size)| {
        let node = item(
            msaa_node(ONE, number, Role::ListItem, Some(name)),
            position,
            size,
        );
        Found::Read(by_ids(
            focus_read_of(ONE, &node, std::slice::from_ref(&desktop)),
            ids(list, -4, i32::try_from(position).expect("a child id")),
        ))
    };
    play(desk, &mut windows, &events[..1], at(&events[0]));
    answer_run(
        desk,
        at(&events[0]) + 1_000,
        file(51, "zz-verbatim-rec-a.txt", (14, 16)),
    );
    play(desk, &mut windows, &events[1..2], at(&events[1]));
    answer_run(
        desk,
        at(&events[1]) + 1_000,
        file(52, "zz-verbatim-rec-b.txt", (15, 16)),
    );
    // Delete, passed at once: a caret key of motion `Other`, as the app
    // posts it, bound to the committed focus as Core takes it.
    let bound = desk.state.caret_binding().map(Box::new);
    desk.keys += 1;
    let delete_us = at(&events[2]) - 38_100;
    desk.step_at(
        delete_us,
        QueueInput::Press(Box::new(Press {
            key_seq: KeySeq::new(desk.keys),
            pressed_at_us: delete_us,
            action: PressAction::CaretKey {
                key: CaretKey {
                    motion: CaretMotion::Other,
                    select: false,
                },
                held: None,
                bound,
            },
        })),
    );
    play(desk, &mut windows, &events[2..], at(&events[2]));
    answer_run(
        desk,
        at(&events[2]) + 1_000,
        file(53, "zz-verbatim-rec-c.txt", (15, 15)),
    );
}

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
        ("validity_desktop_delete", desktop_delete),
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

/// The release run 02 of `settings_dialog_keys` at ce3305a1, its flight
/// recorder dump as Core wrote it (`fixtures/settings_dialog_keys_run02`):
/// the menu check's read of the settings list answers first and says
/// "Speech 1 of 3", and the list's own focus, raised twice behind it, is
/// then the same item at the same ids with no key between, and says
/// nothing. The dump is replayed for its speech only, as its answers are
/// the real outposts'.
#[test]
fn the_recorded_settings_dialog_says_its_item_once() {
    let file = fs::File::open(fixture_path("settings_dialog_keys_run02")).expect("fixture file");
    let contents = dump::read_dump(&mut BufReader::new(file)).expect("fixture parses");
    let inputs: Vec<_> = contents.inputs.iter().map(|r| r.input.clone()).collect();
    let first = replay(&contents.base, &inputs);
    assert_eq!(
        first,
        replay(&contents.base, &inputs),
        "replay is deterministic"
    );
    assert_eq!(
        speech(&first),
        said(&[
            &["Label(\"Desktop\")", "Role(List)"],
            &[
                "Label(\"NVDA\")",
                "Position { position: 3, set_size: Some(13) }"
            ],
            &["Label(\"Verbatim\")"],
            &["Label(\"Context\")", "Role(Menu)"],
            &["Label(\"Settings...\")", "Shortcut(\"s\")"],
            &["Label(\"Verbatim Settings: Speech\")", "Role(Dialog)"],
            &[
                "Label(\"Categories:\")",
                "Role(List)",
                "Shortcut(\"Alt+c\")"
            ],
            &[
                "Label(\"Speech\")",
                "Position { position: 1, set_size: Some(3) }"
            ],
        ])
    );
}

/// The three windows the recorded settings dialog run destroys, as the
/// listener's destroys name them, each raised by a thread of Verbatim's
/// own process, 7832: the context menu's window, then two of the list's
/// edit windows.
const DESTROYED: [u64; 3] = [14_483_604, 19_268_216, 21_299_250];

/// The handles of the windows whose probe verdicts `state` keeps.
fn probed(state: &SrState) -> Vec<u64> {
    let state = serde_json::to_value(state).expect("the state serializes");
    state["queue"]["backend"]["probed"]
        .as_array()
        .expect("the probe verdicts")
        .iter()
        .map(|entry| entry[0]["handle"].as_u64().expect("a window handle"))
        .collect()
}

/// The recorded settings dialog run's three destroys, converted from the
/// tells the listener sent before it sent destroys of their own, with
/// their recorded process: each is passed to Verbatim's own outpost,
/// generation 2, which is ready, and Core clears what it keeps of the
/// window, the context menu's probe verdict the one it held, with nothing
/// recovering and nothing followed or plain left of any of them.
#[test]
fn the_recorded_settings_dialogs_destroys_reach_its_outpost_and_clear_cores_records() {
    let file = fs::File::open(fixture_path("settings_dialog_keys_run02")).expect("fixture file");
    let contents = dump::read_dump(&mut BufReader::new(file)).expect("fixture parses");
    let mut state = contents.base.clone();
    let mut runtime = verbatim_core::Runtime::default();
    let mut destroyed = Vec::new();
    for recorded in &contents.inputs {
        let window = match &recorded.input {
            verbatim_model::Input::Queue {
                input: QueueInput::WindowDestroyed { window, .. },
                ..
            } => Some(*window),
            _ => None,
        };
        let before = probed(&state);
        let effects = verbatim_core::reduce(&mut state, &mut runtime, &recorded.input);
        let Some(window) = window else {
            continue;
        };
        let serialized = serde_json::to_value(&state).expect("the state serializes");
        destroyed.push((
            window.0,
            before.contains(&window.0),
            probed(&state).contains(&window.0),
            serialized["queue"]["table"]["recovery"].clone(),
            serialized["follows"]["named"].clone(),
            serialized["plain"]["object"].clone(),
            effects,
        ));
    }
    let to = verbatim_model::request::Instance {
        reader: verbatim_model::request::Reader::Outpost(verbatim_model::Pid(7832)),
        generation: 2,
    };
    let wake = Effect::Queue(verbatim_model::request::QueueEffect::WakeAt(Some(
        1_795_005_484_235_323,
    )));
    assert_eq!(
        destroyed,
        DESTROYED
            .iter()
            .enumerate()
            .map(|(place, &window)| (
                window,
                place == 0,
                false,
                serde_json::json!({}),
                serde_json::json!([]),
                serde_json::Value::Null,
                vec![
                    Effect::Queue(verbatim_model::request::QueueEffect::WindowDestroyed {
                        to,
                        window: WindowHandle(window),
                    }),
                    wake.clone(),
                ],
            ))
            .collect::<Vec<_>>()
    );
}

/// The desktop's next item is read and spoken after Delete, as Dickson
/// ruled (2026-11-18), where NVDA says nothing.
#[test]
fn the_desktops_next_item_is_spoken_after_delete() {
    assert_eq!(
        replayed("validity_desktop_delete"),
        said(&[
            &["Label(\"Desktop\")", "Role(List)"],
            &[
                "Label(\"zz-verbatim-rec-a.txt\")",
                "Position { position: 14, set_size: Some(16) }"
            ],
            &[
                "Label(\"zz-verbatim-rec-b.txt\")",
                "Position { position: 15, set_size: Some(16) }"
            ],
            &[
                "Label(\"zz-verbatim-rec-c.txt\")",
                "Position { position: 15, set_size: Some(15) }"
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
            // The second arrow's; the first said nothing.
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

/// The flight recorder's dump of `quit_inside_a_file_dialog` (2026-11-20,
/// at 8141d0fa): Import, hidden at its foreground, postponed; its show told
/// as the hook rewrites it, a client object's show of its `#32770` window.
#[test]
fn the_recorded_import_dialog_is_said_with_its_text_at_its_show() {
    let file = fs::File::open(fixture_path("dialog_shown_import")).expect("fixture file exists");
    let contents = dump::read_dump(&mut BufReader::new(file)).expect("fixture parses");
    assert!(!contents.truncated, "the committed fixture is complete");
    let inputs: Vec<_> = contents.inputs.iter().map(|r| r.input.clone()).collect();
    let effects = replay(&contents.base, &inputs);
    assert_eq!(effects, replay(&contents.base, &inputs), "deterministic");
    let show = inputs
        .iter()
        .position(|input| {
            let verbatim_model::Input::Queue {
                input: QueueInput::Tell { tell, .. },
                ..
            } = input
            else {
                return false;
            };
            let TellWindow::Found(facts) = &tell.window else {
                return false;
            };
            tell.kind == EventKind::Show
                && matches!(
                    tell.object,
                    EventObject::Msaa {
                        object: -4,
                        child: 0,
                        ..
                    }
                )
                && facts.window.handle == facts.root.handle
                && facts.window.class == "#32770"
        })
        .expect("the listener told the dialog's show");
    // The show's read, the first current-focus read Core sends from it.
    let number = effects[show..]
        .iter()
        .flatten()
        .find_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. })
                if desk::kind(&ask.request) == "CurrentFocus" =>
            {
                Some(ask.number)
            }
            _ => None,
        })
        .expect("the show reads the focus");
    let answered = inputs
        .iter()
        .position(|input| {
            matches!(input, verbatim_model::Input::Queue {
                input: QueueInput::Answer { answer, .. },
                ..
            } if answer.number == number)
        })
        .expect("the read is answered");
    // NVDA's capture: "Import a Theme  dialog  File name:", "File name:
    // combo box  collapsed", "edit  Alt plus  n", "blank".
    assert_eq!(
        speech(&effects[answered..=answered]),
        said(&[
            &[
                "Label(\"Import a Theme\")",
                "Role(Dialog)",
                "Description(\"File name:\")"
            ],
            &[
                "Label(\"File name:\")",
                "Role(ComboBox)",
                "State(Collapsed)"
            ],
            &["Role(EditableText)", "Shortcut(\"Alt+n\")"],
            &["Message(Blank)"],
        ])
    );
}
