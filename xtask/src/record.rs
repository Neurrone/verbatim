//! `cargo xtask record-terminal-streams [--recipe <name>]...`: records what
//! real programs write to a terminal, for the simulated terminal's
//! conformance corpus (`docs/design/terminal-model.md`, "Testing"). Each
//! recipe in `crates/verbatim-term-sim/conformance/recipes` names a command
//! and the input to type into it; the program runs in a pseudoconsole of
//! 120 by 30, the input is written to it step by step, and everything it
//! writes is saved, with when it came, as a stream of the corpus, split
//! into chunks where the output paused or input was typed. The streams are
//! then recorded in the real terminals by
//! `regenerate-terminal-conformance-expectations`.
//!
//! What a pseudoconsole writes is the console host's rendering of the
//! program's output, which is what a terminal receives from it. The
//! pseudoconsole's own requests to its terminal (win32 input mode, focus
//! events) are left out of the saved stream, since they are not the
//! program's and would change how the conformance player's console answers.
//!
//! A recipe whose program logs its own writes (`write_log`) is saved from
//! that log instead, one chunk a write: the pseudoconsole's rendering
//! merges and redraws what the program wrote, so a program's own ways of
//! writing, such as rewriting a line or a character split between two
//! writes, survive only in its log. Its command names the log as
//! `{write_log}`, and `{bin}` names the folder this program was built
//! into, where the workspace's other programs are built too.

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_term_sim::conformance::{Chunk, Stream, folder};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// A recipe: what to run, and what to type into it.
#[derive(serde::Deserialize)]
struct Recipe {
    /// What the recording exercises.
    about: String,
    /// The command line, run in the repository's root.
    command: String,
    /// The input, in order.
    steps: Vec<Step>,
    /// Whether the stream is the program's log of its own writes rather
    /// than what the pseudoconsole wrote.
    #[serde(default)]
    write_log: bool,
}

/// One step of a recipe's input.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    /// Wait until the program has written nothing for this many
    /// milliseconds, or at most five seconds.
    Quiet(u64),
    /// Wait this many milliseconds, for a program whose output is timed.
    Wait(u64),
    /// Type this text, VT input sequences included, one character at a
    /// time, `each` milliseconds apart (0 for at once).
    Type {
        /// The text.
        text: String,
        /// Milliseconds between characters.
        #[serde(default)]
        each: u64,
    },
}

/// What is left out of the saved stream: the pseudoconsole's own requests
/// to its terminal (win32 input mode, focus events), and programs'
/// questions to the terminal (its version, its keyboard protocol, the
/// cursor's position, its attributes), whose answers would arrive in the
/// conformance player's input ahead of the answer it waits for. None of
/// them changes the screen.
const NOT_THE_PROGRAMS: &[&str] = &[
    "\x1b[?9001h",
    "\x1b[?9001l",
    "\x1b[?1004h",
    "\x1b[?1004l",
    "\x1b[>0q",
    "\x1b[?u",
    "\x1b[6n",
    "\x1b[5n",
    "\x1b[c",
    "\x1b[0c",
    "\x1b[>c",
];

/// `text` without the sequences that set the window's title (OSC 0, 1 and
/// 2), which are not on the screen and would rename the conformance
/// player's window.
fn without_titles(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\x1b]") {
        let after = &rest[at + 2..];
        let is_title =
            after.starts_with("0;") || after.starts_with("1;") || after.starts_with("2;");
        let end = after
            .find('\x07')
            .map(|end| end + 1)
            .or_else(|| after.find("\x1b\\").map(|end| end + 2));
        match end {
            Some(end) if is_title => {
                kept.push_str(&rest[..at]);
                rest = &after[end..];
            }
            _ => {
                kept.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    kept
}

/// The longest a quiet step waits.
const QUIET_AT_MOST: Duration = Duration::from_secs(5);

/// Runs the command line with the recorder's arguments.
pub fn run(args: &[String]) -> ExitCode {
    let mut names = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let ("--recipe", Some(name)) = (arg.as_str(), args.next()) {
            names.push(name.clone());
        } else {
            eprintln!("usage: cargo xtask record-terminal-streams [--recipe <name>]...");
            return ExitCode::FAILURE;
        }
    }
    let recipes = folder().join("recipes");
    let Ok(entries) = std::fs::read_dir(&recipes) else {
        eprintln!("no recipes in {}", recipes.display());
        return ExitCode::FAILURE;
    };
    let mut failed = false;
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    for path in paths {
        let Some(name) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if !names.is_empty() && !names.contains(&name) {
            continue;
        }
        match record(&path) {
            Ok(stream) => {
                let out = folder().join("streams").join(format!("{name}.json"));
                let text = serde_json::to_string_pretty(&stream).expect("a stream serializes");
                if let Err(error) = std::fs::write(&out, text + "\n") {
                    eprintln!("{name}: could not write {}: {error}", out.display());
                    failed = true;
                } else {
                    println!("{name}: {} chunks", stream.chunks.len());
                }
            }
            Err(error) => {
                eprintln!("{name}: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What the reader thread passes on: a read's bytes and when they came.
type Output = (Instant, Vec<u8>);

/// Runs one recipe and returns its stream.
fn record(path: &Path) -> Result<Stream, String> {
    let recipe: Recipe =
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let log = std::env::temp_dir().join(format!("verbatim-write-log-{}.txt", std::process::id()));
    let bin = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .ok_or("this program's folder is unknown")?;
    let command = recipe
        .command
        .replace("{write_log}", &log.to_string_lossy())
        .replace("{bin}", &bin.to_string_lossy());
    if recipe.write_log {
        let _ = std::fs::remove_file(&log);
    }
    let console = Pseudoconsole::start(&command)?;
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel::<Output>();
    let mut output = console
        .output
        .try_clone()
        .map_err(|error| error.to_string())?;
    let reader = std::thread::spawn(move || {
        let mut buffer = vec![0u8; 65536];
        while let Ok(count) = output.read(&mut buffer) {
            if count == 0
                || sender
                    .send((Instant::now(), buffer[..count].to_vec()))
                    .is_err()
            {
                break;
            }
        }
    });
    let mut reads: Vec<Output> = Vec::new();
    // Chunk boundaries forced by input: the index in `reads` before which
    // a boundary falls.
    let mut typed_at: Vec<usize> = Vec::new();
    let mut input = &console.input;
    for step in &recipe.steps {
        match step {
            Step::Quiet(quiet) => {
                let deadline = Instant::now() + QUIET_AT_MOST;
                while let Ok(read) = receiver.recv_timeout(Duration::from_millis(*quiet)) {
                    reads.push(read);
                    if Instant::now() > deadline {
                        break;
                    }
                }
            }
            Step::Wait(milliseconds) => {
                let until = Instant::now() + Duration::from_millis(*milliseconds);
                while let Some(left) = until.checked_duration_since(Instant::now()) {
                    match receiver.recv_timeout(left) {
                        Ok(read) => reads.push(read),
                        Err(_) => break,
                    }
                }
            }
            Step::Type { text, each } => {
                for character in text.chars() {
                    typed_at.push(reads.len());
                    let mut bytes = [0u8; 4];
                    input
                        .write_all(character.encode_utf8(&mut bytes).as_bytes())
                        .map_err(|error| error.to_string())?;
                    input.flush().map_err(|error| error.to_string())?;
                    if *each > 0 {
                        let until = Instant::now() + Duration::from_millis(*each);
                        while let Some(left) = until.checked_duration_since(Instant::now()) {
                            match receiver.recv_timeout(left) {
                                Ok(read) => reads.push(read),
                                Err(_) => break,
                            }
                        }
                    }
                }
            }
        }
    }
    drop(console);
    while let Ok(read) = receiver.recv_timeout(Duration::from_secs(2)) {
        reads.push(read);
    }
    let _ = reader.join();
    if recipe.write_log {
        let text = std::fs::read_to_string(&log)
            .map_err(|error| format!("no write log at {}: {error}", log.display()))?;
        let _ = std::fs::remove_file(&log);
        return logged(recipe.about, &text);
    }
    Ok(chunks(recipe.about, &reads, &typed_at, started))
}

/// A program's log of its own writes as a stream: each line is one write,
/// the milliseconds from the program's start and the bytes in hexadecimal,
/// and each write is a chunk.
fn logged(about: String, log: &str) -> Result<Stream, String> {
    let mut chunks = Vec::new();
    let mut at_ms = Vec::new();
    for line in log.lines() {
        let (at, hex) = line
            .split_once(' ')
            .ok_or_else(|| format!("a write log line without its bytes: {line:?}"))?;
        at_ms.push(
            at.parse::<u64>()
                .map_err(|error| format!("a write's time {at:?}: {error}"))?,
        );
        let bytes = (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|error| format!("a write's bytes: {error}"))?;
        chunks.push(bytes.into());
    }
    Ok(Stream {
        about,
        chunks,
        at_ms,
    })
}

/// The reads joined into chunks: a chunk ends where the output paused for
/// 30 milliseconds or more, or input was typed.
fn chunks(about: String, reads: &[Output], typed_at: &[usize], started: Instant) -> Stream {
    const PAUSE: Duration = Duration::from_millis(30);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut at_ms: Vec<u64> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    let mut flush = |bytes: &mut Vec<u8>, first: &mut Option<Instant>| {
        if bytes.is_empty() {
            return;
        }
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        for request in NOT_THE_PROGRAMS {
            text = text.replace(request, "");
        }
        text = without_titles(&text);
        if !text.is_empty() {
            chunks.push(text.into());
            at_ms.push(
                first
                    .map_or(0, |first| first.duration_since(started).as_millis())
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
        bytes.clear();
        *first = None;
    };
    for (index, (when, read)) in reads.iter().enumerate() {
        let paused = last.is_some_and(|last| when.duration_since(last) >= PAUSE);
        if paused || typed_at.contains(&index) {
            flush(&mut bytes, &mut first);
        }
        first.get_or_insert(*when);
        bytes.extend_from_slice(read);
        last = Some(*when);
    }
    flush(&mut bytes, &mut first);
    Stream {
        about,
        chunks,
        at_ms,
    }
}

/// A program running in a pseudoconsole, with the pipes to its input and
/// from its output; dropping it closes the pseudoconsole, which ends the
/// program's console, and then ends the program if it is still running.
struct Pseudoconsole {
    console: HPCON,
    process: OwnedHandle,
    input: File,
    output: File,
}

impl Pseudoconsole {
    fn start(command: &str) -> Result<Self, String> {
        let error = |error: windows::core::Error| error.to_string();
        let (mut input_read, mut input_write) = (HANDLE::default(), HANDLE::default());
        let (mut output_read, mut output_write) = (HANDLE::default(), HANDLE::default());
        // SAFETY: the out-pointers are valid for the calls.
        unsafe { CreatePipe(&raw mut input_read, &raw mut input_write, None, 0) }.map_err(error)?;
        // SAFETY: as above.
        unsafe { CreatePipe(&raw mut output_read, &raw mut output_write, None, 0) }
            .map_err(error)?;
        // SAFETY: the pipe handles are open; the pseudoconsole duplicates
        // them, so this process's copies are closed below.
        let console =
            unsafe { CreatePseudoConsole(COORD { X: 120, Y: 30 }, input_read, output_write, 0) }
                .map_err(error)?;
        // SAFETY: these handles are this process's and no longer used.
        unsafe {
            let _ = CloseHandle(input_read);
        }
        // SAFETY: as above.
        unsafe {
            let _ = CloseHandle(output_write);
        }
        let mut size = 0;
        // SAFETY: asking the size of a one-attribute list fails by design
        // and writes the size.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &raw mut size) };
        let mut list = vec![0u8; size];
        let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(list.as_mut_ptr().cast::<c_void>());
        // SAFETY: the buffer is the size asked for and outlives its use.
        unsafe { InitializeProcThreadAttributeList(Some(attributes), 1, None, &raw mut size) }
            .map_err(error)?;
        // SAFETY: the attribute's value is the pseudoconsole handle, which
        // outlives the process's creation.
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(console.0 as *const c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
        }
        .map_err(error)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).unwrap_or(0);
        startup.lpAttributeList = attributes;
        // No standard handles of this process's: without the flag, a child
        // of a process whose standard handles are redirected writes to them
        // instead of to the pseudoconsole.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        let mut command_line: Vec<u16> = command.encode_utf16().chain([0]).collect();
        let mut information = PROCESS_INFORMATION::default();
        // SAFETY: the command line is a writable, null-terminated buffer and
        // every structure outlives the call.
        let created = unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR::null(),
                &raw const startup.StartupInfo,
                &raw mut information,
            )
        };
        // SAFETY: the list was initialized above and is not used again.
        unsafe { DeleteProcThreadAttributeList(attributes) };
        created.map_err(error)?;
        // SAFETY: the thread handle is this process's and not needed.
        unsafe {
            let _ = CloseHandle(information.hThread);
        }
        // SAFETY: each handle is open and owned by this process from here.
        let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess.0) };
        // SAFETY: as above.
        let input = unsafe { File::from_raw_handle(input_write.0) };
        // SAFETY: as above.
        let output = unsafe { File::from_raw_handle(output_read.0) };
        Ok(Self {
            console,
            process,
            input,
            output,
        })
    }
}

impl Drop for Pseudoconsole {
    fn drop(&mut self) {
        // SAFETY: the pseudoconsole is open; closing it ends its console.
        unsafe { ClosePseudoConsole(self.console) };
        let process = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(
            &self.process,
        ));
        // SAFETY: the process handle is open for the calls.
        if unsafe { WaitForSingleObject(process, 3000) } != WAIT_OBJECT_0 {
            // SAFETY: as above.
            let _ = unsafe { TerminateProcess(process, 1) };
        }
    }
}
