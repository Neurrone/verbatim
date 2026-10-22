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
run it with `--bind-address 127.0.0.1`, as `docs/tooling/e2e.md` does, and stop
it when the suite is done.

Public API:

- `protocol` — the agent's own wire vocabulary, versioned separately from
  the control plane's (`AGENT_PROTOCOL_VERSION`, currently 21) and framed
  with the same newline-JSON helpers the control plane uses
  (`verbatim_control::protocol::write_message`/`read_message`). Its pids
  are raw OS process ids naming a process a test is driving, not
  `verbatim_model::Pid`, which names an application Verbatim observes.
  `Hello` must come first and is refused on any version mismatch. The
  requests, by what they are for:
  - Processes: `LaunchProcess` (see below), `WriteStdin` (text written
    to the standard input of a child launched with `stdin_piped`, such as
    the `q` that stops a recording's ffmpeg cleanly), `KillProcess` (a
    launched child with everything in its job), `EndLaunched` (every process the
    agent launched that still runs, ended by its own handle: the
    pre-launch sweep, never by image name), `ChildProcesses` (the
    processes whose parent is a given pid, such as Verbatim's synthesizer
    host), `JobExits` (the processes that exited in a launched child's job,
    in order, with exit codes and whether the exit was abnormal: how a test
    finds that one of Verbatim's own processes ended while it ran),
    `WaitForJobEmpty` (a wait, on the job's own messages, until no process
    in a launched child's job runs, answered with every exit in the job:
    how a test checks that Verbatim left none of its processes behind and
    how each ended), and `WaitForExit` (on the process handle, answered
    with whether the process still runs and, once it has exited, its exit
    code).
  - The desktop: `ForegroundInfo` (the foreground window and the visible
    top-level windows, each with its handle, owning pid, title, class,
    program, whether it is cloaked or minimized, whether Windows judges it not responding, `hung`, and whether it has `WS_EX_NOACTIVATE`, so Windows never brings it to the foreground, `no_activate`), `SetForeground`
    (restoring a window from minimized, minimizing it first, without
    activating another window, when it is restored and not in front, then `SetForegroundWindow`, injecting no
    input, so it works whatever input came last, and answering whether the
    window is in front once `SetForegroundWindow` returns; each call
    logs, at info, the window's title and when each step finished,
    measured from the call's start: the minimize requested and observed,
    the restore, and `SetForegroundWindow`'s answer, with whether it
    accepted the window and whether the window was then in front),
    `WaitForWindow` with a `WindowCondition` (`Foreground`, optionally
    requiring the title to mark unsaved changes or not; `Present`; `Absent`; `AllMinimized`), where a cloaked window, kept but
    not shown, counts as not shown in every condition, as it does for
    `CloseWindows`, which neither closes nor waits for one, `MinimizeAll` (the taskbar's Minimize All command,
    waited for, then a minimize sent directly to each window still
    restored, logged at info, since the command once left a window just
    opened restored, suspected but not confirmed to be because the taskbar
    had not taken it in yet; then a wait
    until every window that can be minimized is, then the desktop
    brought to the foreground; answered `Minimized`, which says which of
    the two held, since protocol version 12, so a scenario that cannot
    start names the window that kept the foreground rather than
    reporting windows left restored), `CloseWindows` (an ordinary close
    request to every visible window whose title contains some text, then a
    wait for them to go), `CloseWindow` (the same for one window by its
    handle, while it is still owned by the process and class it was found
    with, whatever its title has become, since protocol version 20, and
    refused since version 21 for a window in the connection's recorded
    starting windows, since every File Explorer folder window shares one
    process and class), and
    `TakeForeignTerminalWindows` (every Windows
    Terminal window shown since the last such request by a process outside
    the jobs of every process the agent launched, forgotten as it is
    reported: a console handed to the default terminal application, or
    anything else reaching the user's own Windows Terminal; the first
    request starts the watch, a `WinEvent` hook for windows shown on a
    thread of its own, and reports nothing).
  - A scenario's windows, proving it left none open whatever their titles
    have become (protocol version 15, private `scenario_windows` module):
    `RecordWindows` records every top-level window there is, shown,
    hidden, or cloaked, titled or not, each identified by its handle with
    its owning process and class, never its title, as the baseline of the
    scenario about to start. A window that existed before the scenario is
    never the scenario's, whatever its state, so a window of the owner's
    that was hidden or cloaked then and is shown later is neither reported
    nor closed. `WaitForNewWindowsGone`, at the scenario's cleanup, waits
    on window events for every shown window (visible, titled, and not
    cloaked, owned ones included, the windows every condition judges by,
    so the Settings app's closed, cloaked window has gone) that is not in the baseline to
    be gone, and answers `NewWindows` with those still shown, closing none.
    The baseline belongs to the connection that recorded it and lives in
    the agent, which outlives each test process. When that connection ends
    while the baseline is still held, because its test process was killed
    before its cleanup or the cleanup found windows open, the scenario is
    abandoned: the agent notes the shown windows not in the baseline at
    that moment, and `CloseAbandonedWindows`, at the next scenario's start,
    sends each of those still shown an ordinary close request by its
    handle, its identity checked again first, and answers
    `AbandonedWindows` with the windows it found and those that would not
    close. Only windows absent from the abandoned scenario's baseline and
    open when its connection ended are ever closed, so the owner's own
    windows, open before the scenario or opened after its test process
    ended, are never touched. `RecordWindows` is refused while a baseline
    is held or abandoned windows are not yet closed, and
    `CloseAbandonedWindows` first waits for a scenario still watched on
    another connection to end, since a killed test process's connection is
    seen to end only once the agent has answered the request it was
    making.
  - State a test fixes its expectations from, read independently of any
    screen reader: `FocusedElement` (the focused object through MSAA, with
    no UI Automation connection: the focus window's client object, the
    child its `accFocus` names, that child's name, whether it is selected
    when it can be, and, for a Win32 list view such as the desktop's, the
    focused item's position and the item count from the list view's own
    messages), `FocusByAutomationId` (focuses the foreground window's
    element with that identifier with UI Automation's `SetFocus`, injecting
    no input, and answers how many children it has), `ControlledItems`
    (of the first element the focused element's `ControllerFor` relation
    names, the set size its first child reports, `SizeOfSet`, and the
    names of the first children asked for, as a search box's suggestions),
    `NameByAutomationId` (the name of the foreground window's element with
    that identifier, read without focusing it, for a control whose name
    differs between machines), `FocusedControl` (the focused element's
    name and UI Automation control type, for a
    control that differs between machines, whose role word a test derives
    from the type), `MisspeltWords` (the
    words of the focused text marked with UI Automation's spelling-error
    annotation, read word by word), and `KeyToggled` (whether a lock key
    is on). The five requests that keep UI Automation, which need facts only
    it has, log how long each took, at warn from two seconds, so their slow
    answers are counted apart from Verbatim's
    (`docs/design/listener-without-connections.md`, section 4.5).
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
    `PressKey` and `ReleaseKey` (protocol version 16) press one key down
    and leave it down, and release it, each a numbered stroke of its own,
    so a key such as Control can be held while `SendKeys` sends others. A
    connection cannot press a key it already holds or release one it does
    not, and every key a connection holds is released when the connection
    ends, so a failed test or capture never leaves a modifier down.
    `SendKeysPaced` and `TypeTextPaced` (protocol version 17), for the
    end-to-end measure set, inject one stroke or character every
    `interval_ms`, timed by a high-resolution waitable timer (the private
    `pace` module): each is due a whole number of intervals after the
    first, so a late one does not delay the rest. Every name or character
    is checked before any is sent, each is numbered as `SendKeys` and
    `TypeText` number theirs, and `KeysSent` and `TextTyped` answer once
    the last is sent.
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
  `stdin_piped` (protocol version 14) gives the child a pipe as its
  standard input, whose writing end the agent keeps with the launch for
  `WriteStdin`: the recording stops ffmpeg by writing `q` to it, since
  killing ffmpeg cuts the end off its video.
  `ignore_foreign_terminals`, set for a launch of Verbatim (protocol
  version 11), names to the child, in `VERBATIM_IGNORE_PIDS`, every
  `WindowsTerminal.exe` process outside the agent's jobs and the
  `OpenConsole.exe` processes they host
  (`process::foreign_terminal_processes`, each opened and checked to run
  the image it was listed with), holds them open with the launch's entry
  so their pids name no other process meanwhile, and answers their pids in
  `Launched`'s `ignored`. A
  launch without a `console_title` is started with `CREATE_NO_WINDOW`: a
  console program, such as Verbatim, mockapp, ffmpeg, or the Windows
  PowerShell that shows the harness's Windows Forms text box, gets a
  console with no window (and a windowless console host process in its
  job), so Windows never hands its console to the default terminal
  application, which on a development machine is the user's own Windows
  Terminal; the flag changes nothing for a program with windows of its
  own. Only the console host the console scenarios start explicitly
  (`conhost.exe`, with a title) has a console window.
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
agent's own environment. A child given a capture file inherits that
file's handle and no other (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`): the
duplicates `TcpStream::try_clone` makes of a connection's socket are
inheritable, and a child that inherited one held the connection open
after the agent closed it, then reset it when the child exited. Before the child runs, it is allowed to take the
foreground (`AllowSetForegroundWindow`), as a program a user starts may,
and the reply says whether Windows let the agent allow it: it does while
the agent injected the last input, which every end-to-end scenario's keys
make so. A launch with `withhold_foreground` (protocol version 13) is
never allowed it, for a program the caller brings forward itself, as
File Explorer's folder window and the console host's window are. The
agent never injects a key to become eligible; a launch whose
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
child it launched, and `WaitForExit` reads such a child's exit code from
that handle: without a handle, an exited process's object, and with it the
exit code, is gone the moment it exits, so the exit code is reported
however long after the child exited it is asked for, until a launch made
five minutes or more after one that found the child, and everything in
its job, finished drops the handle. This is what lets the recording read
ffmpeg's exit code after muxing, through `wait_for_exit`.
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
