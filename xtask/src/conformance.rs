//! `cargo xtask regenerate-terminal-conformance-expectations`: writes each
//! stream of `verbatim-term-sim`'s conformance corpus into the real console
//! host and the end-to-end suite's pinned Windows Terminal, reads each
//! after every chunk with Verbatim's own read program, and saves what it
//! answered as the expectations the simulated terminal is tested against
//! (`docs/design/terminal-model.md`, section 9). It opens terminal windows,
//! so it runs on demand on a desktop, never in continuous integration.
//!
//! The terminal runs this program's `conformance-player`, which connects
//! back over a local socket, writes each chunk it is sent, and then sets
//! the window's title to a token naming the chunk: the terminal changes its
//! title only once it has parsed everything written before, so the new
//! title is the evidence that the chunk is in the buffer the terminal's
//! provider reads, and the runner reads it only then.

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::AsRawHandle as _;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_term_sim::conformance::{Answer, Expectation, expectation_path, streams};
use verbatim_term_sim::{Flavor, Run};
use verbatim_uia::Uia;
use verbatim_uia::text::{Endpoint, TextPatternExt as _, TextRangeExt as _};
use verbatim_uia_rops::{Path, SnapshotQuery, terminal_snapshot};
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
            match run(&uia, &terminal, &name, &stream.chunks, flavor) {
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
    chunks: &[String],
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
    let result = read_stream(uia, &connection, window, &title, chunks, flavor);
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
    let (pattern, _) = verbatim_uia::text::text_pattern(&element).map_err(error)?;
    Ok((element, pattern))
}

/// Sends each chunk and reads the terminal once the player says it is on
/// the screen.
fn read_stream(
    uia: &Uia,
    connection: &TcpStream,
    window: HWND,
    title: &str,
    chunks: &[String],
    flavor: Flavor,
) -> Result<Expectation, String> {
    let (element, pattern) = text_element(uia, window, flavor)?;
    let mut lines = BufReader::new(connection.try_clone().map_err(|error| error.to_string())?);
    let mut writer = connection;
    let mut start: Option<IUIAutomationTextRange> = None;
    let mut screen_rows = 0;
    let mut answers = Vec::new();
    let mut size = (0, 0);
    for (index, chunk) in chunks.iter().enumerate() {
        let hex = chunk.bytes().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        writer
            .write_all(format!("chunk {index} {hex}\n").as_bytes())
            .map_err(|error| error.to_string())?;
        let mut reply = String::new();
        lines
            .read_line(&mut reply)
            .map_err(|error| format!("no reply from the player: {error}"))?;
        if reply.trim() != "written" {
            return Err(format!("the player answered {reply:?}"));
        }
        wait_for_title(window, &format!("{title}#{index}"))?;
        let query = SnapshotQuery {
            element: &element,
            pattern: &pattern,
            start: start.as_ref(),
            screen_rows,
            whole: false,
            caret: None,
        };
        let (snapshot, path) =
            terminal_snapshot(uia, &query, true).map_err(|error| error.to_string())?;
        if let Path::Fallback(error) = path {
            return Err(format!("the read program failed and fell back: {error}"));
        }
        let screen = String::from_utf16_lossy(&snapshot.screen);
        if size == (0, 0) {
            size = screen_size(&screen);
        }
        screen_rows = verbatim_term_sim::screen_rows(&screen);
        start.clone_from(&snapshot.next_start);
        let (caret_line, caret_column) = caret(&pattern).map_err(|error| error.to_string())?;
        answers.push(Answer {
            text: String::from_utf16_lossy(&snapshot.text),
            screen,
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: snapshot.first_row,
            caret_rows_to_end: snapshot.caret_rows_to_end,
            caret_line,
            caret_column,
            colours: colours(&pattern, size).map_err(|error| error.to_string())?,
        });
    }
    Ok(Expectation {
        flavor,
        width: size.0,
        height: size.1,
        answers,
    })
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
/// neighbours agree. Neither a range over several colours nor the
/// providers' format unit can be trusted here: over a stretch of two
/// colours the providers answer 0 rather than "mixed", and the format unit
/// does not stop at every change of colour (measured 2026-10-23). A cell
/// whose colour is not a number reads as `u32::MAX`.
fn stretches(
    start: &IUIAutomationTextRange,
    width: usize,
    runs: &mut Vec<Run>,
) -> windows::core::Result<()> {
    for column in 0..width {
        let cell = cells(start, column, column + 1)?;
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
/// screen's top to the document's last row.
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
    for _ in 0..size.1 {
        row.expand(TextUnit_Line)?;
        let start = row.clone_range()?;
        start.move_endpoint_to(Endpoint::End, &row, Endpoint::Start)?;
        let mut runs = Vec::new();
        stretches(&start, size.0, &mut runs)?;
        rows.push(runs);
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
        let Some((index, hex)) = command
            .strip_prefix("chunk ")
            .and_then(|rest| rest.split_once(' '))
        else {
            return ExitCode::FAILURE;
        };
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .filter_map(|at| u8::from_str_radix(&hex[at..at + 2], 16).ok())
            .collect();
        let marker = format!("\x1b]2;{title}#{index}\x07");
        if output
            .write_all(&bytes)
            .and_then(|()| output.write_all(marker.as_bytes()))
            .and_then(|()| output.flush())
            .is_err()
            || writer.write_all(b"written\n").is_err()
        {
            return ExitCode::FAILURE;
        }
    }
}
