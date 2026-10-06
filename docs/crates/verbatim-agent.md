# verbatim-agent

The in-guest test agent for the M2 VM harness. Runs inside the interactive
session of a Hyper-V guest, or reachable over loopback on a CI runner, and
is the only externally reachable doorway into that machine: host-side E2E
tests reach it over TCP to manage processes and to tunnel through to
Verbatim's own control-plane named pipe, which deliberately never listens
on the network itself (architecture section 10, decision D8).

The agent has no authentication: anyone who can reach its port can launch
and kill processes, read files, and drive Verbatim as the signed-in user.
It binds all interfaces by default, because the Hyper-V host reaches the
guest at an address not known ahead of time. That is acceptable only on an
isolated lab network or a disposable CI runner. On a development machine,
run it with `--bind-address 127.0.0.1`, as `docs/tooling.md` does, and stop
it when the suite is done.

Public API:

- `protocol` — the agent's own wire vocabulary, versioned separately from
  the control plane's (`AGENT_PROTOCOL_VERSION`, currently 6) and framed
  with the same newline-JSON helpers the control plane uses
  (`verbatim_control::protocol::write_message`/`read_message`), reused
  rather than reinvented. Deliberately a distinct vocabulary from
  `verbatim_control::protocol`: this crate's pids are raw OS process ids
  naming a process a test is driving (Notepad, `verbatim.exe` itself), not
  `verbatim_model::Pid`, which names an application Verbatim is
  *observing*. `Request`: `Hello` (must be first, refused outright on any
  version mismatch), `LaunchProcess`, `KillProcess`,
  `KillProcessesByName` (every process with a given image name, for
  sweeping target applications that hand off to another process),
  `BringToForeground` (wait for a visible top-level window of a given
  image name and bring it to the foreground past Windows' foreground lock:
  a Control tap and `SetForegroundWindow`, then the call attached to the
  foreground thread's input queue, then one injected Alt+Tab when a cloaked
  window such as the Start search host holds the foreground; protocol
  version 1), `ProcessStatus`, `SessionInfo`, `ReadFile`, `ListFiles`
  (the names of the files directly inside a directory, so a test can fetch
  logs it cannot name in advance; protocol version 2), `ForegroundInfo`
  (the foreground window and the visible top-level windows, each with its
  title, class, program, and whether it is cloaked), `CloseWindows` (an
  ordinary close request to every visible window whose title contains some
  text, then a wait for them to go), `WriteFile` (a small file, creating any missing parent directories,
  such as the
  document a test opens Notepad on), and `BringToForeground`'s optional
  title filter (protocol version 3; all from the `desktop` and `files`
  modules), `ReadFileChunk` (up to 8 MiB of a file of any size from a
  given offset, answered like `ReadFile` and empty past the end, so a
  client can copy a file too large for `ReadFile`, such as a scenario's
  video; protocol version 4), `SendKeys` (real OS key strokes through
  `verbatim_control::send_keys`, every name validated before any key is
  sent, so NVDA can be driven with no Verbatim running, for
  [the NVDA transcript](../nvda-transcript.md); protocol version 5),
  `TypeText` (a string typed as real key presses: each character mapped
  to its virtual key and Shift, Control, and Alt state in the keyboard
  layout of the foreground window's thread with `VkKeyScanEx`, and pressed
  with `SendInput`, modifiers down, key down and up, modifiers up, each
  event carrying the key's scan code in that layout, so a keyboard hook
  sees ordinary typing, which typed-character echo needs; a control
  character, such as a line break, or a character the layout cannot type
  fails the request before any key is sent, and named keys stay with
  `SendKeys`; from the private `typing` module; protocol version 6),
  `OpenControlTunnel`. `KillOutcome` makes
  "the process was already gone" a first-class non-error reply
  (`AlreadyExited`) distinct from `Terminated`, rather than an error.
  `LaunchProcess` inherits the launched child's stdio (uncaptured) by
  default; its `stderr_to` field, an `Option<String>` defaulted via
  `serde(default)` so an older client that omits it on the wire still
  deserializes, names a path the agent creates (truncating any existing
  content) and redirects both the child's stdout and stderr into, so a
  Verbatim that panics at launch leaves its message somewhere a host-side
  test or `cargo xtask vm logs` can actually read, instead of vanishing
  with the process.
- `server::serve(listener, pipe_name)` — the TCP accept loop, one thread
  per connection; blocking, so callers needing to do other work run it on
  a background thread.
- `session::current()` — session id, whether the process's window station
  is interactive, and the input desktop's name when it can be opened, read
  by `verbatim_process::session` (Verbatim makes the same check at its own
  startup). This is what `Request::SessionInfo` answers, and what the binary checks at
  its own startup: a screen reader test driven from a non-interactive
  session (the "session 0" problem WinRM and PowerShell Direct create) can
  never work, so the agent refuses to even bind a socket in that case,
  with a diagnosis printed instead of a downstream mystery.

Implementation notes: process management (private `process` module)
launches via `std::process::Command`, inheriting the agent's own
interactive session and stdio (never captured) — the reason this exists
at all rather than something reachable over WinRM or PowerShell Direct.
Lookup and termination act on raw pids via `OpenProcess`,
`TerminateProcess`, and `GetExitCodeProcess`, so a test can manage a
process it did not itself spawn. The agent also keeps the handle of every
child it launched until `ProcessStatus` reports that child's exit, and
answers `ProcessStatus` for such a child from that handle: without a
handle, an exited process's object, and with it the exit code, is gone the
moment it exits, so the exit code is now reported however long after the
child exited it is asked for. The first report of the exit releases the
handle. This is what lets the recording read ffmpeg's exit code after
muxing.
Each launched child also runs in a job object of its own: it is created
suspended, assigned to the job, and only then resumed, so everything it
starts is in the job from the start. `KillProcess` on a launched child
terminates its whole job. Before this, killing a launcher that runs the
real program as its own child, such as the Chocolatey shim that `ffmpeg`
on `PATH` often is, left the real program running: ten desktop captures
accumulated this way and slowed window activation enough to make the
Notepad scenario fail. The job does not kill on close, so the agent
exiting leaves its children as it always did.
`KillProcess`'s tolerance for an already-exited process handles two
distinct races: a pid that cannot be opened at all, and one that opens
fine but has already exited — the latter discovered because
`TerminateProcess` on a zombie process object returns access denied
rather than "not found," so the exit code is checked both before
attempting termination and after a failure.

The control-plane tunnel (private `tunnel` module) is the crate's most
intricate corner. Opening the pipe is split from running the relay so a
failure to open is reported in `OpenControlTunnel`'s own reply, before any
byte relaying begins. The pipe handle is opened with
`FILE_FLAG_OVERLAPPED` and uses the identical overlapped-read/write
pattern `verbatim_control::server`'s pipe transport uses on the other end
of the same kind of pipe, for the identical reason documented there: a
synchronous handle serializes reads and writes at the driver level even
across independent handles to the same instance, which would deadlock a
full-duplex relay needing one thread reading and another writing at once.
Two threads copy bytes, one per direction, and whichever direction finishes
first ends the other. It stops the pipe and shuts down the TCP socket. The
socket shutdown ends a blocked socket read. Stopping the pipe signals a
third event that every pipe read and write waits on alongside its own I/O
event; a waiting operation is then cancelled and waited out, and because
the stop event is never reset, an operation issued after the stop ends at
once too. That last case is the one cancellation alone missed: a direction
that was between reads when the other ended would block on its next read
until Verbatim closed the pipe. The tunnel's closing log line gives each
direction's byte count and why it ended, telling Verbatim closing the pipe
apart from a stop and from an error.
