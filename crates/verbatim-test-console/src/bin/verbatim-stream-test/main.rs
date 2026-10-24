//! `verbatim-stream-test`: a program of Verbatim's own that streams text
//! into the terminal it runs in, each way programs stream it, at up to 60
//! frames a second, beside a redrawn status row with a spinner and a timer
//! and a prompt that takes typing (`docs/design/terminal-model.md`, section
//! 12, item 13). See `docs/crates/verbatim-test-console.md` for its phases
//! and how they are driven.
//!
//! `verbatim-stream-test [--run <phase>]... [--pipe <name>] [--fps <n>]
//! [--write-log <path>]`. A phase is named `<way>:<placement>`, with
//! `:alternate` after `repaint:<placement>` for the alternate screen. The
//! `--run` phases run in order at the program's own rate; with `--pipe`,
//! the program then takes commands, one a line, from the pipe
//! `\\.\pipe\<name>-commands` and answers on `\\.\pipe\<name>-replies`:
//! `run <phase>` runs a phase at the program's rate and answers `done
//! <phase>` once its last frame is written; `paced <phase>` starts one
//! whose frames come one for each `frame` command, each answered `frame
//! <number>`, and `done <phase>` after the last; `quit` ends the program.
//! `--write-log` writes a line for each write the program makes: the
//! milliseconds since it started and the bytes in hexadecimal, for the
//! conformance corpus's recorder. Typing at the prompt shows in the next
//! frame while a phase runs, and at once between phases; Control+C ends
//! the program.

mod phase;

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::windows::io::AsRawHandle as _;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use phase::{Phase, Spec};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Globalization::CP_UTF8;
use windows::Win32::System::Console::{
    CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT,
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WRAP_AT_EOL_OUTPUT,
    GetConsoleMode, GetConsoleScreenBufferInfo, SetConsoleCP, SetConsoleMode, SetConsoleOutputCP,
};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// What the program waits for.
enum Event {
    /// Bytes typed at the console.
    Keys(Vec<u8>),
    /// A command from the pipe.
    Command(String),
    /// A phase's next frame is due; the number names the phase.
    Tick(u64),
    /// The console's input or the pipe closed.
    Closed,
}

/// The command line.
struct Args {
    runs: Vec<Spec>,
    pipe: Option<String>,
    fps: u32,
    log: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        runs: Vec::new(),
        pipe: None,
        fps: 60,
        log: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--run" => parsed
                .runs
                .push(Spec::parse(value).ok_or_else(|| format!("no phase {value:?}"))?),
            "--pipe" => parsed.pipe = Some(value.clone()),
            "--fps" => {
                parsed.fps = value
                    .parse()
                    .ok()
                    .filter(|fps| (1..=1000).contains(fps))
                    .ok_or_else(|| format!("no rate {value:?}"))?;
            }
            "--write-log" => parsed.log = Some(value.clone()),
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    Ok(parsed)
}

/// The console's output, each write logged when asked.
struct Output {
    console: File,
    log: Option<File>,
    started: Instant,
}

impl Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(log) = &mut self.log {
            let mut line = self.started.elapsed().as_millis().to_string();
            line.push(' ');
            for byte in bytes {
                let _ = write!(line, "{byte:02x}");
            }
            line.push('\n');
            log.write_all(line.as_bytes())?;
        }
        self.console.write_all(bytes)?;
        self.console.flush()
    }
}

/// A console handle's mode, set for the program and put back as it was
/// when dropped.
struct Mode {
    handle: HANDLE,
    was: CONSOLE_MODE,
}

impl Mode {
    fn set(file: &File, mode: CONSOLE_MODE) -> windows::core::Result<Self> {
        let handle = HANDLE(file.as_raw_handle());
        let mut was = CONSOLE_MODE::default();
        // SAFETY: the handle is the console's, open for the call.
        unsafe { GetConsoleMode(handle, &raw mut was) }?;
        // SAFETY: as above.
        unsafe { SetConsoleMode(handle, mode) }?;
        Ok(Self { handle, was })
    }
}

impl Drop for Mode {
    fn drop(&mut self) {
        // SAFETY: the handle is still open: its file outlives this guard.
        let _ = unsafe { SetConsoleMode(self.handle, self.was) };
    }
}

/// Sends a tick for each of `frames` frames, `fps` a second from now, on a
/// high-resolution timer, so a frame is late only by the time the program
/// takes to write the one before.
fn tick(events: Sender<Event>, phase: u64, frames: usize, fps: u32) {
    std::thread::spawn(move || {
        // SAFETY: no name and no attributes; the handle is closed below.
        let Ok(timer) = (unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }) else {
            let _ = events.send(Event::Closed);
            return;
        };
        let started = Instant::now();
        let frame = Duration::from_secs(1) / fps;
        for index in 0..frames {
            let due = started + frame * u32::try_from(index).unwrap_or(u32::MAX);
            let wait = due.saturating_duration_since(Instant::now());
            // Negative: relative, in units of 100 nanoseconds.
            let relative = -i64::try_from(wait.as_nanos() / 100).unwrap_or(i64::MAX);
            // SAFETY: the timer is open and the due time outlives the call.
            let set = unsafe { SetWaitableTimer(timer, &raw const relative, 0, None, None, false) };
            // SAFETY: the timer is open.
            if set.is_err() || unsafe { WaitForSingleObject(timer, INFINITE) } != WAIT_OBJECT_0 {
                let _ = events.send(Event::Closed);
                break;
            }
            if events.send(Event::Tick(phase)).is_err() {
                break;
            }
        }
        // SAFETY: the timer is open and not used again.
        let _ = unsafe { CloseHandle(timer) };
    });
}

/// A running phase: the phase, its number, its name, and whether its
/// frames come one a `frame` command rather than at the program's rate.
struct Running {
    phase: Phase,
    number: u64,
    name: String,
    paced: bool,
    written: usize,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("verbatim-stream-test: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("verbatim-stream-test: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), String> {
    let error = |error: std::io::Error| error.to_string();
    let open = |name: &str| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .map_err(|error| format!("{name}: {error}"))
    };
    // The console itself, whatever the standard handles are.
    let console = open("CONOUT$")?;
    let mut input = open("CONIN$")?;
    // SAFETY: setting this process's console's code pages.
    unsafe { SetConsoleOutputCP(CP_UTF8) }.map_err(|error| error.to_string())?;
    // SAFETY: as above.
    unsafe { SetConsoleCP(CP_UTF8) }.map_err(|error| error.to_string())?;
    // The mode the conformance player writes with, so a recorded write
    // means the same there: a line feed does not return the carriage.
    let _output_mode = Mode::set(
        &console,
        ENABLE_PROCESSED_OUTPUT
            | ENABLE_WRAP_AT_EOL_OUTPUT
            | ENABLE_VIRTUAL_TERMINAL_PROCESSING
            | DISABLE_NEWLINE_AUTO_RETURN,
    )
    .map_err(|error| error.to_string())?;
    // Keys as they are typed, not echoed, Control+C among them.
    let _input_mode =
        Mode::set(&input, ENABLE_VIRTUAL_TERMINAL_INPUT).map_err(|error| error.to_string())?;
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the handle is the console's output, open for the call.
    unsafe { GetConsoleScreenBufferInfo(HANDLE(console.as_raw_handle()), &raw mut info) }
        .map_err(|error| error.to_string())?;
    let width = usize::try_from(info.srWindow.Right - info.srWindow.Left + 1).unwrap_or(80);
    let height = usize::try_from(info.srWindow.Bottom - info.srWindow.Top + 1).unwrap_or(25);
    let log = match &args.log {
        Some(path) => Some(File::create(path).map_err(error)?),
        None => None,
    };
    let mut output = Output {
        console,
        log,
        started: Instant::now(),
    };
    let (sender, events) = channel();
    let keys = sender.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 256];
        while let Ok(count) = input.read(&mut buffer) {
            if count == 0 || keys.send(Event::Keys(buffer[..count].to_vec())).is_err() {
                break;
            }
        }
        let _ = keys.send(Event::Closed);
    });
    let mut replies = match &args.pipe {
        Some(name) => {
            let commands = File::open(format!(r"\\.\pipe\{name}-commands")).map_err(error)?;
            let replies = std::fs::OpenOptions::new()
                .write(true)
                .open(format!(r"\\.\pipe\{name}-replies"))
                .map_err(error)?;
            let lines = sender.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(commands).lines() {
                    let Ok(line) = line else { break };
                    if lines.send(Event::Command(line)).is_err() {
                        return;
                    }
                }
                let _ = lines.send(Event::Closed);
            });
            Some(replies)
        }
        None => None,
    };
    let mut session = Session {
        width,
        height,
        fps: args.fps,
        sender,
        typed: String::new(),
        running: None,
        ended: None,
        phases: 0,
    };
    let mut queue: Vec<Spec> = args.runs.iter().rev().copied().collect();
    loop {
        if session.running.is_none()
            && let Some(spec) = queue.pop()
        {
            session.start(spec, &name_of(spec), false);
        }
        let reply = session.handle(&events, &mut output).map_err(error)?;
        match reply {
            Reply::None => {}
            Reply::Quit => return Ok(()),
            Reply::Say(text) => {
                if let Some(replies) = &mut replies {
                    replies
                        .write_all(format!("{text}\n").as_bytes())
                        .and_then(|()| replies.flush())
                        .map_err(error)?;
                }
            }
        }
    }
}

/// A phase's name as commands give it.
fn name_of(spec: Spec) -> String {
    let way = phase::Way::ALL
        .iter()
        .find(|(_, way)| *way == spec.way)
        .map_or("", |(name, _)| name);
    let placement = phase::Placement::ALL
        .iter()
        .find(|(_, placement)| *placement == spec.placement)
        .map_or("", |(name, _)| name);
    if spec.alternate {
        format!("{way}:{placement}:alternate")
    } else {
        format!("{way}:{placement}")
    }
}

/// What handling an event asks of the loop.
enum Reply {
    None,
    Quit,
    Say(String),
}

/// The program's state between events.
struct Session {
    width: usize,
    height: usize,
    fps: u32,
    sender: Sender<Event>,
    typed: String,
    running: Option<Running>,
    /// The phase that ran last, which draws the prompt between phases.
    ended: Option<Phase>,
    phases: u64,
}

impl Session {
    fn start(&mut self, spec: Spec, name: &str, paced: bool) {
        self.phases += 1;
        let phase = Phase::new(spec, self.width, self.height, self.fps);
        if !paced {
            tick(self.sender.clone(), self.phases, phase.frames(), self.fps);
        }
        self.running = Some(Running {
            phase,
            number: self.phases,
            name: name.to_owned(),
            paced,
            written: 0,
        });
    }

    /// Writes the running phase's next frame; the phase's name once it has
    /// written its last.
    fn frame(&mut self, output: &mut Output) -> std::io::Result<Option<String>> {
        let Some(running) = &mut self.running else {
            return Ok(None);
        };
        for write in running.phase.next(&self.typed) {
            output.write(&write)?;
        }
        running.written += 1;
        if running.written < running.phase.frames() {
            return Ok(None);
        }
        let running = self.running.take().expect("a running phase");
        self.ended = Some(running.phase);
        Ok(Some(running.name))
    }

    fn handle(&mut self, events: &Receiver<Event>, output: &mut Output) -> std::io::Result<Reply> {
        let Ok(event) = events.recv() else {
            return Ok(Reply::Quit);
        };
        match event {
            Event::Closed => Ok(Reply::Quit),
            Event::Tick(number) => {
                let ours = self
                    .running
                    .as_ref()
                    .is_some_and(|running| running.number == number && !running.paced);
                if !ours {
                    return Ok(Reply::None);
                }
                Ok(match self.frame(output)? {
                    Some(name) => Reply::Say(format!("done {name}")),
                    None => Reply::None,
                })
            }
            Event::Keys(bytes) => {
                if self.typing(&bytes) {
                    return Ok(Reply::Quit);
                }
                if self.running.is_none()
                    && let Some(ended) = &self.ended
                {
                    output.write(&ended.typed(&self.typed))?;
                }
                Ok(Reply::None)
            }
            Event::Command(command) => self.command(command.trim(), output),
        }
    }

    fn command(&mut self, command: &str, output: &mut Output) -> std::io::Result<Reply> {
        let (verb, rest) = command.split_once(' ').unwrap_or((command, ""));
        match verb {
            "quit" => Ok(Reply::Quit),
            "run" | "paced" if self.running.is_some() => Ok(Reply::Say("busy".to_owned())),
            "run" | "paced" => match Spec::parse(rest) {
                Some(spec) => {
                    self.start(spec, rest, verb == "paced");
                    Ok(Reply::None)
                }
                None => Ok(Reply::Say(format!("no phase {rest}"))),
            },
            "frame" => {
                let paced = self.running.as_ref().is_some_and(|running| running.paced);
                if !paced {
                    return Ok(Reply::Say("no paced phase".to_owned()));
                }
                let number = self.running.as_ref().map_or(0, |running| running.written);
                Ok(match self.frame(output)? {
                    Some(name) => Reply::Say(format!("frame {number}\ndone {name}")),
                    None => Reply::Say(format!("frame {number}")),
                })
            }
            _ => Ok(Reply::Say(format!("unknown command {command}"))),
        }
    }

    /// The typed text changed by `bytes`: printable characters added,
    /// Backspace taking the last away, Enter clearing it, escape sequences
    /// (arrows and the like) left out. True when Control+C was typed.
    fn typing(&mut self, bytes: &[u8]) -> bool {
        let text = String::from_utf8_lossy(bytes);
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\x03' => return true,
                '\x08' | '\x7f' => {
                    self.typed.pop();
                }
                '\r' => self.typed.clear(),
                '\x1b' => {
                    // A sequence: its introducer, then up to its final
                    // character.
                    if let Some('[' | 'O') = chars.next() {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                }
                c if !c.is_control() => self.typed.push(c),
                _ => {}
            }
        }
        false
    }
}
