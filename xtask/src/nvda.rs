//! `cargo xtask nvda`: the NVDA transcript add-on and its client
//! (`docs/nvda-transcript.md`).
//!
//! `build` packs `nvda-addon/src` into the committed
//! `nvda-addon/verbatimTranscript.nvda-addon`, and a unit test, so part of
//! `cargo xtask ci`, checks that the committed file is what `build` would
//! write, so the installable
//! add-on never drifts from its source. The package is a zip archive with
//! every entry stored uncompressed, sorted by path, dated 1980-01-01, and
//! with line endings normalized to LF, so the same source gives the same
//! bytes on every machine and checkout.
//!
//! `capture` drives the NVDA running in the agent's session: it runs each
//! step through the agent (a key pressed through real OS input, a program
//! launched, or a window brought to the foreground), waits for NVDA's
//! speech to settle, and prints what NVDA queued for speech after it. With
//! `--verbatim`, it prints what the Verbatim running on this machine queued
//! instead, read from its control pipe, so the same steps give two
//! transcripts that can be compared line by line. With `--json`, every
//! step and entry is printed as one JSON object per line instead, for a
//! script comparing transcripts, since spoken text can hold a line break.
//!
//! `compare` runs one end-to-end scenario in the harness's compare mode
//! (`verbatim_e2e::compare`): its own setup and body against the NVDA
//! running in the agent's session instead of Verbatim, asserting nothing,
//! and prints the report setting what the scenario expects Verbatim to say
//! beside what NVDA said, step by step.
//!
//! The add-on's client itself is `verbatim_e2e::nvda`, shared with the
//! end-to-end harness.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use verbatim_agent::protocol::{WindowCondition, WindowInfo};
use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::{Frame, Request};
use verbatim_e2e::agent_client::AgentClient;
use verbatim_e2e::nvda::{
    POLL_INTERVAL, QuietWait, Settling, Transcript, TranscriptEvent, session_port,
};
use verbatim_e2e::registry::Comparable;
use verbatim_e2e::{artifacts, compare as compare_mode};
use verbatim_model::UtteranceEnding;

/// The add-on's source directory, relative to the workspace root.
const SOURCE_DIR: &str = "nvda-addon/src";
/// The committed package, relative to the workspace root.
const PACKAGE: &str = "nvda-addon/verbatimTranscript.nvda-addon";
/// The image `--front` takes to mean whichever program's window has the
/// title given.
const ANY_IMAGE: &str = "*";

/// Runs `cargo xtask nvda <verb>`.
pub fn run(args: &[String]) -> ExitCode {
    let result = match args.first().map(String::as_str) {
        Some("build") => build(),
        Some("capture") => capture(&args[1..]),
        Some("compare") => compare(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask nvda <verb>");
            eprintln!("verbs:");
            eprintln!("  build    write {PACKAGE} from {SOURCE_DIR}");
            eprintln!(
                "  capture  [--agent <address>] [--quiet-ms <ms>] [--timeout-ms <ms>] [--verbatim] \
                 [--json] <step>...: run each step through the agent and print what NVDA (or, \
                 with --verbatim, the local Verbatim) speaks after it, as JSON lines with \
                 --json; a step is a key, --launch <program> [--arg <arg>]... (or \
                 --launch-minimized, as the end-to-end harness launches a terminal, or \
                 --launch-without-foreground, as it opens a File Explorer folder), \
                 --wait-file <path>, --front \
                 <image>[=<title>] (* for any image), --type <text>, --hold <key> or \
                 --release <key> (a key held down across the steps between them, released \
                 when the capture ends or fails), or --gesture <id> (sent to the local \
                 Verbatim)"
            );
            eprintln!(
                "  compare  [--agent <address>] [--quiet-ms <ms>] <scenario>: run an end-to-end \
                 scenario against the NVDA running in the agent's session, asserting nothing, and \
                 print what the scenario expects Verbatim to say beside what NVDA said, step by \
                 step"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask nvda: {error}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits one level below the workspace root")
        .to_path_buf()
}

fn build() -> io::Result<()> {
    let root = workspace_root();
    let package = package_bytes(&root.join(SOURCE_DIR))?;
    fs::write(root.join(PACKAGE), package)?;
    println!("xtask nvda: wrote {PACKAGE}");
    Ok(())
}

/// Packs every file under `source` into the add-on's zip bytes.
fn package_bytes(source: &Path) -> io::Result<Vec<u8>> {
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort();
    let mut entries = Vec::with_capacity(files.len());
    for name in files {
        let text = fs::read_to_string(source.join(&name))?;
        entries.push((name, text.replace("\r\n", "\n").into_bytes()));
    }
    Ok(stored_zip(&entries))
}

/// Appends the path of every file under `dir`, relative to `base` and
/// joined with forward slashes as zip requires.
fn collect_files(base: &Path, dir: &Path, files: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            // Python's bytecode cache, if NVDA or a check ever ran in place.
            if path.file_name().is_some_and(|name| name == "__pycache__") {
                continue;
            }
            collect_files(base, &path, files)?;
        } else {
            let relative = path.strip_prefix(base).map_err(io::Error::other)?;
            let parts: Vec<_> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            files.push(parts.join("/"));
        }
    }
    Ok(())
}

/// A zip archive of `entries`, each stored without compression, with the
/// earliest date zip can express (1980-01-01, 00:00) so the bytes depend
/// only on the names and contents.
fn stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    const DOS_DATE_1980_01_01: u16 = (1 << 5) | 1;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = u32::try_from(out.len()).expect("the add-on is far below 4 GiB");
        let crc = crc32(data);
        let size = u32::try_from(data.len()).expect("each file is far below 4 GiB");
        let name_len = u16::try_from(name.len()).expect("paths are short");
        // Local file header: signature, version needed 2.0, flags (bit 11:
        // UTF-8 names), method 0 (stored), time, date, crc, sizes, lengths.
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        for field in [20_u16, 1 << 11, 0, 0, DOS_DATE_1980_01_01] {
            out.extend_from_slice(&field.to_le_bytes());
        }
        for field in [crc, size, size] {
            out.extend_from_slice(&field.to_le_bytes());
        }
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory header: as above, plus version made by,
        // comment length, disk number, attributes, and the local offset.
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        for field in [20_u16, 20, 1 << 11, 0, 0, DOS_DATE_1980_01_01] {
            central.extend_from_slice(&field.to_le_bytes());
        }
        for field in [crc, size, size] {
            central.extend_from_slice(&field.to_le_bytes());
        }
        for field in [name_len, 0, 0, 0, 0] {
            central.extend_from_slice(&field.to_le_bytes());
        }
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = u32::try_from(out.len()).expect("the add-on is far below 4 GiB");
    let central_size = u32::try_from(central.len()).expect("the add-on is far below 4 GiB");
    let count = u16::try_from(entries.len()).expect("the add-on has few files");
    out.extend_from_slice(&central);
    // End of central directory record.
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    for field in [0_u16, 0, count, count] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

/// CRC-32 as zip uses it (IEEE 802.3, reflected, polynomial 0xEDB88320).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// How a [`Step::Launch`] starts its program, as the end-to-end harness
/// starts one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchKind {
    /// As a program starts, the agent's right to the foreground passed on
    /// (`AgentClient::launch_process`).
    Normal,
    /// As the harness launches a terminal (`AgentClient::launch_minimized`):
    /// its first window minimized and inactive, the agent's right to the
    /// foreground not withheld.
    Minimized,
    /// As the harness opens a File Explorer folder
    /// (`AgentClient::launch_without_foreground_right`): its window opens
    /// as the program opens it, the agent's right to the foreground
    /// withheld, for a `--front` to bring it forward.
    WithoutForeground,
}

/// One thing `capture` does, in the order given, before waiting for NVDA's
/// speech to settle and printing it.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Press a key combination through the agent, or several joined by
    /// commas in one batch, so `numpad8,numpad8` is a double press.
    Key(String),
    /// Start a program through the agent, in its session, as `how` says.
    Launch {
        program: String,
        args: Vec<String>,
        how: LaunchKind,
    },
    /// Wait, through the agent, for a file to exist, as the harness waits
    /// for a shell to write its process id before bringing its window
    /// forward.
    WaitFile(String),
    /// Bring a window of an image, optionally with a title containing some
    /// text, to the foreground through the agent.
    Front {
        image: String,
        title: Option<String>,
    },
    /// Send a gesture, such as `kb:verbatim+v`, to the Verbatim running on
    /// this machine through its control pipe, so Verbatim's own commands
    /// can be used without pressing a modifier key NVDA also uses.
    Gesture(String),
    /// Type text through the agent, each character mapped with the
    /// foreground window's keyboard layout.
    Type(String),
    /// Press one key, such as `control`, down through the agent and keep it
    /// down, so the keys of the steps after it are pressed with it.
    Hold(String),
    /// Release a key a `Hold` step pressed.
    Release(String),
}

impl Step {
    fn label(&self) -> String {
        match self {
            Self::Key(key) => key.clone(),
            Self::Gesture(identifier) => format!("gesture {identifier}"),
            Self::Type(text) => format!("type {text:?}"),
            Self::Hold(key) => format!("hold {key}"),
            Self::Release(key) => format!("release {key}"),
            Self::Launch { program, args, how } => format!(
                "launch{} {program} {}",
                match how {
                    LaunchKind::Normal => "",
                    LaunchKind::Minimized => " minimized",
                    LaunchKind::WithoutForeground => " without the foreground right",
                },
                args.join(" ")
            ),
            Self::WaitFile(path) => format!("wait for {path}"),
            Self::Front { image, title } => match title {
                Some(title) => format!("front {image} \"{title}\""),
                None => format!("front {image}"),
            },
        }
    }
}

struct CaptureOptions {
    agent: String,
    /// Record the local Verbatim's speech rather than NVDA's.
    verbatim: bool,
    /// Print one JSON object per step and per entry, rather than lines for
    /// reading.
    json: bool,
    quiet: Duration,
    timeout: Duration,
    steps: Vec<Step>,
}

fn parse_capture_args(args: &[String]) -> io::Result<CaptureOptions> {
    let mut options = CaptureOptions {
        agent: format!("127.0.0.1:{}", verbatim_agent::protocol::DEFAULT_PORT),
        verbatim: false,
        json: false,
        quiet: Duration::from_secs(1),
        timeout: Duration::from_secs(10),
        steps: Vec::new(),
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| io::Error::other(format!("{arg} needs a value")))
        };
        let millis = |text: &String| {
            text.parse::<u64>()
                .map(Duration::from_millis)
                .map_err(|_| io::Error::other(format!("not a number of milliseconds: {text}")))
        };
        match arg.as_str() {
            "--agent" => options.agent.clone_from(value()?),
            "--verbatim" => options.verbatim = true,
            "--json" => options.json = true,
            "--quiet-ms" => options.quiet = millis(value()?)?,
            "--timeout-ms" => options.timeout = millis(value()?)?,
            "--launch" => options.steps.push(Step::Launch {
                program: value()?.clone(),
                args: Vec::new(),
                how: LaunchKind::Normal,
            }),
            "--launch-minimized" => options.steps.push(Step::Launch {
                program: value()?.clone(),
                args: Vec::new(),
                how: LaunchKind::Minimized,
            }),
            "--launch-without-foreground" => options.steps.push(Step::Launch {
                program: value()?.clone(),
                args: Vec::new(),
                how: LaunchKind::WithoutForeground,
            }),
            "--wait-file" => options.steps.push(Step::WaitFile(value()?.clone())),
            "--arg" => {
                let arg = value()?.clone();
                match options.steps.last_mut() {
                    Some(Step::Launch { args, .. }) => args.push(arg),
                    _ => {
                        return Err(io::Error::other(
                            "--arg must follow --launch, --launch-minimized, or --launch-without-foreground",
                        ));
                    }
                }
            }
            "--gesture" => options.steps.push(Step::Gesture(value()?.clone())),
            "--type" => options.steps.push(Step::Type(value()?.clone())),
            "--hold" => options.steps.push(Step::Hold(value()?.clone())),
            "--release" => options.steps.push(Step::Release(value()?.clone())),
            "--front" => {
                let target = value()?;
                let (image, title) = match target.split_once('=') {
                    Some((image, title)) => (image.to_owned(), Some(title.to_owned())),
                    None => (target.clone(), None),
                };
                options.steps.push(Step::Front { image, title });
            }
            key => options.steps.push(Step::Key(key.to_owned())),
        }
    }
    if options.steps.is_empty() {
        return Err(io::Error::other(
            "capture needs at least one key, launch, or front",
        ));
    }
    Ok(options)
}

/// Where `capture` reads speech from.
enum Source {
    /// NVDA's transcript add-on: the connection, the last sequence number
    /// read, and the moment the add-on's clock started.
    Nvda {
        transcript: Transcript,
        last_seq: u64,
        started: Instant,
    },
    /// The local Verbatim's speech subscription, read on a thread of its
    /// own, and each queued utterance's text, to name one cut off.
    Verbatim {
        frames: Receiver<io::Result<Frame>>,
        texts: HashMap<u64, String>,
    },
}

impl Source {
    fn nvda(agent: &mut AgentClient) -> io::Result<Self> {
        let session_id = agent.session_info()?.session_id;
        let (transcript, hello) = Transcript::connect(session_port(session_id)?)?;
        let started = Instant::now();
        println!(
            "NVDA {} in session {}",
            hello["nvda_version"]
                .as_str()
                .unwrap_or("(unknown version)"),
            hello["session_id"]
        );
        Ok(Self::Nvda {
            transcript,
            last_seq: 0,
            started,
        })
    }

    fn verbatim() -> io::Result<Self> {
        let mut control = ControlClient::connect_pipe()?;
        verbatim_control::client::ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        let (sender, frames) = mpsc::channel();
        thread::spawn(move || {
            loop {
                let frame = control.next_frame();
                let failed = frame.is_err();
                if sender.send(frame).is_err() || failed {
                    return;
                }
            }
        });
        println!("Verbatim on its control pipe");
        Ok(Self::Verbatim {
            frames,
            texts: HashMap::new(),
        })
    }

    /// Milliseconds on the clock this source stamps its entries with.
    fn now_ms(&self) -> i64 {
        match self {
            Self::Nvda { started, .. } => {
                i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
            }
            Self::Verbatim { .. } => epoch_ms(),
        }
    }

    /// The entries recorded since the last call, each with its time on
    /// this source's clock.
    fn poll(&mut self) -> io::Result<Vec<(i64, Entry)>> {
        match self {
            Self::Nvda {
                transcript,
                last_seq,
                ..
            } => {
                let entries = transcript.entries_after(*last_seq)?;
                Ok(entries
                    .into_iter()
                    .map(|entry| {
                        *last_seq = entry.seq.unwrap_or(*last_seq);
                        (entry.ms, describe(&entry.event))
                    })
                    .collect())
            }
            Self::Verbatim { frames, texts } => {
                let mut lines = Vec::new();
                while let Ok(frame) = frames.try_recv() {
                    match frame? {
                        Frame::Speech {
                            utterance,
                            text,
                            queued_at_ms,
                            ..
                        } => {
                            texts.insert(utterance.0, text.clone());
                            lines.push((
                                millis(queued_at_ms),
                                Entry::Speech {
                                    tag: utterance.to_string(),
                                    text,
                                },
                            ));
                        }
                        Frame::SpeechEnded { utterance, ending } => {
                            let text = texts.remove(&utterance.0).unwrap_or_default();
                            let how = match ending {
                                UtteranceEnding::Completed => continue,
                                UtteranceEnding::Cancelled => "cancelled".to_owned(),
                                UtteranceEnding::Failed(why) => format!("failed ({why})"),
                            };
                            lines.push((
                                epoch_ms(),
                                Entry::Ended {
                                    how,
                                    tag: utterance.to_string(),
                                    text,
                                },
                            ));
                        }
                        Frame::Sound { indication, at_ms } => {
                            lines.push((millis(at_ms), Entry::Sound(indication)));
                        }
                        _ => {}
                    }
                }
                Ok(lines)
            }
        }
    }
}

/// Milliseconds since the Unix epoch, the clock Verbatim's frames carry.
fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| millis_of(since.as_millis()))
}

fn millis(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn millis_of(ms: u128) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

/// What `compare` runs: the scenario, the agent, and the quiet period
/// when one is given.
struct CompareOptions {
    agent: String,
    quiet_ms: Option<u64>,
    scenario: String,
}

fn parse_compare_args(args: &[String]) -> io::Result<CompareOptions> {
    let mut agent = format!("127.0.0.1:{}", verbatim_agent::protocol::DEFAULT_PORT);
    let mut quiet_ms = None;
    let mut scenario = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| io::Error::other(format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "--agent" => agent.clone_from(value()?),
            "--quiet-ms" => {
                let text = value()?;
                quiet_ms = Some(text.parse::<u64>().map_err(|_| {
                    io::Error::other(format!("not a number of milliseconds: {text}"))
                })?);
            }
            name if scenario.is_none() && !name.starts_with("--") => {
                scenario = Some(name.to_owned());
            }
            other => return Err(io::Error::other(format!("unexpected argument {other}"))),
        }
    }
    let scenario = scenario.ok_or_else(|| io::Error::other("compare needs the scenario to run"))?;
    Ok(CompareOptions {
        agent,
        quiet_ms,
        scenario,
    })
}

/// Runs `compare`: checks the scenario can be compared and the agent is
/// reachable, deletes the report an earlier run left, runs the scenario's
/// test in compare mode, and prints the report when the run wrote one;
/// otherwise only the error.
fn compare(args: &[String]) -> io::Result<()> {
    let options = parse_compare_args(args)?;
    let def = verbatim_e2e::registry::find(&options.scenario)
        .ok_or_else(|| io::Error::other(format!("no scenario named {:?}", options.scenario)))?;
    if let Comparable::No(why) = def.comparable {
        return Err(io::Error::other(format!(
            "{} cannot be compared with NVDA: {why}",
            def.name
        )));
    }
    // The agent presses the keys and finds NVDA's session, as for a
    // capture.
    AgentClient::connect(options.agent.as_str())?;
    let report = artifacts::scenario_dir(&artifacts::artifacts_root(), def.name)
        .join(compare_mode::REPORT_FILE_NAME);
    remove_earlier_report(&report)?;
    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "test",
            "-p",
            "verbatim-e2e",
            "--test",
            "e2e",
            "--",
            "--ignored",
            "--exact",
            def.name,
            "--test-threads=1",
        ])
        .env(verbatim_e2e::ENDPOINT_ENV, &options.agent)
        .env(compare_mode::ENV, def.name)
        .current_dir(workspace_root());
    if let Some(quiet_ms) = options.quiet_ms {
        command.env(compare_mode::QUIET_ENV, quiet_ms.to_string());
    }
    println!(
        "xtask nvda: comparing {} with NVDA; the desktop belongs to it until it ends",
        def.name
    );
    let status = command.status()?;
    if let Ok(text) = fs::read_to_string(&report) {
        println!("\n{}:\n{text}", report.display());
    }
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the comparison of {} stopped on a harness error ({status})",
            def.name
        )))
    }
}

/// Deletes `report`, a comparison's report an earlier run left, if there
/// is one, so it is never printed as this run's.
fn remove_earlier_report(report: &Path) -> io::Result<()> {
    match fs::remove_file(report) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io::Error::other(format!(
            "could not delete the earlier report {}: {error}",
            report.display()
        ))),
    }
}

fn capture(args: &[String]) -> io::Result<()> {
    let options = parse_capture_args(args)?;
    let mut agent = AgentClient::connect(options.agent.as_str())?;
    let mut source = if options.verbatim {
        Source::verbatim()?
    } else {
        Source::nvda(&mut agent)?
    };
    let print = |step: usize, ms: Option<i64>, entry: &Entry| {
        if options.json {
            println!("{}", entry.to_json(step, ms));
        } else {
            match ms {
                Some(ms) => println!("  {ms:+} ms  {}", entry.line()),
                None => println!("  ({})", entry.line()),
            }
        }
    };
    // The window the keys go to, by its handle: the one in front when the
    // capture starts, then the one each --front brings forward. A launch
    // leaves none until a --front names its window.
    let mut target = agent
        .foreground_info()?
        .foreground
        .map(|window| window.window);
    // Keys a --hold step holds down, released when the capture ends,
    // whether it ended on an error or not, so no modifier stays down; the
    // agent releases them too if the connection ends first.
    let mut held: Vec<String> = Vec::new();
    let steps = (|| -> io::Result<()> {
        for (index, step) in options.steps.iter().enumerate() {
            let sent_ms = source.now_ms();
            run_step(&mut agent, &mut target, &mut held, step)?;
            if options.json {
                println!("{}", json!({"step": index, "label": step.label()}));
            } else {
                println!("> {}", step.label());
            }
            let mut wait = QuietWait::starting_at(Instant::now(), options.quiet, options.timeout);
            loop {
                thread::sleep(POLL_INTERVAL);
                let entries = source.poll()?;
                if !entries.is_empty() {
                    wait.activity(Instant::now());
                }
                for (ms, entry) in entries {
                    print(index, Some(ms - sent_ms), &entry);
                }
                match wait.settling(Instant::now()) {
                    Settling::Waiting => {}
                    Settling::Settled => break,
                    Settling::TimedOut => {
                        let note = format!("speech did not settle within {:?}", options.timeout);
                        print(index, None, &Entry::Note(note));
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    for key in held.iter().rev() {
        if let Err(error) = agent.release_key(key) {
            eprintln!("xtask nvda: could not release {key}: {error}");
        }
    }
    steps?;
    // Keys the window has not read yet go to whichever window is in front
    // when they are read, or when it closes: a capture that ends with its
    // window behind another, or not responding, says so.
    let foreground = agent.foreground_info()?.foreground;
    in_front(target, foreground.as_ref()).map_err(|why| {
        io::Error::other(format!(
            "at the end of the capture {why}; keys it was sent may not have been read, and \
             closing it would hand them to the window behind it"
        ))
    })
}

/// Does what `step` says through the agent: presses its keys, types its
/// text, launches, brings a window forward, sends a gesture, or holds or
/// releases a key, recording a held key in `held`; `target` is the window
/// the keys go to, by its handle, which a launch or a --front changes.
fn run_step(
    agent: &mut AgentClient,
    target: &mut Option<u64>,
    held: &mut Vec<String>,
    step: &Step,
) -> io::Result<()> {
    match step {
        Step::Key(keys) => {
            // Each key of a batch is checked and pressed on its own.
            for key in keys.split(',') {
                require_in_front(agent, *target, step)?;
                agent.send_keys(&[key.to_owned()])?;
            }
        }
        Step::Launch { program, args, how } => {
            match how {
                LaunchKind::Normal => {
                    agent.launch_process(program, args, None, &[], None)?;
                }
                LaunchKind::Minimized => {
                    agent.launch_minimized(program, args)?;
                }
                LaunchKind::WithoutForeground => {
                    agent.launch_without_foreground_right(program, args)?;
                }
            }
            *target = None;
        }
        Step::WaitFile(path) => {
            if !agent.wait_for_file(path, verbatim_e2e::scenario::WINDOW_TIMEOUT)? {
                return Err(io::Error::other(format!(
                    "{path} was not written within {:?}; the remaining steps were not run",
                    verbatim_e2e::scenario::WINDOW_TIMEOUT
                )));
            }
        }
        Step::Front { image, title } => {
            // A window named by its title is waited for until it is
            // present, as the harness waits before it brings one forward
            // (`Scenario::bring_forward`).
            if let Some(title) = title {
                agent.wait_for_window(
                    WindowCondition::Present {
                        title_contains: title.clone(),
                    },
                    verbatim_e2e::scenario::WINDOW_TIMEOUT,
                )?;
            }
            // The window, found by its image and title, is brought
            // forward with SetForegroundWindow, which injects no input;
            // Windows allows it when the agent injected the last input.
            let window = agent.foreground_info()?.windows.into_iter().find(|window| {
                (image == ANY_IMAGE || window.image.eq_ignore_ascii_case(image))
                    && title
                        .as_deref()
                        .is_none_or(|title| window.title.contains(title))
            });
            let taken = match &window {
                Some(window) => agent.set_foreground(window.window)?,
                None => false,
            };
            *target = window.filter(|_| taken).map(|window| window.window);
            // The steps after it press keys in whatever is in front:
            // without the window, none of them may run.
            if !taken {
                return Err(io::Error::other(format!(
                    "no window of {image} titled {title:?} took the foreground; the remaining steps were not run"
                )));
            }
        }
        Step::Gesture(identifier) => send_gesture(identifier)?,
        Step::Type(text) => {
            // One character at a time, each checked just before it.
            for character in text.chars() {
                require_in_front(agent, *target, step)?;
                agent.type_text(&character.to_string())?;
            }
        }
        Step::Hold(key) => {
            require_in_front(agent, *target, step)?;
            agent.press_key(key)?;
            held.push(key.clone());
        }
        Step::Release(key) => {
            held.retain(|held| held != key);
            agent.release_key(key)?;
        }
    }
    Ok(())
}

/// Fails, just before one key or character of `step` is sent, unless the
/// capture's window, `target` by its handle, is in front and responding
/// (`docs/tooling/nvda.md`, "Recording what NVDA says").
fn require_in_front(agent: &mut AgentClient, target: Option<u64>, step: &Step) -> io::Result<()> {
    let foreground = agent.foreground_info()?.foreground;
    in_front(target, foreground.as_ref()).map_err(|why| {
        io::Error::other(format!(
            "{why}; the rest of {} and the steps after it were not run",
            step.label()
        ))
    })
}

/// Whether the window `target`, by its handle, is `foreground`, the window
/// in front, and responding; why not, otherwise. A key sent to any other
/// window acts on whatever it is. A key sent to a window that is not
/// responding waits unread, and goes to whichever window is in front when
/// it is read, or when that window closes.
fn in_front(target: Option<u64>, foreground: Option<&WindowInfo>) -> Result<(), String> {
    let Some(target) = target else {
        return Err("no window was brought forward after the launch".to_owned());
    };
    match foreground {
        Some(window) if window.window == target && !window.hung => Ok(()),
        Some(window) if window.window == target => Err(format!(
            "the capture's window, {:?} of {}, is not responding",
            window.title, window.image
        )),
        Some(window) => Err(format!(
            "the window in front is {:?} of {} (window {}), not the capture's (window {target})",
            window.title, window.image, window.window
        )),
        None => Err(format!(
            "no window is in front, not the capture's (window {target})"
        )),
    }
}

/// Sends one gesture to the local Verbatim's control pipe.
fn send_gesture(identifier: &str) -> io::Result<()> {
    let mut control = ControlClient::connect_pipe()?;
    match control.request(Request::SendGesture {
        identifier: identifier.to_owned(),
    })? {
        Frame::Error { message, .. } => Err(io::Error::other(format!(
            "Verbatim refused the gesture {identifier}: {message}"
        ))),
        _ => Ok(()),
    }
}

/// One thing a transcript records after a step.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    /// Text queued for speech, tagged with NVDA's priority or Verbatim's
    /// utterance id.
    Speech { tag: String, text: String },
    /// Any other event NVDA records, such as a cancellation, by its name.
    Event(String),
    /// A Verbatim utterance that ended without being heard in full: how it
    /// ended, its id, and its text.
    Ended {
        how: String,
        tag: String,
        text: String,
    },
    /// A sound Verbatim played, by its indication.
    Sound(String),
    /// Something the capture itself has to say about the step.
    Note(String),
}

impl Entry {
    /// The entry as a line for reading.
    fn line(&self) -> String {
        match self {
            Self::Speech { tag, text } => format!("[{tag}] {text}"),
            Self::Event(name) | Self::Note(name) => name.clone(),
            Self::Ended { how, tag, text } => format!("{how} [{tag}] {text}"),
            Self::Sound(indication) => format!("sound: {indication}"),
        }
    }

    /// The entry as one JSON object: the step it follows, its time since
    /// that step was sent (absent for a note), its kind, and its fields.
    fn to_json(&self, step: usize, ms: Option<i64>) -> Value {
        let mut value = match self {
            Self::Speech { tag, text } => json!({"kind": "speech", "tag": tag, "text": text}),
            Self::Event(name) => json!({"kind": "event", "text": name}),
            Self::Ended { how, tag, text } => {
                json!({"kind": "ended", "how": how, "tag": tag, "text": text})
            }
            Self::Sound(indication) => json!({"kind": "sound", "text": indication}),
            Self::Note(note) => json!({"kind": "note", "text": note}),
        };
        value["step"] = json!(step);
        if let Some(ms) = ms {
            value["ms"] = json!(ms);
        }
        value
    }
}

/// The entry for one of the add-on's events: the speech text with its
/// priority, or another event, such as a cancellation, by its name.
fn describe(event: &TranscriptEvent) -> Entry {
    match event {
        TranscriptEvent::Speech { priority, text } => Entry::Speech {
            tag: priority.clone(),
            text: text.clone(),
        },
        TranscriptEvent::Other(name) => Entry::Event(name.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_earlier_report_is_deleted_and_none_is_no_error() {
        let folder = std::env::temp_dir().join(format!("xtask-compare-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("creates the folder");
        let report = folder.join(compare_mode::REPORT_FILE_NAME);
        fs::write(&report, "an earlier run's report").expect("writes the report");
        remove_earlier_report(&report).expect("deletes it");
        assert!(!report.exists());
        remove_earlier_report(&report).expect("none to delete is no error");
        fs::remove_dir(&folder).expect("removes the folder");
    }

    #[test]
    fn an_entry_reads_the_same_as_a_line_and_as_json() {
        let speech = describe(&TranscriptEvent::from_json(
            &json!({"Speech": {"priority": "normal", "text": "a\nb"}}),
        ));
        assert_eq!(speech.line(), "[normal] a\nb");
        assert_eq!(
            speech.to_json(3, Some(-5)),
            json!({"kind": "speech", "tag": "normal", "text": "a\nb", "step": 3, "ms": -5})
        );
        assert_eq!(
            describe(&TranscriptEvent::from_json(&json!("CANCEL"))),
            Entry::Event("cancel".to_owned())
        );
        assert_eq!(
            Entry::Note("late".to_owned()).to_json(0, None),
            json!({"kind": "note", "text": "late", "step": 0})
        );
    }

    fn window(handle: u64, pid: u32, hung: bool) -> WindowInfo {
        WindowInfo {
            window: handle,
            pid,
            title: "capture".to_owned(),
            class: "ConsoleWindowClass".to_owned(),
            image: "conhost.exe".to_owned(),
            cloaked: false,
            minimized: false,
            hung,
            no_activate: false,
        }
    }

    #[test]
    fn keys_go_only_to_the_capture_s_window_in_front_and_responding() {
        assert_eq!(in_front(Some(7), Some(&window(7, 10, false))), Ok(()));
        // Another window of the same process is not the capture's.
        assert!(in_front(Some(7), Some(&window(8, 10, false))).is_err());
        assert!(in_front(Some(7), Some(&window(7, 10, true))).is_err());
        assert!(in_front(Some(7), None).is_err());
        assert!(in_front(None, Some(&window(7, 10, false))).is_err());
    }

    #[test]
    fn json_is_an_option_anywhere_among_the_steps() {
        let args = ["tab", "--json", "--type", "hi"].map(str::to_owned);
        let options = parse_capture_args(&args).unwrap();
        assert!(options.json);
        assert_eq!(
            options.steps,
            [Step::Key("tab".to_owned()), Step::Type("hi".to_owned())]
        );
    }

    #[test]
    fn a_key_is_held_across_the_steps_between_hold_and_release() {
        let args = ["--hold", "control", "shift+tab", "--release", "control"].map(str::to_owned);
        let options = parse_capture_args(&args).unwrap();
        assert_eq!(
            options.steps,
            [
                Step::Hold("control".to_owned()),
                Step::Key("shift+tab".to_owned()),
                Step::Release("control".to_owned()),
            ]
        );
    }

    #[test]
    fn a_minimized_launch_takes_its_args_and_a_file_is_waited_for() {
        let args = [
            "--launch-minimized",
            "wt.exe",
            "--arg",
            "-w",
            "--wait-file",
            r"C:\run\pid",
        ]
        .map(str::to_owned);
        let options = parse_capture_args(&args).unwrap();
        assert_eq!(
            options.steps,
            [
                Step::Launch {
                    program: "wt.exe".to_owned(),
                    args: vec!["-w".to_owned()],
                    how: LaunchKind::Minimized,
                },
                Step::WaitFile(r"C:\run\pid".to_owned()),
            ]
        );
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn committed_package_matches_its_source() {
        let root = workspace_root();
        let expected = package_bytes(&root.join(SOURCE_DIR)).unwrap();
        let committed = fs::read(root.join(PACKAGE)).unwrap_or_default();
        assert!(
            committed == expected,
            "{PACKAGE} does not match {SOURCE_DIR}; run `cargo xtask nvda build` and commit the result"
        );
    }
}
