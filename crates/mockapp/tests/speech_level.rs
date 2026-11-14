//! Prototype of a speech-level test (`prototype/speech-level-experiment`):
//! the real `verbatim.exe`, which starts the real supervisor, its relay,
//! the focus listener and the outposts, runs on the test's own desktop
//! with `mockapp` as the application, and the test reads what Verbatim
//! says on its control plane, as the end-to-end suite does.
//!
//! No window on a desktop made for a test has the foreground or the
//! keyboard focus, so the test world ([`verbatim_process::test_world`])
//! supplies those facts to every process, and tells the listener of a UI
//! Automation focus event at its focus callback.
//!
//! The case: the table-cells fixture of the end-to-end scenario
//! `table_cells`, its focus on the first cell, comes to the foreground and
//! its focus is told; Verbatim says what that scenario expects as the
//! window opens.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use verbatim_control::client::Client;
use verbatim_control::protocol::{Frame, Request};
use verbatim_model::UtteranceEnding;
use verbatim_process::test_world::{ENV as WORLD_ENV, World};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
use windows::core::HSTRING;

fn main() {
    harness::run_isolated(&[(
        "a_focused_list_item_in_a_window_coming_to_the_front_is_spoken",
        a_focused_list_item_in_a_window_coming_to_the_front_is_spoken,
    )]);
}

/// How long Verbatim has to become ready, and to become idle: bounds on a
/// hang only.
const BOUND: Duration = Duration::from_secs(30);

/// `verbatim.exe`, built beside this test into the same target directory.
fn verbatim_exe() -> PathBuf {
    let test = std::env::current_exe().expect("the test's own path");
    let debug = test
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the test sits in deps under the profile's directory");
    let exe = debug.join("verbatim.exe");
    assert!(
        exe.is_file(),
        "{} is missing: build verbatim-app, verbatim-listener, verbatim-outpost and verbatim-synth-host first",
        exe.display()
    );
    exe
}

/// The running Verbatim, quit through its control plane at the end.
struct Verbatim {
    child: Child,
}

impl Drop for Verbatim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts Verbatim in `world` and waits until it says it is ready.
fn start_verbatim(world: &World) -> Verbatim {
    let ready_name = format!("{}.ready", world.name());
    // SAFETY: a named manual-reset event, no pointer passed.
    let ready = unsafe { CreateEventW(None, true, false, &HSTRING::from(ready_name.as_str())) }
        .expect("the ready event");
    let child = Command::new(verbatim_exe())
        .env(WORLD_ENV, world.name())
        .env("VERBATIM_TEST_AUDIO", "null")
        .env("VERBATIM_READY_EVENT", &ready_name)
        .creation_flags(windows::Win32::System::Threading::DETACHED_PROCESS.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("verbatim.exe starts");
    let process = windows::Win32::Foundation::HANDLE(
        std::os::windows::io::AsRawHandle::as_raw_handle(&child),
    );
    let verbatim = Verbatim { child };
    // SAFETY: both handles are open.
    let waited = unsafe {
        WaitForMultipleObjects(
            &[ready, process],
            false,
            u32::try_from(BOUND.as_millis()).unwrap(),
        )
    };
    // SAFETY: the event was created above and is closed once.
    let _ = unsafe { CloseHandle(ready) };
    assert_eq!(
        waited, WAIT_OBJECT_0,
        "Verbatim did not become ready within {BOUND:?} (or exited)"
    );
    verbatim
}

/// One utterance as the test asserts it: its text, and whether it was
/// heard in full.
#[derive(Debug, PartialEq)]
struct Heard {
    text: String,
    ending: UtteranceEnding,
}

/// Reads frames on `control` until the reply to request `id` and the
/// ending of every utterance announced before it, collecting each
/// utterance with its ending; fails on an error reply. The endings are the
/// evidence that an utterance was heard in full: speech plays in real time.
fn speech_until_reply(control: &mut Client, id: u64) -> Vec<Heard> {
    let mut heard: Vec<(verbatim_model::UtteranceId, String, Option<UtteranceEnding>)> = Vec::new();
    let mut replied = false;
    while !replied || heard.iter().any(|(_, _, ending)| ending.is_none()) {
        match control.next_frame().expect("the control connection") {
            Frame::Speech {
                utterance, text, ..
            } => heard.push((utterance, text, None)),
            Frame::SpeechEnded { utterance, ending } => {
                let entry = heard
                    .iter_mut()
                    .find(|(id, ..)| *id == utterance)
                    .expect("an ending of an utterance announced on this connection");
                entry.2 = Some(ending);
            }
            Frame::Sound { indication, .. } => heard.push((
                verbatim_model::UtteranceId(u64::MAX),
                format!("sound: {indication}"),
                Some(UtteranceEnding::Completed),
            )),
            Frame::Reply { to, .. } if to == id => replied = true,
            Frame::Error { to, message } if to == id => {
                panic!("Verbatim answered request {id} with an error: {message}")
            }
            _ => {}
        }
    }
    heard
        .into_iter()
        .map(|(_, text, ending)| Heard {
            text,
            ending: ending.expect("every utterance has ended"),
        })
        .collect()
}

/// Waits until Verbatim is idle, every utterance it queued before having
/// ended, and answers them.
fn idle(control: &mut Client) -> Vec<Heard> {
    let id = control
        .send(Request::AwaitIdle {
            after_input: None,
            timeout_ms: u64::try_from(BOUND.as_millis()).unwrap(),
        })
        .expect("asks whether Verbatim is idle");
    speech_until_reply(control, id)
}

fn heard_in_full(texts: &[&str]) -> Vec<Heard> {
    texts
        .iter()
        .map(|text| Heard {
            text: (*text).to_owned(),
            ending: UtteranceEnding::Completed,
        })
        .collect()
}

/// mockapp's window comes to the front, its focus on the first cell of
/// "Grid A", and the listener is told of the focus: Verbatim says the
/// window, the list and the cell, exactly as the end-to-end scenario
/// `table_cells` expects as its window opens.
fn a_focused_list_item_in_a_window_coming_to_the_front_is_spoken() {
    let started = Instant::now();
    let name = format!(r"Local\verbatim-world-{}", std::process::id());
    let world = World::create(&name).expect("the test world");
    let verbatim = start_verbatim(&world);
    let ready_at = started.elapsed();
    let mut control = Client::connect_pipe_named(&world.control_pipe()).expect("the control pipe");
    let subscribe = control
        .send(Request::SubscribeSpeech)
        .expect("subscribes to speech");
    let mut startup = speech_until_reply(&mut control, subscribe);
    startup.extend(idle(&mut control));
    assert_eq!(
        startup,
        heard_in_full(&["sound: start"]),
        "the startup speech"
    );
    let startup_at = started.elapsed();

    let title = common::unique_title("speech-level");
    let app = common::spawn("speech_table_cells.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    world.set_foreground(hwnd);
    assert!(
        world.tell_focus(hwnd, u32::try_from(BOUND.as_millis()).unwrap()),
        "the listener did not take the told focus in within {BOUND:?}"
    );
    let told_at = started.elapsed();
    let heard = idle(&mut control);
    let heard_at = started.elapsed();
    assert_eq!(
        heard,
        heard_in_full(&[
            "Speech Level Table Cells window",
            "Grid A list",
            "First row 1 column 2 2 of 6",
        ]),
        "the focus's speech"
    );

    // Verbatim quits, and its processes end with it.
    let quit = control.send(Request::Quit).expect("asks Verbatim to quit");
    let _ = quit;
    drop(control);
    let mut verbatim = verbatim;
    let status = verbatim.child.wait().expect("Verbatim's exit");
    assert!(status.success(), "Verbatim exited with {status}");
    app.quit();
    eprintln!(
        "speech-level run took {:?}: ready {ready_at:?}, startup speech ended {startup_at:?}, focus told {told_at:?}, its speech ended {heard_at:?}",
        started.elapsed()
    );
}
