# verbatim-process

Contained child processes (architecture section 1): what Core launches
outposts, the focus listener, and synthesizer hosts with. The code was
extracted from `verbatim-outpost`'s supervisor when the synthesizer host
(decision D18) needed the same containment, so both kinds of child are
launched the same way. It also reads the calling process's
session, which Verbatim and the test agent check at startup. It is a
Windows-specific crate and depends only on `tracing` and the Windows
bindings.

Public API:

- `ChildSpec` — what to launch: `exe`, the executable; `arguments`, a
  closure given the raw values of the child's two inherited pipe handles
  (the one it reads commands from, then the one it writes messages to)
  that returns the rest of the command line; `log_stem`, which names the
  child's log file `<log_stem>.log`; `memory_cap`, an optional
  per-process memory limit; and `from_child_buffer`, the buffer size in
  bytes to ask for on the pipe the child writes to, `0` for the system
  default.
- `launch(spec)` — launches the child over two new anonymous pipes and
  returns a `Contained` and a `ChildPipes`, or the error of whichever
  step failed (creating the job, creating the pipes, or spawning).
- `launch_with_ends(spec, ends)` — launches the child over pipes the
  caller made, typically with `overlapped::pipe`: `ChildEnds` holds the
  child's two ends (`commands`, the one it reads, and `messages`, the one
  it writes), which the child inherits and which are closed in Core once
  it has, whether or not the spawn succeeded; the caller keeps its own
  ends, overlapped or synchronous as it made them. Returns the
  `Contained`. `spec.from_child_buffer` is not used, since the caller's
  pipes have their buffers already. The containment, the inheritance list
  and the log are `launch`'s. The supervisor launches outposts and the
  listener with it, over `overlapped::pipe`s whose child ends are
  overlapped too (since communication step 6), and the relay
  floor's benchmark launches its children with it; the synthesizer host
  still uses `launch` until migration step 9 of
  `docs/design/communication.md` moves it.
- `Contained` — the launched child: `job`, the kill-on-close job handle,
  whose closing kills the process; `process`, the process handle (closing
  it only releases the reference); and `pid`, for logs. Dropping a
  `Contained` ends the child. `has_exited()` says whether the process has
  exited, once the system has finished ending it; `wait_for_exit(timeout)`
  waits up to `timeout` for it to; `exit_code()` reads its exit code once
  it has; and `kill()` ends everything in its job with
  `KILLED_EXIT_CODE` and returns once the child has ended, so a kill made
  on purpose, the supervisor's fallback when a child does not shut down in
  time, says so in the exit code, apart from a clean exit (0) or a crash
  (an exception code).
- `ChildPipes` — Core's ends of the two pipes: `to_child`, where Core
  writes commands, and `from_child`, where Core reads the child's
  messages. Both are `std::fs::File`.
- `launch_log_dir(exe_dir)` — this Verbatim launch's child log directory,
  `logs\<Verbatim's pid>` next to the executables.
- `prepare_launch_logs(exe_dir)` — prepares that directory at startup,
  best effort (see Logs below). The outpost supervisor calls it.
- `inherited_pipes(pipe_in, pipe_out)` — the child side: turns the two
  handle values its command line carried into the files it reads commands
  from and writes messages to. It checks that the two values differ and
  that each names an open pipe handle, answering an error otherwise, so a
  malformed command line fails the start. It is still `unsafe` because
  each value must be an inherited pipe handle the process owns and nothing
  else uses, which no check can prove. `verbatim-synth-host` uses it.
  Its ends must be synchronous: a `File` must never hold an overlapped
  handle.
- `inherited_pipe_handles(pipe_in, pipe_out)` — the same checks, giving
  the two handles as `OwnedHandle`s, for a child whose ends are
  overlapped (`launch_with_ends` over `overlapped::pipe` with
  `End::Overlapped`), which it uses only through `overlapped`'s and
  `transport`'s types. The outpost binary, as an outpost and as the
  listener, uses it, its ends going to `transport::ChildIo`.
  `inherited_pipes` is this with each handle wrapped in a `File`.
- `overlapped` (`docs/design/communication.md`, sections 4 and 6) —
  `pipe(flow, buffer, other)` makes a pipe with no name, as the standard
  library makes its child pipes (`NtCreateNamedPipeFile` on the named
  pipe file system's root with an empty name, the other end opened
  relative to the first with `NtOpenFile`): Verbatim's end overlapped,
  the other end overlapped or synchronous as `End` says, neither
  inheritable. `Port` is an I/O completion port: `associate(handle,
  key)` joins a handle and skips the port for operations that complete at
  once, `post(key)` wakes the waiting thread, and `wait` takes
  completions into `Completions`. `PipeIo` is one operation slot, a read
  (`read`, of its read size, or `read_up_to` a size given) or a write
  (`write`, or `write_with`, which puts the bytes in the slot's buffer
  and, when it puts none, writes nothing and is done at once) at a
  time, whose `OVERLAPPED` and buffer stay put, untouched
  and not freed, while its operation is in flight. The relay floor's
  benchmark (`verbatim-relay-floor`) and `transport` use it.
- `scheduling` (Dickson's decision of 2026-10-08;
  `docs/design/communication.md`, section 6.1) —
  `opt_out_of_power_throttling()` opts the process out of execution-speed
  power throttling (`EcoQoS`, `SetProcessInformation` with
  `ProcessPowerThrottling`), which Windows otherwise applies to a
  windowless background process; the power plan is untouched.
  `prioritize_current_thread(priority)` runs the calling thread at a
  `ThreadPriority` (`Normal`, `AboveNormal`, `Highest`, `TimeCritical`);
  `prioritize_role(role, priority)` does the same and logs a refusal as
  a warning, the thread carrying on as it was. The threads that only move
  messages run at `Highest`: the keyboard hook, verbatim.exe's relay
  thread and its reader for each outpost, the listener's and each
  outpost's I/O thread (and, for an outpost made in a test's process, its
  sender's finishing thread), the `WinEvent` threads, and both ends'
  synthesizer host readers. Each caller names its
  level once per role in a constant of its own. Every other thread keeps
  normal priority: raising the threads that compute did not pay when
  measured (`docs/design/communication.md`, section 11, step 3).
  `opt_out_of_power_throttling()` and
  `opt_current_thread_out_of_power_throttling()` opt the process or the
  calling thread out of execution-speed power throttling; only the relay
  floor's benchmark calls them, since the same measurement found no gain
  that would pay for the energy. Timer resolution is not raised.
- `transport` — the frames and senders of the pipes between verbatim.exe
  and its outposts and listener (`docs/design/communication.md`, section
  4). A frame is `Content-Length: ` and the body's length in ten digits, a
  blank line, then the body, at most `MAX_FRAME_BYTES` (32 MB); the fixed
  width lets a frame be encoded in place. `put_frame(buffer, put_body)`
  appends one to a buffer, so several can go in one write;
  `read_frame(reader)` reads one whole from a
  `BufRead`, the header then the body at once, returning `None` at the end
  of the stream between frames and an error for one cut short, a header
  that is not one, or a length over the limit (refused before any body is
  read). `EVENT_PIPE_BUFFER` is the event pipes' 64 KiB.
  `BlockingReader::new(pipe)` is a `Read` over an overlapped end for a
  thread that blocks on each read, as verbatim.exe's outpost reader
  threads do; a closed writer reads as the end of the stream.
  `read_within(buffer, timeout)` reads as `read` does but waits at most
  `timeout`, then cancels the read (a read may be cancelled; a write
  never is) and answers `None`; `available()` says how many bytes wait
  unread (`PeekNamedPipe`), which a test that has the writer's word that
  everything is written checks to know nothing more was said.
  `BlockingWriter::new(pipe)` is the matching `Write`, for a child that
  writes one message at a time and has no port, such as the supervisor
  tests' stand-in children.
  `PortReader::new(file)` reads an end associated with a `Port`, for the
  thread serving the port: `take(completion, port, frame)`, given its
  read's completion or a wake posted with its key, hands `frame` the body
  of each whole frame in order, and reads on while reads complete at
  once, until one waits (`Reading::Waiting`), sixteen have been made
  (`Reading::Yielded`, its key posted so it goes on after whatever else
  waits on the port), or the reading ends (`Reading::Ended`, `Ok` for the
  writer closing between frames, an error for a pipe that failed, ended
  inside a frame or brought something that is not one, or a frame
  `frame` refused). A read asks for 64 KiB while the next frame's length
  is unknown and for the whole rest of a frame once it is, so a large
  frame is read in one read into a buffer of its size. `Sender` writes an
  overlapped end: `send(item)` puts the item in the `Backlog`, a caller's
  policy (`push`, `pop`, `is_empty`, and `encode`, which appends an
  item's frame just before it is written, whichever thread writes it, so
  a stamp taken then is the write's and frames are encoded in the order
  they are written; an item that appends nothing is a marker, reached
  only once every write before it has completed), and writes what waits
  at once when no write is in flight, on the sending thread, so in the
  common case no other thread wakes. Every item passes through the
  backlog, so its policy sees every item in order. `send_all(items)`
  sends several together: with no write in flight their frames go in one
  buffer and one `WriteFile`, each whole with its own header, so the
  reader wakes once for all of them, and a batch larger than the pipe's
  buffer pends and is finished whole; with one in flight they wait as
  `send` would add them. `with_backlog(inspect)` runs `inspect` on the
  backlog under the sender's lock. A write that pended is finished by the
  thread serving the sender's port, which writes what waited, in order:
  `Sender::on_port(pipe, backlog, port, key)` shares a port, whose thread
  hands each completion carrying `key` to `complete(completion)`, and
  `is_finished()` says when no completion of the sender's will come again
  (verbatim.exe's relay serves every child's sender this way);
  `Sender::start(pipe, backlog, name, started)` gives the sender a port
  and a finishing thread of its own, named `name`, which first runs
  `started`, wakes only when a write that pended completes, and ends once
  the sender has finished; it is returned with the sender, so joining it
  waits for every write. `close()` takes nothing
  more, writes what waits, then drops the pipe so the reader sees its
  end. A failed write closes the sender (`SendError::Closed`); a backlog's
  refusal is `SendError::Refused`. A started frame is always finished:
  nothing cancels a write.
  `ChildIo` is a child's side of its two pipes (sections 4.6 and 6.3 to
  6.4). `ChildIo::new(commands, messages, backlog)` takes the child's two
  inherited overlapped ends and gives the I/O side and the `Sender` every
  thread of the child writes its messages through. `serve(sender,
  command)`, on the child's I/O thread, waits only on the port, hands
  `command` the body of each command frame in order, and finishes the
  sender's writes that pend, until `command` answers `Serving::Stop`
  (`Served::Stopped`) or verbatim.exe closes the command pipe
  (`Served::CommandsEnded`). `finish(sender)`, once the sender is closed,
  serves the port until every write in flight has completed and the pipe
  is closed, so a child never exits with a write in flight, which its
  exit would cancel, cutting the frame.
- `session::current()` — the calling process's `Session`: its session
  `id`, whether its window station is interactive
  (`interactive_window_station`), and the input desktop's name when it can
  be opened (`input_desktop_name`). A process in a non-interactive window
  station (the "session 0" that WinRM, PowerShell Direct, and services
  start processes in) can never be a working screen reader; Verbatim exits
  with a diagnosis then, and the agent refuses to bind its socket. A locked
  or secure input desktop is not refused.

Implementation notes, launching. `launch` creates the job, then the two
anonymous pipes (`launch_with_ends` is given its pipes instead), then asks `arguments` for the command line with the
child ends' handle values, and spawns the executable with
`CREATE_NO_WINDOW`, naming the job in a `PROC_THREAD_ATTRIBUTE_JOB_LIST`
attribute, so `CreateProcessW` creates the process already inside the
job. It is contained from its first instruction, and creation and
containment are one step: a Core that dies while launching a child
cannot leave it outside a job, running or suspended. (An earlier version
spawned the child suspended and then called `AssignProcessToJobObject`;
a Verbatim killed between the two steps left a suspended orphan.) Core
holds the only job handle, so however Core ends, the kernel closes the
handle and kills every child.

The job. It always carries `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. With a
`memory_cap` it also carries `JOB_OBJECT_LIMIT_PROCESS_MEMORY` with that
limit, past which the child's allocations fail. Outposts pass 200 MB;
synthesizer hosts pass none.

Pipes and inheritance. Each pipe is created with inheritable handles, and
the parent end is then marked non-inheritable, so only the child ends can
cross. The spawn goes further: `bInheritHandles` is true, but a
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, in the same attribute list as the
job, names exactly the handles this child
inherits, its two pipe ends and its log file. Without the list, two
launches running at once on different threads could each inherit the
other's pipe ends and keep that pipe open after its own child died, so
Core would never see the end of the stream. After the spawn, Core closes
its copies of the child ends whether the spawn succeeded or not, so that
the child's exit is seen as the end of the stream on `from_child`.
`from_child_buffer` is passed to `CreatePipe` for the child-to-Core pipe
only; the command pipe always uses the default. A small buffer makes a
child that streams data block on its writes when Core stops reading,
which is how the synthesizer host is paced.

Logs. Each child's standard output and error (and, harmlessly, standard
input) are a file handle opened for appending in this launch's log
directory, passed with `STARTF_USESTDHANDLES`, so the child's own
`tracing` output lands in `logs\<Verbatim's pid>\<log_stem>.log`. Append
mode means a child that crashed and its replacement share one file, and
appends at the end of file are atomic, so concurrent writers do not
interleave. Opening the log is best effort: if the directory or file
cannot be created, the child is spawned without redirection, never left
unspawned. The directory is per launch so the end-to-end harness collects
exactly one launch's logs, and a reused application pid never appends to
an older application's log. `prepare_launch_logs` empties this launch's
directory if an earlier process with the same pid left one, keeps only
the newest ten launch directories (by modification time), and deletes
any `.log` file the earlier flat layout left directly in `logs`. Removal
is partial for a launch still running, since the files its children hold
open stay until a later launch removes them.

Tests: the crate has no tests of its own. The synthesizer host's tests in
`crates/verbatim-synth-host/tests/hosting.rs` launch real children
through it, and every end-to-end run launches outposts, the listener,
and a synthesizer host through it.
