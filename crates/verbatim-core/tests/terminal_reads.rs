//! A terminal's window reads judged at the time their answers reach Core,
//! through the public reducer as the shell drives it (`desk`): the
//! terminal focused, its baseline answered, and each change the outpost
//! tells answered with the window Core then asks for, each read at a time
//! of the test's choosing.

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{Desk, Heard, ONE, focus_read_of, window_ask};
use verbatim_core::terminal_window::{MarkPlace, MarkSearch, TerminalWindow, outcomes_making};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{Outcome, QueueInput, Reply};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, NodeDetails, NodeSnapshot, Role, SegmentContent, StateSet, TerminalText, TextAnchor,
    TraceId,
};

/// The focused terminal's node.
const TERMINAL: u64 = 31;

/// The screen's width in cells.
const WIDTH: u32 = 40;

/// The outpost's tell that the focused terminal changed.
fn changed(at_us: u64) -> Event {
    Event {
        trace_id: TraceId::from_raw(8),
        kind: EventKind::TerminalChanged,
        process: ONE.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us,
        object: EventObject::Terminal {
            node: ONE.node(TERMINAL),
            window: ONE.window(),
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::NoWindow,
        checked: None,
    }
}

/// A window of `lines`, each one row, the whole document on the screen,
/// with no rows written past the mark Core named and no caret; its end
/// mark `end`.
fn window(lines: &[&str], end: u64) -> TerminalWindow {
    let rows = u32::try_from(lines.len()).expect("a few rows");
    TerminalWindow {
        rows: TerminalText {
            text: lines.join("\r\n"),
            lengths: vec![(rows, WIDTH)],
        },
        above: rows,
        screen_top: rows,
        screen_rows: rows,
        width: WIDTH,
        screen_at_start: true,
        caret: None,
        caret_rows_to_end: None,
        end: TextAnchor(end),
        first_row: TerminalText::unpadded(lines[0]),
        document_rows: rows,
        end_place: Some(MarkPlace::Rows {
            written: 0,
            opening: None,
        }),
        aside_place: None,
        found: MarkSearch::NotAsked,
        colours: None,
        began_us: 0,
        ended_us: 0,
    }
}

/// Answers the window read Core asked for last with `lines`, at `at_us`:
/// what was heard.
fn read_at(desk: &mut Desk, at_us: u64, lines: &[&str], end: u64) -> Vec<Heard> {
    let (number, ask) = window_ask(desk).expect("Core asks for the terminal's window");
    let mut answered = window(lines, end);
    if ask.end.is_none() {
        answered.end_place = None;
    }
    if ask.colours {
        answered.colours = Some(vec![Vec::new(); lines.len()]);
    }
    desk.step_at(
        at_us,
        QueueInput::Answer {
            from: ONE.instance(),
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(number),
                outcome: Outcome::Result(Reply::Steps(outcomes_making(&answered, &ask))),
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    )
}

/// The words of every utterance `heard` holds.
fn said(heard: &[Heard]) -> Vec<String> {
    heard
        .iter()
        .filter_map(|heard| match heard {
            Heard::Say(_, segments) => Some(
                segments
                    .iter()
                    .filter_map(|segment| match &segment.content {
                        SegmentContent::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect()
}

/// A program rewrites a row more than [`VOLATILE_US`] after its last
/// rewrite: the read judges the change at the time its answer reached
/// Core, so the row is not volatile and its changed word is spoken at once
/// (`docs/parity/terminals.md`, "New terminal output"). A read judged at
/// no time at all found every rewrite within the limit of the last.
///
/// [`VOLATILE_US`]: verbatim_text::terminal_window::VOLATILE_US
#[test]
fn a_row_rewritten_seconds_apart_is_spoken_at_each_read() {
    let mut desk = Desk::new();
    let terminal = NodeSnapshot {
        id: ONE.node(TERMINAL),
        backend: Backend::Uia,
        role: Role::Terminal,
        name: Some("Terminal".to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let _ = desk.focus_read(ONE, focus_read_of(ONE, &terminal, &[]));
    let start = desk.at_us;
    let _ = read_at(&mut desk, start + 1_000, &["$ watch", "status: one"], 1);

    let at = start + 1_000_000;
    let _ = desk.step_at(
        at,
        QueueInput::Tell {
            from: ONE.instance(),
            tell: Box::new(changed(at)),
        },
    );
    let heard = read_at(&mut desk, at + 2_000, &["$ watch", "status: two"], 2);
    assert_eq!(said(&heard), ["two"]);

    let at = start + 6_000_000;
    let _ = desk.step_at(
        at,
        QueueInput::Tell {
            from: ONE.instance(),
            tell: Box::new(changed(at)),
        },
    );
    let heard = read_at(&mut desk, at + 2_000, &["$ watch", "status: three"], 3);
    assert_eq!(said(&heard), ["three"]);
}
