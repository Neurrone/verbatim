//! `verbatim-inspect` — developer tooling over the control plane
//! (architecture section 9).
//!
//! Live event stream, gesture and key injection, speech capture, and
//! latency timelines against a running Verbatim instance. Deliberately not
//! a child of Core and not in any job object; it attaches through the named
//! pipe like any control-plane client (architecture section 10).
//!
//! Output is plain text, one fact per line: no tables, no spinners, no
//! ASCII art, so it reads well piped, redirected, or through a screen
//! reader. These are developer-facing strings, not user-facing Verbatim
//! output, so the workspace's no-hardcoded-strings rule (D10) does not
//! apply to them.

mod timestamp;

use std::io;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use verbatim_control::client::{Client, ok_or_error};
use verbatim_control::protocol::{
    Frame, LatencyRecord, OutpostState, ReplyPayload, Request, StatusInfo,
};
use verbatim_model::{NodeSnapshot, NormalizedEvent, PropertyChange, TreeNode};

/// Developer inspection CLI over Verbatim's control plane.
#[derive(Parser)]
#[command(
    name = "verbatim-inspect",
    about = "Developer inspection CLI over the control plane"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Control-plane address: "tcp:HOST:PORT" for TCP, anything else is a
    /// pipe path, or omit for the well-known local pipe.
    #[arg(long, global = true)]
    connect: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Print pid, version, active synth, and outpost status.
    Status,
    /// Subscribe to and print normalized accessibility events.
    WatchEvents,
    /// Subscribe to and print captured speech.
    WatchSpeech,
    /// Subscribe to both events and speech on one connection, printed
    /// interleaved in arrival order; lines are prefixed `event` or `speech`
    /// and share trace ids for correlation.
    Watch,
    /// Route a gesture identifier through the gesture router, e.g.
    /// `kb:verbatim+v`.
    SendGesture {
        /// The gesture identifier.
        identifier: String,
    },
    /// Synthesize real keyboard input, e.g. `downarrow enter "shift+tab"`.
    SendKeys {
        /// One or more key combinations, each a plus-joined name such as
        /// `shift+tab`.
        keys: Vec<String>,
    },
    /// Print recent end-to-end latency timelines.
    Latency {
        /// At most this many timelines, newest first.
        #[arg(long, default_value_t = 10)]
        last: u32,
    },
    /// Dump the foreground application's accessibility tree from its
    /// top-level window, one node per line, indented two spaces per depth
    /// level.
    DumpTree,
    /// Ask Verbatim to write its flight recorder's current contents to
    /// disk and print the path it wrote to.
    DumpRecorder,
    /// Ask Verbatim to exit cleanly.
    Quit,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command, cli.connect.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Opens a control-plane connection: `address` of the form `tcp:HOST:PORT`
/// connects over TCP, anything else is treated as a pipe path, and `None`
/// connects to the well-known local pipe.
fn connect_client(address: Option<&str>) -> io::Result<Client> {
    match address {
        None => Client::connect_pipe(),
        Some(address) => match address.strip_prefix("tcp:") {
            Some(host_port) => Client::connect_tcp(host_port),
            None => Client::connect_pipe_named(address),
        },
    }
}

fn run(command: Command, connect: Option<&str>) -> io::Result<()> {
    let mut client = connect_client(connect)?;
    match command {
        Command::Status => status(&mut client),
        Command::WatchEvents => watch_events(&mut client),
        Command::WatchSpeech => watch_speech(&mut client),
        Command::Watch => watch_both(&mut client),
        Command::SendGesture { identifier } => send_gesture(&mut client, identifier),
        Command::SendKeys { keys } => send_keys(&mut client, keys),
        Command::Latency { last } => latency(&mut client, last),
        Command::DumpTree => dump_tree(&mut client),
        Command::DumpRecorder => dump_recorder(&mut client),
        Command::Quit => quit(&mut client),
    }
}

fn status(client: &mut Client) -> io::Result<()> {
    let frame = ok_or_error(client.request(Request::Status)?)?;
    let Frame::Reply {
        payload: ReplyPayload::Status(status),
        ..
    } = frame
    else {
        return Err(io::Error::other(format!(
            "unexpected reply to Status: {frame:?}"
        )));
    };
    print_status(&status);
    Ok(())
}

fn print_status(status: &StatusInfo) {
    println!("pid: {}", status.pid);
    println!("version: {}", status.version);
    match &status.active_synth {
        Some(synth) => println!("active synth: {synth}"),
        None => println!("active synth: none"),
    }
    if status.outposts.is_empty() {
        println!("outposts: none");
        return;
    }
    for outpost in &status.outposts {
        let outpost_pid = outpost
            .outpost_pid
            .map_or_else(|| "not yet running".to_owned(), |pid| pid.to_string());
        let state = match outpost.state {
            OutpostState::Starting => "starting",
            OutpostState::Ready => "ready",
            OutpostState::Restarting => "restarting",
            _ => "unknown",
        };
        println!(
            "outpost: target pid {}, outpost pid {outpost_pid}, state {state}",
            outpost.target_pid
        );
    }
}

fn watch_events(client: &mut Client) -> io::Result<()> {
    ok_or_error(client.request(Request::SubscribeEvents)?)?;
    loop {
        if let Some(line) = event_line(&client.next_frame()?) {
            println!("{line}");
        }
    }
}

/// Both subscriptions on one connection; frames print interleaved in
/// arrival order, so each event reads directly above the speech it caused.
fn watch_both(client: &mut Client) -> io::Result<()> {
    ok_or_error(client.request(Request::SubscribeEvents)?)?;
    ok_or_error(client.request(Request::SubscribeSpeech)?)?;
    loop {
        let frame = client.next_frame()?;
        if let Some(line) = event_line(&frame) {
            println!("event {line}");
        } else if let Some(line) = speech_line(&frame) {
            println!("speech {line}");
        }
    }
}

/// Formats an event frame as one line, or `None` for other frame kinds.
fn event_line(frame: &Frame) -> Option<String> {
    let Frame::Event {
        trace_id,
        source,
        backend,
        event,
        ..
    } = frame
    else {
        return None;
    };
    Some(format!(
        "trace {trace_id} source {source} backend {backend:?}: {}",
        summarize_event(event)
    ))
}

fn summarize_event(event: &NormalizedEvent) -> String {
    match event {
        NormalizedEvent::FocusChanged { node } => format!("focus: {}", summarize_node(node)),
        NormalizedEvent::PropertyChanged { node_id, change } => match change {
            PropertyChange::Name(name) => format!(
                "{node_id:?} name changed to {}",
                name.as_deref().unwrap_or("(none)")
            ),
            PropertyChange::Value(value) => format!(
                "{node_id:?} value changed to {}",
                value.as_deref().unwrap_or("(none)")
            ),
            other_change => format!("{node_id:?} property changed: {other_change:?}"),
        },
        NormalizedEvent::ValueChanged { node_id, value } => format!(
            "value change: {node_id:?} = {}",
            value.as_deref().unwrap_or("(none)")
        ),
        other => format!("{other:?}"),
    }
}

fn summarize_node(node: &NodeSnapshot) -> String {
    format!(
        "{:?} {}",
        node.role,
        node.name.as_deref().unwrap_or("(no name)")
    )
}

fn watch_speech(client: &mut Client) -> io::Result<()> {
    ok_or_error(client.request(Request::SubscribeSpeech)?)?;
    loop {
        if let Some(line) = speech_line(&client.next_frame()?) {
            println!("{line}");
        }
    }
}

/// Formats a speech frame as one line, or `None` for other frame kinds.
fn speech_line(frame: &Frame) -> Option<String> {
    let Frame::Speech {
        trace_id,
        text,
        event_observed_at_ms,
        queued_at_ms,
        audio_started_at_ms,
    } = frame
    else {
        return None;
    };
    // A frame with an audio-start time is the follow-up sent when the first
    // buffer reached the device — the true event-to-audio latency; a frame
    // without one was sent at queue time, before any audio existed.
    let line = if let Some(started) = audio_started_at_ms {
        let delta = event_observed_at_ms.map_or_else(
            || {
                format!(
                    " (+{} ms after queue)",
                    started.saturating_sub(*queued_at_ms)
                )
            },
            |observed| format!(" (+{} ms after event)", started.saturating_sub(observed)),
        );
        format!(
            "trace {trace_id} audio started {}{delta}: {text}",
            timestamp::local(*started)
        )
    } else {
        let delta = event_observed_at_ms.map_or_else(String::new, |observed| {
            format!(
                " (+{} ms after event)",
                queued_at_ms.saturating_sub(observed)
            )
        });
        format!(
            "trace {trace_id} queued {}{delta}: {text}",
            timestamp::local(*queued_at_ms)
        )
    };
    Some(line)
}

fn send_gesture(client: &mut Client, identifier: String) -> io::Result<()> {
    ok_or_error(client.request(Request::SendGesture { identifier })?)?;
    println!("gesture sent");
    Ok(())
}

fn send_keys(client: &mut Client, keys: Vec<String>) -> io::Result<()> {
    ok_or_error(client.request(Request::SendKeys { keys })?)?;
    println!("keys sent");
    Ok(())
}

fn latency(client: &mut Client, last: u32) -> io::Result<()> {
    let frame = ok_or_error(client.request(Request::Latency { last_n: last })?)?;
    let Frame::Reply {
        payload: ReplyPayload::Latency(records),
        ..
    } = frame
    else {
        return Err(io::Error::other(format!(
            "unexpected reply to Latency: {frame:?}"
        )));
    };
    if records.is_empty() {
        println!("no latency timelines recorded yet");
        return Ok(());
    }
    for record in &records {
        print_latency_record(record);
    }
    Ok(())
}

fn print_latency_record(record: &LatencyRecord) {
    let observed = record.event_observed_at_ms;
    print!(
        "trace {}: event observed {}",
        record.trace_id,
        timestamp::local(observed)
    );
    if let Some(queued) = record.speech_queued_at_ms {
        print!(", speech queued +{} ms", queued.saturating_sub(observed));
    }
    if let Some(started) = record.audio_started_at_ms {
        print!(", audio started +{} ms", started.saturating_sub(observed));
    }
    println!();
}

fn dump_tree(client: &mut Client) -> io::Result<()> {
    let frame = ok_or_error(client.request(Request::DumpTree)?)?;
    let Frame::Reply {
        payload: ReplyPayload::DumpTree { root, truncated },
        ..
    } = frame
    else {
        return Err(io::Error::other(format!(
            "unexpected reply to DumpTree: {frame:?}"
        )));
    };
    print_tree_node(&root, 0);
    if truncated {
        println!("(tree truncated: depth or node-count cap reached)");
    }
    Ok(())
}

/// Prints one tree node per line, indented two spaces per depth level,
/// reusing [`summarize_node`]'s role-and-name formatting.
fn print_tree_node(node: &TreeNode, depth: usize) {
    println!("{}{}", "  ".repeat(depth), summarize_node(&node.snapshot));
    for child in &node.children {
        print_tree_node(child, depth + 1);
    }
}

fn dump_recorder(client: &mut Client) -> io::Result<()> {
    let frame = ok_or_error(client.request(Request::DumpRecorder)?)?;
    let Frame::Reply {
        payload: ReplyPayload::DumpRecorder { path },
        ..
    } = frame
    else {
        return Err(io::Error::other(format!(
            "unexpected reply to DumpRecorder: {frame:?}"
        )));
    };
    println!("{path}");
    Ok(())
}

fn quit(client: &mut Client) -> io::Result<()> {
    ok_or_error(client.request(Request::Quit)?)?;
    println!("Verbatim is exiting");
    Ok(())
}
