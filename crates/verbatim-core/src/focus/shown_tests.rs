//! A dialog read once it is shown, by postponement: the design's sequences
//! in Import's recorded order (`tests/fixtures/hidden_dialog/`), a key put
//! between the hidden answer and the show; "stand-in order" marks those
//! with no real application recorded (Dickson, 2026-11-20).

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{
    CandidateAnswer, FocusAncestor, FocusCaret, FocusNode, FocusRead, Front, HeldIds, MsaaIds,
    NodeRead, NodeWindow, PressAction, QueueInput, Reply, Request as Wire,
};
use verbatim_model::tell::{AcceptanceFacts, EventObject, Relation, TellWindow, WindowFacet};
use verbatim_model::{
    CaretKey, CaretMotion, Effect as Said, KeySeq, Pid, ReviewCommand, Role, SegmentContent,
    TraceId, WindowHandle,
};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, NOTHING, OUTPOST, answer, msaa_tell, node, ready,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Foreground, Input, Instance, Reader, Timed};
use verbatim_model::Effect;
use verbatim_model::request::QueueEffect;

/// The settings dialog that owns the file dialog, in front before it.
const OWNER: u64 = 4;
/// A button of the owner's.
const OWNER_BUTTON: u64 = 5;
/// The file dialog, hidden until its show.
const DIALOG: u64 = 30;
/// The file dialog's File name field.
const FIELD: u64 = 31;
/// A message box the hidden dialog owns.
const BOX: u64 = 40;
/// A popup menu of the attended application.
const MENU: u64 = 50;
/// Another application, in the background.
const OTHER_APP: Pid = Pid(77);

fn facet(handle: u64, class: &str, process: Pid) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process,
        thread: 41,
    }
}

/// A tooltip, a top-level window of a show class.
const TOOLTIP: u64 = 60;

/// The class of `window`, of top-level window `root`: a dialog's when it is
/// its own top-level window.
fn class_of(window: u64, root: u64) -> &'static str {
    match window {
        TOOLTIP => "tooltips_class32",
        window if window == root => "#32770",
        _ => "Edit",
    }
}

/// The facts of a tell for `window`, of top-level window `root` owned by
/// `owner`, as the listener read them with `front` in front: accepted by
/// the root owner when `related`, else unrelated to the foreground.
fn tell_facts(
    (window, root, owner): (u64, u64, u64),
    front: u64,
    process: Pid,
    related: bool,
) -> AcceptanceFacts {
    AcceptanceFacts {
        window: facet(window, class_of(window, root), process),
        parent_class: String::new(),
        root: facet(root, "#32770", process),
        root_owner: facet(owner, "#32770", process),
        window_topmost: false,
        root_topmost: window == MENU,
        foreground: WindowHandle(front),
        foreground_root_owner: WindowHandle(OWNER),
        window_to_foreground: if related {
            Relation::Under
        } else {
            Relation::Outside
        },
        root_owner_to_foreground: if related {
            Relation::Equal
        } else {
            Relation::Outside
        },
        root_owner_is_foregrounds: related,
        active_window: WindowHandle(front),
        window_to_active: Relation::Outside,
        desktop: false,
        hidden_frame: false,
        hidden_frame_owned: false,
    }
}

/// An MSAA tell of `kind` on `window`'s client, inside `root` owned by
/// `owner`, raised at `at_us` with `front` in front.
fn tell(
    kind: EventKind,
    (window, root, owner): (u64, u64, u64),
    (front, at_us): (u64, u64),
    (process, related): (Pid, bool),
) -> Input<Entry> {
    let mut event = msaa_tell(-4);
    event.kind = kind;
    event.process = process;
    event.at_us = at_us;
    event.trace_id = TraceId::from_raw(at_us);
    event.object = EventObject::Msaa {
        window: WindowHandle(window),
        object: -4,
        child: 0,
    };
    event.window = TellWindow::Found(Box::new(tell_facts(
        (window, root, owner),
        front,
        process,
        related,
    )));
    Input::Entry(Entry::of_tell(LISTENER, event).expect("an entry"))
}

/// A show of the top-level `window`, as the listener tells it: the hook's
/// rewrite of its window object's show to its client object.
fn show(window: u64, at_us: u64) -> Input<Entry> {
    tell(
        EventKind::Show,
        (window, window, OWNER),
        (window, at_us),
        (APP, true),
    )
}

/// The dialog's foreground tell, raised at `at_us`.
fn dialog_foreground(at_us: u64) -> Input<Entry> {
    tell(
        EventKind::Foreground,
        (DIALOG, DIALOG, OWNER),
        (DIALOG, at_us),
        (APP, true),
    )
}

/// The File name field's focus, raised at `at_us`.
fn field_focus(at_us: u64) -> Input<Entry> {
    tell(
        EventKind::MsaaFocus,
        (FIELD, DIALOG, OWNER),
        (DIALOG, at_us),
        (APP, true),
    )
}

/// The listener's report that `window`, owned by `owner`, of `process`,
/// came to the foreground.
fn in_front(window: u64, owner: u64, process: Pid) -> Input<Entry> {
    Input::Foreground(Foreground {
        window: WindowHandle(window),
        root_owner: WindowHandle(owner),
        process,
        thread: 41,
        by_class: None,
        shell: false,
        ..Foreground::BLANK
    })
}

fn values(name: &str, role: Role) -> verbatim_model::property::PropertyValues {
    verbatim_model::property::PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::Name,
            value: Some(PropertyValue::Text(name.to_owned())),
        },
        PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(role)),
        },
    ])
}

fn window_of(handle: u64, top_level: u64, owner: u64) -> NodeWindow {
    NodeWindow {
        handle: WindowHandle(handle),
        top_level: WindowHandle(top_level),
        root_owner: WindowHandle(owner),
        process: APP,
        thread: 41,
        desktop: false,
    }
}

/// The hidden dialog as the outpost answers it.
fn hidden_dialog() -> Reply {
    Reply::HiddenDialog(window_of(DIALOG, DIALOG, OWNER))
}

fn focus_node(number: u64, window: NodeWindow, name: &str, role: Role) -> FocusNode {
    FocusNode {
        read: NodeRead {
            node: Some(node(number)),
            window,
            values: values(name, role),
        },
        runtime_id: None,
    }
}

/// A focus read of `name`, a `role` in `window`, under the ancestors
/// `ancestors`, nearest first, read by `ids`.
fn read_of(
    (number, name, role): (u64, &str, Role),
    window: NodeWindow,
    ancestors: Vec<FocusAncestor>,
    ids: (u64, i32),
) -> Reply {
    Reply::Candidate(CandidateAnswer::focused(FocusRead {
        focus: focus_node(number, window, name, role),
        msaa: Some(HeldIds {
            event: MsaaIds {
                window: WindowHandle(ids.0),
                object: ids.1,
                child: 0,
            },
            held_child: 0,
        }),
        ancestors,
        met: None,
        foreground: None,
        value_from_selection: false,
        runtime_id_reused: false,
        caret: FocusCaret::NoText,
    }))
}

/// The dialog as an ancestor, with the text it gives once shown, or none
/// while hidden.
fn dialog_ancestor(text: Option<&str>) -> FocusAncestor {
    let mut node = focus_node(
        20,
        window_of(DIALOG, DIALOG, OWNER),
        "Import a Theme",
        Role::Dialog,
    );
    // The outpost gives a dialog's text as its description.
    if let Some(text) = text {
        node.read.values.0.push(PropertyAnswer {
            id: PropertyId::Description,
            value: Some(PropertyValue::Text(text.to_owned())),
        });
    }
    FocusAncestor {
        node,
        simple_child: false,
    }
}

/// The File name field read, under the dialog with `text`: the stand-in
/// for the dialog's read once shown, whose focus is the field.
fn field_read(text: Option<&str>) -> Reply {
    read_of(
        (21, "File name:", Role::EditableText),
        window_of(FIELD, DIALOG, OWNER),
        vec![dialog_ancestor(text)],
        (FIELD, -4),
    )
}

/// The owner's button read, under the owner.
fn owner_button_read() -> Reply {
    read_of(
        (11, "Import...", Role::Button),
        window_of(OWNER_BUTTON, OWNER, OWNER),
        vec![FocusAncestor {
            node: focus_node(
                10,
                window_of(OWNER, OWNER, OWNER),
                "Verbatim Settings: Theme",
                Role::Dialog,
            ),
            simple_child: false,
        }],
        (OWNER_BUTTON, -4),
    )
}

/// What a step did, as these tests read it: each request sent by its
/// reader and kind, each cancel by its reader and reason, each utterance
/// by its words.
fn step(held: &mut Held, at_us: u64, input: Input<Entry>) -> Vec<String> {
    feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { to, ask }) => {
                Some(format!("send {:?} {}", to.reader, sent(&ask.request)))
            }
            Effect::Queue(QueueEffect::Cancel { to, cancel }) => {
                Some(format!("cancel {:?} {:?}", to.reader, cancel.reason))
            }
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding) => {
                Some("not responding".to_owned())
            }
            said => spoken(&said),
        })
        .collect()
}

/// An utterance's words, a key's own included.
fn spoken(said: &Said) -> Option<String> {
    match said {
        Said::Speak(utterance, _) => Some(
            utterance
                .segments
                .iter()
                .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                .map(|segment| format!("{:?}", segment.content))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Said::ForKey(_, said) => spoken(said),
        _ => None,
    }
}

/// What answering the read out with `reply` at `at_us` said, each
/// utterance with the key number its speech is fenced by, which the key's
/// cut compares: speech carrying a lower number than the key's is cut.
fn keyed(held: &mut Held, at_us: u64, reply: Reply) -> Vec<(String, Option<u64>)> {
    fn of(said: &Said) -> Option<(String, Option<u64>)> {
        match said {
            Said::Speak(utterance, key) => Some((
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect::<Vec<_>>()
                    .join(" "),
                key.map(KeySeq::get),
            )),
            Said::ForKey(_, said) => of(said),
            _ => None,
        }
    }
    let number = out(held);
    feed(
        &mut held.state,
        &mut held.runtime,
        Timed {
            at_us,
            input: answer(OUTPOST, number, reply),
        },
    )
    .unwrap()
    .iter()
    .filter_map(of)
    .collect()
}

/// A request as these tests name it.
fn sent(request: &Wire) -> String {
    match request {
        Wire::MsaaRun(run) => format!("MsaaRun {} {:?}", run.candidate.ids.window.0, run.hidden),
        Wire::CurrentFocus { hidden, .. } => format!("CurrentFocus {hidden:?}"),
        Wire::CompleteUia(complete) => format!("CompleteUia {:?}", complete.hidden),
        Wire::ReadObject(_) => "ReadObject".to_owned(),
        Wire::Follow(_) => "Follow".to_owned(),
        other => format!("{other:?}"),
    }
}

/// The number of the request out, of the newest live.
fn out(held: &Held) -> u64 {
    held.state.queue.last_number_for_tests()
}

const ASK_DIALOG: &str = "send Outpost(Pid(40)) MsaaRun 30 Check";
const ASK_CURRENT: &str = "send Outpost(Pid(40)) CurrentFocus Check";
const ASK_AS_IT_IS: &str = "send Outpost(Pid(40)) CurrentFocus ReadAnyway";

/// The owner's title and the dialog's text, then the field, as NVDA's
/// capture speaks them.
const DIALOG_SPOKEN: [&str; 2] = [
    "Label(\"Import a Theme\") Role(Dialog) Description(\"File name:\")",
    "Label(\"File name:\") Role(EditableText)",
];

/// What the read at a key says: the dialog hidden, with no text, then its
/// field, both fenced by Enter, key 1, so key 2's cut silences them.
const KEYED: [(&str, Option<u64>); 2] = [
    ("Label(\"Import a Theme\") Role(Dialog)", Some(1)),
    ("Label(\"File name:\") Role(EditableText)", Some(1)),
];

/// The owner's button committed, the owner in front, Enter pressed on its
/// "Import...", key 1, and the dialog come to the foreground hidden, its
/// first read asked.
fn dialog_asked() -> Held {
    let mut held = ready();
    step(&mut held, 100, in_front(OWNER, OWNER, APP));
    let _ = step(&mut held, 200, answer(OUTPOST, 1, owner_button_read()));
    let _ = step(&mut held, 900, press(1, 900, typed("\r")));
    assert_eq!(
        step(&mut held, 1_000, in_front(DIALOG, OWNER, APP)),
        NOTHING
    );
    assert_eq!(
        step(&mut held, 1_010, dialog_foreground(1_010)),
        [ASK_DIALOG]
    );
    held
}

/// The dialog's first read answered hidden: postponed, nothing said and
/// nothing held.
fn postponed() -> Held {
    let mut held = dialog_asked();
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), NOTHING);
    assert!(held.state.queue.front_for_tests().is_none());
    held
}

/// Answers the read out with `reply` at `at_us`.
fn answer_out(held: &mut Held, at_us: u64, reply: Reply) -> Vec<String> {
    let number = out(held);
    step(held, at_us, answer(OUTPOST, number, reply))
}

fn shown(held: &mut Held, at_us: u64) -> Vec<String> {
    step(held, at_us, show(DIALOG, at_us))
}

/// The dialog's show read and answered shown: the dialog with its text,
/// then its field, and the postponement ended.
fn shown_speaks_the_dialog(held: &mut Held, at_us: u64) {
    assert_eq!(shown(held, at_us), [ASK_CURRENT]);
    let read = field_read(Some("File name:"));
    assert_eq!(answer_out(held, at_us + 50, read), DIALOG_SPOKEN);
    assert_eq!(held.state.versions.postponed, None);
}

fn key(action: PressAction) -> Input<Entry> {
    press(2, 1_150, action)
}

/// Right Arrow, a caret key, bound to nothing.
fn right_arrow() -> PressAction {
    let key = CaretKey {
        motion: CaretMotion::NextCharacter,
        select: false,
    };
    PressAction::CaretKey {
        key,
        held: None,
        bound: None,
    }
}

/// Each input at 1,150 µs while postponed says what is given and leaves the
/// dialog for its show, which speaks it with its text: Import's field
/// postponing too; a caret key and a focus elsewhere refused
/// by its read; the listener ending, whose re-take postpones again; the
/// foreground leaving, a show meanwhile asking nothing, and the report
/// naming the dialog reading it (a listener's replacement).
#[test]
fn what_leaves_the_dialog_for_its_show() {
    let hidden = Some(hidden_dialog());
    let field = Some("send Outpost(Pid(40)) MsaaRun 31 Check");
    let elsewhere = Some("send Outpost(Pid(40)) MsaaRun 6 Check");
    let cases = [
        (field_focus(1_150), field, hidden.clone()),
        (key(right_arrow()), None, None),
        (
            owner_control_focus(6, DIALOG, 1_150),
            elsewhere,
            Some(unfocused()),
        ),
        (Input::ReaderEnded(LISTENER), Some(ASK_CURRENT), hidden),
        (in_front(70, 70, OTHER_APP), None, None),
    ];
    for (input, asked, reply) in cases {
        let mut held = postponed();
        let asked: Vec<&str> = asked.into_iter().collect();
        assert_eq!(step(&mut held, 1_150, input), asked);
        if let Some(reply) = reply {
            assert_eq!(answer_out(&mut held, 1_160, reply), NOTHING);
        }
        if held.state.queue.foreground_process() == Some(OTHER_APP) {
            assert_eq!(shown(&mut held, 1_170), NOTHING);
            let back = in_front(DIALOG, OWNER, APP);
            assert_eq!(step(&mut held, 1_180, back), [ASK_CURRENT]);
            let read = field_read(Some("File name:"));
            assert_eq!(answer_out(&mut held, 1_190, read), DIALOG_SPOKEN);
        } else {
            shown_speaks_the_dialog(&mut held, 1_200);
        }
    }
}

/// A modifier alone cuts speech and joins no entry: the show's entry,
/// joining after the cut, is spoken, the dialog with its text.
#[test]
fn a_modifier_alone_leaves_the_dialog_for_its_show() {
    let mut held = postponed();
    let cut = QueueInput::SpeechCut {
        key: KeySeq::new(2),
    };
    let trace = (TraceId::from_raw(1_150), 1_150);
    let _ = crate::entries::route(&mut held.state, &mut held.runtime, trace, &cut);
    shown_speaks_the_dialog(&mut held, 1_200);
}

/// The show told while the dialog's first read is out is judged at its
/// turn, after the hidden answer; the Settings dialog, read shown at its
/// foreground, asks nothing at its show; a tooltip's show stays an
/// announcement.
#[test]
fn a_show_is_judged_at_its_turn() {
    let mut held = dialog_asked();
    assert_eq!(shown(&mut held, 1_050), NOTHING);
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), [ASK_CURRENT]);
    let said = answer_out(&mut held, 1_150, field_read(Some("File name:")));
    assert_eq!(said, DIALOG_SPOKEN);
    let mut held = dialog_asked();
    let said = answer_out(&mut held, 1_100, field_read(Some("File name:")));
    assert_eq!(said, DIALOG_SPOKEN);
    assert_eq!(shown(&mut held, 1_107), NOTHING);
    let Input::Entry(Entry::Announcement(_)) = show(TOOLTIP, 1) else {
        panic!("a tooltip's show is an announcement");
    };
}

/// "a" typed while Import is hidden reads it first, as it is, under
/// Enter's fence, and is echoed past its own cut: heard "a" alone, and the
/// show asks nothing. Fails with the read stamped with no key or with the
/// key's own. "a" before Import's second burst of field focuses: the burst
/// is a duplicate of the field "a" committed, so nothing more is said. A
/// command and a passed key read the dialog first alike, and are judged
/// against the field.
#[test]
fn a_typed_key_or_a_command_reads_the_dialog_first_under_the_older_fence() {
    let command = PressAction::Command {
        command: ReviewCommand::ReportFocus,
        repeat: 0,
    };
    let echo = Some("Character(\"a\")");
    for (action, burst, tail) in [
        (typed("a"), false, echo),
        (typed("a"), true, echo),
        (command, false, None),
        // A passed key as the app posts it (key_reports.rs, KeyReport::Passed).
        (
            PressAction::CaretKey {
                key: CaretKey {
                    motion: CaretMotion::Other,
                    select: false,
                },
                held: None,
                bound: None,
            },
            false,
            None,
        ),
    ] {
        let mut held = postponed();
        assert_eq!(step(&mut held, 1_150, key(action)), [ASK_AS_IT_IS]);
        if burst {
            assert_eq!(step(&mut held, 1_160, field_focus(1_160)), NOTHING);
        }
        let mut expected = KEYED.map(|(said, key)| (said.to_owned(), key)).to_vec();
        expected.extend(tail.map(|echo| (echo.to_owned(), Some(2))));
        assert_eq!(keyed(&mut held, 1_200, field_read(None)), expected);
        assert_eq!(held.state.versions.postponed, None);
        let focus = held
            .state
            .focus
            .object()
            .map(|focus| focus.snapshot.name.clone());
        assert_eq!(focus, Some(Some("File name:".to_owned())));
        if tail.is_some() {
            assert!(held.state.queue.front_for_tests().is_none());
            assert_eq!(shown(&mut held, 1_300), NOTHING);
        }
    }
}

/// The same dialog postponed again, by its field's focus after a caret key
/// (key 2), keeps Enter's fence: the read at "a" (key 3) carries key 1.
#[test]
fn the_same_dialog_postponed_again_keeps_the_older_fence() {
    let mut held = postponed();
    assert_eq!(step(&mut held, 1_150, key(right_arrow())), NOTHING);
    let _ = step(&mut held, 1_160, field_focus(1_160));
    assert_eq!(answer_out(&mut held, 1_170, hidden_dialog()), NOTHING);
    let _ = step(&mut held, 1_180, press(3, 1_180, typed("a")));
    let mut expected = KEYED.map(|(said, key)| (said.to_owned(), key)).to_vec();
    expected.push(("Character(\"a\")".to_owned(), Some(3)));
    assert_eq!(keyed(&mut held, 1_200, field_read(None)), expected);
}

/// A hidden dialog answered to the read at a press, made as it is, breaks
/// its rule: undecodable, where a read made to check decodes it.
#[test]
fn a_hidden_dialog_answered_to_a_read_as_it_is_is_undecodable() {
    use super::super::retake::{AsItIs, CurrentFocusRead};
    use crate::event_queue::Request;
    assert!(CurrentFocusRead::<AsItIs>::decode(hidden_dialog()).is_none());
    let checked = CurrentFocusRead::<crate::focus::msaa_run::Full>::decode(hidden_dialog());
    assert!(matches!(checked, Some(super::Hidden::Dialog(_))));
}

/// Escape closing Import, alone before its show or after "a": Escape
/// reads the dialog first as typed text does, and the return to Settings,
/// in Import's recorded closing order, says the dialog and the button, as
/// NVDA's capture does after "a" (s7 `import-a-1` to `-3`).
#[test]
fn escape_closing_the_hidden_dialog_says_the_return() {
    for typed_first in [false, true] {
        let mut held = postponed();
        if typed_first {
            let _ = step(&mut held, 1_150, key(typed("a")));
            let _ = answer_out(&mut held, 1_200, field_read(None));
        }
        let escape = press(3, 2_000, PressAction::EditingChord);
        let asked: &[&str] = if typed_first { &[] } else { &[ASK_AS_IT_IS] };
        assert_eq!(step(&mut held, 2_000, escape), asked);
        if !typed_first {
            let said = answer_out(&mut held, 2_005, field_read(None));
            assert_eq!(said, KEYED.map(|(said, _)| said));
        }
        let destroyed = Input::WindowDestroyed {
            window: WindowHandle(DIALOG),
            process: Some(APP),
        };
        let _ = step(&mut held, 2_008, destroyed);
        let _ = step(&mut held, 2_010, in_front(OWNER, OWNER, APP));
        let owner = (OWNER, OWNER, OWNER);
        let foreground = tell(EventKind::Foreground, owner, (OWNER, 2_011), (APP, true));
        let asked = step(&mut held, 2_011, foreground);
        assert_eq!(asked, ["send Outpost(Pid(40)) MsaaRun 4 Check"]);
        assert_eq!(
            answer_out(&mut held, 2_100, owner_button_read()),
            [
                "Label(\"Verbatim Settings: Theme\") Role(Dialog)",
                "Label(\"Import...\") Role(Button)"
            ]
        );
        let button = owner_control_focus(OWNER_BUTTON, OWNER, 2_180);
        assert_eq!(step(&mut held, 2_180, button), NOTHING);
    }
}

/// Tab while hidden. Its read finding the field: "Files of type"
/// postpones again, and is spoken at the show. Its read finding "Files of
/// type", Import having moved its focus first: committed under Enter's
/// fence, its own focus event its duplicate, nothing heard after Tab (the
/// race Dickson ruled kept, 2026-11-20).
#[test]
fn tab_while_hidden_in_both_orders() {
    let files = read_of(
        (22, "Files of type:", Role::ComboBox),
        window_of(32, DIALOG, OWNER),
        vec![dialog_ancestor(None)],
        (32, -4),
    );
    let ids = (32, DIALOG, OWNER);
    let focus = || tell(EventKind::MsaaFocus, ids, (DIALOG, 1_250), (APP, true));
    let mut held = postponed();
    assert_eq!(step(&mut held, 1_150, key(typed("\t"))), [ASK_AS_IT_IS]);
    let said = answer_out(&mut held, 1_200, field_read(None));
    assert_eq!(said[..2], KEYED.map(|(said, _)| said));
    let asked = step(&mut held, 1_250, focus());
    assert_eq!(asked, ["send Outpost(Pid(40)) MsaaRun 32 Check"]);
    assert_eq!(answer_out(&mut held, 1_260, hidden_dialog()), NOTHING);
    assert_eq!(shown(&mut held, 1_300), [ASK_CURRENT]);
    let said = answer_out(&mut held, 1_350, files.clone());
    assert_eq!(said, ["Label(\"Files of type:\") Role(ComboBox)"]);
    let mut held = postponed();
    let _ = step(&mut held, 1_150, key(typed("\t")));
    let said = keyed(&mut held, 1_200, files);
    let files_said = [
        (KEYED[0].0.to_owned(), Some(1)),
        (
            "Label(\"Files of type:\") Role(ComboBox)".to_owned(),
            Some(1),
        ),
    ];
    assert_eq!(said, files_said);
    assert_eq!(step(&mut held, 1_250, focus()), NOTHING);
    assert_eq!(shown(&mut held, 1_300), NOTHING);
}

/// The outpost ending: its re-take waits for the next generation and
/// postpones again; the show then speaks the dialog.
#[test]
fn the_outpost_ending_postpones_again_and_the_show_speaks() {
    let mut held = postponed();
    assert_eq!(step(&mut held, 1_150, Input::ReaderEnded(OUTPOST)), NOTHING);
    let next = Instance {
        reader: Reader::Outpost(APP),
        generation: GENERATION + 1,
    };
    held.state.queue.start_for_tests(next);
    let ready_next = Input::ReaderReady(next);
    assert_eq!(step(&mut held, 1_160, ready_next), [ASK_CURRENT]);
    let hidden = answer(next, out(&held), hidden_dialog());
    assert_eq!(step(&mut held, 1_170, hidden), NOTHING);
    assert_eq!(shown(&mut held, 1_200), [ASK_CURRENT]);
    let read = answer(next, out(&held), field_read(Some("File name:")));
    assert_eq!(step(&mut held, 1_250, read), DIALOG_SPOKEN);
}

/// What ends the postponement, the dialog unsaid: its destruction, at once;
/// a focus elsewhere that executes, said alone; the dialog closed by
/// hiding, the owner's focus said; Core's own commit at the secure desktop. A destroy before the hidden answer leaves a postponement
/// of a window gone, which the next commit ends.
#[test]
fn what_ends_the_postponement() {
    let destroyed = Input::WindowDestroyed {
        window: WindowHandle(DIALOG),
        process: Some(APP),
    };
    let mut held = postponed();
    let _ = step(&mut held, 1_150, destroyed.clone());
    assert_eq!(held.state.versions.postponed, None);
    assert_eq!(shown(&mut held, 1_200), NOTHING);
    let mut held = postponed();
    let _ = step(&mut held, 1_150, owner_control_focus(7, DIALOG, 1_150));
    let said = answer_out(&mut held, 1_160, owner_control_read(7, "Rename..."));
    assert_eq!(said, ["Label(\"Rename...\") Role(Button)"]);
    assert_eq!(shown(&mut held, 1_200), NOTHING);
    let mut held = postponed();
    assert_eq!(step(&mut held, 1_150, in_front(OWNER, OWNER, APP)), NOTHING);
    let _ = step(&mut held, 1_160, owner_control_focus(6, OWNER, 1_160));
    let said = answer_out(&mut held, 1_200, owner_control_read(6, "Export..."));
    assert_eq!(said, ["Label(\"Export...\") Role(Button)"]);
    assert_eq!(held.state.versions.postponed, None);
    let mut held = dialog_asked();
    let _ = step(&mut held, 1_050, destroyed);
    let _ = answer_out(&mut held, 1_100, hidden_dialog());
    assert!(held.state.versions.postponed.is_some());
    let _ = step(&mut held, 1_150, owner_control_focus(7, OWNER, 1_150));
    let _ = answer_out(&mut held, 1_200, owner_control_read(7, "Rename..."));
    assert_eq!(held.state.versions.postponed, None);
}

/// An entry that speaks at its turn while the dialog is postponed, a
/// notification of the application holding attention, is spoken then,
/// before the dialog, which its show speaks after (an accepted change of
/// order).
#[test]
fn an_entry_that_speaks_while_postponed_speaks_before_the_dialog() {
    let mut held = postponed();
    let mut notification = crate::entries::tests::uia_tell(9, Vec::new());
    notification.kind = EventKind::UiaNotification;
    notification.payload = verbatim_model::tell::Payload::Notification {
        text: "Saved".to_owned(),
        kind: verbatim_model::NotificationKind::ActionCompleted,
        processing: verbatim_model::NotificationProcessing::All,
        activity: String::new(),
    };
    let said = step(&mut held, 1_150, crate::entries::tests::entry(notification));
    assert_eq!(said, ["Text(\"Saved\")"]);
    shown_speaks_the_dialog(&mut held, 1_200);
}

/// The secure desktop while postponed: the switch away commits Core's
/// sleeping focus, which ends the postponement; on the return, the read of
/// the focus in front finds the dialog still hidden and postpones it
/// again, and its show speaks it.
#[test]
fn the_secure_desktop_while_postponed() {
    let mut held = postponed();
    let switch = |held: &mut Held, at_us, open| {
        let input = crate::input_desktop::tests::switch(open);
        let trace = (TraceId::from_raw(at_us), at_us);
        crate::entries::route(&mut held.state, &mut held.runtime, trace, &input).said()
    };
    let _ = switch(&mut held, 1_150, false);
    assert_eq!(held.state.versions.postponed, None);
    let _ = switch(&mut held, 2_000, true);
    let due = 2_000 + crate::input_desktop::RETURN_DELAY_US;
    let trace = (TraceId::from_raw(due), due);
    let returned =
        crate::entries::route(&mut held.state, &mut held.runtime, trace, &QueueInput::Due);
    let asked: Vec<String> = returned
        .said()
        .into_iter()
        .filter_map(|said| match said {
            Said::Queue(verbatim_model::request::QueueEffect::Send { to, ask }) => {
                Some(format!("send {:?} {}", to.reader, sent(&ask.request)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(asked, [ASK_CURRENT]);
    assert_eq!(answer_out(&mut held, due + 50, hidden_dialog()), NOTHING);
    assert!(held.state.versions.postponed.is_some());
    shown_speaks_the_dialog(&mut held, due + 100);
}

/// A hang while postponed asks nothing, so no sound; a key's read that the
/// shell answers hung plays it, and the dialog stays postponed. A dialog
/// never shown holds nothing: a key 13 seconds later reads it at once.
#[test]
fn a_hang_or_a_dialog_never_shown_holds_nothing() {
    let mut held = postponed();
    let hung = Input::HangCheck {
        window: WindowHandle(DIALOG),
        hung: true,
    };
    assert_eq!(step(&mut held, 1_150, hung), NOTHING);
    let _ = step(&mut held, 1_200, key(typed("a")));
    let in_front_hung = Input::InFront {
        number: verbatim_model::request::RequestNumber(out(&held)),
        front: Front::Hung,
    };
    assert_eq!(step(&mut held, 1_250, in_front_hung), ["not responding"]);
    assert!(held.state.versions.postponed.is_some());
    let mut held = postponed();
    let late = press(2, 14_000_000, typed("a"));
    assert_eq!(step(&mut held, 14_000_000, late), [ASK_AS_IT_IS]);
}

/// A UI Automation completion answered hidden postpones its dialog; the
/// show reads the current focus. Only the request is pinned: no real
/// application was recorded.
#[test]
fn a_uia_completion_postpones_and_the_show_reads_the_current_focus() {
    let mut held = crate::entries::tests::asked_completion();
    let hidden = Reply::HiddenDialog(crate::entries::tests::win(4));
    assert_eq!(
        step(&mut held, 3_000, answer(OUTPOST, 2, hidden.clone())),
        NOTHING
    );
    assert!(held.state.versions.postponed.is_some());
    // The first foreground report's startup read and its marker each read
    // it, and it postpones again.
    assert_eq!(step(&mut held, 3_050, in_front(4, 4, APP)), [ASK_CURRENT]);
    assert_eq!(answer_out(&mut held, 3_060, hidden.clone()), [ASK_CURRENT]);
    assert_eq!(answer_out(&mut held, 3_070, hidden), NOTHING);
    let shown = show(4, 3_100);
    assert_eq!(step(&mut held, 3_100, shown), [ASK_CURRENT]);
}

/// Stand-in order, no real application recorded: a message box the hidden
/// dialog owns, in front after the postponement or while the dialog's read
/// is out, is spoken, its commit ending the postponement; after OK the
/// dialog postpones again and its show speaks it (a difference Dickson
/// approved, 2026-11-20: NVDA says it at once).
#[test]
fn an_owned_message_box_in_either_order() {
    for before in [false, true] {
        let mut held = dialog_asked();
        if before {
            let _ = step(&mut held, 1_050, in_front(BOX, OWNER, APP));
        }
        let _ = answer_out(&mut held, 1_100, hidden_dialog());
        let _ = step(&mut held, 1_150, in_front(BOX, OWNER, APP));
        let asked = step(&mut held, 1_160, box_foreground(1_160));
        assert_eq!(asked, ["send Outpost(Pid(40)) MsaaRun 40 Check"]);
        let said = answer_out(&mut held, 1_200, box_read());
        assert_eq!(said, ["Label(\"Replace it?\") Role(Dialog)"]);
        assert_eq!(held.state.versions.postponed, None);
        let back = in_front(DIALOG, OWNER, APP);
        assert_eq!(step(&mut held, 2_000, back), NOTHING);
        let foreground = dialog_foreground(2_010);
        assert_eq!(step(&mut held, 2_010, foreground), [ASK_DIALOG]);
        assert_eq!(answer_out(&mut held, 2_050, hidden_dialog()), NOTHING);
        shown_speaks_the_dialog(&mut held, 2_100);
    }
}

/// Stand-in order, no real application recorded: a tray menu over a hidden
/// dialog-based main window is spoken alone; after Escape, the check after
/// the menu postpones the dialog and nothing is said. The startup read
/// postpones it first, and the foreground report's marker behind it reads
/// it again, hidden still.
#[test]
fn a_tray_menu_over_a_hidden_dialog_then_escape() {
    let mut held = ready();
    let _ = step(&mut held, 100, in_front(DIALOG, DIALOG, APP));
    assert_eq!(answer_out(&mut held, 200, hidden_dialog()), [ASK_CURRENT]);
    assert_eq!(answer_out(&mut held, 300, hidden_dialog()), NOTHING);
    let asked = step(&mut held, 1_150, menu_start(APP, 1_150));
    assert_eq!(asked, ["send Outpost(Pid(40)) MsaaRun 50 ReadAnyway"]);
    let said = answer_out(&mut held, 1_200, menu_read());
    assert_eq!(said, ["Label(\"Context\") Role(Menu)"]);
    let mut end = msaa_tell(-4);
    end.kind = EventKind::PopupEnd;
    let _ = step(&mut held, 2_000, crate::entries::tests::entry(end));
    assert_eq!(step(&mut held, 52_000, Input::Tick), [ASK_CURRENT]);
    assert_eq!(answer_out(&mut held, 52_100, hidden_dialog()), NOTHING);
    assert!(held.state.versions.postponed.is_some());
}

/// A focus on the owner's window `window`, its own control, raised at
/// `at_us` with `front` in front, accepted by its root owner.
fn owner_control_focus(window: u64, front: u64, at_us: u64) -> Input<Entry> {
    tell(
        EventKind::MsaaFocus,
        (window, OWNER, OWNER),
        (front, at_us),
        (APP, true),
    )
}

/// The owner's control in window `window` read, named `name`.
fn owner_control_read(window: u64, name: &str) -> Reply {
    read_of(
        (12, name, Role::Button),
        window_of(window, OWNER, OWNER),
        vec![FocusAncestor {
            node: focus_node(
                10,
                window_of(OWNER, OWNER, OWNER),
                "Verbatim Settings: Theme",
                Role::Dialog,
            ),
            simple_child: false,
        }],
        (window, -4),
    )
}

/// The message box's foreground tell and its read: the box itself.
fn box_foreground(at_us: u64) -> Input<Entry> {
    tell(
        EventKind::Foreground,
        (BOX, BOX, OWNER),
        (BOX, at_us),
        (APP, true),
    )
}

fn box_read() -> Reply {
    read_of(
        (40, "Replace it?", Role::Dialog),
        window_of(BOX, BOX, OWNER),
        Vec::new(),
        (BOX, -4),
    )
}

/// A popup menu's start in `process`'s menu window.
fn menu_start(process: Pid, at_us: u64) -> Input<Entry> {
    tell(
        EventKind::PopupStart,
        (MENU, MENU, MENU),
        (DIALOG, at_us),
        (process, false),
    )
}

/// The popup menu read: its role, read for a menu start, and the menu.
fn menu_read() -> Reply {
    let Reply::Candidate(mut answer) = read_of(
        (50, "Context", Role::Menu),
        window_of(MENU, MENU, MENU),
        Vec::new(),
        (MENU, -4),
    ) else {
        unreachable!("a candidate's answer")
    };
    answer.facts = verbatim_model::request::CandidateFacts::Found {
        focused: None,
        role: Some(Role::Menu),
    };
    Reply::Candidate(answer)
}

/// An answer finding a candidate without the focused state.
fn unfocused() -> Reply {
    Reply::Candidate(crate::entries::tests::not_focused())
}

/// A key press of `action`, the `seq`th key.
fn press(seq: u64, at_us: u64, action: verbatim_model::request::PressAction) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(crate::keys::entry::PressEntry::new(
        TraceId::from_raw(at_us),
        verbatim_model::request::Press {
            key_seq: KeySeq::new(seq),
            pressed_at_us: at_us,
            action,
        },
    ))))
}

fn typed(text: &str) -> verbatim_model::request::PressAction {
    verbatim_model::request::PressAction::Typed {
        text: text.to_owned(),
    }
}
