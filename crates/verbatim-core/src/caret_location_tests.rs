//! Report caret location as an event-queue entry: one read of the
//! committed focus's caret at its turn, holding the queue until its
//! answer, "No caret" with nothing committed, "Not supported" for a focus
//! with no text, and silence for any outcome but a result.
use crate::entries::tests::found_at;
use crate::readers::Found;

use verbatim_model::batch::{Located, TextOffset};
use verbatim_model::request::{Outcome as WireOutcome, Press, PressAction, Reply};
use verbatim_model::{KeySeq, ReviewCommand, Role, TextPoint, TraceId};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready, values,
};
use crate::event_queue::{Foreground, Input};
use crate::keys::entry::PressEntry;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: verbatim_model::WindowHandle(4),
    root_owner: verbatim_model::WindowHandle(4),
    process: Some(APP),
    thread: 41,
    by_class: None,
    ..Foreground::BLANK
};

/// The startup read answered with the focus `name` of `role` committed,
/// its `Follow` request 2, so the next request is 3.
fn committed(name: &str, role: Role) -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let mut read = focus_read(None);
    read.focus.read.values = values(name, role);
    let _ = found_at(&mut state, 600, (OUTPOST, 1), Found::Read(read));
    state
}

/// `command` pressed as key `key`.
fn press(key: u64, command: ReviewCommand) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action: PressAction::Command { command, repeat: 0 },
        },
    ))))
}

/// The answer to request 3, the location read.
fn located(reply: Reply) -> Input<Entry> {
    answer(OUTPOST, 3, reply)
}

/// A location answered at `screen`, with its `offset` where the text
/// counts by offsets.
fn at(screen: Option<(i32, i32)>, offset: Option<TextOffset>) -> Reply {
    crate::text_batches::located(Located { screen, offset })
}

#[test]
fn it_asks_for_the_carets_place_and_says_it() {
    let mut state = committed("Body", Role::EditableText);
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReportCaretLocation)
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        state.queue.batch_for_tests(3),
        Some(crate::text_batches::locate(node(2), TextPoint::Caret))
    );
    assert_eq!(
        feed_at(&mut state, 2_000, located(at(Some((108, 200)), None))),
        ["Phrase(Positioned { x: 108, y: 200 })"]
    );
}

/// In text measured by character offsets the caret's location is said as
/// how far through the text it is, then its place on the screen.
#[test]
fn an_offset_measured_location_says_the_percentage_and_the_place() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReportCaretLocation),
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            located(at(
                Some((108, 200)),
                Some(TextOffset {
                    offset: 1,
                    length: 4
                })
            ))
        ),
        ["Phrase(OffsetPositioned { offset: 1, length: 4, point: Some((108, 200)) })"]
    );
}

/// The command holds the queue until its answer: a press behind it waits.
#[test]
fn a_press_behind_it_waits_for_its_answer() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReportCaretLocation),
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_100,
            press(2, ReviewCommand::ReportCaretLocation)
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        feed_at(&mut state, 2_000, located(at(Some((1, 2)), None))),
        [
            "Phrase(Positioned { x: 1, y: 2 })",
            "send Outpost(Pid(40)) other"
        ]
    );
}

#[test]
fn with_nothing_committed_it_says_no_caret() {
    let mut state = ready();
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReportCaretLocation)
        ),
        ["Message(NoCaret)"]
    );
}

#[test]
fn a_focus_with_no_text_is_not_supported() {
    let mut state = committed("OK", Role::Button);
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReportCaretLocation)
        ),
        ["Message(NotSupported)"]
    );
}

#[test]
fn a_node_not_there_says_nothing() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReportCaretLocation),
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer_outcome(OUTPOST, 3, WireOutcome::NotThere)
        ),
        Vec::<String>::new()
    );
}
