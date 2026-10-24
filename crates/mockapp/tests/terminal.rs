//! A terminal's reading (`docs/design/terminal-model.md`, sections 3 to 5)
//! against mockapp's UIA text provider, whose `set-text` command rewrites a
//! node's text the way a terminal's buffer changes, here as a terminal
//! gives it, every row padded to the width and ended by a carriage return
//! and line feed, and whose `screen` command puts only the text's last
//! rows on screen, as a terminal shows the end of its buffer. Each test
//! drives `verbatim-outpost`'s terminal module on this thread exactly as
//! the outpost's worker does once it has the focused terminal's text
//! pattern, since the worker finds a UIA focus by reading the system's
//! keyboard focus, which a test must not take from the desktop it runs on.
//!
//! What is checked: a read's reading rules say the new output, line by
//! line, and the read costs one call remotely; and a terminal whose text
//! changes with no caret event, as the console host's does while typing,
//! has the reading rules echo the typing and say what Backspace deleted,
//! Core speaking it, with the keys told to the terminal as the keyboard
//! hook's thread tells its outpost.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CallCounts, CaretKey, CaretMotion, Effect, Input, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Pid, Role, SegmentContent, StateSet, TerminalItem, TerminalSpeech,
    TextOp, TextReply, TraceId,
};
use verbatim_outpost::protocol::now_us;
use verbatim_outpost::terminal::{Terminal, read};
use verbatim_outpost::text::uia::UiaText;
use verbatim_uia::Uia;
use verbatim_uia_rops::CaretLineQuery;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

/// The terminal's width in cells.
const WIDTH: usize = 12;

/// mockapp's terminal's element, found by its name
/// (`common::named_element`), which passes the window's title bar by.
fn terminal_element(uia: &Uia, hwnd: HWND) -> IUIAutomationElement {
    common::named_element(uia, hwnd, "Terminal")
}

/// `rows` as a terminal's text: each padded to [`WIDTH`] and ended by a
/// carriage return and line feed, escaped for `set-text`; and the UTF-16
/// offset of the end of the last row's text, where the caret is. The text's
/// last line break gives mockapp an empty last line, so that moving by
/// lines from the caret to the document's end counts the caret's row as
/// the terminals' providers do (`verbatim_uia_rops::Snapshot::caret_rows_to_end`).
fn terminal_text(rows: &[&str]) -> (String, usize) {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:WIDTH$}\r\n")).collect();
    let text = padded.concat();
    let before: usize = padded[..rows.len() - 1]
        .iter()
        .map(|row| row.encode_utf16().count())
        .sum();
    let caret = before + rows.last().map_or(0, |row| row.encode_utf16().count());
    (text.replace('\r', "\\r").replace('\n', "\\n"), caret)
}

/// Shows `rows` in mockapp's terminal, the caret at the end of the last
/// row's text, raising no event: the console host's pattern while typing.
fn show(app: &mut common::MockApp, hwnd: HWND, rows: &[&str]) {
    let (text, caret) = terminal_text(rows);
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, &format!("caret term {caret} {caret}"));
}

/// One read of the terminal, as the worker makes it on a change of its
/// text, and what its reading rules said.
fn read_terminal(
    uia: &Uia,
    source: &UiaText,
    terminal: &mut Terminal,
    remote: bool,
) -> TerminalSpeech {
    let caret = CaretLineQuery {
        element: source.element(),
        pattern: source.pattern(),
        pattern2: source.pattern2(),
        max_text: 4096,
        since: None,
    };
    read(
        uia,
        (source.element(), source.pattern()),
        caret,
        terminal,
        (remote, false),
    )
    .expect("the terminal reads")
    .speech
}

fn uia_calls(uia: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa: 0,
        window_messages: 0,
        connections: 0,
    }
}

/// The outpost's source for mockapp's terminal.
fn source(uia: &Uia, hwnd: HWND, remote: bool) -> UiaText {
    let element = terminal_element(uia, hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    UiaText::new(element, pattern, pattern2, true).remote(remote)
}

/// Output written below the rows read is said line by line, and a read is
/// one remote program, its own call.
fn new_output_is_said_line_by_line(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-output");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    // The screen: three rows, and the empty last line.
    common::apply(&mut app, hwnd, "screen term 4");
    show(&mut app, hwnd, &["one", "two", "ready>"]);
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new(verbatim_model::TerminalSettings::default());
    assert_eq!(
        read_terminal(&uia, &source, &mut terminal, remote),
        TerminalSpeech::default(),
        "the first read says nothing"
    );
    show(&mut app, hwnd, &["one", "two", "three", "four", "ready>"]);
    let _ = verbatim_uia::calls::take();
    let speech = read_terminal(&uia, &source, &mut terminal, remote);
    let calls = verbatim_uia::calls::take();
    assert_eq!(
        speech.items,
        vec![
            TerminalItem::Output("three".to_owned()),
            TerminalItem::Output("four".to_owned()),
        ]
    );
    if remote {
        assert_eq!(calls, uia_calls(1), "one program, one call");
    }
    app.quit();
}

fn new_output_is_said_line_by_line_remote() {
    new_output_is_said_line_by_line(true);
}

fn new_output_is_said_line_by_line_classic() {
    new_output_is_said_line_by_line(false);
}

/// The node Core knows mockapp's terminal by.
fn terminal_node() -> NodeId {
    NodeId::in_outpost(OutpostId(1), 1)
}

/// What Core spoke of `effects`; a caret key's watch is answered, as the
/// outpost answers one when the reading rules answer the key, with nothing
/// Core speaks.
fn handle(state: &mut SrState, effects: Vec<Effect>, spoken: &mut Vec<Vec<SegmentContent>>) {
    let mut pending = effects;
    while !pending.is_empty() {
        let mut inputs = Vec::new();
        for effect in pending {
            match effect {
                Effect::Text(request) => match &request.op {
                    TextOp::AwaitCaret(_) => inputs.push(Input::TextCompleted {
                        trace_id: TraceId::mint(),
                        query_id: request.query_id,
                        reply: TextReply::WatchEnded,
                    }),
                    op => panic!("Core asked the terminal {op:?}"),
                },
                Effect::Speak(utterance) => spoken.push(
                    utterance
                        .segments
                        .iter()
                        .map(|segment| segment.content.clone())
                        .collect(),
                ),
                _ => {}
            }
        }
        pending = inputs
            .iter()
            .flat_map(|input| reduce(state, input))
            .collect();
    }
}

/// Tells the terminal of a key that reached it at `at_ms`, as the keyboard
/// hook's thread tells its outpost of a key it passes on.
fn tell(terminal: &mut Terminal, kind: verbatim_model::TerminalKeyKind, at_ms: u64) {
    let speech = terminal.key(&verbatim_model::TerminalKey { kind, at_ms });
    assert!(speech.is_empty(), "a key alone says nothing");
}

/// What the terminal said, given to Core.
fn said(speech: TerminalSpeech, observed_at_ms: u64) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::TerminalSpeech {
            node_id: terminal_node(),
            speech,
        },
    }
}

/// The console host raises no caret event for every character typed, only
/// its text's change: typing "helo" after the prompt is echoed character
/// by character as the terminal shows it, and Backspace says exactly "o".
fn backspace_says_what_it_deleted_without_caret_events(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-backspace");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    // The screen: three rows, and the empty last line.
    common::apply(&mut app, hwnd, "screen term 4");
    show(&mut app, hwnd, &["one", "two", "ready>"]);
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new(verbatim_model::TerminalSettings::default());
    let _ = read_terminal(&uia, &source, &mut terminal, remote);
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: terminal_node(),
                    backend: Backend::Uia,
                    role: Role::Terminal,
                    name: Some("Terminal".to_owned()),
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
                text: None,
            },
        },
    );
    let mut spoken = Vec::new();
    let mut prompt = "ready>".to_owned();
    for character in ["h", "e", "l", "o"] {
        let at_ms = now_us() / 1_000;
        tell(&mut terminal, verbatim_core::typed_kind(character), at_ms);
        let effects = reduce(
            &mut state,
            &Input::CharacterTyped {
                trace_id: TraceId::mint(),
                text: character.to_owned(),
                at_ms,
            },
        );
        handle(&mut state, effects, &mut spoken);
        prompt.push_str(character);
        show(&mut app, hwnd, &["one", "two", &prompt]);
        let speech = read_terminal(&uia, &source, &mut terminal, remote);
        let effects = reduce(&mut state, &said(speech, now_us() / 1_000));
        handle(&mut state, effects, &mut spoken);
    }
    let at_ms = now_us() / 1_000;
    let backspace = CaretKey {
        motion: CaretMotion::Backspace,
        select: false,
    };
    tell(
        &mut terminal,
        verbatim_core::caret_key_kind(backspace),
        at_ms,
    );
    let effects = reduce(
        &mut state,
        &Input::CaretKey {
            trace_id: TraceId::mint(),
            key: backspace,
            pressed_at_ms: at_ms,
        },
    );
    handle(&mut state, effects, &mut spoken);
    prompt.pop();
    show(&mut app, hwnd, &["one", "two", &prompt]);
    let speech = read_terminal(&uia, &source, &mut terminal, remote);
    let effects = reduce(&mut state, &said(speech, now_us() / 1_000));
    handle(&mut state, effects, &mut spoken);
    let character = |text: &str| vec![SegmentContent::Character(text.to_owned())];
    assert_eq!(
        spoken,
        [
            character("h"),
            character("e"),
            character("l"),
            character("o"),
            character("o"),
        ]
    );
    app.quit();
}

fn backspace_says_what_it_deleted_without_caret_events_remote() {
    backspace_says_what_it_deleted_without_caret_events(true);
}

fn backspace_says_what_it_deleted_without_caret_events_classic() {
    backspace_says_what_it_deleted_without_caret_events(false);
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "new_output_is_said_line_by_line_remote",
            new_output_is_said_line_by_line_remote,
        ),
        (
            "new_output_is_said_line_by_line_classic",
            new_output_is_said_line_by_line_classic,
        ),
        (
            "backspace_says_what_it_deleted_without_caret_events_remote",
            backspace_says_what_it_deleted_without_caret_events_remote,
        ),
        (
            "backspace_says_what_it_deleted_without_caret_events_classic",
            backspace_says_what_it_deleted_without_caret_events_classic,
        ),
    ]);
}
