//! The reducer's order of arrival with the relay reading the outposts'
//! pipes (`docs/design/communication.md`, sections 4.5, 4.8, 6.2 and 6.8;
//! migration step 7). The test stands in for verbatim.exe's reducer: it
//! takes its inputs from a port queue, as the reducer does, into which the
//! supervisor's sink posts and the test posts its own keys, and passes the
//! supervisor's messages through the supervisor's `Frames`, as the reducer
//! does. "The reducer held" is the test taking nothing from the queue.
//!
//! This test binary runs itself three ways: as an application, a process
//! with no windows that runs until its standard input closes; as a
//! stand-in for the outpost binary; and as a stand-in for the focus
//! listener. Nothing makes a UI Automation or `WinEvent` call, so the test
//! touches nothing on the desktop. Its own `main` recognizes each command
//! line.
//!
//! The stand-in outpost answers pings and writes, for each `Cancel` Core
//! sends it, the script that request number names (`script`), with
//! blocking writes, so it gets past a frame only once the pipe has taken
//! it; for requests 8 and 9 it first writes a frame that is not a message,
//! and after request 9 it writes one more frame when Core shuts it down. The stand-in listener says it is ready, answers pings, and sets a
//! named event each time Core tells it an outpost's hook is in, which Core
//! does as the relay handles that outpost's `Ready`: since the relay reads
//! an outpost's pipe in order, the event is the evidence that the relay
//! has read every frame the outpost wrote before `Ready`, with the reducer
//! held.

use std::collections::HashMap;
use std::io::{BufReader, Read as _, Write as _};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use verbatim_model::{Backend, NodeId, NormalizedEvent, OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    EventTiming, OutpostToSupervisor, SupervisorToOutpost, read_message, write_message,
};
use verbatim_outpost::supervisor::{
    EventSink, Frames, LARGE_FRAME_BYTES, OutpostMessage, ReducerStall, ShutdownSummary, Supervisor,
};
use verbatim_process::port_queue::{PortQueue, port_queue};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::HSTRING;

/// The application's argument.
const APPLICATION: &str = "--stand-in-application";

/// The environment variable naming the event the stand-in listener sets.
const SIGNAL_ENV: &str = "VERBATIM_TEST_ARRIVAL_SIGNAL";

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits: past the four heartbeats, three ping
/// intervals apart, a reducer's stall is reported on.
const WAIT: Duration = Duration::from_secs(30);

/// The value after `name` on the command line.
fn value(args: &[String], name: &str) -> String {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| panic!("the command line names {name}: {args:?}"))
}

/// A fault whose detail is `name`, padded with `padding` more bytes: how
/// the stand-in outpost writes a frame the test can tell apart.
fn fault(name: &str, padding: usize) -> OutpostToSupervisor {
    let mut detail = name.to_owned();
    if padding > 0 {
        detail.push(' ');
        detail.push_str(&"x".repeat(padding));
    }
    OutpostToSupervisor::Fault { detail }
}

/// A value change of node 7 to `value`: two of them are merged by the
/// outpost's outbound rules whenever the second is sent while the first
/// waits behind a write in flight.
fn value_change(value: &str) -> OutpostToSupervisor {
    OutpostToSupervisor::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        backend: Backend::Uia,
        window: None,
        application: None,
        timing: EventTiming::default(),
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(7),
            value: Some(value.to_owned()),
        },
    }
}

/// What the stand-in outpost writes for request number `request`, in
/// order; `None` for the end of its process, with code 3.
fn script(request: u64, ready: &OutpostToSupervisor) -> Vec<Option<OutpostToSupervisor>> {
    let large = LARGE_FRAME_BYTES + 1;
    match request {
        1 => vec![Some(fault("A", 0)), Some(ready.clone())],
        2 => vec![Some(fault("B", 0))],
        3 => vec![
            Some(fault("L", large)),
            Some(fault("S1", 0)),
            Some(ready.clone()),
        ],
        4 => vec![Some(fault("E2", 0)), Some(ready.clone())],
        5 => vec![
            Some(fault("F1", 0)),
            Some(fault("F2", 0)),
            Some(fault("F3", large)),
            None,
        ],
        6 => {
            // Far more than the pipe's 64 KiB and the relay's one read
            // can hold, each frame parsed on the reducer's thread.
            let mut frames: Vec<_> = (0..8)
                .map(|index| Some(fault(&format!("filler{index}"), 48 * 1024)))
                .collect();
            frames.extend([
                Some(value_change("one")),
                Some(value_change("two")),
                Some(ready.clone()),
            ]);
            frames
        }
        7 => vec![Some(fault("C", 0))],
        8 => vec![Some(fault("G1", 0)), Some(ready.clone())],
        _ => Vec::new(),
    }
}

/// Writes `message` to Core.
fn send(messages: &mut BlockingWriter, message: &OutpostToSupervisor) {
    write_message(messages, message).expect("a message is written");
}

/// Writes Core a whole frame whose body is not a message.
fn send_unreadable(messages: &mut BlockingWriter) {
    let mut frame = Vec::new();
    verbatim_process::transport::put_frame(&mut frame, |body| {
        body.extend_from_slice(b"{\"NoSuchMessage\":1}");
        Ok(())
    })
    .expect("framed");
    messages.write_all(&frame).expect("the frame is written");
}

/// A stand-in child, as the module says.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .parse()
            .unwrap_or_else(|_| panic!("{name} is a handle value"))
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
    let listener = args.iter().any(|arg| arg == "--listener");
    let target_pid = if listener {
        Pid(0)
    } else {
        Pid(value(args, "--target-pid").parse().expect("a pid"))
    };
    let ready = OutpostToSupervisor::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    };
    let signal = listener.then(|| {
        let name = std::env::var(SIGNAL_ENV).expect("the test names its event");
        // SAFETY: opens the test's named event, or makes it; the handle is
        // owned here until the process ends.
        unsafe { CreateEventW(None, false, false, &HSTRING::from(name)) }.expect("the test's event")
    });
    if listener {
        send(&mut messages, &ready);
    }
    // Whether to write a frame when shut down.
    let mut farewell = false;
    let mut commands = BufReader::new(commands);
    loop {
        let command = match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(command)) => command,
            Ok(None) => std::process::exit(0),
            Err(error) => panic!("the command pipe failed: {error}"),
        };
        match command {
            SupervisorToOutpost::Ping { seq, .. } => send(
                &mut messages,
                &OutpostToSupervisor::Pong {
                    seq,
                    parked_count: 0,
                    written_at_us: 0,
                },
            ),
            SupervisorToOutpost::Shutdown => {
                if farewell {
                    send(&mut messages, &fault("Z", 0));
                }
                std::process::exit(0)
            }
            SupervisorToOutpost::HookInstalled { .. } => {
                if let Some(signal) = signal {
                    // SAFETY: an event handle this process holds.
                    unsafe { SetEvent(signal) }.expect("the test's event is set");
                }
            }
            SupervisorToOutpost::Cancel { request_id } if !listener => {
                if matches!(request_id, 8 | 9) {
                    send_unreadable(&mut messages);
                }
                farewell |= request_id == 9;
                for message in script(request_id, &ready) {
                    match message {
                        Some(message) => send(&mut messages, &message),
                        None => std::process::exit(3),
                    }
                }
            }
            _ => {}
        }
    }
}

/// The named event the stand-in listener sets.
struct Signal(HANDLE);

impl Signal {
    /// Names this process's event for the stand-in listeners it starts,
    /// and makes it.
    fn new() -> Self {
        let name = format!("Local\\verbatim-arrival-order-{}", std::process::id());
        // SAFETY: the environment is set before any thread that reads it
        // starts: the supervisors, which launch the children, come after.
        unsafe { std::env::set_var(SIGNAL_ENV, &name) };
        // SAFETY: makes a named, auto-reset event; the handle is closed on
        // drop.
        let event =
            unsafe { CreateEventW(None, false, false, &HSTRING::from(name)) }.expect("an event");
        Self(event)
    }

    /// Waits for the listener to say an outpost's hook is in: the relay
    /// has handled that outpost's `Ready`, and every frame before it.
    fn wait(&self, what: &str) {
        let milliseconds = u32::try_from(WAIT.as_millis()).expect("in range");
        // SAFETY: an event handle this process holds.
        let waited = unsafe { WaitForSingleObject(self.0, milliseconds) };
        assert_eq!(
            waited, WAIT_OBJECT_0,
            "the relay did not handle {what} while the reducer was held"
        );
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        // SAFETY: the handle is this value's, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// One of the stand-in reducer's inputs.
enum Input {
    Outpost(OutpostMessage),
    Key(u32),
}

/// The stand-in reducer: its port, the supervisor posting to it, and how
/// it names what it takes.
struct Reducer {
    queue: PortQueue<Input>,
    keys: verbatim_process::port_queue::Poster<Input>,
    frames: Frames,
    supervisor: Supervisor,
    stalls: Receiver<ReducerStall>,
    names: HashMap<OutpostId, String>,
}

impl Reducer {
    /// A supervisor launching this binary's stand-ins, posting to a port
    /// queue, with the queue watched.
    fn start() -> Self {
        let (poster, queue) = port_queue::<Input>().expect("a port");
        let (stalls_tx, stalls) = unbounded();
        let progress = poster.clone();
        let deliver = poster.clone();
        let sink = EventSink::new(move |message| {
            deliver
                .send(Input::Outpost(message))
                .map_err(|_| verbatim_outpost::Undelivered::Gone)
        })
        .watched(
            move || progress.progress(),
            move |stall| {
                let _ = stalls_tx.send(stall);
            },
        );
        let exe = std::env::current_exe().expect("this test binary's path");
        let supervisor = Supervisor::with_executable(
            sink,
            OutpostOptions::default(),
            exe,
            LIMIT,
            std::sync::Arc::default(),
        )
        .expect("the supervisor starts");
        let frames = supervisor.frames();
        Self {
            queue,
            keys: poster,
            frames,
            supervisor,
            stalls,
            names: HashMap::new(),
        }
    }

    /// Posts key `number`, behind everything posted before it.
    fn press(&self, number: u32) {
        assert!(
            self.keys.send(Input::Key(number)).is_ok(),
            "the key is posted"
        );
    }

    /// Takes inputs in order, as the reducer does, until `count` things
    /// have been handled, and names each.
    fn take(&mut self, count: usize) -> Vec<String> {
        let mut handled = Vec::new();
        while handled.len() < count {
            let input = self
                .queue
                .recv_timeout(WAIT)
                .unwrap_or_else(|| panic!("nothing more came after {handled:?}"));
            match input {
                Input::Key(number) => handled.push(format!("key {number}")),
                Input::Outpost(message) => {
                    let mut decoded = Vec::new();
                    self.frames.arrived(message, &mut decoded);
                    for message in decoded {
                        handled.push(self.name(message));
                    }
                }
            }
        }
        handled
    }

    /// Names a message the supervisor delivered, after `Frames`.
    fn name(&mut self, message: OutpostMessage) -> String {
        match message {
            OutpostMessage::ListenerReady { .. } => "listener ready".to_owned(),
            OutpostMessage::Started { outpost, .. } => {
                let name = format!("O{}", self.names.len() + 1);
                self.names.insert(outpost, name.clone());
                format!("{name} started")
            }
            OutpostMessage::Event {
                outpost, message, ..
            } => {
                let from = &self.names[&outpost];
                match *message {
                    OutpostToSupervisor::Fault { detail } => {
                        let name = detail.split(' ').next().unwrap_or_default();
                        format!("{from}: {name}")
                    }
                    OutpostToSupervisor::Ready { .. } => format!("{from}: ready"),
                    OutpostToSupervisor::Event {
                        event: NormalizedEvent::ValueChanged { node_id, value },
                        ..
                    } => format!(
                        "{from}: node {} value {}",
                        node_id.number(),
                        value.unwrap_or_default()
                    ),
                    other => format!("{from}: {other:?}"),
                }
            }
            OutpostMessage::Ended {
                outpost, reason, ..
            } => format!("{} ended: {reason}", self.names[&outpost]),
            OutpostMessage::Reincarnated { outpost, .. } => {
                format!("{} replaced", self.names[&outpost])
            }
            other => format!("{other:?}"),
        }
    }

    /// The supervisor started with its listener ready, and an outpost
    /// started for each of `applications`, in order.
    fn with_outposts(&mut self, applications: &[Pid]) {
        assert_eq!(self.take(1), ["listener ready"]);
        for (index, &pid) in applications.iter().enumerate() {
            self.supervisor.ensure_spawned(pid);
            assert_eq!(self.take(1), [format!("O{} started", index + 1)]);
        }
    }

    /// Cues outpost `name` to write request `request`'s script.
    fn cue(&self, name: &str, request: u64) {
        let outpost = self
            .names
            .iter()
            .find_map(|(outpost, known)| (known == name).then_some(*outpost))
            .expect("a started outpost");
        self.supervisor
            .send_to_outpost(
                outpost,
                SupervisorToOutpost::Cancel {
                    request_id: request,
                },
            )
            .expect("the cue is queued");
    }

    /// Shuts the supervisor down, expecting `summary`, and asserts nothing
    /// more was delivered.
    fn finish(mut self, summary: ShutdownSummary) {
        assert_eq!(self.supervisor.shutdown(), summary);
        let mut rest = Vec::new();
        while let Some(input) = self.queue.try_recv() {
            match input {
                Input::Key(number) => rest.push(format!("key {number}")),
                Input::Outpost(message) => rest.push(format!("{message:?}")),
            }
        }
        assert!(rest.is_empty(), "nothing more was delivered: {rest:?}");
        assert!(!self.frames.parsing(), "no frame is still being parsed");
        assert!(
            self.stalls.try_recv().is_err(),
            "no stall was reported but those asserted"
        );
    }
}

/// Starts an application, a process of this binary's that runs until its
/// standard input closes.
fn application() -> (Child, Pid) {
    let exe = std::env::current_exe().expect("this test binary's path");
    let child = Command::new(exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let pid = Pid(child.id());
    (child, pid)
}

/// Ends an application started by [`application`].
fn end(mut application: Child) {
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
}

/// A frame an outpost wrote before a key was pressed, while the reducer
/// was held, is reduced before the key; one it wrote after the key, after
/// it.
fn a_frame_written_before_a_key_is_reduced_before_it_and_one_written_after_it_after(
    signal: &Signal,
) {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    // Held from here: the outpost writes A, then Ready, which the relay
    // handles by telling the listener.
    reducer.cue("O1", 1);
    signal.wait("A and Ready");
    reducer.press(1);
    reducer.cue("O1", 2);
    assert_eq!(reducer.take(4), ["O1: A", "O1: ready", "key 1", "O1: B"]);

    reducer.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    end(app);
}

/// A large frame from one outpost is parsed off the reducer's thread: a
/// key and another outpost's frames that arrived after it are handled
/// first, and that outpost's own later frames come after it, in order.
fn a_large_frame_delays_no_key_and_no_other_outposts_frame_and_its_outposts_next_frame_follows_it(
    signal: &Signal,
) {
    let (first_app, first_pid) = application();
    let (second_app, second_pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[first_pid, second_pid]);

    // Held from here: all of it is in the reducer's port before it takes
    // any, the large frame first.
    reducer.cue("O1", 3);
    signal.wait("O1's large frame, S1 and Ready");
    reducer.press(1);
    reducer.cue("O2", 4);
    signal.wait("O2's E2 and Ready");
    assert_eq!(
        reducer.take(6),
        [
            "key 1",
            "O2: E2",
            "O2: ready",
            "O1: L",
            "O1: S1",
            "O1: ready"
        ]
    );

    reducer.finish(ShutdownSummary {
        clean: 3,
        exited: 0,
        killed: 0,
    });
    end(first_app);
    end(second_app);
}

/// An outpost's pipe closing reaches the app, as its end, after every
/// frame it wrote before, the last a large one parsed by the helper. It
/// ended before its `Ready`, so the app hears it replaced, after its frames
/// as its end is, and after its end its replacement's start.
fn a_closed_pipe_reaches_the_app_after_every_frame_before_it() {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    reducer.cue("O1", 5);
    assert_eq!(
        reducer.take(6),
        [
            "O1: F1",
            "O1: F2",
            "O1: F3",
            "O1 replaced",
            "O1 ended: exited",
            "O2 started"
        ]
    );

    // The outpost exited with code 3 on its own; the listener and its
    // replacement shut down cleanly.
    reducer.finish(ShutdownSummary {
        clean: 2,
        exited: 1,
        killed: 0,
    });
    end(app);
}

/// The reducer held for longer than three ping intervals ends no outpost,
/// since the relay reads the outposts' pongs itself, and is reported as
/// its own stall, once.
fn a_reducer_held_past_three_ping_intervals_ends_no_outpost_and_is_reported_as_its_own_stall() {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    // Held from here, with a key waiting.
    reducer.press(1);
    let stall = reducer
        .stalls
        .recv_timeout(WAIT)
        .expect("the reducer's stall is reported");
    assert_eq!(stall.waiting, 1, "the key waited: {stall:?}");

    assert_eq!(reducer.take(1), ["key 1"]);
    // The outpost was not ended, and still answers.
    reducer.cue("O1", 7);
    assert_eq!(reducer.take(1), ["O1: C"]);

    reducer.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    end(app);
}

/// While the reducer is held, the relay keeps draining an outpost's pipe:
/// the outpost writes far more than the pipe holds, and each of its writes
/// completes, so nothing it sends waits behind a write in flight, which is
/// the only time its merge rules act; two value changes of one node both
/// arrive, in order.
fn while_the_reducer_is_held_the_relay_drains_every_frame_and_two_value_changes_both_arrive(
    signal: &Signal,
) {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    // Held from here.
    reducer.cue("O1", 6);
    signal.wait("everything the outpost wrote, up to Ready");
    let mut expected: Vec<String> = (0..8).map(|index| format!("O1: filler{index}")).collect();
    expected.extend([
        "O1: node 7 value one".to_owned(),
        "O1: node 7 value two".to_owned(),
        "O1: ready".to_owned(),
    ]);
    assert_eq!(reducer.take(expected.len()), expected);

    reducer.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    end(app);
}

/// A frame that is not a message has its outpost ended, and nothing that
/// outpost sent after it reaches the reducer but its end: a frame and its
/// `Ready`, read by the relay behind it while the reducer was held, are
/// dropped. It had said `Ready`, so its replacement starts after its end.
fn nothing_an_outpost_sent_after_a_frame_that_is_not_a_message_reaches_the_reducer_but_its_end(
    signal: &Signal,
) {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    // Held from here: the outpost writes the frame that is not a message,
    // then G1, then Ready, which the relay handles by telling the
    // listener.
    reducer.cue("O1", 8);
    signal.wait("the unreadable frame, G1 and Ready");
    assert_eq!(reducer.take(2), ["O1 ended: exited", "O2 started"]);

    reducer.finish(ShutdownSummary {
        clean: 3,
        exited: 0,
        killed: 0,
    });
    end(app);
}

/// What an outpost the supervisor has ended itself writes afterwards is
/// dropped: here a frame it writes as the retirement shuts it down, which
/// the relay reads before the pipe ends, and before the supervisor's
/// shutdown returns. It had not said `Ready`, so the app hears it replaced
/// before its end, and its replacement's start after.
fn nothing_an_outpost_writes_after_the_supervisor_ended_it_reaches_the_reducer() {
    let (app, pid) = application();
    let mut reducer = Reducer::start();
    reducer.with_outposts(&[pid]);

    reducer.cue("O1", 9);
    assert_eq!(
        reducer.take(3),
        ["O1 replaced", "O1 ended: exited", "O2 started"]
    );

    // The retirement's shutdown message had the outpost write Z, then
    // exit cleanly; `finish` asserts Z never arrived. The listener and the
    // replacement shut down cleanly too.
    reducer.finish(ShutdownSummary {
        clean: 3,
        exited: 0,
        killed: 0,
    });
    end(app);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == APPLICATION) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return;
    }
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // libtest's own options, such as a name filter, are accepted and
    // ignored: the binary runs its tests in turn.
    let signal = Signal::new();
    a_frame_written_before_a_key_is_reduced_before_it_and_one_written_after_it_after(&signal);
    println!(
        "test a_frame_written_before_a_key_is_reduced_before_it_and_one_written_after_it_after ... ok"
    );
    a_large_frame_delays_no_key_and_no_other_outposts_frame_and_its_outposts_next_frame_follows_it(
        &signal,
    );
    println!(
        "test a_large_frame_delays_no_key_and_no_other_outposts_frame_and_its_outposts_next_frame_follows_it ... ok"
    );
    a_closed_pipe_reaches_the_app_after_every_frame_before_it();
    println!("test a_closed_pipe_reaches_the_app_after_every_frame_before_it ... ok");
    while_the_reducer_is_held_the_relay_drains_every_frame_and_two_value_changes_both_arrive(
        &signal,
    );
    println!(
        "test while_the_reducer_is_held_the_relay_drains_every_frame_and_two_value_changes_both_arrive ... ok"
    );
    nothing_an_outpost_sent_after_a_frame_that_is_not_a_message_reaches_the_reducer_but_its_end(
        &signal,
    );
    println!(
        "test nothing_an_outpost_sent_after_a_frame_that_is_not_a_message_reaches_the_reducer_but_its_end ... ok"
    );
    nothing_an_outpost_writes_after_the_supervisor_ended_it_reaches_the_reducer();
    println!(
        "test nothing_an_outpost_writes_after_the_supervisor_ended_it_reaches_the_reducer ... ok"
    );
    a_reducer_held_past_three_ping_intervals_ends_no_outpost_and_is_reported_as_its_own_stall();
    println!(
        "test a_reducer_held_past_three_ping_intervals_ends_no_outpost_and_is_reported_as_its_own_stall ... ok"
    );
}
