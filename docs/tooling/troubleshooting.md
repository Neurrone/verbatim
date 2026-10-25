# Troubleshooting

The traps that have cost this project time, each with its rule and the reason for it. The trap above all, the interactive-session rule, is in [the tooling guide](readme.md).

**The agent is unreachable after a checkpoint restore, but perfectly
healthy inside the guest.** Restoring a running checkpoint resumes the
guest with the DHCP lease it held when the checkpoint was taken, and
Hyper-V's Default Switch regenerates its NAT subnet on every host reboot —
so a golden checkpoint restored after the host rebooted leaves the guest
on an unroutable address: the agent listens, PowerShell Direct works, but
no host-side TCP probe can connect. Every restore path
(`vm restore`, `vm test --restore`) renews the guest's lease over PowerShell
Direct before waiting for the agent, so this fixes itself; if an agent
wait ever times out anyway, compare the guest's IP
(`Get-VMNetworkAdapter`) against the host's `vEthernet (Default Switch)`
subnet first.

**Starting the agent on a machine whose own Windows Terminal is in use.**
Start it from a shell with a hidden console of its own, so neither the
agent nor anything it starts is attached to a terminal you are using, for
example from PowerShell:
`Start-Process powershell.exe -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile','-Command',"& 'target\debug\verbatim-agent.exe' --bind-address 127.0.0.1 --port 44001 *> agent.log"`.
Stop it afterwards by the process ids of the agent and of that shell.

**A port band around 47xxx silently refuses binds on some machines.** Keep
the agent's port out of the band around 47600: on at least one development
machine every port there failed to bind with "address already in use"
while neither `Get-NetTCPConnection` nor
`netsh interface ipv4/ipv6 show excludedportrange` showed anything holding
it, almost certainly a reservation by security software, so
`verbatim_agent::protocol::DEFAULT_PORT` is 44001. If 44001 itself
turns out to be blocked on a given machine, override it everywhere
consistently: `verbatim-agent.exe --port <N>`, and the matching endpoint
value wherever the suite or `xtask vm` expects one (`xtask/src/vm/mod.rs`'s
`AGENT_PORT` constant, for the VM path).

**A locked desktop makes `SendInput` inject nothing.** `send-keys`,
`Scenario::send_keys`, and anything else that goes through real synthetic
input needs an unlocked, interactive desktop with a foreground window that
can actually receive it — a locked screen or an active screensaver silently
swallows the injection with no error anywhere in the chain (the control
plane still replies `Ok`, since sending the input is fire-and-forget from
its point of view). `Initialize-VerbatimHarness.ps1` disables the
screensaver and workstation locking by machine-wide policy for exactly this
reason, but a manual Win+L during a debugging session, or a plain (not
Enhanced Session) RDP disconnect that locks the workstation behind it,
reintroduces the problem. If gestures and status queries succeed but
nothing is ever heard or focus never seems to move, check the VM's console
is actually sitting at an unlocked desktop before looking anywhere else.

**A desktop with no real foreground window makes launched applications
silent.** Attention moves only when a window becomes the foreground, and
events from other windows are dropped, so on a desktop with no foreground
window (`GetForegroundWindow` returns null, and `SendInput` injects
nothing), or with the foreground held by a cloaked window such as the
Start search host's `Windows.UI.Core.CoreWindow`, the applications the
suite launches never take the foreground and the scenarios that launch
them fail. Before a local run, check that `GetForegroundWindow`
returns a window and that `DwmGetWindowAttribute` with
`DWMWA_CLOAKED` (14) reads 0 for it; if not, click into a real window on
the session's console first.

**Windows' foreground lock keeps launched applications behind.** A newly
launched application is refused the foreground unless the process that
allows it may set the foreground itself: Windows raises its foreground
event but keeps the previous window in front, and raises no second event
if the application is given the foreground later. Verbatim, like NVDA,
then rightly says nothing about it. The agent allows each launch the
foreground (`AllowSetForegroundWindow`), which Windows permits while the
agent injected the last input; every scenario's keys go through the agent,
so within a run it does. After you type or click on the machine yourself,
it does not until the agent has injected input again. No scenario needs
the right. The console host (`Scenario::launch_console`) is launched
without it (the launch's `withhold_foreground`): with it, the console
window opened minimized took the foreground by itself 300 to 500 ms
before the harness brought it forward, before its shell had started and
while its title still had the "Administrator: " prefix on GitHub's
elevated runner, and the harness's own bring-forward was then a repeat
Verbatim did not announce. File Explorer (`Scenario::open_folder`) is
launched without it too: the desktop's shell,
which holds the foreground as a scenario starts, opens the folder window
in front by its own right, and when it does not, the window opens
restored and inactive and is brought forward as msinfo32's is. It is not
opened minimized: a folder window the shell opens minimized takes the
foreground while minimized, and once restored its file list has no
keyboard focus, so arrow keys say nothing in NVDA or Verbatim (measured
2026-10-09; `docs/design/phase6-design.md`, "File Explorer opened without the
foreground right"). A Settings page (`Scenario::open_settings_page`) opens
minimized and inactive and is brought forward like the harness's own
windows. Run a scenario that sends keys first, such as
`lock_key_announcements`. Never tap keys by hand, or have a script send
keys, to fix it, and note that the harness itself never injects a key to
take the foreground: a window that refuses the foreground fails the test.
The same happens when anything else injects input, or anyone types,
between the agent's last key and a launch: another agent's or a test's
`SendInput` takes the right away from the agent just as a person's typing
does. Of Windows' other grounds for setting the foreground, none applies
to the agent at a scenario's start (it is not the foreground process, nor
started by it, and there is a foreground window, the desktop's), and the
foreground lock never expires on this machine
(`SPI_GETFOREGROUNDLOCKTIMEOUT` reads 2147483647 ms, where Windows'
default is 200000). So the harness does not rely on the right for its
own windows: `Scenario::launch_titled`, which launches mockapp and the
Windows Forms text box, opens the window minimized and inactive and then
restores it and sets it as the foreground, as Notepad's documents have
always been brought forward, which works whatever input came last. The
console host and Windows Terminal scenarios open theirs the same way
(`Scenario::launch_console` and `Scenario::launch_owning_window`): both
honor the launch's `SW_SHOWMINNOACTIVE`, the console host for the console
window it creates and Windows Terminal for its first window, which the
harness found minimized when it appeared, so Windows Terminal needs no
option of its own. `system_information_tree` launches msinfo32 through
`launch_titled` too, but msinfo32 ignores the show state and opens its
window restored and inactive. The agent's `SetForeground` therefore
minimizes a window that is restored and not in front, without
activating another window (`SW_SHOWMINNOACTIVE`, since `SW_MINIMIZE`
activates the next top-level window in Z order, which once was the
taskbar, and Verbatim announced its Start button), waits on window
events until it is minimized, and only then restores it and sets it as
the foreground: `SetForegroundWindow` on a restored background window
needs the agent's right, while a window restored from minimized takes the
foreground at once. Each such window is logged by the agent at info. A window restored from minimized and set as the
foreground is in front when `SetForegroundWindow` returns (37 of 37,
measured 2026-10-09), so `SetForeground` reads the foreground then.
Every `SetForeground` logs a "bring forward steps" line at info, with the
window's title and when each step finished from the call's start (the
minimize requested and observed, the restore, and
`SetForegroundWindow`'s answer), so a slow switch can be explained. Over
148 switches in 50 runs of `conhost_leave_flood` on 2026-10-11 the whole
switch took 150 to 282 milliseconds, and once 1.4 seconds; a scenario
that switches while speech plays must give the switch that long.
`docs/design/phase6-design.md`, "Test isolation and the foreground lock (2026-10-08)",
has the evidence.

**Windows 11 Notepad restores tabs from its previous session.** Notepad
keeps the tabs of a window that was closed, rather than whose tabs were
closed, and opens them again next time, adding a harness document as one
more tab. The harness therefore closes its documents as tabs, never by
closing Notepad's window, and closes every harness tab (named
`verbatim-e2e-`) it finds, including ones an earlier run left. A restored
tab whose file has been deleted shows "Cannot find the file" when Notepad
starts, and that dialog blocks closing tabs until it is dismissed. Any
other restored tab, one of your own, keeps Notepad running after the
harness tab closes, and the Notepad scenarios then fail saying so: close
those tabs (or set Notepad to start a new session) before running them.

**Crash dumps.** Any of Verbatim's own processes that exits unexpectedly
during a scenario fails it, read from the exits the agent records for
Verbatim's job. To keep a dump of the crash too, run
`vm\scripts\Enable-VerbatimCrashDumps.ps1` once from an elevated
PowerShell: it sets Windows Error Reporting's LocalDumps keys for
`verbatim.exe`, `verbatim-outpost.exe`, and `verbatim-synth-host.exe`,
writing dumps into `C:\ProgramData\Verbatim\CrashDumps`, and the
suite copies every dump written during a scenario into its artifacts.
Verbatim and the synthesizer host get minidumps. An outpost gets a
full-memory dump, with handle data and thread information (DumpType 0,
CustomDumpFlags 0x1826), so that the heap, and with it the UI Automation
event and handler group behind a crash inside UI Automation's own code,
can be read in cdb; expect each to be 100 to 300 megabytes, and at most
10 are kept. The dumps come from Windows Error Reporting rather than from
the outpost itself because a fail-fast, one of the two crashes seen so
far, ends a process without running any handler of its own. Windows holds
the crashed process until its dump is written, so the dump is complete
when the suite sees the exit. A machine set up before the outpost's dumps
became full ones needs the script run again. Until it has been run, an
unexpected exit still fails the run, without a dump. The script is never
run by the harness; Dickson runs it himself.

**Counting outpost crashes.** After each scenario the suite writes a line
to stderr, shown for a passing scenario as well as a failing one, naming
the outpost crashes that scenario saw, each with its pid, its exit code
(the exception code, such as `0xC0000005` for an access violation or
`0xC0000409` for a fail-fast) and its dump, followed by the running total
for the test process, as in "in this test run so far: 1 in 20
scenario(s)". A full suite runs in one test process, so its last
scenario's line is the suite's total. The same crashes are kept in each
scenario's `summary.txt`, as an `outpost_crashes` count and one
`outpost_crash` line each, and `cargo xtask vm test`, which runs each
scenario in a process of its own, prints them under each scenario of its
run summary and then the run's total. To count crashes across separate
runs, such as twenty runs of one scenario with `--exact`, read the
`outpost_crashes` lines of the archived summaries under
`target/e2e-artifacts/history/<scenario>`.

**Stray processes survive a failed run.** Cleanup closes what a scenario
opened, by process id or window, and fails the run when something will
not close; `Scenario`'s `Drop` impl does the same after a panic, and ends
Verbatim by its process id when it will not quit. Nothing is ever ended
by its image name, so a Notepad or terminal you have open yourself is left
alone. Notepad is opened on a harness document whose title holds
`verbatim-e2e-`, as NVDA's system tests open it on a uniquely named file,
and only when no Notepad window is open, so the window and its process are
the scenario's own; a Notepad scenario with a Notepad window already open
fails saying so. The harness closes its tab with Control+W, saving it
first when it has unsaved changes, waits for Notepad to exit, and deletes
the document, and the folders the harness made for Explorer and terminal
windows, each named with `verbatim-e2e-`; nothing without that marker in
its name is ever deleted.

Closing by title cannot prove nothing was left, since a window can lose
its title: File Explorer folder windows left open by killed runs were
retitled with the parent folder once their folders were deleted, escaped
every title-based close, and were later found on the desktop. So the
agent records every top-level window there is when a scenario starts,
before it opens anything, and the cleanup's last step waits, on window
events and up to the close timeout, for every shown window not among
those to be gone. A window still open fails the run with its title,
class, process id, and program, and is left open for you to see.

None of the cleanup runs if the test process itself is
killed outright (Ctrl+C, a CI job cancellation). The agent outlives the
test process and notices its connection end with the scenario's starting
windows still recorded, and notes the windows the scenario had open that
were not among them. The next launch then
sweeps: Notepad harness tabs are closed as tabs first, since a Windows 11
Notepad ended by its handle keeps its tabs and opens them again next
time, without their documents, behind a "Cannot find the file" dialog
that blocks closing them; then every process the agent launched that is
still running is ended by its own handle (`EndLaunched`); then each
window the abandoned scenario left that is still open is printed, with
its title, class, and process id, and closed by its handle, the launch
failing only if one will not close; then harness windows
are closed by title, and harness files and folders in `target/e2e-stage` are
deleted, before the desktop is minimized. Closing by handle there is
safe: only windows that were not open when the abandoned scenario
started, and were open when its test process ended, are closed, so a
window of your own, opened before the run or after you stopped it, is
never touched. The same happens to the windows a cleanup reported left
open. Restarting the agent forgets the recorded windows, and then only
the title sweep remains. A launch that fails after
opening its Notepad document closes that tab and deletes the document
before it reports the failure. Minimizing sends the taskbar's Minimize
All and waits for the taskbar to handle it, then logs and minimizes
directly, without activating another window, each window still restored: once, a document opened just before
stayed restored, and the suspected cause, not confirmed, is that Minimize
All skips a window the taskbar has not taken in yet. A stray guest-side process is
also cleared by restoring a checkpoint (`cargo xtask vm restore`).

**A gesture sent immediately after launch can silently do nothing.** The
control server starts (and so a tunnel connection succeeds) before
`verbatim-app`'s GUI thread finishes wiring its gesture handle, and
before the focus listener and the outpost reading Verbatim's own windows
are ready; a `SendGesture` that arrives then is dropped by the router, or
its menu is never seen, even though the control plane still answers `Ok`.
Verbatim's status reports when all of these are ready (`ready` in
`StatusInfo`). `Scenario::launch` waits for it without asking again and
again: it creates a named event through the agent and passes its name in
`VERBATIM_READY_EVENT`, and Verbatim sets the event once it is ready. A
hand-rolled script can do the same, or check `Status` before it starts,
rather than pausing for a fixed time.

**Speech frames that Verbatim wrote never reach the suite.** If it recurs,
read the agent log's `control tunnel closed` line first: this flake was
seen once and never explained, and that line gives each direction's byte
total and why it ended, telling a closed pipe from a stop and from an
error.

**A provider probe answers "no" after about three seconds, and the focus
is read through MSAA, spoken as "pane", or announced twice.** The
outposts and the focus listener both probe with the raw probe, one
`WM_GETOBJECT` that waits at most 3 seconds, logged at debug as `raw
probe` with its outcome (a timeout, a refusal, a gone window, or the
window's answer) and then `arbitration probed` with its `elapsed_ms`. A
"no" after about 3 seconds is the outcome `TimedOut`: the window's thread
did not answer in time, because the application was busy, starting up or
hung. The raw probe makes no UIA connection, so neither UIA fault that
gives `UiaHasServerSideProvider` a slow false "no" (`docs/uia-bugs.md`,
"A new connection fails while another process's remote operation is
queued" and "UiaHasServerSideProvider says no for a new conhost window
while another process has an event handler") reaches it.

**`measure_focus_explorer` times out on File Explorer's "Working on
it..." placeholder.** This is a known File Explorer fault of the measure
set: File Explorer sometimes gives the focus to the placeholder and raises
no later focus, so the wait for the item times out with no fault of
Verbatim's (2 of 3 runs on 2026-10-21,
`target/e2e-evidence/fixes-round-fifteen-2026-10-21/mfe-1-fail` and
`mfe-2-fail`; `docs/roadmap.md`, M4, "Follow-ups from M4.1", records it), and the run is
recorded and repeated.

**A window's title-bar information takes seconds during a parallel
mockapp run.** On the development VM the Desktop Window Manager does
about 30 ms of work for every hidden top-level window, about 200 ms for
every visible one, each made or destroyed anywhere in the session, and
about 110 to 120 ms for every desktop made (destroying one was not
measured), and does it in turn, so anything that
waits on it, such as `WM_GETTITLEBARINFOEX`, which the MSAA proxy sends,
can wait seconds while `cargo test -p mockapp` runs its tests in
parallel: up to 13 s measured on 2026-10-19, and on 2026-10-21 a
longest wait of 0.5 to 1.4 s in each of nine full runs, against 35 ms on
an idle machine, from a probe sending the message to a window of its own
every 100 ms. The mockapp runner makes one desktop per worker thread, not
one per test (169 desktops with 12 worker threads in a full run, not
219), and the longest wait did not change with it, so reusing desktops
does not measurably shorten the waits. That most of the work is the
tests' own windows is not measured apart: the probe's window was on the
interactive desktop, not a runner desktop; its wait includes the time
for the window's own thread to be scheduled and answer, and no control
message that does not go through the Desktop Window Manager was sent
beside it to tell that from processor contention; and the runs of each
arm were not alternated, the first of each block, started after a gap,
having the shortest longest wait. It matters because a test that
times or tightly bounds such a read fails from load elsewhere in the
session, not from its own code, so no test does (`docs/testing.md`,
"Waits are for evidence").

**Minimizing a console after a flood takes 30 to 45 seconds, and
`conhost_leave_flood` fails at "brings the first window back".** On the
development VM, which renders the desktop in software (WARP), the Desktop
Window Manager works through a flooded console's repaints for about 40
seconds after the flood, using up to two cores, and a minimize waits for
it in `win32kfull!DwmSyncNotifyMinimizing`; the console's own window thread
is idle meanwhile. It happens with no Verbatim running (27 to 45 s over
four runs, measured on 2026-10-24 with a trace and with Verbatim stopped),
so it is not Verbatim's to fix, and Dickson decided to leave the scenario's
failure on this VM unaddressed. The same switch took 150 to 282 ms on
2026-10-11, before a Windows update (installed 2026-10-13 and 14) replaced
the Desktop Window Manager's files; the VM has rebooted since, on
2026-10-20, and the backlog remains, so a restart does not clear it.

**A scenario run soon after a flood fails its setup with "no window
titled ... opened within 30s", the foreground being that very window.**
It is the same backlog: a new window waits for the Desktop Window Manager
before it is shown. The console host makes its window hidden and already
titled, and its window thread then waits on the manager's API port
(`\Sessions\<n>\Windows\DwmApiPort`, seen with the wait chain API) before
its first `ShowWindow`, for more than 30 seconds while the manager keeps
two cores busy for 40 seconds and more after a flood; Windows Terminal's
window does not appear in that time either. The hidden window can hold
the foreground, so the failure names it as the foreground window, marked
"hidden, not responding": the harness waits for a shown window, and none
was shown. Measured 2026-10-25: in chains of scenarios run after
`conhost_leave_flood`, 15 of 28 `conhost_` and `windows_terminal_` runs
failed their setup so, while 12 of 12 alternating runs of
`conhost_same_flood` and `windows_terminal_commands` started on an idle
manager passed. The manager stays as busy with the recording off
(`VERBATIM_E2E_RECORD=0`), so the recording is not the cause.

**`cargo xtask trace scenario` and `cargo xtask measure` refuse to start
while `VERBATIM_E2E_VERBATIM_EXE` is set.** Name the other build with
`--verbatim-exe <path>` instead, because the commands once discarded the
variable silently, so a comparison of two builds measured the current
build in both arms.
