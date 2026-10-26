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
    /// Rows the terminal shows before the next search Core asks for, read
    /// by a live read sent to Core before the search runs, as when a
    /// program keeps printing while a search is on its way.
    before_find: Option<(&'a mut common::MockApp, HWND, Vec<String>)>,
    /// How many searches Core asked for.
    finds: u32,
    /// Rows the terminal shows before the next history read Core asks
    /// for, as [`Session::before_find`] before a search.
    before_history: Option<(&'a mut common::MockApp, HWND, Vec<String>)>,
    /// How many history reads Core asked for.
    histories: u32,
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
                // The session reads only when a test or Core asks.
                TextOp::TerminalHold => TextReply::Done,
                TextOp::TerminalHistory { above, whole } => {
                    self.histories += 1;
                    if let Some((app, hwnd, shown)) = self.before_history.take() {
                        let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
                        show(app, hwnd, &shown);
                        self.live();
                    }
                    let history = verbatim_outpost::terminal::history(
                        self.uia,
                        (self.source.element(), self.source.pattern()),
                        &self.terminal,
                        (*above, *whole, self.remote),
                    )
                    .expect("the history is read");
                    // The path asked for answered, never the fallback.
                    let path = history.path.name();
                    assert_eq!(path, if self.remote { "remote" } else { "classic" });
                    TextReply::TerminalHistory {
                        text: history.text,
                        above: history.above,
                        read: history.read,
                    }
                }
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
                TextOp::TerminalFind { needle, from, rows } => {
                    self.finds += 1;
                    if let Some((app, hwnd, shown)) = self.before_find.take() {
                        let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
                        show(app, hwnd, &shown);
                        self.live();
                    }
                    let found = verbatim_outpost::terminal::find(
                        self.uia,
                        (self.source.element(), self.source.pattern()),
                        &self.terminal,
                        (needle, *from, *rows),
                        self.remote,
                    )
                    .expect("the terminal is searched");
                    // The path asked for answered, never the fallback.
                    let path = found.path.name();
                    assert_eq!(path, if self.remote { "remote" } else { "classic" });
                    TextReply::TerminalFound {
                        matches: found.matches,
                        read: found.read,
                    }
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
        before_find: None,
        finds: 0,
        before_history: None,
        histories: 0,
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
        before_find: None,
        finds: 0,
        before_history: None,
        histories: 0,
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

/// A command is typed at the prompt and entered, so Core marks its line;
/// output carries it above the rows Core keeps; Verbatim+Alt+Up Arrow then
/// finds it with the outpost's search, moves the review cursor there and
/// speaks it, and a second press, with no mark above, says nothing.
fn command_marks_move_the_review_cursor(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-marks");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let mut session = session_with_a_command(&uia, &mut app, hwnd, remote);
    session.input(&command(ReviewCommand::PreviousCommandMark, 0));
    let text = |text: &str| SegmentContent::Text(text.to_owned());
    assert_eq!(session.spoken, [vec![text("ready>ls")]]);
    session.spoken.clear();
    session.input(&command(ReviewCommand::PreviousCommandMark, 0));
    assert_eq!(session.spoken, Vec::<Vec<SegmentContent>>::new());
    app.quit();
}

fn command_marks_move_the_review_cursor_remote() {
    command_marks_move_the_review_cursor(true);
}

fn command_marks_move_the_review_cursor_classic() {
    command_marks_move_the_review_cursor(false);
}

/// The rows of [`session_with_a_command`]'s terminal: `ls` entered, and
/// `lines` lines of output after its answer.
fn rows_after_ls(lines: u32) -> Vec<String> {
    let mut rows: Vec<String> = ["one", "ready>ls", "a.txt"].map(str::to_owned).into();
    rows.extend((1..=lines).map(|line| format!("line {line}")));
    rows.push("ready>".to_owned());
    rows
}

/// As [`command_marks_move_the_review_cursor`], but the program prints three
/// more lines before the outpost searches, read and sent to Core first: the
/// search's answer is counted from that newer read, and the move still
/// lands on the command's line, then says nothing past it.
fn command_marks_are_found_while_output_scrolls(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-marks-scrolling");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    {
        let mut session = session_with_a_command(&uia, &mut app, hwnd, remote);
        session.before_find = Some((&mut app, hwnd, rows_after_ls(13)));
        session.input(&command(ReviewCommand::PreviousCommandMark, 0));
        let text = |text: &str| vec![SegmentContent::Text(text.to_owned())];
        assert_eq!(session.finds, 1);
        // The new output waits in the flood policy's queue behind the
        // earlier output, which this session never plays.
        assert_eq!(session.spoken, [text("ready>ls")]);
        session.spoken.clear();
        session.input(&command(ReviewCommand::ReviewNextLine, 0));
        assert_eq!(session.spoken, [text("a.txt")]);
    }
    app.quit();
}

fn command_marks_are_found_while_output_scrolls_remote() {
    command_marks_are_found_while_output_scrolls(true);
}

fn command_marks_are_found_while_output_scrolls_classic() {
    command_marks_are_found_while_output_scrolls(false);
}

/// The review cursor on the command's line above the rows Core keeps, as
/// [`command_marks_move_the_review_cursor`] puts it there; the program
/// prints three more lines before the outpost reads the history the next
/// line needs, read and sent to Core first: the history's rows are
/// counted from that newer read's start, the screen more Core asked for
/// still holds the command's output, and the review cursor moves to it,
/// with one history read on the remote path.
fn the_history_is_placed_while_output_scrolls_remote() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-history-scrolling");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    {
        let mut session = session_with_a_command(&uia, &mut app, hwnd, true);
        session.input(&command(ReviewCommand::PreviousCommandMark, 0));
        let text = |text: &str| vec![SegmentContent::Text(text.to_owned())];
        assert_eq!(session.spoken, [text("ready>ls")]);
        session.spoken.clear();
        session.histories = 0;
        session.before_history = Some((&mut app, hwnd, rows_after_ls(13)));
        session.input(&command(ReviewCommand::ReviewNextLine, 0));
        assert!(session.before_history.is_none(), "the history was read");
        assert_eq!(session.histories, 1);
        // The new output waits in the flood policy's queue, which this
        // session never plays.
        assert_eq!(session.spoken, [text("a.txt")]);
    }
    app.quit();
}

/// A session on a terminal of four rows where `ls` was typed at the prompt
/// and entered, so Core marked its line, and ten lines of output then
/// carried it above the rows Core keeps.
fn session_with_a_command<'a>(
    uia: &'a Uia,
    app: &mut common::MockApp,
    hwnd: HWND,
    remote: bool,
) -> Session<'a> {
    common::apply(app, hwnd, "screen term 4");
    show(app, hwnd, &["one", "ready>"]);
    let mut session = session(uia, hwnd, remote);
    let mut prompt = "ready>".to_owned();
    for character in ["l", "s", "\r"] {
        session.input(&Input::CharacterTyped {
            trace_id: TraceId::mint(),
            text: character.to_owned(),
            at_ms: now_us() / 1_000,
        });
        if character == "\r" {
            show(app, hwnd, &["one", &prompt, "a.txt", "ready>"]);
        } else {
            prompt.push_str(character);
            show(app, hwnd, &["one", &prompt]);
        }
        session.live();
    }
    let rows = rows_after_ls(10);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    show(app, hwnd, &rows);
    session.live();
    session.spoken.clear();
    session
}

/// The outpost's search for `needle` in `terminal`'s text, outward from the
/// row `from` rows above where its next read starts, on the path under
/// test alone: the remote program, never falling back, or the classic one.
fn find_on_path(
    uia: &Uia,
    source: &UiaText,
    terminal: &Terminal,
    (needle, from): (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    let start = terminal.start_range().expect("a read placed the start");
    let query = verbatim_uia_rops::FindQuery {
        element: source.element(),
        pattern: source.pattern(),
        start: &start,
        needle,
        from,
        rows: 1,
    };
    let found = if remote {
        verbatim_uia_rops::terminal_find_remote(uia, &query)
    } else {
        verbatim_uia_rops::terminal_find_classic(uia, &query)
    };
    found
        .expect("the search runs on its path")
        .into_iter()
        .map(|found| verbatim_model::TerminalMatch {
            above: found.above,
            text: found.text,
        })
        .collect()
}

/// A provider whose `FindText` fails (`terminal_find_fails.json`), as
/// Windows Terminal's not-found range does: the search finds nothing, and
/// is no error.
fn a_failing_search_finds_nothing(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-fails");
    let mut app = common::spawn("terminal_find_fails.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, (None, remote));
    let found = find_on_path(&uia, &source, &terminal, ("two", 1), remote);
    assert_eq!(found, []);
    app.quit();
}

fn a_failing_search_finds_nothing_remote() {
    a_failing_search_finds_nothing(true);
}

fn a_failing_search_finds_nothing_classic() {
    a_failing_search_finds_nothing(false);
}

/// A provider whose `FindText` fails when it searches backward
/// (`terminal_find_backward_fails.json`): the failure closes the search's
/// upward direction alone, and the search goes on downward from its row.
fn a_failing_search_goes_on_in_the_other_direction(remote: bool) {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    let found = search_in("terminal_find_backward_fails.json", &rows, ("y", 2), remote);
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(1, "y 4"),
            found_row(0, "y 5"),
        ]
    );
}

fn a_failing_search_goes_on_in_the_other_direction_remote() {
    a_failing_search_goes_on_in_the_other_direction(true);
}

fn a_failing_search_goes_on_in_the_other_direction_classic() {
    a_failing_search_goes_on_in_the_other_direction(false);
}

/// The outpost's search for `needle` over every row of `rows`, outward
/// from the row `from` rows above where the next read starts, on a screen
/// of three rows.
fn search(
    rows: &[&str],
    needle_from: (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    search_in("terminal.json", rows, needle_from, remote)
}

/// As [`search`], on the terminal of `fixture`.
fn search_in(
    fixture: &str,
    rows: &[&str],
    (needle, from): (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find");
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    show(&mut app, hwnd, rows);
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, (None, remote));
    let found = find_on_path(&uia, &source, &terminal, (needle, from), remote);
    app.quit();
    found
}

/// A match: its rows above the start, and its row's text as read.
fn found_row(above: i32, text: &str) -> verbatim_model::TerminalMatch {
    verbatim_model::TerminalMatch {
        above,
        text: format!("{text:WIDTH$}\r\n"),
    }
}

/// A short needle found within rows far more often than at a row's start,
/// as a short command's text is, sought up from below the document's end:
/// only the rows that start with it are matches, nearest first, a longer
/// row starting with it among them for Core to check by its line's hash,
/// and the hits within rows do not use up the matches a search takes.
fn the_search_takes_rows_starting_with_the_needle(remote: bool) {
    let within = " y y y y y y";
    let mut rows = vec!["y", "yes"];
    rows.extend([within; 5]);
    rows.push("ready>");
    let found = search(&rows, ("y", -10), remote);
    assert_eq!(found, [found_row(5, "yes"), found_row(6, "y")]);
}

fn the_search_takes_rows_starting_with_the_needle_remote() {
    the_search_takes_rows_starting_with_the_needle(true);
}

fn the_search_takes_rows_starting_with_the_needle_classic() {
    the_search_takes_rows_starting_with_the_needle(false);
}

/// From a row in the middle, the search takes that row's match first, then
/// alternately the next above and the next below.
fn the_search_works_outward_from_its_row(remote: bool) {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    // The document's seven rows end with an empty line, and the start is
    // the screen's top, three rows from the end: "y 3", on row 2, is two
    // rows above it.
    let found = search(&rows, ("y", 2), remote);
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(3, "y 2"),
            found_row(1, "y 4"),
            found_row(4, "y 1"),
            found_row(0, "y 5"),
        ]
    );
}

fn the_search_works_outward_from_its_row_remote() {
    the_search_works_outward_from_its_row(true);
}

fn the_search_works_outward_from_its_row_classic() {
    the_search_works_outward_from_its_row(false);
}

/// A needle found within rows more often than the search's `FindText`
/// calls allow ends the search before a row starting with it, further
/// away, is reached.
fn the_search_ends_after_its_calls(remote: bool) {
    let within = " y y y y y y";
    let mut rows = vec!["y"];
    // Six hits a row, a row more than the calls allow.
    rows.extend(vec![within; verbatim_uia_rops::FIND_CALLS / 6 + 1]);
    rows.push("ready>");
    assert_eq!(search(&rows, ("y", -10), remote), []);
}

fn the_search_ends_after_its_calls_remote() {
    the_search_ends_after_its_calls(true);
}

fn the_search_ends_after_its_calls_classic() {
    the_search_ends_after_its_calls(false);
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
        before_find: None,
        finds: 0,
        before_history: None,
        histories: 0,
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
        (
            "command_marks_move_the_review_cursor_remote",
            command_marks_move_the_review_cursor_remote,
        ),
        (
            "command_marks_move_the_review_cursor_classic",
            command_marks_move_the_review_cursor_classic,
        ),
        (
            "command_marks_are_found_while_output_scrolls_remote",
            command_marks_are_found_while_output_scrolls_remote,
        ),
        (
            "command_marks_are_found_while_output_scrolls_classic",
            command_marks_are_found_while_output_scrolls_classic,
        ),
        (
            "the_history_is_placed_while_output_scrolls_remote",
            the_history_is_placed_while_output_scrolls_remote,
        ),
        (
            "the_search_works_outward_from_its_row_remote",
            the_search_works_outward_from_its_row_remote,
        ),
        (
            "the_search_works_outward_from_its_row_classic",
            the_search_works_outward_from_its_row_classic,
        ),
        (
            "a_failing_search_finds_nothing_remote",
            a_failing_search_finds_nothing_remote,
        ),
        (
            "a_failing_search_finds_nothing_classic",
            a_failing_search_finds_nothing_classic,
        ),
        (
            "a_failing_search_goes_on_in_the_other_direction_remote",
            a_failing_search_goes_on_in_the_other_direction_remote,
        ),
        (
            "a_failing_search_goes_on_in_the_other_direction_classic",
            a_failing_search_goes_on_in_the_other_direction_classic,
        ),
        (
            "the_search_takes_rows_starting_with_the_needle_remote",
            the_search_takes_rows_starting_with_the_needle_remote,
        ),
        (
            "the_search_takes_rows_starting_with_the_needle_classic",
            the_search_takes_rows_starting_with_the_needle_classic,
        ),
        (
            "the_search_ends_after_its_calls_remote",
            the_search_ends_after_its_calls_remote,
        ),
        (
            "the_search_ends_after_its_calls_classic",
            the_search_ends_after_its_calls_classic,
        ),
    ]);
}
