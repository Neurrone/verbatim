//! A dialog read once it is shown (Dickson, 2026-11-19): one test per cell
//! of the design's table for Core's front entry waiting for a hidden
//! dialog's show, each with its exact speech, the three sequences its
//! review derived fed in the limiter's recorded split (the dialog's
//! foreground alone, then the run holding its field), each in both orders:
//! the ending arriving while the dialog's first read is out, and after its
//! wait has begun.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{
    CandidateAnswer, FocusAncestor, FocusCaret, FocusNode, FocusRead, HeldIds, MsaaIds, NodeRead,
    NodeWindow, ObjectClass, Outcome as WireOutcome, ProcessFact, Reply, Request as Wire,
};
use verbatim_model::tell::{AcceptanceFacts, EventObject, Relation, TellWindow, WindowFacet};
use verbatim_model::{Effect as Said, KeySeq, Pid, Role, SegmentContent, TraceId, WindowHandle};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, NOTHING, OUTPOST, answer, answer_outcome, msaa_tell, node,
    ready,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Effect, Foreground, Input, Instance, Reader, Timed};

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
        window: facet(window, "Edit", process),
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
    Input::Entry(Entry::of_tell(LISTENER, event).expect("a candidate"))
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
        class: ObjectClass::Plain,
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
        dialog_text: text.map(str::to_owned),
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
            dialog_text: None,
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
            Effect::Send { to, ask } => {
                Some(format!("send {:?} {}", to.reader, sent(&ask.request)))
            }
            Effect::Cancel { to, cancel } => {
                Some(format!("cancel {:?} {:?}", to.reader, cancel.reason))
            }
            Effect::NotRespondingSound => Some("not responding".to_owned()),
            Effect::Said(said) => spoken(&said),
            _ => None,
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

/// A request as these tests name it.
fn sent(request: &Wire) -> String {
    match request {
        Wire::MsaaRun(run) => format!("MsaaRun {} {:?}", run.candidate.ids.window.0, run.hidden),
        Wire::CurrentFocus { hidden, .. } => format!("CurrentFocus {hidden:?}"),
        Wire::AwaitShown { window } => format!("AwaitShown {}", window.0),
        Wire::CompleteUia(complete) => format!("CompleteUia {:?}", complete.hidden),
        Wire::Follow(_) => "Follow".to_owned(),
        other => format!("{other:?}"),
    }
}

/// The number of the request out, of the newest live.
fn out(held: &Held) -> u64 {
    held.state.queue.last_number_for_tests()
}

const ASK_DIALOG: &str = "send Outpost(Pid(40)) MsaaRun 30 Check";
const ASK_FIELD: &str = "send Outpost(Pid(40)) MsaaRun 31 Check";
const AWAIT: &str = "send Listener AwaitShown 30";
const CANCEL_LEFT: &str = "cancel Listener LeftHiddenDialog";

/// The owner's title and the dialog's text, then the field, as NVDA's
/// capture speaks them.
const DIALOG_SPOKEN: [&str; 2] = [
    "Label(\"Import a Theme\") Role(Dialog) Description(\"File name:\")",
    "Label(\"File name:\") Role(EditableText)",
];

/// A ready Core whose committed focus is the owner's button, the owner in
/// front, the file dialog not yet come.
fn on_the_owner() -> Held {
    let mut held = ready();
    step(&mut held, 100, in_front(OWNER, OWNER, APP));
    let _ = step(&mut held, 200, answer(OUTPOST, 1, owner_button_read()));
    held
}

/// The dialog came to the foreground hidden, its foreground alone in the
/// first run and its field's focus behind it, the first read asked.
fn dialog_asked() -> Held {
    let mut held = on_the_owner();
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

/// The dialog's first read answered hidden: the listener asked to tell
/// its show.
fn waiting() -> Held {
    let mut held = dialog_asked();
    let number = out(&held);
    assert_eq!(
        step(&mut held, 1_100, answer(OUTPOST, number, hidden_dialog())),
        [AWAIT]
    );
    held
}

/// Answers the read out with `reply` at `at_us`.
fn answer_out(held: &mut Held, at_us: u64, reply: Reply) -> Vec<String> {
    let number = out(held);
    step(held, at_us, answer(OUTPOST, number, reply))
}

/// The listener answers the wait out: shown.
fn shown(held: &mut Held, at_us: u64) -> Vec<String> {
    let number = out(held);
    step(held, at_us, answer(LISTENER, number, Reply::Shown))
}

#[test]
fn a_hidden_dialog_is_read_again_once_shown_and_speaks_its_text_then_its_field() {
    let mut held = waiting();
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
    assert_eq!(
        answer_out(&mut held, 1_250, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
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
            dialog_text: None,
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

const ASK_DIALOG_ANYWAY: &str = "send Outpost(Pid(40)) MsaaRun 30 ReadAnyway";
const CANCEL_KEY: &str = "cancel Listener Withdrawn";

/// The dialog's name with no text, as a key read it hidden, then the field.
const DIALOG_SPOKEN_HIDDEN: [&str; 2] = [
    "Label(\"Import a Theme\") Role(Dialog)",
    "Label(\"File name:\") Role(EditableText)",
];

/// Import's recorded split: the foreground alone in the first run, the
/// field's focus in the run behind. Read once shown, the dialog speaks its
/// text then its field, and the field's own run behind is its duplicate.
#[test]
fn the_field_behind_the_dialog_is_said_once_with_it() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_020, field_focus(1_020)), NOTHING);
    let number = out(&held);
    assert_eq!(
        step(&mut held, 1_100, answer(OUTPOST, number, hidden_dialog())),
        [AWAIT]
    );
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
    assert_eq!(
        answer_out(&mut held, 1_250, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// The foreground moving to the hidden dialog again changes the record
/// alone: the wait goes on.
#[test]
fn the_foreground_coming_to_the_dialog_leaves_the_wait() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, in_front(DIALOG, OWNER, APP)),
        NOTHING
    );
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
}

/// A focus inside the hidden dialog read while the record still names the
/// owner, its own foreground report not yet come: nothing is spoken and no
/// wait begins. The dialog's own foreground tell behind it reads it again,
/// and the dialog and its field are spoken once, after the show.
#[test]
fn a_focus_inside_the_dialog_before_its_foreground_report_is_spoken_after_the_show() {
    let mut held = on_the_owner();
    assert_eq!(step(&mut held, 1_000, field_focus(1_000)), [ASK_FIELD]);
    assert_eq!(answer_out(&mut held, 1_050, hidden_dialog()), NOTHING);
    assert_eq!(
        step(&mut held, 1_060, in_front(DIALOG, OWNER, APP)),
        NOTHING
    );
    assert_eq!(
        step(&mut held, 1_070, dialog_foreground(1_070)),
        [ASK_DIALOG]
    );
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), [AWAIT]);
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
    assert_eq!(
        answer_out(&mut held, 1_250, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
}

/// A message box the hidden dialog owns takes the foreground while the
/// dialog waits for its show: the wait ends, nothing said for the dialog,
/// and the box is spoken. After OK, the dialog, read once shown, speaks its
/// text, then its field.
#[test]
fn an_owned_message_box_during_the_wait_is_spoken_and_the_dialog_after_it() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, in_front(BOX, OWNER, APP)),
        [CANCEL_LEFT]
    );
    message_box_then_dialog(&mut held);
}

/// The same, the box coming to the foreground while the dialog's first read
/// is out: the wait never begins.
#[test]
fn an_owned_message_box_before_the_wait_is_spoken_and_the_dialog_after_it() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_050, in_front(BOX, OWNER, APP)), NOTHING);
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), NOTHING);
    message_box_then_dialog(&mut held);
}

/// The box's foreground tell and its speech; after OK, the dialog in front
/// again, read shown.
fn message_box_then_dialog(held: &mut Held) {
    assert_eq!(
        step(held, 1_160, box_foreground(1_160)),
        ["send Outpost(Pid(40)) MsaaRun 40 Check"]
    );
    assert_eq!(
        answer_out(held, 1_200, box_read()),
        ["Label(\"Replace it?\") Role(Dialog)"]
    );
    assert_eq!(step(held, 2_000, in_front(DIALOG, OWNER, APP)), NOTHING);
    assert_eq!(step(held, 2_010, dialog_foreground(2_010)), [ASK_DIALOG]);
    assert_eq!(
        answer_out(held, 2_050, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
}

/// A dialog closed by hiding before its show, while it waits: the
/// foreground returns to the owner, the wait ends with nothing said for the
/// dialog, and the owner's focus is spoken.
#[test]
fn a_dialog_closed_by_hiding_during_the_wait_speaks_the_owners_focus() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, in_front(OWNER, OWNER, APP)),
        [CANCEL_LEFT]
    );
    owners_focus_spoken(&mut held);
}

/// The same, closed while the dialog's first read is out.
#[test]
fn a_dialog_closed_by_hiding_before_the_wait_speaks_the_owners_focus() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_050, in_front(OWNER, OWNER, APP)), NOTHING);
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), NOTHING);
    owners_focus_spoken(&mut held);
}

fn owners_focus_spoken(held: &mut Held) {
    assert_eq!(
        step(held, 1_160, owner_control_focus(6, OWNER, 1_160)),
        ["send Outpost(Pid(40)) MsaaRun 6 Check"]
    );
    assert_eq!(
        answer_out(held, 1_200, owner_control_read(6, "Export...")),
        ["Label(\"Export...\") Role(Button)"]
    );
}

/// A dialog-based main window, hidden and never shown, in front, as a tray
/// icon's menu opens over it: the hidden dialog's wait, its window its own
/// owner, and the menu's start behind it.
fn tray_waiting() -> Held {
    let mut held = ready();
    step(&mut held, 100, in_front(DIALOG, DIALOG, APP));
    let _ = step(
        &mut held,
        200,
        answer_outcome(OUTPOST, 1, WireOutcome::NotThere),
    );
    held
}

/// The menu start behind ends the wait, and the menu is spoken as Core
/// speaks a menu, nothing said for the dialog.
#[test]
fn a_menu_start_during_the_wait_ends_it_and_the_menu_is_spoken() {
    let mut held = tray_waiting();
    assert_eq!(
        step(&mut held, 1_010, dialog_foreground(1_010)),
        [ASK_DIALOG]
    );
    assert_eq!(answer_out(&mut held, 1_100, hidden_dialog()), [AWAIT]);
    assert_eq!(
        step(&mut held, 1_150, menu_start(APP, 1_150)),
        [CANCEL_LEFT, "send Outpost(Pid(40)) MsaaRun 50 ReadAnyway"]
    );
    assert_eq!(
        answer_out(&mut held, 1_200, menu_read()),
        ["Label(\"Context\") Role(Menu)"]
    );
}

/// The same, the menu starting while the dialog's first read is out.
#[test]
fn a_menu_start_before_the_wait_ends_it_and_the_menu_is_spoken() {
    let mut held = tray_waiting();
    assert_eq!(
        step(&mut held, 1_010, dialog_foreground(1_010)),
        [ASK_DIALOG]
    );
    assert_eq!(step(&mut held, 1_050, menu_start(APP, 1_050)), NOTHING);
    assert_eq!(
        answer_out(&mut held, 1_100, hidden_dialog()),
        ["send Outpost(Pid(40)) MsaaRun 50 ReadAnyway"]
    );
    assert_eq!(
        answer_out(&mut held, 1_200, menu_read()),
        ["Label(\"Context\") Role(Menu)"]
    );
}

/// A background application's menu start joins the queue behind, and the
/// wait goes on: the dialog is spoken at its show.
#[test]
fn a_background_menu_start_leaves_the_wait() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, menu_start(OTHER_APP, 1_150)),
        NOTHING
    );
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
    let said = answer_out(&mut held, 1_250, field_read(Some("File name:")));
    assert_eq!(said[..2], DIALOG_SPOKEN);
}

/// A background application's focus, refused by acceptance, joins the
/// queue behind, and the wait goes on: the dialog is spoken at its show,
/// with its text, then its field.
#[test]
fn a_background_focus_refused_by_acceptance_leaves_the_wait() {
    let mut held = waiting();
    let background = tell(
        EventKind::MsaaFocus,
        (60, 60, 60),
        (DIALOG, 1_150),
        (OTHER_APP, false),
    );
    assert_eq!(step(&mut held, 1_150, background), NOTHING);
    assert_eq!(shown(&mut held, 1_200), [ASK_DIALOG]);
    assert_eq!(
        answer_out(&mut held, 1_250, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
}

/// A focus of the owner's that the rows needing no read accept ends the
/// wait, then is refused by its read as not focused while the dialog is
/// still in front: its run executed nothing, so the current focus is read
/// as a re-take, which waits for the show and speaks the dialog with its
/// text, then its field.
#[test]
fn an_ending_focus_refused_by_its_read_reads_the_dialog_again_once_shown() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, owner_control_focus(6, DIALOG, 1_150)),
        [CANCEL_LEFT, "send Outpost(Pid(40)) MsaaRun 6 Check"]
    );
    assert_eq!(
        answer_out(&mut held, 1_200, unfocused()),
        ["send Outpost(Pid(40)) CurrentFocus Check"]
    );
    assert_eq!(answer_out(&mut held, 1_250, hidden_dialog()), [AWAIT]);
    assert_eq!(
        shown(&mut held, 1_300),
        ["send Outpost(Pid(40)) CurrentFocus Check"]
    );
    assert_eq!(
        answer_out(&mut held, 1_350, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
}

/// A run carrying the ending focus whose newest candidate is refused and
/// whose older candidate executes joins no re-take.
#[test]
fn an_ending_run_whose_older_candidate_executes_reads_nothing_again() {
    let mut held = dialog_asked();
    assert_eq!(
        step(&mut held, 1_020, owner_control_focus(7, DIALOG, 1_020)),
        NOTHING
    );
    assert_eq!(
        step(&mut held, 1_030, owner_control_focus(6, DIALOG, 1_030)),
        NOTHING
    );
    assert_eq!(
        answer_out(&mut held, 1_100, hidden_dialog()),
        ["send Outpost(Pid(40)) MsaaRun 6 Check"]
    );
    assert_eq!(
        answer_out(&mut held, 1_150, unfocused()),
        ["send Outpost(Pid(40)) MsaaRun 7 Check"]
    );
    assert_eq!(
        answer_out(&mut held, 1_200, owner_control_read(7, "Rename...")),
        ["Label(\"Rename...\") Role(Button)"]
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// The ending focus merged into a newer tell for the same object, as the
/// limiter merges a run's candidates, leaves the newer one the hidden
/// dialog whose wait it ended.
#[test]
fn an_ending_focus_merged_into_a_newer_one_keeps_the_dialog() {
    let held = on_the_owner();
    let candidate = |at_us| {
        let Input::Entry(entry) = owner_control_focus(6, DIALOG, at_us) else {
            unreachable!("an entry")
        };
        entry
    };
    let mut ending = candidate(1_000);
    if let Entry::Candidate(focus) = &mut ending {
        focus.ended_wait = Some(WindowHandle(DIALOG));
    }
    let mut waiting = std::collections::VecDeque::new();
    crate::event_queue::arrive::<crate::entries::Entries>(&held.state, &mut waiting, ending);
    crate::event_queue::arrive::<crate::entries::Entries>(
        &held.state,
        &mut waiting,
        candidate(1_500),
    );
    let carried: Vec<(u64, Option<WindowHandle>)> = waiting
        .iter()
        .flat_map(|item| match item {
            crate::event_queue::Item::Run(entries) => entries.clone(),
            _ => Vec::new(),
        })
        .filter_map(|entry| match entry {
            Entry::Candidate(focus) => Some((focus.tell.at_us, focus.ended_wait)),
            _ => None,
        })
        .collect();
    assert_eq!(carried, [(1_500, Some(WindowHandle(DIALOG)))]);
}

/// A key pressed while the dialog waits for its show cancels the wait:
/// the focus is read at once, hidden, committed and spoken, the dialog
/// with no text, then its field; then the key is handled against that
/// focus, its typed character echoed in the field.
#[test]
fn a_key_during_the_wait_reads_the_dialog_at_once_then_is_handled() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, press(1, 1_150, typed("a"))),
        [CANCEL_KEY, ASK_DIALOG_ANYWAY]
    );
    let said = answer_out(&mut held, 1_200, field_read(None));
    assert_eq!(said[..2], DIALOG_SPOKEN_HIDDEN);
    assert_eq!(said[2..], ["Character(\"a\")"]);
}

/// The same, the key pressed while the dialog's first read is out: no
/// wait begins.
#[test]
fn a_key_before_the_wait_reads_the_dialog_at_once_then_is_handled() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_050, press(1, 1_050, typed("a"))), NOTHING);
    assert_eq!(
        answer_out(&mut held, 1_100, hidden_dialog()),
        [ASK_DIALOG_ANYWAY]
    );
    let said = answer_out(&mut held, 1_200, field_read(None));
    assert_eq!(said[..2], DIALOG_SPOKEN_HIDDEN);
    assert_eq!(said[2..], ["Character(\"a\")"]);
}

/// Tab pressed while the dialog waits: the dialog and its field are
/// committed and spoken first, and the Tab is judged from the field.
#[test]
fn tab_during_the_wait_is_judged_from_the_field() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_150, press(1, 1_150, typed("\t"))),
        [CANCEL_KEY, ASK_DIALOG_ANYWAY]
    );
    let said = answer_out(&mut held, 1_200, field_read(None));
    assert_eq!(said[..2], DIALOG_SPOKEN_HIDDEN);
    assert_eq!(
        held.state
            .focus
            .object()
            .map(|focus| focus.snapshot.name.clone()),
        Some(Some("File name:".to_owned()))
    );
}

/// A key and another ending both holding, the other wins: nothing is said
/// for the dialog.
#[test]
fn a_key_with_the_user_elsewhere_says_nothing_for_the_dialog() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_050, press(1, 1_050, typed("a"))), NOTHING);
    assert_eq!(step(&mut held, 1_060, in_front(OWNER, OWNER, APP)), NOTHING);
    let said = answer_out(&mut held, 1_100, hidden_dialog());
    assert!(!said.iter().any(|said| said.contains("Import a Theme")));
}

/// The dialog destroyed while it waits: answered not there, its run ends
/// and nothing is said.
#[test]
fn a_dialog_destroyed_during_the_wait_says_nothing() {
    let mut held = waiting();
    let number = out(&held);
    assert_eq!(
        step(
            &mut held,
            1_200,
            answer_outcome(LISTENER, number, WireOutcome::NotThere)
        ),
        NOTHING
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// The dialog's application exits while it waits: Core ends the wait not
/// there and cancels it to the listener, which has no exit to end it.
#[test]
fn the_dialogs_application_exiting_ends_the_wait_and_cancels_it() {
    let mut held = waiting();
    assert_eq!(
        step(
            &mut held,
            1_200,
            Input::Process(ProcessFact::ApplicationExited(APP))
        ),
        [CANCEL_KEY]
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A dialog never shown, with no key and no other ending, holds the queue
/// until the due time, 13 seconds from its read, then ends unspoken.
#[test]
fn a_dialog_never_shown_ends_at_the_due_time_unspoken() {
    let mut held = waiting();
    assert_eq!(step(&mut held, 13_000_000, Input::Tick), NOTHING);
    assert_eq!(
        step(&mut held, 13_001_010, Input::Tick),
        ["cancel Listener DueTime"]
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// The live hang check finding the dialog hung: not responding, as today,
/// the sound while its application holds attention.
#[test]
fn a_dialog_hung_during_the_wait_is_not_responding() {
    let mut held = waiting();
    assert_eq!(
        step(
            &mut held,
            1_200,
            Input::HangCheck {
                window: WindowHandle(DIALOG),
                hung: true
            }
        ),
        ["cancel Listener Hung", "not responding"]
    );
}

/// The listener ends during the wait, the committed focus read by MSAA:
/// the run ends, the re-take reads the dialog, whose wait waits for the
/// replacement listener, and the dialog and its field are spoken once
/// shown.
#[test]
fn the_listener_ending_during_the_wait_reads_the_dialog_again_once_shown() {
    let mut held = waiting();
    assert_eq!(
        step(&mut held, 1_200, Input::ReaderEnded(LISTENER)),
        ["send Outpost(Pid(40)) CurrentFocus Check"]
    );
    assert_eq!(answer_out(&mut held, 1_250, hidden_dialog()), NOTHING);
    let replacement = Instance {
        reader: Reader::Listener,
        generation: 2,
    };
    held.state.queue.start_for_tests(replacement);
    assert_eq!(
        step(&mut held, 1_300, Input::ReaderReady(replacement)),
        [AWAIT]
    );
    let number = out(&held);
    assert_eq!(
        step(&mut held, 1_350, answer(replacement, number, Reply::Shown)),
        ["send Outpost(Pid(40)) CurrentFocus Check"]
    );
    assert_eq!(
        answer_out(&mut held, 1_400, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
}

/// The application's outpost ends during the wait: the read after the
/// show goes to its next generation.
#[test]
fn the_outpost_ending_during_the_wait_reads_from_its_next_generation() {
    let mut held = waiting();
    let _ = step(&mut held, 1_200, Input::ReaderEnded(OUTPOST));
    let next = Instance {
        reader: Reader::Outpost(APP),
        generation: GENERATION + 1,
    };
    held.state.queue.start_for_tests(next);
    assert_eq!(step(&mut held, 1_250, Input::ReaderReady(next)), NOTHING);
    assert_eq!(shown(&mut held, 1_300), [ASK_DIALOG]);
    let number = out(&held);
    assert_eq!(
        step(
            &mut held,
            1_350,
            answer(next, number, field_read(Some("File name:")))
        ),
        [
            DIALOG_SPOKEN[0],
            DIALOG_SPOKEN[1],
            "send Outpost(Pid(40)) CurrentFocus Check"
        ]
    );
    // The outpost's end joined a re-take behind, which reads the same
    // focus and takes it silently.
    let number = out(&held);
    assert_eq!(
        step(
            &mut held,
            1_400,
            answer(next, number, field_read(Some("File name:")))
        ),
        NOTHING
    );
}

/// The request a read answered hidden asks of the listener names the
/// dialog's window, and a read answered with no hidden dialog asks the
/// listener nothing.
#[test]
fn a_shown_dialog_asks_nothing_of_the_listener() {
    let mut held = dialog_asked();
    assert_eq!(
        answer_out(&mut held, 1_100, field_read(Some("File name:"))),
        DIALOG_SPOKEN
    );
    assert!(
        !held
            .state
            .queue
            .requests()
            .any(|request| matches!(request, Wire::AwaitShown { .. }))
    );
}

/// A UI Automation focus whose completion answers a hidden dialog waits
/// for its show, then is completed again from the listener's route, and
/// executed.
#[test]
fn a_uia_focus_in_a_hidden_dialog_is_completed_again_once_shown() {
    let mut held = crate::entries::tests::asked_completion();
    assert_eq!(
        step(
            &mut held,
            3_000,
            answer(
                OUTPOST,
                2,
                Reply::HiddenDialog(crate::entries::tests::win(4))
            )
        ),
        ["send Listener AwaitShown 4"]
    );
    assert_eq!(
        step(&mut held, 3_100, answer(LISTENER, 3, Reply::Shown)),
        ["send Outpost(Pid(40)) CompleteUia Check"]
    );
    let read = crate::entries::tests::focus_read(None);
    assert_eq!(
        step(
            &mut held,
            3_200,
            answer(OUTPOST, 4, Reply::Focus(Box::new(read)))
        ),
        crate::entries::tests::SPOKEN_FOLLOWED
    );
}

/// The front's function rebuilt from its checkpoint, as after a clone or a
/// restored dump, once a key ended the wait before it began: the rebuilt
/// function takes the ending from the checkpoint, not from the queue as
/// its base had it, which held no key, and goes on alike.
#[test]
fn a_wait_ended_by_a_key_before_it_began_rebuilds_alike() {
    let mut held = dialog_asked();
    assert_eq!(step(&mut held, 1_050, press(1, 1_050, typed("a"))), NOTHING);
    assert_eq!(
        answer_out(&mut held, 1_100, hidden_dialog()),
        [ASK_DIALOG_ANYWAY]
    );
    let mut rebuilt = held.clone();
    let said = answer_out(&mut rebuilt, 1_200, field_read(None));
    assert_eq!(said[..2], DIALOG_SPOKEN_HIDDEN);
    assert_eq!(said[2..], ["Character(\"a\")"]);
}
