# verbatim-agent

The in-guest test agent for the M2 VM harness. Runs inside the interactive
session of a Hyper-V guest, or reachable over loopback on a CI runner, and
is the only externally reachable doorway into that machine: host-side E2E
tests reach it over TCP to manage processes and to tunnel through to
Verbatim's own control-plane named pipe, which deliberately never listens
on the network itself (architecture section 10, decision D8).

Public API:

- `protocol` — the agent's own wire vocabulary, versioned separately from
  the control plane's (`AGENT_PROTOCOL_VERSION`, currently 0) and framed
  with the same newline-JSON helpers the control plane uses
  (`verbatim_control::protocol::write_message`/`read_message`), reused
  rather than reinvented. Deliberately a distinct vocabulary from
  `verbatim_control::protocol`: this crate's pids are raw OS process ids
  naming a process a test is driving (Notepad, `verbatim.exe` itself), not
  `verbatim_model::Pid`, which names an application Verbatim is
  *observing*. `Request`: `Hello` (must be first, refused outright on any
  version mismatch), `LaunchProcess`, `KillProcess`, `ProcessStatus`,
  `SessionInfo`, `ReadFile`, `OpenControlTunnel`. `KillOutcome` makes
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
  is interactive, and the input desktop's name when it can be opened. This
  is what `Request::SessionInfo` answers, and what the binary checks at
  its own startup: a screen reader test driven from a non-interactive
  session (the "session 0" problem WinRM and PowerShell Direct create) can
  never work, so the agent refuses to even bind a socket in that case,
  with a diagnosis printed instead of a downstream mystery.

Implementation notes: process management (private `process` module)
launches via `std::process::Command`, inheriting the agent's own
interactive session and stdio (never captured) — the reason this exists
at all rather than something reachable over WinRM or PowerShell Direct.
Lookup and termination act on raw pids via `OpenProcess`,
`TerminateProcess`, and `GetExitCodeProcess` rather than tracking handles
from launch, so a test can manage a process it did not itself spawn.
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
Two threads copy bytes in each direction; whichever direction finishes
first cancels the pipe's pending I/O (`CancelIoEx`) and shuts down the TCP
socket, so the other thread also unwinds instead of hanging.
