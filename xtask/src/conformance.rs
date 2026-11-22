//! `cargo xtask regenerate-terminal-conformance-expectations`: writes each
//! stream of `verbatim-term-sim`'s conformance corpus into the real console
//! host and the end-to-end suite's pinned Windows Terminal, reads each
//! after every chunk by the simulator's query, call by call ([`Read`]),
//! with each screen row's colours read cell by cell, and saves what it
//! answered as the expectations the simulated terminal is tested against
//! (`books/dev/src/explanation/design/terminal-model.md`, "Testing"). It
//! opens terminal windows, so it runs on demand on a desktop, never in
//! continuous integration.
//!
//! The terminal runs this program's `conformance-player`, which connects
//! back over a local socket, writes each chunk it is sent, and then sets
//! the window's title to a token naming the chunk: the terminal changes its
//! title only once it has parsed everything written before, so the new
//! title is the evidence that the chunk is in the buffer the terminal's
//! provider reads, and the runner reads it only then. After a chunk that
//! ends inside a character's encoding, the title is set with the console's
//! title call rather than a sequence, which would end the character; the
//! terminal shows nothing for the character's first bytes.

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::AsRawHandle as _;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_term_sim::conformance::{
    Answer, Expectation, Stream, expectation_path, inside_character, streams,
};
use verbatim_term_sim::{Flavor, Run};
use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt as _, TextRangeExt as _};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::CP_UTF8;
use windows::Win32::System::Console::{
    DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WRAP_AT_EOL_OUTPUT, SetConsoleMode, SetConsoleOutputCP, SetConsoleTitleW,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit_Character,
    TextUnit_Line, TreeScope_Subtree, UIA_BackgroundColorAttributeId, UIA_ClassNamePropertyId,
    UIA_ForegroundColorAttributeId, UIA_IsTextPatternAvailablePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowTextW, PostMessageW, WM_CLOSE,
};
use windows::core::{BSTR, HSTRING, PCWSTR};

/// How long anything the runner waits for may take: a terminal starting,
/// or answering a chunk.
const PATIENCE: Duration = Duration::from_secs(30);

/// The flavors' names on the command line and in messages.
fn flavor_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::ConsoleHost => "console-host",
        Flavor::WindowsTerminal => "windows-terminal",
    }
}

/// `regenerate-terminal-conformance-expectations [--stream <name>]...
/// [--flavor console-host|windows-terminal]...`: all streams in both
/// terminals unless narrowed.
pub fn regenerate(args: &[String]) -> ExitCode {
    let mut names = Vec::new();
    let mut flavors = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--stream", Some(name)) => names.push(name.clone()),
            ("--flavor", Some(name)) if name == "console-host" => flavors.push(Flavor::ConsoleHost),
            ("--flavor", Some(name)) if name == "windows-terminal" => {
                flavors.push(Flavor::WindowsTerminal);
            }
            _ => {
                eprintln!(
                    "usage: cargo xtask regenerate-terminal-conformance-expectations [--stream <name>]... [--flavor console-host|windows-terminal]..."
                );
                return ExitCode::FAILURE;
            }
        }
    }
    if flavors.is_empty() {
        flavors = vec![Flavor::ConsoleHost, Flavor::WindowsTerminal];
    }
    let uia = match Uia::new() {
        Ok(uia) => uia,
        Err(error) => {
            eprintln!("UI Automation is not available: {error}");
            return ExitCode::FAILURE;
        }
    };
    let terminal =
        match verbatim_e2e::windows_terminal::prepare(&verbatim_e2e::scenario::stage_directory()) {
            Ok(folder) => folder.join(verbatim_e2e::windows_terminal::EXECUTABLE),
            Err(error) => {
                eprintln!("could not prepare Windows Terminal: {error}");
                return ExitCode::FAILURE;
            }
        };
    let mut failed = false;
    for (name, stream) in streams() {
        if !names.is_empty() && !names.contains(&name) {
            continue;
        }
        for &flavor in &flavors {
            match run(&uia, &terminal, &name, &stream, flavor) {
                Ok(expectation) => {
                    let path = expectation_path(&name, flavor);
                    let text = serde_json::to_string_pretty(&expectation)
                        .expect("an expectation serializes");
                    if let Err(error) = std::fs::write(&path, text + "\n") {
                        eprintln!(
                            "{name} in {}: could not write {}: {error}",
                            flavor_name(flavor),
                            path.display()
                        );
                        failed = true;
                    } else {
                        println!(
                            "{name} in {}: {} reads",
                            flavor_name(flavor),
                            expectation.answers.len()
                        );
                    }
                }
                Err(error) => {
                    eprintln!("{name} in {}: {error}", flavor_name(flavor));
                    failed = true;
                }
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// One stream in one terminal.
fn run(
    uia: &Uia,
    windows_terminal: &std::path::Path,
    name: &str,
    stream: &Stream,
    flavor: Flavor,
) -> Result<Expectation, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let title = format!(
        "verbatim-conformance-{name}-{}-{}",
        flavor_name(flavor),
        std::process::id()
    );
    let player = std::env::current_exe().map_err(|error| error.to_string())?;
    let player = player.to_string_lossy().into_owned();
    let player_args = [
        "conformance-player".to_owned(),
        port.to_string(),
        title.clone(),
    ];
    let mut command = match flavor {
        Flavor::ConsoleHost => {
            // Started as the shell starts it, through `start`: given this
            // process's console or any standard handles, even null ones, the
            // console host serves them as a pseudoconsole instead of opening
            // a window of its own.
            let mut command = Command::new("cmd.exe");
            command
                .args(["/d", "/c", "start", "", "conhost.exe"])
                .arg(&player)
                .args(&player_args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command
        }
        Flavor::WindowsTerminal => {
            let mut command = Command::new(windows_terminal);
            command
                .args([
                    "-w", "new", "--size", "120,30", "new-tab", "--title", &title,
                ])
                .arg(&player)
                .args(&player_args);
            command
        }
    };
    command
        .spawn()
        .map_err(|error| format!("could not start the terminal: {error}"))?;
    let connection = accept(&listener)?;
    // Found once, by the title the player set before writing anything: a
    // stream may set the title itself.
    let window = find_window(&title).ok_or("the terminal's window did not appear")?;
    let result = read_stream(uia, &connection, window, &title, stream, flavor);
    let mut writer = &connection;
    let _ = writer.write_all(b"quit\n");
    // SAFETY: posting a message to a window handle is sound whatever the
    // window's state.
    let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    result
}

/// The player's connection, once it has made it.
fn accept(listener: &TcpListener) -> Result<TcpStream, String> {
    let (sender, receiver) = mpsc::channel();
    let listener = listener.try_clone().map_err(|error| error.to_string())?;
    std::thread::spawn(move || {
        let _ = sender.send(listener.accept().map(|(stream, _)| stream));
    });
    let stream = receiver
        .recv_timeout(PATIENCE)
        .map_err(|_| "the player did not connect".to_owned())?
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(PATIENCE))
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

/// The terminal's window, by its title, once it has one.
fn find_window(title: &str) -> Option<HWND> {
    let title = HSTRING::from(title);
    let deadline = Instant::now() + PATIENCE;
    loop {
        // SAFETY: both strings outlive the call.
        if let Ok(window) = unsafe { FindWindowW(PCWSTR::null(), &title) }
            && !window.is_invalid()
        {
            return Some(window);
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until the window's title reads `title`: the player sets it after
/// each chunk, and a terminal changes its title only once it has parsed
/// everything written before, so the title is the evidence that the chunk
/// is in the buffer the terminal's provider reads. Windows Terminal's own
/// parser handles the title, where a cursor-position query could be
/// answered by the pseudoconsole in front of it.
fn wait_for_title(window: HWND, title: &str) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    let mut buffer = [0u16; 512];
    loop {
        // SAFETY: the buffer outlives the call.
        let length = unsafe { GetWindowTextW(window, &mut buffer) };
        let length = usize::try_from(length).unwrap_or(0);
        if String::from_utf16_lossy(&buffer[..length]) == title {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("the window's title never became {title:?}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The element with the terminal's text, and its text pattern.
fn text_element(
    uia: &Uia,
    window: HWND,
    flavor: Flavor,
) -> Result<(IUIAutomationElement, IUIAutomationTextPattern), String> {
    let client = uia.client();
    let error = |error: windows::core::Error| error.to_string();
    // SAFETY: each call below is made on an initialized UI Automation
    // client with arguments that outlive it.
    let root = unsafe { client.ElementFromHandle(window) }.map_err(error)?;
    // SAFETY: as above.
    let has_text = unsafe {
        client.CreatePropertyCondition(UIA_IsTextPatternAvailablePropertyId, &VARIANT::from(true))
    }
    .map_err(error)?;
    // SAFETY: as above.
    let control = unsafe {
        client.CreatePropertyCondition(
            UIA_ClassNamePropertyId,
            &VARIANT::from(BSTR::from("TermControl")),
        )
    }
    .map_err(error)?;
    // SAFETY: as above.
    let both = unsafe { client.CreateAndCondition(&has_text, &control) }.map_err(error)?;
    // Windows Terminal's tab title is text too, so its terminal is its
    // `TermControl`, waited for until it exists; the console host's text
    // area is its window's one element with text.
    let condition = match flavor {
        Flavor::WindowsTerminal => &both,
        Flavor::ConsoleHost => &has_text,
    };
    let deadline = Instant::now() + PATIENCE;
    let element = loop {
        // SAFETY: as above.
        match unsafe { root.FindFirst(TreeScope_Subtree, condition) } {
            Ok(element) => break element,
            Err(failure) if Instant::now() > deadline => return Err(error(failure)),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let pattern = verbatim_uia::text::text_pattern(&element).map_err(error)?;
    Ok((element, pattern))
}

/// Sends each chunk and reads the terminal once the player says it is on
/// the screen.
fn read_stream(
    uia: &Uia,
    connection: &TcpStream,
    window: HWND,
    title: &str,
    stream: &Stream,
    flavor: Flavor,
) -> Result<Expectation, String> {
    let (_, pattern) = text_element(uia, window, flavor)?;
    let mut lines = BufReader::new(connection.try_clone().map_err(|error| error.to_string())?);
    let mut writer = connection;
    let mut start: Option<IUIAutomationTextRange> = None;
    let mut screen_rows = 0;
    let mut answers = Vec::new();
    let size = empty_screen_size(&pattern)?;
    let inside = inside_character(stream);
    for (index, (chunk, inside)) in stream.chunks.iter().zip(inside).enumerate() {
        let hex = chunk
            .as_bytes()
            .iter()
            .fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        // A chunk that ends inside a character is marked by the console's
        // title call, since a title sequence written after it would end the
        // character; the terminal holds the character's first bytes and
        // shows nothing new for them.
        let mark = if inside { "call" } else { "sequence" };
        writer
            .write_all(format!("chunk {index} {mark} {hex}\n").as_bytes())
            .map_err(|error| error.to_string())?;
        let mut reply = String::new();
        lines
            .read_line(&mut reply)
            .map_err(|error| format!("no reply from the player: {error}"))?;
        if reply.trim() != "written" {
            return Err(format!("the player answered {reply:?}"));
        }
        wait_for_title(window, &format!("{title}#{index}"))?;
        let snapshot =
            Read::of(&pattern, (start.as_ref(), screen_rows)).map_err(|error| error.to_string())?;
        let screen = snapshot.screen.clone();
        screen_rows = verbatim_term_sim::screen_rows(&screen);
        start = Some(snapshot.next_start.clone());
        let (caret_line, caret_column) = caret(&pattern).map_err(|error| error.to_string())?;
        let colours = colours(&pattern, size).map_err(|error| error.to_string())?;
        answers.push(Answer {
            text: snapshot.text.clone(),
            screen,
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: snapshot.first_row,
            caret_rows_to_end: snapshot.caret_rows_to_end,
            caret_line,
            caret_column,
            colours,
        });
    }
    Ok(Expectation {
        flavor,
        width: size.0,
        height: size.1,
        answers,
    })
}

/// What the simulator's query reads of a real terminal
/// (`verbatim_term_sim::Query`), call by call: from the start the last
/// read gave, when the text still reaches it, else from the screen's top,
/// to the document's end.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the simulator's answer's facts, as its `Answer` holds them"
)]
struct Read {
    text: String,
    screen: String,
    from_start: bool,
    shrank: bool,
    at_document_start: bool,
    alternate: bool,
    first_row: String,
    caret_rows_to_end: u32,
    next_start: IUIAutomationTextRange,
}

/// A copy of `range` collapsed to its start.
fn collapsed(range: &IUIAutomationTextRange) -> windows::core::Result<IUIAutomationTextRange> {
    let copy = range.clone_range()?;
    copy.move_endpoint_to(Endpoint::End, &copy, Endpoint::Start)?;
    Ok(copy)
}

impl Read {
    /// The read from `start`, the next start the last read gave, of a
    /// screen `screen_rows` rows high as the last read found it, 0 for a
    /// first read.
    fn of(
        pattern: &IUIAutomationTextPattern,
        (start, screen_rows): (Option<&IUIAutomationTextRange>, u32),
    ) -> windows::core::Result<Self> {
        let document = pattern.document_range()?;
        let screen = pattern
            .visible_ranges()?
            .into_iter()
            .next()
            .unwrap_or_else(|| document.clone());
        let screen_text = String::from_utf16_lossy(&screen.text(-1)?);
        let alternate = document.compare_endpoints(Endpoint::Start, &screen, Endpoint::Start)? == 0;
        let next = if screen_rows > 0 {
            let next = document.clone_range()?;
            next.move_endpoint_to(Endpoint::Start, &document, Endpoint::End)?;
            next.move_by(
                TextUnit_Line,
                -i32::try_from(screen_rows).unwrap_or(i32::MAX),
            )?;
            next
        } else {
            collapsed(&screen)?
        };
        let mut from = if screen_rows > 0 {
            next.clone_range()?
        } else {
            collapsed(&screen)?
        };
        let (mut from_start, mut shrank) = (false, false);
        if let Some(kept) = start {
            let at = || -> windows::core::Result<(IUIAutomationTextRange, i32)> {
                let at = collapsed(kept)?;
                let order = at.compare_endpoints(Endpoint::Start, &next, Endpoint::Start)?;
                Ok((at, order))
            };
            match at() {
                Ok((at, order)) if order <= 0 => {
                    from = at;
                    from_start = true;
                }
                Ok(_) | Err(_) => shrank = true,
            }
        }
        let read = from.clone_range()?;
        read.move_endpoint_to(Endpoint::End, &document, Endpoint::End)?;
        let text = String::from_utf16_lossy(&read.text(-1)?);
        let at_document_start =
            from.compare_endpoints(Endpoint::Start, &document, Endpoint::Start)? == 0;
        let first = collapsed(&document)?;
        first.expand(TextUnit_Line)?;
        let first_row = String::from_utf16_lossy(&first.text(-1)?);
        let caret_rows_to_end = match pattern.selection()?.into_iter().next() {
            Some(caret) => {
                u32::try_from(collapsed(&caret)?.move_by(TextUnit_Line, 1_000_000)?).unwrap_or(0)
            }
            None => 0,
        };
        Ok(Self {
            text,
            screen: screen_text,
            from_start,
            shrank,
            at_document_start,
            alternate,
            first_row,
            caret_rows_to_end,
            next_start: next,
        })
    }
}

/// The screen's size, read from the empty terminal before the first chunk,
/// when each row is a line of its own: a program that fills every row to its
/// last column, as Microsoft Edit does, wraps each row into the next, so its
/// screen reads as one line.
fn empty_screen_size(pattern: &IUIAutomationTextPattern) -> Result<(usize, usize), String> {
    let screen = pattern
        .visible_ranges()
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or("the terminal has no visible range")?;
    let text = screen.text(-1).map_err(|error| error.to_string())?;
    Ok(screen_size(&String::from_utf16_lossy(&text)))
}

/// The screen's width and height from its first text: the narrowest line's
/// length, leaving out the last line, which Windows Terminal cuts one cell
/// short when it is the buffer's last row; and the rows its lines fill.
fn screen_size(screen: &str) -> (usize, usize) {
    let lines: Vec<&str> = screen
        .strip_suffix("\r\n")
        .unwrap_or(screen)
        .split("\r\n")
        .collect();
    let measured = if lines.len() > 1 {
        &lines[..lines.len() - 1]
    } else {
        &lines[..]
    };
    let width = measured
        .iter()
        .map(|line| line.chars().count())
        .filter(|&cells| cells > 0)
        .min()
        .unwrap_or(0);
    let height = verbatim_term_sim::screen_rows(screen) as usize;
    (width, height)
}

/// The caret's row as the provider expands the caret to a line, and the
/// caret's column in it.
fn caret(pattern: &IUIAutomationTextPattern) -> windows::core::Result<(String, usize)> {
    let Some(caret) = pattern.selection()?.into_iter().next() else {
        return Ok((String::new(), 0));
    };
    let line = caret.clone_range()?;
    line.expand(TextUnit_Line)?;
    let text = String::from_utf16_lossy(&line.text(-1)?);
    let before = line.clone_range()?;
    before.move_endpoint_to(Endpoint::End, &caret, Endpoint::Start)?;
    let column = if before.compare_endpoints(Endpoint::Start, &caret, Endpoint::Start)? <= 0 {
        String::from_utf16_lossy(&before.text(-1)?).chars().count()
    } else {
        0
    };
    Ok((text, column))
}

/// A colour attribute's value, `None` when it is mixed over the range.
fn colour(
    range: &IUIAutomationTextRange,
    attribute: windows::Win32::UI::Accessibility::UIA_TEXTATTRIBUTE_ID,
) -> windows::core::Result<Option<u32>> {
    // SAFETY: the range is a live provider range.
    let value = unsafe { range.GetAttributeValue(attribute)? };
    Ok(i32::try_from(&value).ok().map(i32::cast_unsigned))
}

/// The cells `from` to `to` of the row starting at `start`, as a range.
fn cells(
    start: &IUIAutomationTextRange,
    from: usize,
    to: usize,
) -> windows::core::Result<IUIAutomationTextRange> {
    let range = start.clone_range()?;
    let count = |cells: usize| i32::try_from(cells).unwrap_or(i32::MAX);
    if to > 0 {
        range.move_endpoint_by_unit(Endpoint::End, TextUnit_Character, count(to))?;
    }
    if from > 0 {
        range.move_endpoint_by_unit(Endpoint::Start, TextUnit_Character, count(from))?;
    }
    Ok(range)
}

/// The stretches of one foreground and background in the `width` cells of
/// the row starting at `start`, read cell by cell and joined where
/// neighbours agree: the reference the read program's colours are checked
/// against, which nothing about the providers' answers over stretches can
/// disturb (a stretch answers its first cell's colour when every cell but
/// its last has it, and the format unit stops only at rows, measured
/// 2026-10-24). A cell whose colour is not a number reads as `u32::MAX`.
fn stretches(
    start: &IUIAutomationTextRange,
    offset: usize,
    width: usize,
    runs: &mut Vec<Run>,
) -> windows::core::Result<()> {
    for column in 0..width {
        let cell = cells(start, offset + column, offset + column + 1)?;
        let foreground = colour(&cell, UIA_ForegroundColorAttributeId)?.unwrap_or(u32::MAX);
        let background = colour(&cell, UIA_BackgroundColorAttributeId)?.unwrap_or(u32::MAX);
        match runs.last_mut() {
            Some(run) if run.foreground == foreground && run.background == background => {
                run.length += 1;
            }
            _ => runs.push(Run {
                column,
                length: 1,
                foreground,
                background,
            }),
        }
    }
    Ok(())
}

/// Each screen row's stretches of one foreground and background, from the
/// screen's top to the document's last row. A line wrapped over several
/// rows, as a program that fills each row to its last column writes them,
/// is read as those rows, each `size.0` cells from the line's start.
fn colours(
    pattern: &IUIAutomationTextPattern,
    size: (usize, usize),
) -> windows::core::Result<Vec<Vec<Run>>> {
    let Some(screen) = pattern.visible_ranges()?.into_iter().next() else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    let row = screen.clone_range()?;
    row.move_endpoint_to(Endpoint::End, &screen, Endpoint::Start)?;
    while rows.len() < size.1 {
        row.expand(TextUnit_Line)?;
        let start = row.clone_range()?;
        start.move_endpoint_to(Endpoint::End, &row, Endpoint::Start)?;
        let text = String::from_utf16_lossy(&row.text(-1)?);
        let cells = text.trim_end_matches(['\r', '\n']).chars().count();
        let wrapped = cells.div_ceil(size.0.max(1)).max(1);
        for part in 0..wrapped {
            if rows.len() == size.1 {
                break;
            }
            let mut runs = Vec::new();
            stretches(&start, part * size.0, size.0, &mut runs)?;
            rows.push(runs);
        }
        if row.move_by(TextUnit_Line, 1)? == 0 {
            break;
        }
    }
    Ok(rows)
}

/// `conformance-player <port> <title>`: run by the terminal; writes each
/// chunk the runner sends to its console, then sets the window's title to
/// `<title>#<chunk's index>` for the runner to wait for.
pub fn player(args: &[String]) -> ExitCode {
    let (Some(port), Some(title)) = (args.first(), args.get(1)) else {
        eprintln!("usage: conformance-player <port> <title>");
        return ExitCode::FAILURE;
    };
    // The console itself, not the standard handles, which the terminal may
    // have been handed from whatever started it.
    let Ok(mut output) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    else {
        return ExitCode::FAILURE;
    };
    // SAFETY: the title outlives the call.
    let titled = unsafe { SetConsoleTitleW(&HSTRING::from(title.as_str())) };
    // SAFETY: setting this process's console's code page.
    let utf8 = unsafe { SetConsoleOutputCP(CP_UTF8) };
    // The mode programs run with: processed output, wrapping at the end of
    // a line, VT processing, and a line feed that does not return the
    // carriage.
    // SAFETY: the handle is this process's console output, open for the
    // call.
    let moded = unsafe {
        SetConsoleMode(
            HANDLE(output.as_raw_handle()),
            ENABLE_PROCESSED_OUTPUT
                | ENABLE_WRAP_AT_EOL_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                | DISABLE_NEWLINE_AUTO_RETURN,
        )
    };
    if titled.is_err() || utf8.is_err() || moded.is_err() {
        return ExitCode::FAILURE;
    }
    let Ok(connection) = TcpStream::connect(format!("127.0.0.1:{port}")) else {
        return ExitCode::FAILURE;
    };
    let mut lines = BufReader::new(connection.try_clone().expect("the connection clones"));
    let mut writer = &connection;
    loop {
        let mut command = String::new();
        if lines.read_line(&mut command).unwrap_or(0) == 0 {
            return ExitCode::FAILURE;
        }
        let command = command.trim();
        if command == "quit" {
            return ExitCode::SUCCESS;
        }
        let Some((index, mark, hex)) = command
            .strip_prefix("chunk ")
            .and_then(|rest| rest.split_once(' '))
            .and_then(|(index, rest)| {
                let (mark, hex) = rest.split_once(' ')?;
                Some((index, mark, hex))
            })
        else {
            return ExitCode::FAILURE;
        };
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .filter_map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
            .collect();
        let token = format!("{title}#{index}");
        let written = match mark {
            "sequence" => output
                .write_all(&bytes)
                .and_then(|()| output.write_all(format!("\x1b]2;{token}\x07").as_bytes()))
                .and_then(|()| output.flush())
                .is_ok(),
            "call" => {
                output.write_all(&bytes).and_then(|()| output.flush()).is_ok()
                    // SAFETY: the title outlives the call.
                    && unsafe { SetConsoleTitleW(&HSTRING::from(token.as_str())) }.is_ok()
            }
            _ => false,
        };
        if !written || writer.write_all(b"written\n").is_err() {
            return ExitCode::FAILURE;
        }
    }
}
