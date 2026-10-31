# verbatim-process

Contained child processes (architecture section 1): what Core launches
outposts, the focus listener, and synthesizer hosts (decision D18) with,
so every kind of child is launched and contained the same way. It also
reads the calling process's
session, which Verbatim and the test agent check at startup. And it holds
what every end of Core's children shares: the message layer over the
transport, the launch arguments, the processes held open by handle, a
child's start and end, and the alarm a thread waits on. It is a
Windows-specific crate and depends on `verbatim-model`, `serde`,
`serde_json`, `simdutf8`, `tracing`, `tracing-subscriber` and the Windows
bindings.

Public API:

- `ChildSpec` — what to launch: `exe`, the executable; `arguments`, a
  closure given the raw values of the child's two inherited pipe handles
  (the one it reads commands from, then the one it writes messages to)
  that returns the rest of the command line; `log_stem`, which names the
  child's log file `<log_stem>.log`; and `memory_cap`, an optional
  per-process memory limit.
- `launch_with_ends(spec, ends)` — launches the child over pipes the
  caller made, typically with `overlapped::pipe`: `ChildEnds` holds the
  child's two ends (`commands`, the one it reads, and `messages`, the one
  it writes), which the child inherits and which are closed in Core once
  it has, whether or not the spawn succeeded; the caller keeps its own
  ends, overlapped or synchronous as it made them, with the buffers it
  gave them. Returns the `Contained`, or the error of whichever step
  failed (creating the job or spawning). It is the only way this crate
  launches a child. The supervisor launches outposts and the listener
  with it, over `overlapped::pipe`s whose child ends are overlapped too;
  `verbatim-synth-hosted` launches the synthesizer host with it over
  `overlapped::pipe`s.
- `Contained` — the launched child: `job`, the kill-on-close job handle,
  whose closing kills the process; `process`, the process handle (closing
  it only releases the reference); and `pid`, for logs. Dropping a
  `Contained` ends the child. `has_exited()` says whether the process has
  exited, once the system has finished ending it; `wait(timeout)` waits
  up to `timeout` for it to and says whether it exited or the time ran
  out (`Waited`), or returns the error when the wait itself fails, and
  `wait_for_exit(timeout)` says only whether it exited; `exit_code()` reads its exit code once
  it has; and `kill()` ends everything in its job with
  `KILLED_EXIT_CODE` and returns once the child has ended, so a kill made
  on purpose, the supervisor's fallback when a child does not shut down in
  time, says so in the exit code, apart from a clean exit (0) or a crash
  (an exception code).
- `launch_log_dir(exe_dir)` — this Verbatim launch's child log directory,
  `logs\<Verbatim's pid>` next to the executables.
- `prepare_launch_logs(exe_dir)` — prepares that directory at startup,
  best effort (see Logs below). The outpost supervisor calls it.
- `inherited_pipe_handles(pipe_in, pipe_out)` — the child side: turns
  the two handle values its command line carried into the `OwnedHandle`s
  it reads commands from and writes messages to, whether its ends are
  synchronous or overlapped (`launch_with_ends` over `overlapped::pipe`).
  It checks that the two values differ and that each names an open pipe
  handle, answering an error otherwise, so a malformed command line fails
  the start. It is still `unsafe` because each value must be an inherited
  pipe handle the process owns and nothing else uses, which no check can
  prove. It makes both handles non-inheritable, as they arrive
  inheritable, so no process the child creates, or a library inside it
  creates, can hold Core's pipes open after the child exits. The outpost
  binary, as an outpost and as the listener, and the synthesizer host use
  it.
- `clock` (`docs/design/communication.md`, section 3.3) — the clock every
  process of a run stamps the times it compares across processes with:
  `now_us()`, the performance counter in microseconds plus one offset to
  the Unix epoch. `offset()` is the offset the launching process handed on
  in `VERBATIM_CLOCK_OFFSET_US`, or, in the first process, the system's
  clock less the counter, taken once; `hand_on()`, called first thing in
  verbatim.exe's `main`, puts it in the environment every child inherits.
  The counter is monotonic and the same in every process, so a key's
  press, an outpost's read and Core's receipt compare on one clock however
  the system's clock is set meanwhile. `Clock` and `ObservationClock`
  are the clocks a test may replace: `Instant::now`, and the shared
  clock's `now_us`, as `Arc`'d closures.
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
  and not freed, while its operation is in flight. The buffer is reused
  from one operation to the next; once a write is done, a buffer larger
  than `WRITE_KEPT_BYTES` (64 KiB) is given back down to it, so a large
  message, such as a terminal's whole document, is not held until the
  next. A slot runs its
  operations on any `OverlappedFile`: a `PortFile`, whose operations that
  wait finish through its port (`complete`), or an `EventFile`
  (`EventFile::new(handle)`), whose operations set its event as they
  finish, for a thread that waits on events rather than on a port; on
  one of those, `poll` asks with a zero wait whether the operation has
  finished, `wait` waits for it, and `cancel_read` cancels a read in
  flight and waits for it to end (only a read is ever cancelled).
  `transport` uses it.
- `event` (`docs/design/communication.md`, sections 7.3 and 7.4) —
  `Event`, an unnamed manual-reset event (`new`, `set`, `reset`,
  `is_set`), and `wait_any(events, timeout)`, which waits until any of
  up to 64 events is set and returns the lowest one's index, or `None`
  when the time runs out, its timeout rounded up to whole milliseconds as
  every wait in the crate rounds it, so a wait never ends before its
  deadline and a caller looping to one never spins: what the synth thread waits on for its host's
  next frame and its utterance's cancel together, and a synthesizer host
  for its write to Core and Core's next command.
- `port_queue` (`docs/design/communication.md`, sections 4.5 and 6.2) —
  how verbatim.exe's reducer takes its inputs: `port_queue()` gives a
  `Poster<T>`, cloned for every thread that posts, and a `PortQueue<T>`,
  the one taker, on a completion port of their own. `Poster::send(item)`
  posts the item as one packet, its box's address carried as the
  packet's `OVERLAPPED` pointer, with one `PostQueuedCompletionStatus`
  that never waits, and gives the item back when the taker has gone or
  the port refuses the packet; `Poster::is_open()` tells the two apart,
  true while the taker is there, so a refused item may be sent again.
  `PortQueue::recv`, `recv_timeout` and `try_recv` (a zero-timeout
  dequeue) take the items in the order the port received them, which is
  first in, first out across every poster, up to 64 per wait; `recv`
  answers `None` once every poster has gone and every item is taken,
  and `recv_timeout` answers a `Received`: `Item`, `TimedOut` while
  posters remain, or `Closed` at that end, so a caller with a deadline of
  its own never takes a wait that ran out for the queue's end.
  `Poster::progress()` gives the counts posted and taken (`Progress`,
  with `waiting()`), which the supervisor's relay reads to watch the
  reducer. Items still on the port when everything is dropped are
  dropped with it.
- `scheduling` (`docs/design/communication.md`, section 6.1) —
  `prioritize_current_thread(priority)` runs the calling thread at a
  `ThreadPriority` (`Normal`, `AboveNormal`, `Highest`, `TimeCritical`);
  `prioritize_role(role, priority)` does the same and logs a refusal as
  a warning, the thread carrying on as it was. The threads that only move
  messages run at `Highest`: the keyboard hook, verbatim.exe's relay
  thread, the listener's and each
  outpost's I/O thread (and, for an outpost made in a test's process, its
  sender's finishing thread), and the `WinEvent` threads. Each caller
  names its level once per role in a constant of its own. Every other thread keeps
  normal priority: raising the threads that compute did not pay when
  measured (`docs/design/communication.md`, section 11, step 3).
  Power throttling is left to Windows, since the same measurement found
  no gain from opting out that would pay for the energy. Timer resolution
  is not raised.
- `transport` — the frames and senders of the pipes between verbatim.exe
  and its outposts, listener and synthesizer hosts (`docs/design/communication.md`, section
  4). A frame is `Content-Length: ` and the body's length in ten digits;
  for a frame with a raw section, a line break and `Raw-Length: ` with
  the raw section's length in ten digits; a blank line; then the body, a
  JSON message, and after it the raw section, bytes whose places the
  message gives (a terminal's text and colours, a synthesizer host's
  samples). A frame without a raw section has no `Raw-Length` line. The
  body and raw section together take at most `MAX_FRAME_BYTES` (32 MB);
  the fixed width lets a frame be encoded in place. A reader hands a
  frame out as its `Parts`, `body` and `raw` (`None` when the header names
  no raw section, which differs from an empty one); `without_raw()` gives
  the body of a frame whose message may not have one and refuses one
  that does. `put_frame(buffer, put_body)` appends a frame without a raw
  section to a buffer, so several can go in one write, and
  `put_frame_with_raw(buffer, put_body, put_raw)` one with, the raw
  section appended right after the body in the same buffer, so the
  frame is still one write. `read_frame(reader)` reads one whole from a
  `BufRead` as an `OwnedFrame`, whose `parts()` give its body and raw
  section: the header, then the rest at once, returning `None` at the end
  of the stream between frames and an error for one cut short, a header
  that is not one, or lengths over the limit (refused before any body is
  read). `EVENT_PIPE_BUFFER` is the event pipes' 64 KiB.
  `BlockingReader::new(pipe)` is a `Read` over an overlapped end for a
  thread that blocks on each read, as the tests' harnesses and stand-in
  children do; a closed writer reads as the end of the stream.
  `read_within(buffer, timeout)` reads as `read` does but waits at most
  `timeout`, then cancels the read (a read may be cancelled; a write
  never is) and answers `None`; `available()` says how many bytes wait
  unread (`PeekNamedPipe`), which a test that has the writer's word that
  everything is written checks to know nothing more was said.
  `BlockingWriter::new(pipe)` is the matching `Write`, for a child that
  writes one message at a time and has no port, such as the supervisor
  tests' stand-in children.
  `EventReader::new(pipe, read_size)` reads an overlapped end associated
  with no port, for a thread that waits on its `event()` with other
  events: `take(frame)` hands `frame` the parts of every whole frame read
  so far, reading first, never waiting, and returns `Taken::Frames`, with
  no read in flight, so a reader that handles each frame before taking
  more reads no faster than it handles; `Taken::Waiting`, a read in
  flight whose finish sets the event; or `Taken::Ended`, as for
  `PortReader` below. It asks for `read_size` bytes while a frame's
  length is unknown and for the whole rest of a frame once it is.
  `EventWriter::new(pipe)` writes one write at a time: `start(put)`
  writes the frames `put` appends with one `WriteFile`, answering
  whether it completed at once; `poll()` and `finish()` ask, or wait,
  for one in flight, and `write(put)` does both. A write once started is
  never cancelled. These carry the synthesizer host's pipes, in both
  processes.
  `PortReader::new(file)` reads an end associated with a `Port`, for the
  thread serving the port: `take(completion, port, frame)`, given its
  read's completion or a wake posted with its key, hands `frame` the parts
  of each whole frame in order, and reads on while reads complete at
  once, until one waits (`Reading::Waiting`), sixteen have been made
  (`Reading::Yielded`, its key posted so it goes on after whatever else
  waits on the port), or the reading ends (`Reading::Ended`, `Ok` for the
  writer closing between frames, an error for a pipe that failed, ended
  inside a frame or brought something that is not one, or a frame
  `frame` refused). A read asks for 64 KiB while the next frame's length
  is unknown and for the whole rest of a frame once it is, so a large
  frame is read in one read into a buffer of its size.
  `take_bodies(completion, port, frame)` is the same turn, handing each
  frame out as a `FrameBody`: `Borrowed`, its parts lent for the call, or
  `Owned`, an `OwnedFrame` in the reader's own buffer given up (the whole
  frame, its header ahead of its body), which it is for a frame gathered
  over several reads that fills the buffer with nothing of the next frame
  after it, a large frame above all, so a caller that keeps the frame
  does not copy it. `parts()` reads either; `into_owned()` gives the
  `OwnedFrame`, copying only a lent one. `Sender` writes an
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
  `command` the parts of each command frame in order, and finishes the
  sender's writes that pend, until `command` answers `Serving::Stop`
  (`Served::Stopped`) or verbatim.exe closes the command pipe
  (`Served::CommandsEnded`). `finish(sender)`, once the sender is closed,
  serves the port until every write in flight has completed and the pipe
  is closed, so a child never exits with a write in flight, which its
  exit would cancel, cutting the frame.
- `message` — the message layer of the private protocol between Core and
  its children. The messages themselves, one enum per direction and role,
  their kinds, their merging and the types they carry, are
  `verbatim_model::protocol`'s ([verbatim-model](verbatim-model.md), "The
  wire protocol"); this module re-exports none of them. It holds the
  framing below and `peek`, which reads a frame's kind from its first
  bytes (`message_kind`) as a `Peeked`, the variants the supervisor's
  relay handles itself by name, so its frame handlers match it
  exhaustively and a test pins every variant's name.

  Each message is stamped as its write begins, by whichever thread writes
  it: `listener_written_at_us` on a listener's fact or menu end,
  `written_at_us` on an outpost's `Event`, `Reply`, or `Pong`; the listener
  adds `listener_handled_at_us`. verbatim.exe stamps its relay times on a
  routed fact or query as it writes it, and a ping or a hand-over notice
  to the listener with `sent_at_us`; the listener logs each hop at trace
  under `verbatim::stage`.

  Framing. Each message is one frame of `transport`, a `Content-Length`
  header and compact JSON, through `write_message` and `read_message`,
  for any type that implements `Message` (`put_frame` and `from_parts`):
  the four direction enums, the outpost's with the raw section below.
  `write_message` writes the whole frame in one `WriteFile`,
  `encode_message` gives the bytes, `put_message` appends them to a
  sender's buffer, and `decode_message` parses a frame's `Parts`. A
  message that carries a terminal's text (an event with a
  `TerminalSnapshot`, or a reply with a `TerminalSnapshot`,
  `TerminalHistory`, or `TerminalFound`) has a raw section (the private
  `message::raw` module; `docs/design/communication.md`, section 4.2): each
  text is UTF-8 there and a range in the JSON, `"raw":[start,length]` in a
  `TerminalText` and `"text":[start,length]` for a history's or match's
  text, and colour runs are 16-byte records of four little-endian `u32`s
  (column, length, foreground, background), the JSON giving
  `"colours":{"raw":[start,length],"rows":[...]}` with each row's run
  count. The module converts while encoding, borrowing from the message,
  and back while decoding, validating text with `simdutf8`; the model's
  types know nothing of it. A range outside the section, invalid UTF-8,
  records that do not fill their range, a section longer or shorter than
  its ranges, or a section on any other message is a protocol error. For
  example, a history whose text is `abc` has a header with
  `Raw-Length: 0000000003`, a body whose outcome is
  `{"Done":{"Text":{"TerminalHistory":{"text":[0,3],"above":0,"read":0}}}}`,
  and then the bytes `abc`. `message_kind(body)` reads a frame's variant
  name from its first bytes without parsing the rest; the relay reads it
  on every outpost frame.
- `arguments` — the launch arguments the supervisor gives its children,
  defined once for the end that builds them and the end that parses them:
  `OutpostOptions` (`remote_operations`, which `--classic-uia` turns off),
  `IGNORE_PIDS_ARG` (`--ignore-pids`) and `HOOKS_IN_ARG` (`--hooks-in`,
  with `format_hooks_in` and `parse_hooks_in` for its `pid:outpost`
  pairs).
- `held` — processes held open by handle, so a pid names one process and
  no other while held. `Target::open(pid)` holds a target application
  (`NotHeld` when it has exited or cannot be opened), with `has_exited`
  and `image_stem` (`stem_of` reads any process handle's lower-cased
  executable stem). `IgnoredProcesses` (`IGNORE_PIDS_ENV`,
  `VERBATIM_IGNORE_PIDS`; `from_env`, `hold`, `contains`, `list`,
  `parse_pids`) are the processes Verbatim ignores, named by the
  end-to-end harness: its own Windows Terminal. Each is held open; the
  supervisor starts no outpost for one and routes none of its facts, and
  the listener, given `--ignore-pids`, drops them before queuing.
- `child` — what a child does as it starts and ends: `init_tracing`
  installs its log's subscriber (`RUST_LOG`, `info` by default),
  `report_error!` writes to standard error without panicking, and
  `end_without_detach(code)` ends the process with `TerminateProcess`, so
  no DLL's detach code runs (`docs/architecture.md`, "Process lifetime").
- `alarm` — `Alarm`, a high-resolution waitable timer and an event waited
  on together by one thread (`docs/design/wake-ups.md`, item 3): `set`,
  `cancel`, `wake`, `wait` (whether the timer ended it) and
  `wait_for_event`. An outpost's mailbox and worker inbox and the
  listener's watchdog wait on one.
- `session::current()` — the calling process's `Session`: its session
  `id`, whether its window station is interactive
  (`interactive_window_station`), and the input desktop's name when it can
  be opened (`input_desktop_name`). A process in a non-interactive window
  station (the "session 0" that WinRM, PowerShell Direct, and services
  start processes in) can never be a working screen reader; Verbatim exits
  with a diagnosis then, and the agent refuses to bind its socket. A locked
  or secure input desktop is not refused.

Implementation notes, launching. `launch_with_ends` creates the job,
then asks `arguments` for the command line with the child ends' handle
values, and spawns the executable with
`CREATE_NO_WINDOW`, naming the job in a `PROC_THREAD_ATTRIBUTE_JOB_LIST`
attribute, so `CreateProcessW` creates the process already inside the
job. It is contained from its first instruction, and creation and
containment are one step: a Core that dies while launching a child
cannot leave it outside a job, running or suspended, as spawning it
suspended and then assigning it to the job could. Core holds the only job handle, so however Core ends, the kernel closes the
handle and kills every child.

The job. It always carries `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. With a
`memory_cap` it also carries `JOB_OBJECT_LIMIT_PROCESS_MEMORY` with that
limit, past which the child's allocations fail. Outposts pass 200 MB;
synthesizer hosts pass none.

Pipes and inheritance. The caller makes the pipes with every end
non-inheritable, as `overlapped::pipe` does; the child ends are marked
inheritable only for the `CreateProcessW` call. The spawn goes further: `bInheritHandles` is true, but a
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, in the same attribute list as the
job, names exactly the handles this child
inherits, its two pipe ends and its log file. Without the list, two
launches running at once on different threads could each inherit the
other's pipe ends and keep that pipe open after its own child died, so
Core would never see the end of the stream. The child, in turn, makes
its inherited ends non-inheritable as it takes them
(`inherited_pipe_handles`), so a process it creates cannot keep them
open either: the relay waits for every outpost's pipe to end at exit,
and a write end held open elsewhere would hold that wait. After the spawn, Core closes
its copies of the child ends whether the spawn succeeded or not, so that
the child's exit is seen as the end of the stream on the caller's end.
Each pipe's buffer is whatever the caller gave it; a small one on the
pipe from the child makes a child that streams data block on its writes
when Core stops reading, which is how the synthesizer host is paced.

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

Tests: the message layer's round trips and raw sections, the launch
arguments' round trip, and the held processes are unit-tested here. The
synthesizer host's tests in
`crates/verbatim-synth-host/tests/hosting.rs` launch real children
through it, and every end-to-end run launches outposts, the listener,
and a synthesizer host through it.
