//! Caret keys, caret tells and selection changes, with Core's reducer
//! driving a real outpost watching mockapp's text
//! (`docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
//! this build" and "Selection and the caret's text facts"; Tables 7, 15 and
//! 17).
//!
//! A caret key runs at its turn: a starting read, its stroke sent, then the
//! outpost's watch, answered by a check that finds the key's effect,
//! polled or brought forward by a UI Automation caret event within 60 ms
//! of the pass, or at the bound, after which Core speaks the key's unit. A focus judged pending withdraws the
//! watch and the key says nothing. A caret tell, the outpost's for a UI
//! Automation caret event or the listener's for the system caret, asks one
//! caret read naming Core's selection baseline, and Core speaks the
//! difference, in general terms when the text-changed flag is set or a
//! value change of an edit control waits. The tests of what the outpost's
//! reads answer, which Core's speech does not show, ask the outpost
//! through its request driver.
//!
//! Core's time is the run's shared clock (`CoreTime::Real`), on which the
//! outpost times a watch from the pass Core stamps. Each test runs on a
//! desktop of its own (`harness::run_isolated`), so no other client sees
//! mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::request::{Outcome, Reply, Request};
use verbatim_model::{
    CaretKey, CaretMotion, Message, NodeId, Phrase, Role, SegmentContent, SelectionChange,
    SelectionDifference, SelectionRead, SelectionReadReply, SelectionText, TextAnchor, TextFormat,
    TextOp, TextPoint, TextPosition, TextProvider, TextReply,
};
use verbatim_outpost::Heard;
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

use common::core_driver::{CoreDriver, CoreTime, Utterance};
use common::outpost::OutpostUnderTest;

/// What a step said that is nothing.
const NOTHING: [Utterance; 0] = [];

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// mockapp's `fixture` in its UIA backend, with Core driving an outpost
/// watching it on the run's shared clock.
fn start(fixture: &str, name: &str) -> (common::MockApp, HWND, CoreDriver) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let core = CoreDriver::on_time(&app, OutpostUnderTest::new(&app), CoreTime::Real);
    (app, hwnd, core)
}

/// The caret key of `motion`, Shift up.
fn key(motion: CaretMotion) -> CaretKey {
    CaretKey {
        motion,
        select: false,
    }
}

/// The caret key of `motion` with Shift: it selects, so it passes at once.
fn selecting(motion: CaretMotion) -> CaretKey {
    CaretKey {
        motion,
        select: true,
    }
}

/// What Core says for mockapp's caret watch fixture's window and its
/// "Notes" document with the caret on "alpha beta": its line with its
/// formatting at the line's start, every detail being reported, as NVDA
/// speaks a focus's line (`speech/speech.py` 904 to 922).
fn notes_focus() -> Vec<Utterance> {
    vec![
        vec![
            SegmentContent::Label("Mockapp Caret Watch Fixture".to_owned()),
            SegmentContent::Role(Role::Window),
        ],
        vec![
            SegmentContent::Label("Notes".to_owned()),
            SegmentContent::Role(Role::EditableText),
        ],
        vec![
            SegmentContent::Format(TextFormat::FontName("Consolas".to_owned())),
            SegmentContent::Format(TextFormat::FontSize("11.0 pt".to_owned())),
            SegmentContent::Format(TextFormat::Color("black".to_owned())),
            SegmentContent::Text("alpha beta".to_owned()),
        ],
    ]
}

/// mockapp's caret watch fixture with the "Notes" document focused and
/// spoken, its caret at the text's start, nothing selected.
fn on_notes(name: &str) -> (common::MockApp, HWND, CoreDriver) {
    let (mut app, hwnd, mut core) = start("caret_watch.json", name);
    app.send("set-focus doc");
    assert_eq!(core.uia_focus(&focused(hwnd)).spoken(), notes_focus());
    (app, hwnd, core)
}

/// Has mockapp run `lines` and raise a UI Automation caret event on the
/// document, and tells Core the outpost's caret tell: what Core said once
/// its read was answered.
fn caret_event(app: &mut common::MockApp, core: &mut CoreDriver, lines: &[&str]) -> Vec<Utterance> {
    for line in lines {
        app.send(line);
    }
    app.send("caret-event doc");
    core.told(1).spoken()
}

/// One utterance of one phrase.
fn phrase(phrase: Phrase) -> Vec<Utterance> {
    vec![vec![SegmentContent::Phrase(phrase)]]
}

/// `text`, five characters or more, selected.
fn selected(text: &str) -> Vec<Utterance> {
    phrase(Phrase::Selected(SelectionText::Text(text.to_owned())))
}

/// `text`, five characters or more, unselected.
fn unselected(text: &str) -> Vec<Utterance> {
    phrase(Phrase::Unselected(SelectionText::Text(text.to_owned())))
}

/// One character, said as a key's unit.
fn character(text: &str) -> Vec<Utterance> {
    vec![vec![SegmentContent::Character(text.to_owned())]]
}

/// A key's move found by the watch's first check after its stroke reached
/// the application, with no event: its unit at the caret where the key took
/// it, "l" at offset 1. NVDA: `editableText.py` 104 to 192.
fn a_keys_move_found_by_its_watch_says_its_unit() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-found");
    let mut said = core
        .caret_key(key(CaretMotion::NextCharacter), || app.send("caret doc 1"))
        .spoken();
    said.extend(core.answered().spoken());
    assert_eq!(said, character("l"));
    drop(core);
    app.quit();
}

/// A focus's line and a key's unit are said with their formatting, the
/// line's read with the focus and the unit's in the watch's program, as
/// NVDA speaks both with the formatting the settings report
/// (`speech/speech.py` 904 to 922), each change from the formatting last
/// said. The focus says mockapp's line from its bold "alpha" to its
/// misspelt "beta", so Control+Right Arrow onto "beta" says only the word;
/// Control+Left Arrow onto "alpha" says it bold, out of the spelling error,
/// and the change its space makes; Control+Right Arrow onto "beta" again
/// says the spelling error.
fn a_focus_line_and_a_keys_unit_are_said_with_their_formatting() {
    let (mut app, hwnd, mut core) = start("text.json", "mockapp-caret-watch-formatting");
    app.send("set-focus doc");
    let focus = core.uia_focus(&focused(hwnd)).spoken();
    assert_eq!(
        focus.last(),
        Some(&vec![
            SegmentContent::Format(TextFormat::FontName("Consolas".to_owned())),
            SegmentContent::Format(TextFormat::FontSize("11.0 pt".to_owned())),
            SegmentContent::Format(TextFormat::Color("black".to_owned())),
            SegmentContent::Format(TextFormat::Bold),
            SegmentContent::Text("alpha".to_owned()),
            SegmentContent::Format(TextFormat::NotBold),
            SegmentContent::Format(TextFormat::SpellingError),
            SegmentContent::Text("beta".to_owned()),
        ])
    );
    let mut key_to = |motion, caret: &str| {
        let mut said = core.caret_key(key(motion), || app.send(caret)).spoken();
        said.extend(core.answered().spoken());
        said
    };
    assert_eq!(
        key_to(CaretMotion::NextWord, "caret doc 6"),
        [vec![SegmentContent::Text("beta".to_owned())]]
    );
    // The word's space after it is not bold, a change the word carries.
    assert_eq!(
        key_to(CaretMotion::PreviousWord, "caret doc 0"),
        [vec![
            SegmentContent::Format(TextFormat::Bold),
            SegmentContent::Format(TextFormat::NotSpellingError),
            SegmentContent::Text("alpha".to_owned()),
            SegmentContent::Format(TextFormat::NotBold),
        ]]
    );
    assert_eq!(
        key_to(CaretMotion::NextWord, "caret doc 6"),
        [vec![
            SegmentContent::Format(TextFormat::SpellingError),
            SegmentContent::Text("beta".to_owned()),
        ]]
    );
    drop(core);
    app.quit();
}

/// A UI Automation caret event raised as the key moves the caret, within
/// 60 ms of the pass, brings a check forward, which finds the move
/// (Dickson, 2026-11-17: an event counts only if the caret is no longer
/// where it was when the key was pressed): the key says its unit, "b", and
/// the caret tell behind it, judged after the key, says what the move
/// unselected, "alpha", selected with no key before it.
fn a_caret_event_within_60_ms_of_the_pass_brings_a_check_that_finds_the_move() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-event");
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 5"]),
        selected("alpha")
    );
    let mut said = core
        .caret_key(key(CaretMotion::NextCharacter), || {
            app.send("caret doc 6");
            app.send("caret-event doc");
        })
        .spoken();
    said.extend(core.answered().spoken());
    let mut expected = character("b");
    expected.extend(unselected("alpha"));
    assert_eq!(said, expected);
    drop(core);
    app.quit();
}

/// A held key's watch compares with the caret its starting read found,
/// though the application moved the caret there without reporting it: the
/// unreported move to offset 3 is not the key's effect, and the key's own
/// move to offset 4 says "a". NVDA reads the caret before it sends the key
/// (`editableText.py` 181 to 188).
fn a_keys_watch_compares_with_its_starting_read() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-starting");
    app.send("caret doc 3");
    let mut said = core
        .caret_key(key(CaretMotion::NextCharacter), || app.send("caret doc 4"))
        .spoken();
    said.extend(core.answered().spoken());
    assert_eq!(said, character("a"));
    drop(core);
    app.quit();
}

/// A key the application ignores, raising nothing: its watch runs to its
/// 100 ms bound, and the key says the unit at the caret, "a", as NVDA's
/// `_caretScriptPostMovedHelper` speaks it whether or not the caret moved
/// (`editableText.py` 165 to 192). A caret event raised afterwards with
/// nothing changed says nothing.
fn a_key_that_moves_nothing_says_its_unit_at_the_bound() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-nothing");
    let mut said = core
        .caret_key(key(CaretMotion::NextCharacter), || {})
        .spoken();
    said.extend(core.answered().spoken());
    assert_eq!(said, character("a"));
    assert_eq!(caret_event(&mut app, &mut core, &[]), NOTHING);
    drop(core);
    app.quit();
}

/// A focus judged pending while a key's watch is out withdraws the watch:
/// the key says nothing, and the focus is spoken. NVDA:
/// `editableText.py` 96 to 99 with `NVDAObjects/behaviors.py` 255 to 257.
/// The speech is the design's, derived from NVDA's source, awaiting NVDA's
/// capture (design, "A later key and a pending focus": needs capture).
fn a_focus_during_a_keys_watch_withdraws_it_and_the_key_says_nothing() {
    let (mut app, hwnd, mut core) = on_notes("mockapp-caret-watch-focus");
    // Right Arrow, which the application ignores: its watch is out.
    assert_eq!(
        core.caret_key(key(CaretMotion::NextCharacter), || {})
            .spoken(),
        NOTHING
    );
    app.send("set-focus ok");
    let mut said = core.uia_focus(&focused(hwnd)).spoken();
    said.extend(core.answered().spoken());
    assert_eq!(
        said,
        [vec![
            SegmentContent::Label("OK".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );
    drop(core);
    app.quit();
}

/// A key's watch out on a text field when the application gives the
/// field's runtime id to another field and moves the focus there: the
/// focus withdraws the watch, the key says nothing, and a caret move in the
/// new field afterwards is that field's caret tell, compared with the
/// baseline its focus minted, never the key's effect. The key's silence is
/// the design's, awaiting NVDA's capture, as in
/// `a_focus_during_a_keys_watch_withdraws_it_and_the_key_says_nothing`.
fn a_focus_on_a_field_that_took_the_runtime_id_withdraws_the_watch() {
    let (mut app, hwnd, mut core) = start("reuse_text.json", "mockapp-caret-watch-reuse");
    app.send("set-focus first");
    let _ = core.uia_focus(&focused(hwnd));
    assert_eq!(
        core.caret_key(key(CaretMotion::NextCharacter), || {})
            .spoken(),
        NOTHING
    );
    app.send("take-runtime-id second first");
    app.send("set-focus second");
    let mut said = core.uia_focus(&focused(hwnd)).spoken();
    said.extend(core.answered().spoken());
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Right".to_owned()),
                SegmentContent::Role(Role::Group)
            ],
            vec![
                SegmentContent::Label("Second".to_owned()),
                SegmentContent::Role(Role::EditableText)
            ],
            vec![
                SegmentContent::Format(verbatim_model::TextFormat::FontName("Consolas".to_owned())),
                SegmentContent::Format(verbatim_model::TextFormat::FontSize("11.0 pt".to_owned())),
                SegmentContent::Format(verbatim_model::TextFormat::Color("black".to_owned())),
                SegmentContent::Text("beta line".to_owned()),
            ],
        ]
    );
    app.send("caret second 0 4");
    app.send("caret-event second");
    assert_eq!(core.told(1).spoken(), selected("beta"));
    drop(core);
    app.quit();
}

/// A text field committed from Core's read of the current focus at its
/// start, with no focus event, is the committed focus as a told focus is:
/// a selection no key made is said by its caret tell.
fn a_caret_tell_of_a_focus_read_at_the_start_is_said() {
    common::init_com();
    let title = common::unique_title("mockapp-caret-watch-focus-now");
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("set-focus doc");
    let outpost = OutpostUnderTest::new(&app);
    outpost.read_focus_as(focused(hwnd).live().element());
    outpost.set_foreground(hwnd);
    let mut core = CoreDriver::on_time(&app, outpost, CoreTime::Real);
    assert_eq!(core.startup, notes_focus());
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 5"]),
        selected("alpha")
    );
    drop(core);
    app.quit();
}

/// Windows 11 Notepad, starting up, once answered that its document had
/// no text pattern (`docs/parity/text.md`, "A text pattern missing at the
/// focus"). A caret tell's read adopts the element its event came from,
/// forgetting that answer, as only an element with text raises a caret
/// event (design, "Selection and the caret's text facts"): the first read
/// after the provider answers names no baseline, since the focus had no
/// caret, and mints one, saying nothing; the next says what changed from
/// it. Without the adoption both reads would find no text and say
/// nothing.
fn a_caret_tells_read_adopts_its_element_when_the_node_had_no_text_pattern() {
    let (mut app, hwnd, mut core) = start("caret_watch.json", "mockapp-caret-watch-not-ready");
    app.send("refuse-text on");
    app.send("set-focus doc");
    assert_eq!(
        core.uia_focus(&focused(hwnd)).spoken(),
        [
            vec![
                SegmentContent::Label("Mockapp Caret Watch Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Notes".to_owned()),
                SegmentContent::Role(Role::EditableText),
            ],
        ]
    );
    app.send("refuse-text off");
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 5"]),
        NOTHING
    );
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 10"]),
        phrase(Phrase::Selected(SelectionText::Text(" beta".to_owned())))
    );
    drop(core);
    app.quit();
}

/// The system caret's location change, told by the listener, is a caret
/// tell of the readers' last focus whatever its backend, as NVDA handles
/// the MSAA caret's events as the last queued focus's for every focus but
/// a UI Automation Word document (`IAccessibleHandler/__init__.py` 634 to
/// 666): a UI Automation text field that raises no caret event of its own
/// has its selection said.
fn the_system_carets_move_is_a_caret_tell_of_a_uia_focus() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-system-caret");
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 0 5", "system-caret"],
            &[WinEventKind::Caret],
            1
        )
        .spoken(),
        selected("alpha")
    );
    drop(core);
    app.quit();
}

/// A UI Automation field that raises its own caret event and has the
/// system caret move too: both are caret tells, and the second, finding
/// the selection the first said, says nothing more.
fn a_system_caret_tell_after_the_uia_one_of_the_same_move_says_nothing() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-both-carets");
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 5"]),
        selected("alpha")
    );
    assert_eq!(
        core.raise(&mut app, &["system-caret"], &[WinEventKind::Caret], 1)
            .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// mockapp's classic edit control focused through MSAA, with Core driving
/// an outpost watching it, and "alpha" selected and said by the system
/// caret's tell.
fn on_edit(name: &str) -> (common::MockApp, CoreDriver, HWND) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The keyboard focus cannot be given to the control on a desktop of
    // its own, so its host has the focused state, which an MSAA focus
    // needs on the object or an ancestor.
    app.send("set-focus root");
    let mut core = CoreDriver::on_time(&app, OutpostUnderTest::new(&app), CoreTime::Real);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    let _ = core.msaa_event(
        verbatim_model::event_kind::EventKind::MsaaFocus,
        edit,
        (OBJID_CLIENT.0, 0),
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 0 5", "system-caret"],
            &[WinEventKind::Caret],
            1
        )
        .spoken(),
        selected("alpha")
    );
    (app, core, edit)
}

/// The system caret's show from the focused classic edit control, read
/// through MSAA, is its caret tell, as NVDA handles a show of the caret
/// object (`IAccessibleHandler/__init__.py` 636 to 639): what the move
/// unselected is said.
fn a_classic_edit_controls_caret_show_is_a_caret_tell() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-show");
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 6", "system-caret show"],
            &[WinEventKind::Caret, WinEventKind::Show],
            1
        )
        .spoken(),
        unselected("alpha")
    );
    drop(core);
    app.quit();
}

/// A selection no key made, as the mouse or the application makes it, is
/// said by the caret tell that follows it, from Core's baseline: "alpha"
/// selected from the caret at the start, then unselected as the caret
/// collapses at its end. NVDA: `editableText.py` 429 to 451.
fn a_selection_no_key_made_is_said_by_its_caret_tell() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-selection");
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 5"]),
        selected("alpha")
    );
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 5"]),
        unselected("alpha")
    );
    drop(core);
    app.quit();
}

/// A selection made around the caret, as a double-click inside a word
/// makes it, is one change, the whole selection, and so is a caret left
/// inside it, the whole selection unselected, as NVDA speaks each when the
/// other side holds nothing (`speech/speech.py` 1340 to 1418).
fn a_selection_around_the_caret_is_one_change_each_way() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-around");
    assert_eq!(caret_event(&mut app, &mut core, &["caret doc 5"]), NOTHING);
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 3 8"]),
        selected("ha be")
    );
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 5"]),
        unselected("ha be")
    );
    drop(core);
    app.quit();
}

/// A selection made with no event, then a key: the starting read compares
/// no selection (NVDA's caret scripts read the caret only, `editableText.py`
/// 70 to 192), so the key says its unit alone, the space at offset 5 where
/// Right Arrow collapses the selection, and nothing of "alpha".
fn a_keys_starting_read_says_no_selection_made_with_no_event() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-before-selection");
    app.send("caret doc 0 5");
    let mut said = core
        .caret_key(key(CaretMotion::NextCharacter), || app.send("caret doc 5"))
        .spoken();
    said.extend(core.answered().spoken());
    assert_eq!(said, character(" "));
    drop(core);
    app.quit();
}

/// Control+A in a field, its caret event not yet raised, then a UI
/// Automation focus event on the same element, which still has the
/// keyboard focus: a duplicate, dropped unread, as NVDA ignores it
/// (`UIAHandler/__init__.py` 902 to 916), so nothing is said; the caret
/// event that follows says the selection.
fn a_duplicate_focus_says_nothing_and_the_caret_tell_says_the_selection() {
    let (mut app, hwnd, mut core) = on_notes("mockapp-caret-watch-repeat-selection");
    app.send("caret doc 0 5");
    assert_eq!(core.uia_focus(&focused(hwnd)).spoken(), NOTHING);
    assert_eq!(caret_event(&mut app, &mut core, &[]), selected("alpha"));
    drop(core);
    app.quit();
}

/// A Shift key passes at once and says nothing for itself; its caret tell
/// says what it selected, and a later caret event with nothing changed
/// says nothing (design, "The hook and Core's stroke line"; NVDA:
/// `NVDAObjects/behaviors.py` 331 to 360).
fn a_selecting_keys_selection_is_said_by_its_caret_tell() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-key-selection");
    // The caret event's tell is the evidence, counted from before the key:
    // it may reach Core within the key's step or after it.
    let before = core.tells();
    let mut said = core
        .caret_key(selecting(CaretMotion::NextWord), || {
            app.send("caret doc 0 5");
            app.send("caret-event doc");
        })
        .spoken();
    said.extend(core.until_told(before + 1).spoken());
    assert_eq!(said, selected("alpha"));
    assert_eq!(caret_event(&mut app, &mut core, &[]), NOTHING);
    drop(core);
    app.quit();
}

/// mockapp's caret watch fixture with an outpost alone: the outpost, and
/// the "Notes" document's node and the selection baseline its focus
/// completion minted, the caret at the text's start.
fn notes_outpost(name: &str) -> (common::MockApp, HWND, OutpostUnderTest, NodeId, TextAnchor) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    app.send("set-focus doc");
    let focused = outpost.uia_focus_answer(&focused(hwnd));
    let verbatim_model::request::FocusCaret::Read(read) = &focused.read.caret else {
        panic!(
            "the focus came with {:?}, not its caret",
            focused.read.caret
        );
    };
    let baseline = read.baseline;
    outpost.settled();
    (app, hwnd, outpost, focused.node(), baseline)
}

/// The answer to `op` on `node`, asked as Core asks it.
fn text_answer(outpost: &OutpostUnderTest, hwnd: HWND, node: NodeId, op: TextOp) -> TextReply {
    let number = outpost.request(hwnd, Request::Text { node, op });
    match outpost.answer(number).outcome {
        Outcome::Result(Reply::Text(reply)) => reply,
        other => panic!("the outpost answered {other:?}, not a text reply"),
    }
}

/// A caret tell's read of `node` naming `baseline`.
fn read_selection(
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    node: NodeId,
    baseline: TextAnchor,
) -> SelectionReadReply {
    match text_answer(
        outpost,
        hwnd,
        node,
        TextOp::ReadSelection(SelectionRead {
            provider: TextProvider::Uia,
            baseline: Some(baseline),
            adopt: None,
        }),
    ) {
        TextReply::SelectionRead(read) => *read,
        other => panic!("the outpost answered {other:?}, not the selection's read"),
    }
}

/// A selection whose provider's caret is its end, as Windows 11 Notepad's
/// `TextPattern2` caret is after Shift+Control+Right Arrow: the caret is
/// the selection's start, as NVDA's is (`NVDAObjects/UIA/__init__.py` 488
/// to 498), so a caret tell's read gives the start's line with the caret
/// at the start, which is also the selection's start, so Core knows what
/// comes before it (`docs/parity/text.md`, "TextInfo-equivalent layer"),
/// whichever line the provider's caret is on; a selection from the text's
/// start has its line marked the text's first.
fn a_selections_caret_is_its_start_whichever_end_the_providers_caret_is_at() {
    let (mut app, hwnd, outpost, node, baseline) = notes_outpost("mockapp-caret-watch-start");
    app.send("caret doc 6 10 end");
    let read = read_selection(&outpost, hwnd, node, baseline);
    let caret = read.caret;
    assert_eq!(
        (
            caret.line.text.as_str(),
            caret.line.offset,
            caret.selection.as_ref().map(|s| s.range.start)
        ),
        (
            "alpha beta\n",
            6,
            Some(TextPosition {
                anchor: caret.line.start,
                offset: 6,
            })
        )
    );
    // The provider's caret on the next line: the caret is still the
    // start, on the line before.
    app.send("caret doc 6 13 end");
    let caret = read_selection(&outpost, hwnd, node, read.baseline).caret;
    assert_eq!(
        (
            caret.line.text.as_str(),
            caret.line.offset,
            caret.selection.as_ref().map(|s| s.range.start)
        ),
        (
            "alpha beta\n",
            6,
            Some(TextPosition {
                anchor: caret.line.start,
                offset: 6,
            })
        )
    );
    app.send("caret doc 0 5 end");
    let caret = read_selection(&outpost, hwnd, node, baseline).caret;
    assert_eq!(
        (
            caret.line.first,
            caret.selection.as_ref().map(|s| s.range.start)
        ),
        (
            true,
            Some(TextPosition {
                anchor: caret.line.start,
                offset: 0,
            })
        )
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A selection Verbatim made, as the review copy's first press makes it,
/// mints the next baseline, so the caret tell's read after it compares
/// with it and finds no change: Verbatim's own moves are not said as
/// selection changes (design, "Selection and the caret's text facts",
/// `keep_made`).
fn a_selection_verbatim_made_is_the_next_baseline() {
    let (app, hwnd, outpost, node, baseline) = notes_outpost("mockapp-caret-watch-own");
    let line = read_selection(&outpost, hwnd, node, baseline)
        .caret
        .line
        .start;
    let made = match text_answer(
        &outpost,
        hwnd,
        node,
        TextOp::Select {
            start: TextPoint::At(TextPosition {
                anchor: line,
                offset: 0,
            }),
            end: TextPoint::At(TextPosition {
                anchor: line,
                offset: 5,
            }),
            through: None,
        },
    ) {
        TextReply::SelectionPlaced { baseline, .. } => baseline,
        other => panic!("the outpost answered {other:?}, not the selection made"),
    };
    let read = read_selection(&outpost, hwnd, node, made);
    assert_eq!(
        read.difference,
        Some(SelectionDifference {
            changes: Vec::new(),
            new_selection: Some(SelectionChange {
                selected: true,
                text: "alpha".to_owned(),
                characters: 5,
            }),
        })
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A Shift key after a selection Verbatim made compares with it, the
/// baseline its answer minted: Shift+Right Arrow after "alpha" was selected
/// says " " selected. So does one after Verbatim moved the caret, from the
/// caret where it was moved: Shift+Control+Right Arrow there says "beta"
/// selected.
fn a_selection_after_verbatims_compares_with_it() {
    let (mut app, hwnd, outpost, node, baseline) = notes_outpost("mockapp-caret-watch-after-own");
    let caret = read_selection(&outpost, hwnd, node, baseline).caret;
    let at = |offset| {
        TextPoint::At(TextPosition {
            anchor: caret.line.start,
            offset,
        })
    };
    let made = match text_answer(
        &outpost,
        hwnd,
        node,
        TextOp::Select {
            start: at(0),
            end: at(5),
            through: None,
        },
    ) {
        TextReply::SelectionPlaced { baseline, .. } => baseline,
        other => panic!("the outpost answered {other:?}, not the selection made"),
    };
    app.send("caret doc 0 6");
    assert_eq!(
        read_selection(&outpost, hwnd, node, made).difference,
        Some(SelectionDifference {
            changes: vec![SelectionChange {
                selected: true,
                text: " ".to_owned(),
                characters: 1,
            }],
            new_selection: Some(SelectionChange {
                selected: true,
                text: "alpha ".to_owned(),
                characters: 6,
            }),
        })
    );
    let moved = match text_answer(&outpost, hwnd, node, TextOp::MoveCaret(at(6))) {
        TextReply::CaretPlaced { baseline, .. } => baseline,
        other => panic!("the outpost answered {other:?}, not the caret moved"),
    };
    app.send("caret doc 6 10");
    assert_eq!(
        read_selection(&outpost, hwnd, node, moved).difference,
        Some(SelectionDifference {
            changes: vec![SelectionChange {
                selected: true,
                text: "beta".to_owned(),
                characters: 4,
            }],
            new_selection: Some(SelectionChange {
                selected: true,
                text: "beta".to_owned(),
                characters: 4,
            }),
        })
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// "x" typed over the selected "a" in a document read through UI
/// Automation, its caret event raised before its text change: the caret
/// tell's read compares with the baseline's range as it now is, which
/// holds the "x", so "x" is said unselected, as NVDA says it in Windows 11
/// Notepad (`target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-notepad-1.txt`;
/// Dickson, 2026-11-16, matched to NVDA). mockapp's ranges are offsets into
/// the text as it now is, so the old range holds what a range that follows
/// the edit would: one character.
fn a_uia_selection_typed_over_says_what_its_range_now_holds() {
    let (mut app, _, mut core) = on_notes("mockapp-caret-watch-typed-over");
    assert_eq!(
        caret_event(&mut app, &mut core, &["caret doc 0 1"]),
        phrase(Phrase::Selected(SelectionText::Character("a".to_owned())))
    );
    app.send(r"set-text doc xlpha beta\ngamma\n");
    app.send("caret doc 1");
    app.send("caret-event doc");
    app.send("text-changed doc");
    let mut said = core.told(1).spoken();
    core.outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::UiaTextSelection,
        Heard::UiaTextChanged,
    ]);
    said.extend(core.pump().spoken());
    assert_eq!(
        said,
        phrase(Phrase::Unselected(SelectionText::Character("x".to_owned())))
    );
    drop(core);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control, its
/// caret event raised before its value change, as the Windows Forms text
/// box raises them (R1): the value change waiting at the caret tell's
/// answer makes its terms general, the selection removed, as NVDA's edit
/// control does (`NVDAObjects/window/edit.py` 1075 to 1080;
/// `target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-textbox-1-type-over.txt`).
fn an_edit_controls_caret_tell_before_its_value_change_is_said_in_general_terms() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-typed-over");
    assert_eq!(
        core.raise_at_once(
            &mut app,
            &[
                r"set-text doc x beta\ngamma\n",
                "caret doc 1",
                "system-caret",
                "text-changed doc"
            ],
            &[WinEventKind::Caret, WinEventKind::ValueChange],
            3
        )
        .spoken(),
        [vec![SegmentContent::Message(Message::SelectionRemoved)]]
    );
    drop(core);
    app.quit();
}

/// The selected last line deleted from a classic edit control, as
/// Backspace over a selection to the text's end deletes it, its events as
/// the Windows Forms text box raises them (R1): the old selection's
/// offsets now hold no text, and the change is still an unselection, which
/// the value change waiting at the caret tell's answer says as the
/// selection removed, as NVDA lists the old selection whatever it now
/// holds (`speech/speech.py` 1364 to 1365, 1408 to 1417).
fn an_edit_controls_selection_deleted_to_the_end_says_the_selection_removed() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-deleted-to-end");
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 12 17", "system-caret"],
            &[WinEventKind::Caret],
            1
        )
        .spoken(),
        // Apart from the old selection: the outpost lists the old one
        // unselected, then the new one selected, and Core says every
        // selected part first, as NVDA does (`speech/speech.py` 1389 to
        // 1407).
        selected("gamma")
            .into_iter()
            .chain(unselected("alpha"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        core.raise_at_once(
            &mut app,
            &[
                r"set-text doc alpha beta\n",
                "caret doc 12",
                "system-caret",
                "text-changed doc"
            ],
            &[WinEventKind::Caret, WinEventKind::ValueChange],
            3
        )
        .spoken(),
        [vec![SegmentContent::Message(Message::SelectionRemoved)]]
    );
    drop(core);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control, its value
/// change judged before its caret tell: the value change sets the
/// text-changed flag, and the caret tell's answer says its change in
/// general terms (Table 17, columns G and D). NVDA:
/// `NVDAObjects/window/edit.py` 1083 to 1084.
fn an_edit_controls_value_change_judged_first_has_the_next_change_said_in_general_terms() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-changed-first");
    assert_eq!(
        core.raise(
            &mut app,
            &[r"set-text doc x beta\ngamma\n", "text-changed doc"],
            &[WinEventKind::ValueChange],
            2
        )
        .spoken(),
        NOTHING
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 1", "system-caret"],
            &[WinEventKind::Caret],
            1
        )
        .spoken(),
        [vec![SegmentContent::Message(Message::SelectionRemoved)]]
    );
    drop(core);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control, its
/// events as the Windows Forms text box raises them for Control+Z: a value
/// change, a caret tell, the client's value change and a last caret tell.
/// The first caret tell's answer, with the flag set and the second value
/// change waiting, says the change in general terms and counts that value
/// change, which then sets no flag (Table 17); the last caret tell finds
/// the selection's offsets unchanged and says nothing, as NVDA's
/// `Edit.event_caret` returns before comparing (`NVDAObjects/window/edit.py`
/// 1068 to 1074). So the next comparison, as "x" is selected, is specific.
fn an_edit_controls_value_change_counted_by_a_caret_answer_leaves_the_next_change_specific() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-counted");
    assert_eq!(
        core.raise_at_once(
            &mut app,
            &[
                r"set-text doc x beta\ngamma\n",
                "text-changed doc",
                "caret doc 1",
                "system-caret",
                "text-changed doc",
                "system-caret"
            ],
            &[WinEventKind::Caret, WinEventKind::ValueChange],
            5
        )
        .spoken(),
        [vec![SegmentContent::Message(Message::SelectionRemoved)]]
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["caret doc 0 1", "system-caret"],
            &[WinEventKind::Caret],
            1
        )
        .spoken(),
        phrase(Phrase::Selected(SelectionText::Character("x".to_owned())))
    );
    drop(core);
    app.quit();
}

/// The selected "alpha" deleted from a classic edit control with Delete,
/// its caret and value change told after the key: the key's watch finds
/// the characters beside the caret changed and says the unit at the
/// caret, the space now at the text's start, and the caret tell, with the
/// value change waiting at its answer, says the selection removed, as
/// NVDA's edit control does at the key's caret event
/// (`target/e2e-evidence/m42-selection-keys-2026-10-22/textbox-word-delete.txt`).
fn an_edit_controls_delete_over_a_selection_says_its_unit_then_the_selection_removed() {
    let (mut app, mut core, _) = on_edit("mockapp-caret-watch-edit-delete");
    let mut said = core
        .caret_key(key(CaretMotion::Delete), || {
            app.send(r"set-text doc  beta\ngamma\n");
            app.send("caret doc 0");
        })
        .spoken();
    said.extend(core.answered().spoken());
    said.extend(
        core.raise_at_once(
            &mut app,
            &["system-caret", "text-changed doc"],
            &[WinEventKind::Caret, WinEventKind::ValueChange],
            2,
        )
        .spoken(),
    );
    let mut expected = character(" ");
    expected.push(vec![SegmentContent::Message(Message::SelectionRemoved)]);
    assert_eq!(said, expected);
    drop(core);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    harness::run_isolated(&[
        (
            "a_keys_move_found_by_its_watch_says_its_unit",
            a_keys_move_found_by_its_watch_says_its_unit,
        ),
        (
            "a_focus_line_and_a_keys_unit_are_said_with_their_formatting",
            a_focus_line_and_a_keys_unit_are_said_with_their_formatting,
        ),
        (
            "a_caret_event_within_60_ms_of_the_pass_brings_a_check_that_finds_the_move",
            a_caret_event_within_60_ms_of_the_pass_brings_a_check_that_finds_the_move,
        ),
        (
            "a_keys_watch_compares_with_its_starting_read",
            a_keys_watch_compares_with_its_starting_read,
        ),
        (
            "a_key_that_moves_nothing_says_its_unit_at_the_bound",
            a_key_that_moves_nothing_says_its_unit_at_the_bound,
        ),
        (
            "a_focus_during_a_keys_watch_withdraws_it_and_the_key_says_nothing",
            a_focus_during_a_keys_watch_withdraws_it_and_the_key_says_nothing,
        ),
        (
            "a_focus_on_a_field_that_took_the_runtime_id_withdraws_the_watch",
            a_focus_on_a_field_that_took_the_runtime_id_withdraws_the_watch,
        ),
        (
            "a_caret_tell_of_a_focus_read_at_the_start_is_said",
            a_caret_tell_of_a_focus_read_at_the_start_is_said,
        ),
        (
            "a_caret_tells_read_adopts_its_element_when_the_node_had_no_text_pattern",
            a_caret_tells_read_adopts_its_element_when_the_node_had_no_text_pattern,
        ),
        (
            "the_system_carets_move_is_a_caret_tell_of_a_uia_focus",
            the_system_carets_move_is_a_caret_tell_of_a_uia_focus,
        ),
        (
            "a_system_caret_tell_after_the_uia_one_of_the_same_move_says_nothing",
            a_system_caret_tell_after_the_uia_one_of_the_same_move_says_nothing,
        ),
        (
            "a_classic_edit_controls_caret_show_is_a_caret_tell",
            a_classic_edit_controls_caret_show_is_a_caret_tell,
        ),
        (
            "a_selection_no_key_made_is_said_by_its_caret_tell",
            a_selection_no_key_made_is_said_by_its_caret_tell,
        ),
        (
            "a_selection_around_the_caret_is_one_change_each_way",
            a_selection_around_the_caret_is_one_change_each_way,
        ),
        (
            "a_keys_starting_read_says_no_selection_made_with_no_event",
            a_keys_starting_read_says_no_selection_made_with_no_event,
        ),
        (
            "a_duplicate_focus_says_nothing_and_the_caret_tell_says_the_selection",
            a_duplicate_focus_says_nothing_and_the_caret_tell_says_the_selection,
        ),
        (
            "a_selecting_keys_selection_is_said_by_its_caret_tell",
            a_selecting_keys_selection_is_said_by_its_caret_tell,
        ),
        (
            "a_selections_caret_is_its_start_whichever_end_the_providers_caret_is_at",
            a_selections_caret_is_its_start_whichever_end_the_providers_caret_is_at,
        ),
        (
            "a_selection_verbatim_made_is_the_next_baseline",
            a_selection_verbatim_made_is_the_next_baseline,
        ),
        (
            "a_selection_after_verbatims_compares_with_it",
            a_selection_after_verbatims_compares_with_it,
        ),
        (
            "a_uia_selection_typed_over_says_what_its_range_now_holds",
            a_uia_selection_typed_over_says_what_its_range_now_holds,
        ),
        (
            "an_edit_controls_caret_tell_before_its_value_change_is_said_in_general_terms",
            an_edit_controls_caret_tell_before_its_value_change_is_said_in_general_terms,
        ),
        (
            "an_edit_controls_selection_deleted_to_the_end_says_the_selection_removed",
            an_edit_controls_selection_deleted_to_the_end_says_the_selection_removed,
        ),
        (
            "an_edit_controls_value_change_judged_first_has_the_next_change_said_in_general_terms",
            an_edit_controls_value_change_judged_first_has_the_next_change_said_in_general_terms,
        ),
        (
            "an_edit_controls_value_change_counted_by_a_caret_answer_leaves_the_next_change_specific",
            an_edit_controls_value_change_counted_by_a_caret_answer_leaves_the_next_change_specific,
        ),
        (
            "an_edit_controls_delete_over_a_selection_says_its_unit_then_the_selection_removed",
            an_edit_controls_delete_over_a_selection_says_its_unit_then_the_selection_removed,
        ),
    ]);
}
