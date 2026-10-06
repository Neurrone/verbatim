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
//! `capture` drives the NVDA running in the agent's session: it sends each
//! key through the agent's real OS input, waits for NVDA's speech to
//! settle, and prints what NVDA queued for speech after that key.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use verbatim_e2e::agent_client::AgentClient;

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
                "  capture  [--agent <address>] [--quiet-ms <ms>] [--timeout-ms <ms>] <key>...: \
                 press each key through the agent and print what NVDA speaks"
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

struct CaptureOptions {
    agent: String,
    quiet: Duration,
    timeout: Duration,
    keys: Vec<String>,
}

fn parse_capture_args(args: &[String]) -> io::Result<CaptureOptions> {
    let mut options = CaptureOptions {
        agent: format!("127.0.0.1:{}", verbatim_agent::protocol::DEFAULT_PORT),
        quiet: Duration::from_secs(1),
        timeout: Duration::from_secs(10),
        keys: Vec::new(),
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
            "--quiet-ms" => options.quiet = millis(value()?)?,
            "--timeout-ms" => options.timeout = millis(value()?)?,
            key => options.keys.push(key.to_owned()),
        }
    }
    if options.keys.is_empty() {
        return Err(io::Error::other("capture needs at least one key"));
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

fn capture(args: &[String]) -> io::Result<()> {
    let options = parse_capture_args(args)?;
    let mut agent = AgentClient::connect(options.agent.as_str())?;
    let session_id = agent.session_info()?.session_id;
    let port = u16::try_from(u32::from(PORT_BASE) + session_id)
        .map_err(|_| io::Error::other(format!("session id {session_id} is out of range")))?;
    let (mut transcript, hello) = Transcript::connect(port)?;
    let started = Instant::now();
    println!(
        "NVDA {} in session {}",
        hello["nvda_version"]
            .as_str()
            .unwrap_or("(unknown version)"),
        hello["session_id"]
    );
    let mut last_seq = 0;
    for key in &options.keys {
        let sent_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        agent.send_keys(std::slice::from_ref(key))?;
        println!("> {key}");
        let mut last_activity = Instant::now();
        let deadline = last_activity + options.timeout;
        loop {
            thread::sleep(POLL_INTERVAL);
            let entries = transcript.entries_after(last_seq)?;
            if !entries.is_empty() {
                last_activity = Instant::now();
            }
            for entry in entries {
                last_seq = entry["seq"].as_u64().unwrap_or(last_seq);
                let offset = entry["ms"].as_i64().unwrap_or(0) - sent_ms;
                println!("  {offset:+} ms  {}", describe(&entry["event"]));
            }
            let now = Instant::now();
            if now.duration_since(last_activity) >= options.quiet {
                break;
            }
            if now >= deadline {
                println!("  (speech did not settle within {:?})", options.timeout);
                break;
            }
        }
    }
    Ok(())
}

/// One line for an entry's event: the speech text with its priority, or a
/// cancellation.
fn describe(event: &Value) -> String {
    if let Some(speech) = event.get("Speech") {
        format!(
            "[{}] {}",
            speech["priority"].as_str().unwrap_or("?"),
            speech["text"].as_str().unwrap_or("")
        )
    } else {
        event.as_str().unwrap_or("?").to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
