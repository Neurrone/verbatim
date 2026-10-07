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

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::{Frame, Request};
use verbatim_e2e::agent_client::AgentClient;
use verbatim_model::UtteranceEnding;

/// The add-on's source directory, relative to the workspace root.
const SOURCE_DIR: &str = "nvda-addon/src";
/// The committed package, relative to the workspace root.
const PACKAGE: &str = "nvda-addon/verbatimTranscript.nvda-addon";
/// The add-on listens on this port plus its Windows session id.
const PORT_BASE: u16 = 44100;
/// The add-on protocol version this client speaks.
const PROTOCOL_VERSION: u64 = 1;
/// How often `capture` polls for new entries. NVDA's own system tests poll
/// no faster than every 10 ms so as not to starve NVDA's core.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// The image `--front` takes to mean whichever program's window has the
/// title given.
const ANY_IMAGE: &str = "*";

/// Runs `cargo xtask nvda <verb>`.
pub fn run(args: &[String]) -> ExitCode {
    let result = match args.first().map(String::as_str) {
        Some("build") => build(),
        Some("capture") => capture(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask nvda <verb>");
            eprintln!("verbs:");
            eprintln!("  build    write {PACKAGE} from {SOURCE_DIR}");
            eprintln!(
                "  capture  [--agent <address>] [--quiet-ms <ms>] [--timeout-ms <ms>] [--verbatim] \
                 [--json] <step>...: run each step through the agent and print what NVDA (or, \
                 with --verbatim, the local Verbatim) speaks after it, as JSON lines with \
                 --json; a step is a key, --launch <program> [--arg <arg>]..., --front \
                 <image>[=<title>] (* for any image), --type <text>, or --gesture <id> (sent \
                 to the local Verbatim)"
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

/// One thing `capture` does, in the order given, before waiting for NVDA's
/// speech to settle and printing it.
enum Step {
    /// Press a key combination through the agent, or several joined by
    /// commas in one batch, so `numpad8,numpad8` is a double press.
    Key(String),
    /// Start a program through the agent, in its session.
    Launch { program: String, args: Vec<String> },
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
}

impl Step {
    fn label(&self) -> String {
        match self {
            Self::Key(key) => key.clone(),
            Self::Gesture(identifier) => format!("gesture {identifier}"),
            Self::Type(text) => format!("type {text:?}"),
            Self::Launch { program, args } => format!("launch {program} {}", args.join(" ")),
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
            }),
            "--arg" => {
                let arg = value()?.clone();
                match options.steps.last_mut() {
                    Some(Step::Launch { args, .. }) => args.push(arg),
                    _ => return Err(io::Error::other("--arg must follow --launch")),
                }
            }
            "--gesture" => options.steps.push(Step::Gesture(value()?.clone())),
            "--type" => options.steps.push(Step::Type(value()?.clone())),
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

/// A connection to the add-on, past its `Hello`.
struct Transcript {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl Transcript {
    fn connect(port: u16) -> io::Result<(Self, Value)> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| {
            io::Error::other(format!(
                "no NVDA transcript add-on on port {port} ({error}); is NVDA running in the \
                 agent's session with the add-on installed?"
            ))
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut transcript = Self {
            stream,
            reader,
            next_id: 1,
        };
        let hello =
            transcript.request(&json!({"Hello": {"protocol_version": PROTOCOL_VERSION}}))?;
        Ok((transcript, hello["Hello"].clone()))
    }

    fn request(&mut self, request: &Value) -> io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({"id": id, "request": request}).to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply)? == 0 {
            return Err(io::Error::other("the add-on closed the connection"));
        }
        let frame: Value = serde_json::from_str(&reply).map_err(io::Error::other)?;
        if let Some(message) = frame["Error"]["message"].as_str() {
            return Err(io::Error::other(format!("the add-on refused: {message}")));
        }
        Ok(frame["Reply"]["payload"].clone())
    }

    fn entries_after(&mut self, seq: u64) -> io::Result<Vec<Value>> {
        let payload = self.request(&json!({"Read": {"after": seq}}))?;
        Ok(payload["Entries"]["entries"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }
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
        let port = u16::try_from(u32::from(PORT_BASE) + session_id)
            .map_err(|_| io::Error::other(format!("session id {session_id} is out of range")))?;
        let (transcript, hello) = Transcript::connect(port)?;
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
                        *last_seq = entry["seq"].as_u64().unwrap_or(*last_seq);
                        (entry["ms"].as_i64().unwrap_or(0), describe(&entry["event"]))
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
    for (index, step) in options.steps.iter().enumerate() {
        let sent_ms = source.now_ms();
        let mut note = None;
        match step {
            Step::Key(keys) => {
                let keys: Vec<String> = keys.split(',').map(str::to_owned).collect();
                agent.send_keys(&keys)?;
            }
            Step::Launch { program, args } => {
                agent.launch_process(program, args, None, &[], None)?;
            }
            Step::Front { image, title } => {
                let image = if image == ANY_IMAGE {
                    titled_window_image(&mut agent, title.as_deref(), options.timeout)?
                } else {
                    Some(image.clone())
                };
                let taken = match image {
                    Some(image) => {
                        agent.bring_to_foreground(&image, title.as_deref(), options.timeout)?
                    }
                    None => false,
                };
                if !taken {
                    note = Some("no matching window took the foreground".to_owned());
                }
            }
            Step::Gesture(identifier) => send_gesture(identifier)?,
            Step::Type(text) => agent.type_text(text)?,
        }
        if options.json {
            println!("{}", json!({"step": index, "label": step.label()}));
        } else {
            println!("> {}", step.label());
        }
        if let Some(note) = note {
            print(index, None, &Entry::Note(note));
        }
        let mut last_activity = Instant::now();
        let deadline = last_activity + options.timeout;
        loop {
            thread::sleep(POLL_INTERVAL);
            let entries = source.poll()?;
            if !entries.is_empty() {
                last_activity = Instant::now();
            }
            for (ms, entry) in entries {
                print(index, Some(ms - sent_ms), &entry);
            }
            let now = Instant::now();
            if now.duration_since(last_activity) >= options.quiet {
                break;
            }
            if now >= deadline {
                let note = format!("speech did not settle within {:?}", options.timeout);
                print(index, None, &Entry::Note(note));
                break;
            }
        }
    }
    Ok(())
}

/// The image of the first visible window whose title contains `title`,
/// waiting up to `timeout` for one to appear, for `--front *=<title>`: a
/// console window belongs to the console host, or to the shell it runs,
/// depending on how it was started, so the title is what names it.
fn titled_window_image(
    agent: &mut AgentClient,
    title: Option<&str>,
    timeout: Duration,
) -> io::Result<Option<String>> {
    let Some(title) = title else {
        return Err(io::Error::other(format!(
            "--front {ANY_IMAGE} needs a title: {ANY_IMAGE}=<title>"
        )));
    };
    let deadline = Instant::now() + timeout;
    loop {
        let info = agent.foreground_info()?;
        if let Some(window) = info
            .windows
            .iter()
            .find(|window| window.title.contains(title))
        {
            return Ok(Some(window.image.clone()));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL_INTERVAL);
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
fn describe(event: &Value) -> Entry {
    if let Some(speech) = event.get("Speech") {
        Entry::Speech {
            tag: speech["priority"].as_str().unwrap_or("?").to_owned(),
            text: speech["text"].as_str().unwrap_or("").to_owned(),
        }
    } else {
        Entry::Event(event.as_str().unwrap_or("?").to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_the_same_as_a_line_and_as_json() {
        let speech = describe(&json!({"Speech": {"priority": "normal", "text": "a\nb"}}));
        assert_eq!(speech.line(), "[normal] a\nb");
        assert_eq!(
            speech.to_json(3, Some(-5)),
            json!({"kind": "speech", "tag": "normal", "text": "a\nb", "step": 3, "ms": -5})
        );
        assert_eq!(
            describe(&json!("CANCEL")),
            Entry::Event("cancel".to_owned())
        );
        assert_eq!(
            Entry::Note("late".to_owned()).to_json(0, None),
            json!({"kind": "note", "text": "late", "step": 0})
        );
    }

    #[test]
    fn json_is_an_option_anywhere_among_the_steps() {
        let args = ["tab", "--json", "--type", "hi"].map(str::to_owned);
        let options = parse_capture_args(&args).unwrap();
        assert!(options.json);
        assert_eq!(options.steps.len(), 2);
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
