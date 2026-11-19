//! The relay hands on the listener's window destroyed: a
//! `ListenerToCore::Destroyed` the current listener writes reaches the app
//! as an `OutpostMessage::Listener` carrying it whole, with or without a
//! process, in the order written, so Core can clear the window and tell its
//! outpost (`docs/design/readers-tell-core-asks-build1.md`, "Window
//! destruction").
//!
//! This test binary runs itself as a stand-in for the focus listener, which
//! says it is ready, writes two destroys, answers pings and shuts down when
//! asked, and as a stand-in for the outpost binary, whose spares say they
//! are ready, answer pings and shut down when asked. Nothing makes a UI
//! Automation or `WinEvent` call, so the test touches nothing on the
//! desktop. Its own `main` recognizes each command line.

use std::io::BufReader;
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_model::{OutpostId, Pid, WindowHandle};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::{read_message, write_message};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use verbatim_supervisor::{Executables, OutpostMessage, Supervisor};

mod common;
use common::Delivered;

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// The two destroys the stand-in listener writes after `Ready`: one whose
/// raising thread it could open, one whose it could not.
fn destroys() -> [ListenerToCore; 2] {
    [
        ListenerToCore::Destroyed {
            window: WindowHandle(7),
            process: Some(Pid(9)),
            at_us: 1,
        },
        ListenerToCore::Destroyed {
            window: WindowHandle(8),
            process: None,
            at_us: 2,
        },
    ]
}

/// The value after `name` on the command line, if it is there.
fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// A stand-in child: the listener says `Ready` and writes [`destroys`]; a
/// spare says `SpareReady`. Each answers pings, and exits cleanly for the
/// shutdown message or the end of its command pipe.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} is a handle value: {args:?}"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (commands, messages) = unsafe {
        verbatim_process::inherited_pipe_handles(handle("--pipe-in"), handle("--pipe-out"))
    }
    .expect("the inherited pipes");
    let (commands, mut messages) = (
        BlockingReader::new(commands).expect("a reader"),
        BlockingWriter::new(messages).expect("a writer"),
    );
    let mut commands = BufReader::new(commands);
    let own_pid = Pid(std::process::id());
    if args.iter().any(|arg| arg == "--listener") {
        write_message(
            &mut messages,
            &ListenerToCore::Ready {
                outpost_pid: own_pid,
                target_pid: Pid(0),
            },
        )
        .expect("a message is written");
        for destroyed in destroys() {
            write_message(&mut messages, &destroyed).expect("a message is written");
        }
        loop {
            match read_message::<_, CoreToListener>(&mut commands) {
                Ok(Some(CoreToListener::Ping { .. })) => write_message(
                    &mut messages,
                    &ListenerToCore::Pong {
                        parked_count: 0,
                        written_at_us: 0,
                    },
                )
                .expect("a message is written"),
                Ok(Some(CoreToListener::Shutdown) | None) => std::process::exit(0),
                Ok(Some(_)) => {}
                Err(error) => panic!("the command pipe failed: {error}"),
            }
        }
    }
    write_message(
        &mut messages,
        &OutpostToCore::SpareReady {
            outpost_pid: own_pid,
        },
    )
    .expect("a message is written");
    loop {
        match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(CoreToOutpost::Ping { .. })) => write_message(
                &mut messages,
                &OutpostToCore::Pong {
                    parked_count: 0,
                    parked_on_hung: 0,
                    written_at_us: 0,
                },
            )
            .expect("a message is written"),
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// The listener's two destroys reach the app whole, from the first
/// listener, in the order it wrote them, and nothing else the listener
/// wrote is handed on as a listener message.
fn the_listeners_window_destroyed_is_handed_on_to_the_app() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    let mut handed = Vec::new();
    while handed.len() < 2 {
        match events
            .recv_timeout(WAIT)
            .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
        {
            OutpostMessage::Listener { listener, message } => handed.push((listener, *message)),
            OutpostMessage::ListenerReady { .. } | OutpostMessage::SpareReady { .. } => {}
            other => panic!("the supervisor said {other:?}"),
        }
    }
    assert_eq!(
        handed,
        destroys()
            .into_iter()
            .map(|destroyed| (OutpostId(1), destroyed))
            .collect::<Vec<_>>()
    );
    let _ = supervisor.shutdown();
    let more: Vec<OutpostMessage> = events
        .try_iter()
        .filter(|message| matches!(message, OutpostMessage::Listener { .. }))
        .collect();
    assert!(more.is_empty(), "nothing more was handed on: {more:?}");
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    let arguments = libtest_mimic::Arguments::from_args();
    let trials = vec![libtest_mimic::Trial::test(
        "the_listeners_window_destroyed_is_handed_on_to_the_app",
        || {
            the_listeners_window_destroyed_is_handed_on_to_the_app();
            Ok(())
        },
    )];
    libtest_mimic::run(&arguments, trials).exit_code()
}
