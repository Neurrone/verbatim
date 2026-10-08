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
  the control plane's (`AGENT_PROTOCOL_VERSION`, currently 9) and framed
  with the same newline-JSON helpers the control plane uses
  (`verbatim_control::protocol::write_message`/`read_message`). Its pids
  are raw OS process ids naming a process a test is driving, not
  `verbatim_model::Pid`, which names an application Verbatim observes.
  `Hello` must come first and is refused on any version mismatch. The
  requests, by what they are for:
  - Processes: `LaunchProcess` (see below), `KillProcess` (a launched
    child with everything in its job), `EndLaunched` (every process the
    agent launched that still runs, ended by its own handle: the
    pre-launch sweep, never by image name), `ChildProcesses` (the
    processes whose parent is a given pid, such as Verbatim's synthesizer
    host), `JobExits` (the processes that exited in a launched child's job,
    in order, with exit codes and whether the exit was abnormal: how a test
    finds that one of Verbatim's own processes ended while it ran),
    `WaitForExit` (on the process handle), and `ProcessStatus`.
  - The desktop: `ForegroundInfo` (the foreground window and the visible
    top-level windows, each with its handle, owning pid, title, class,
    program, and whether it is cloaked or minimized), `SetForeground`
    (`SetForegroundWindow` on a window, injecting no input),
    `WaitForWindow` with a `WindowCondition` (`Foreground`, optionally
    requiring the title to mark unsaved changes or not; `NotForeground`;
    `Absent`; `AllMinimized`, where a cloaked window, kept but not shown,
    counts as not shown), `MinimizeAll` (the taskbar's Show Desktop command,
    a wait until every window that can be minimized is, then the desktop
    brought to the foreground), and `CloseWindows` (an ordinary close
    request to every visible window whose title contains some text, then a
    wait for them to go).
  - State a test fixes its expectations from, read independently of any
    screen reader: `FocusedElement` (UI Automation's focused element: its
    name, its position and the size of its set, counted among its parent's
    children when UI Automation reports none, as for a Win32 list view's
    items, and whether it is selected), `FocusByAutomationId` (focuses the
    foreground window's element with that identifier with UI Automation's
    `SetFocus`, injecting no input, and answers how many children it has),
    `MisspeltWords` (the words of the focused text marked with UI
    Automation's spelling-error annotation, read word by word), and
    `KeyToggled` (whether a lock key is on).
  - Evidence: `WaitForFile` (a file appearing, checked again on each change
    notification for its folder), `CreateEvent` and `WaitForEvent` (a named
    manual-reset event a launched process sets when it reaches a point a
    test waits for, such as Verbatim being ready; the wait ends early if a
    given process exits first).
  - Files: `ReadFile`, `ReadFileChunk` (up to 8 MiB from an offset, for a
    file too large for `ReadFile`, such as a video), `WriteFile` (creating
    missing parent folders), `DeleteFile`, `ListFiles`, `ListFolders`, and
    `DeleteFolder` (a file or folder already gone counting as success).
  - Input: `SendKeys` (real key strokes through
    `verbatim_control::send_keys`, every name validated before any key is
    sent) and `TypeText` (each character mapped to its key and modifier
    state in the foreground window's keyboard layout with `VkKeyScanEx`
    and pressed with `SendInput`, with its scan code, so a keyboard hook
    sees ordinary typing). The agent numbers every key event it injects in
    its `dwExtraInfo` (`verbatim_input::harness`), the last of each key or
    character marked last, and `KeysSent` and `TextTyped` answer with the
    last number, which Verbatim reports once it has handled that input.
  - `SessionInfo` and `OpenControlTunnel`.

  `KillProcessesByName`, `BringToForeground`, and the Control tap and
  Alt+Tab it injected are gone: nothing is ended by its image name, and the
  agent never injects input a test did not ask for. `KillOutcome` makes
  "the process was already gone" a non-error reply (`AlreadyExited`).
  `LaunchProcess` gives the child no standard handles by default, as a
  program a user starts has none; `stderr_to` names a file the child's
  standard output and error go to, so a Verbatim that panics at launch
  leaves its message; `console_title` titles the console window a console
  program opens from its first frame (`STARTUPINFO`'s title), since the
  console host started directly otherwise shows its own path until the
  shell sets a title. Both fields default when omitted on the wire.
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
launches via `CreateProcessW`, inheriting the agent's own interactive
session — the reason this exists at all rather than something reachable
over WinRM or PowerShell Direct — but not its standard handles: those of
an agent started with its output redirected to a log made the console
host, started explicitly as `conhost.exe`, take them as a pseudoconsole's
input and output, open no window, and exit. The command line is quoted
by the C runtime's rules, and an environment override is set over the
agent's own environment. Before the child runs, it is allowed to take the
foreground (`AllowSetForegroundWindow`), as a program a user starts may,
and the reply says whether Windows let the agent allow it: it does while
the agent injected the last input, which every end-to-end scenario's keys
make so. The agent never injects a key to become eligible; a launch whose
window does not take the foreground fails the test. A console window
opened under the foreground lock raised its focus events while refused
the foreground, and Verbatim, which drops a refused window's events as
NVDA does, never announced it.

Every wait is event-driven and ends before the client's read timeout,
which the client sets ten seconds past the wait it asked for. Window
waits (private `wait` module) check their condition once, then install
out-of-context WinEvent hooks for the events that can change it (a
top-level window created, destroyed, shown, hidden, renamed, cloaked or
uncloaked, minimized or restored, or brought to the foreground) and check
again on each, blocking in `GetMessageW` between them, with a thread timer
bounding the wait. Process exits are recorded from each launched child's
job object completion port (private `jobs` module), file waits use
`FindFirstChangeNotification`, and event waits use the event's handle.
Nothing polls or sleeps.
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
