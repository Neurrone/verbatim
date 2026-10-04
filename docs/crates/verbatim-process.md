# verbatim-process

Contained child processes (architecture section 1): what Core launches
outposts, the focus listener, and synthesizer hosts with. The code was
extracted from `verbatim-outpost`'s supervisor when the synthesizer host
(decision D18) needed the same containment, so both kinds of child are
launched the same way. It is a Windows-specific crate and depends only on
`tracing` and the Windows bindings.

Public API:

- `ChildSpec` — what to launch: `exe`, the executable; `arguments`, a
  closure given the raw values of the child's two inherited pipe handles
  (the one it reads commands from, then the one it writes messages to)
  that returns the rest of the command line; `log_stem`, which names the
  child's log file `<log_stem>.log`; `memory_cap`, an optional
  per-process memory limit; and `from_child_buffer`, the buffer size in
  bytes to ask for on the pipe the child writes to, `0` for the system
  default.
- `launch(spec)` — launches the child and returns a `Contained` and a
  `ChildPipes`, or the error of whichever step failed (creating the job,
  creating the pipes, or spawning).
- `Contained` — the launched child: `job`, the kill-on-close job handle,
  whose closing kills the process; `process`, the process handle (closing
  it only releases the reference); and `pid`, for logs. Dropping a
  `Contained` ends the child.
- `ChildPipes` — Core's ends of the two pipes: `to_child`, where Core
  writes commands, and `from_child`, where Core reads the child's
  messages. Both are `std::fs::File`.
- `launch_log_dir(exe_dir)` — this Verbatim launch's child log directory,
  `logs\<Verbatim's pid>` next to the executables.
- `prepare_launch_logs(exe_dir)` — prepares that directory at startup,
  best effort (see Logs below). The outpost supervisor calls it.
- `inherited_pipes(pipe_in, pipe_out)` — the child side: turns the two
  handle values its command line carried into the files it reads commands
  from and writes messages to. It is `unsafe` because each value must be
  an inherited pipe handle the process owns and nothing else uses.
  `verbatim-synth-host` uses it; the outpost binary still converts its
  handles itself.

Implementation notes, launching. `launch` creates the job, then the two
anonymous pipes, then asks `arguments` for the command line with the
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
