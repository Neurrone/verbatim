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
//! Each read is given to Core's reducer as the outpost sends it, and Core's
//! requests of the terminal (the screen's shape, a read at once, the
//! colours after a key) are answered as the worker answers them.
//!
//! What is checked: Core says the new output a read shows, line by line,
//! and the read costs one call remotely; and a terminal whose text changes
//! with no caret event, as the console host's does while typing, has
//! Core's reading rules echo the typing and say what Backspace deleted,
//! the keys reaching them through Core; and "Report status bar" reads the
//! rows below the caret after a fresh read.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CallCounts, CaretKey, CaretMotion, Effect, Input, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Pid, ReviewCommand, Role, SegmentContent, StateSet, TerminalScope,
    TerminalSnapshot, TextOp, TextReply, TraceId,
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

/// One read of the terminal of `scope` (a live read for `None`), as the
/// worker makes it, and the snapshot it sends Core.
fn read_terminal(
    uia: &Uia,
    source: &UiaText,
    terminal: &mut Terminal,
    (scope, remote): (Option<TerminalScope>, bool),
) -> TerminalSnapshot {
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
        (scope, remote, false),
    )
    .expect("the terminal reads")
    .snapshot
}

/// mockapp's terminal read by the outpost's terminal module, and Core's
/// reducer it sends its reads to.
struct Session<'a> {
    uia: &'a Uia,
    source: UiaText,
    terminal: Terminal,
    remote: bool,
    state: SrState,
    /// What Core said, each utterance's segments but its index marks.
    spoken: Vec<Vec<SegmentContent>>,
}

impl Session<'_> {
    /// A live read, sent to Core as the outpost sends it.
    fn live(&mut self) {
        let input = self.live_input();
        self.input(&input);
    }

    /// A live read, as the input that brings it to Core.
    fn live_input(&mut self) -> Input {
        let snapshot = read_terminal(
            self.uia,
            &self.source,
            &mut self.terminal,
            (None, self.remote),
        );
        Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: snapshot.started_ms,
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::TerminalSnapshot {
                node_id: terminal_node(),
                snapshot: Box::new(snapshot),
            },
        }
    }

    /// Gives Core `input`, and handles what it does.
    fn input(&mut self, input: &Input) {
        let effects = reduce(&mut self.state, input);
        self.handle(effects);
    }

    /// Keeps what Core spoke of `effects` and answers its requests as the
    /// worker does; a caret key's watch is answered as ended, with nothing
    /// Core speaks.
    fn handle(&mut self, effects: Vec<Effect>) {
        let mut pending = effects;
        while !pending.is_empty() {
            let mut inputs = Vec::new();
            for effect in pending {
                match effect {
                    Effect::ForKey(_, effect) => {
                        if let Some(input) = self.effect(*effect) {
                            inputs.push(input);
                        }
                    }
                    effect => {
                        if let Some(input) = self.effect(effect) {
                            inputs.push(input);
                        }
                    }
                }
            }
            pending = inputs
                .iter()
                .flat_map(|input| reduce(&mut self.state, input))
                .collect();
        }
    }

    /// One effect of Core's: speech kept, a request answered.
    fn effect(&mut self, effect: Effect) -> Option<Input> {
        let reply = match effect {
            Effect::Text(request) => match &request.op {
                TextOp::AwaitCaret(_) => TextReply::WatchEnded,
                TextOp::TerminalShape { height, width } => {
                    self.terminal.shape(*height, *width);
                    TextReply::Done
                }
                TextOp::TerminalReadNow(scope) => {
                    TextReply::TerminalSnapshot(Box::new(read_terminal(
                        self.uia,
                        &self.source,
                        &mut self.terminal,
                        (Some(*scope), self.remote),
                    )))
                }
                TextOp::TerminalColours { from_ms, until_ms } => {
                    // A read at once is sent as a live read; the request's
                    // own answer, done, says nothing to Core.
                    if self.terminal.colours((*from_ms, *until_ms)) {
                        return Some(self.live_input());
                    }
                    TextReply::Done
                }
                op => panic!("Core asked the terminal {op:?}"),
            }
            .into_input(request.query_id),
            Effect::Speak(utterance) => {
                self.spoken.push(
                    utterance
                        .segments
                        .iter()
                        .map(|segment| segment.content.clone())
                        .filter(|content| !matches!(content, SegmentContent::Mark(_)))
                        .collect(),
                );
                return None;
            }
            _ => return None,
        };
        Some(reply)
    }
}

/// An answer as the input that brings it to Core.
trait IntoInput {
    fn into_input(self, query_id: verbatim_model::QueryId) -> Input;
}

impl IntoInput for TextReply {
    fn into_input(self, query_id: verbatim_model::QueryId) -> Input {
        Input::TextCompleted {
            trace_id: TraceId::mint(),
            query_id,
            reply: self,
        }
    }
}

/// The focus on mockapp's terminal, as Core takes it.
fn focus() -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        observed_at_us: 0,
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
    }
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
    let mut session = Session {
        uia: &uia,
        source: source(&uia, hwnd, remote),
        terminal: Terminal::new(),
        remote,
        state: SrState::new(),
        spoken: Vec::new(),
    };
    session.input(&focus());
    session.spoken.clear();
    session.live();
    assert_eq!(
        session.spoken,
        Vec::<Vec<SegmentContent>>::new(),
        "the first read says nothing"
    );
    show(&mut app, hwnd, &["one", "two", "three", "four", "ready>"]);
    let _ = verbatim_uia::calls::take();
    let snapshot = read_terminal(&uia, &session.source, &mut session.terminal, (None, remote));
    let calls = verbatim_uia::calls::take();
    if remote {
        assert_eq!(calls, uia_calls(1), "one program, one call");
    }
    session.input(&Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: snapshot.started_ms,
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::TerminalSnapshot {
            node_id: terminal_node(),
            snapshot: Box::new(snapshot),
        },
    });
    let text = |text: &str| vec![SegmentContent::Text(text.to_owned())];
    assert_eq!(session.spoken, [text("three"), text("four")]);
    app.quit();
}

fn new_output_is_said_line_by_line_remote() {
    new_output_is_said_line_by_line(true);
}

fn new_output_is_said_line_by_line_classic() {
    new_output_is_said_line_by_line(false);
}

/// Shows `rows` in mockapp's terminal with the caret at the end of row
/// `caret`'s text.
fn show_caret_on(app: &mut common::MockApp, hwnd: HWND, rows: &[&str], caret: usize) {
    let (text, _) = terminal_text(rows);
    let row_units = WIDTH + 2;
    let at = caret * row_units + rows[caret].encode_utf16().count();
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, &format!("caret term {at} {at}"));
}

/// A session over mockapp's terminal, focused and read once.
fn session(uia: &Uia, hwnd: HWND, remote: bool) -> Session<'_> {
    let mut session = Session {
        uia,
        source: source(uia, hwnd, remote),
        terminal: Terminal::new(),
        remote,
        state: SrState::new(),
        spoken: Vec::new(),
    };
    session.input(&focus());
    session.live();
    session.spoken.clear();
    session
}

/// A command's first press.
fn command(command: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        trace_id: TraceId::mint(),
        command,
        repeat,
    }
}

/// "Report status bar" reads the rows below the caret, after the fresh read
/// Core asks the outpost for, and nothing else.
fn the_status_bar_is_read_below_the_caret(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-status-bar");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 5");
    show_caret_on(&mut app, hwnd, &["one", "ready>", "status: busy"], 1);
    let mut session = session(&uia, hwnd, remote);
    show_caret_on(&mut app, hwnd, &["one", "ready>", "status: done"], 1);
    session.input(&command(ReviewCommand::ReportStatusBar, 0));
    let text = |text: &str| SegmentContent::Text(text.to_owned());
    assert_eq!(
        session.spoken,
        [vec![text("done")], vec![text("status: done")]]
    );
    app.quit();
}

fn the_status_bar_is_read_below_the_caret_remote() {
    the_status_bar_is_read_below_the_caret(true);
}

fn the_status_bar_is_read_below_the_caret_classic() {
    the_status_bar_is_read_below_the_caret(false);
}

/// The node Core knows mockapp's terminal by.
fn terminal_node() -> NodeId {
    NodeId::in_outpost(OutpostId(1), 1)
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
    let mut session = Session {
        uia: &uia,
        source: source(&uia, hwnd, remote),
        terminal: Terminal::new(),
        remote,
        state: SrState::new(),
        spoken: Vec::new(),
    };
    session.input(&focus());
    session.live();
    session.spoken.clear();
    let mut prompt = "ready>".to_owned();
    for character in ["h", "e", "l", "o"] {
        session.input(&Input::CharacterTyped {
            trace_id: TraceId::mint(),
            text: character.to_owned(),
            at_ms: now_us() / 1_000,
        });
        prompt.push_str(character);
        show(&mut app, hwnd, &["one", "two", &prompt]);
        session.live();
    }
    session.input(&Input::CaretKey {
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::Backspace,
            select: false,
        },
        pressed_at_ms: now_us() / 1_000,
    });
    prompt.pop();
    show(&mut app, hwnd, &["one", "two", &prompt]);
    session.live();
    let character = |text: &str| vec![SegmentContent::Character(text.to_owned())];
    assert_eq!(
        session.spoken,
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
        (
            "the_status_bar_is_read_below_the_caret_remote",
            the_status_bar_is_read_below_the_caret_remote,
        ),
        (
            "the_status_bar_is_read_below_the_caret_classic",
            the_status_bar_is_read_below_the_caret_classic,
        ),
    ]);
}
