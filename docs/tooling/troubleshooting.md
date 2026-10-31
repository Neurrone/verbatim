# Troubleshooting

The traps that have cost this project time, each with its symptom, its cause, the rule, and the reason for the rule. The trap above all, the interactive-session rule, is in [the tooling guide](readme.md).

**The agent is unreachable after a checkpoint restore, but healthy inside
the guest.** Restoring a running checkpoint resumes the guest with the
DHCP lease it held when the checkpoint was taken, and Hyper-V's Default
Switch regenerates its NAT subnet on every host reboot, so the guest can
come back on an unroutable address: the agent listens and PowerShell
Direct works, but no host-side TCP probe connects. Every restore path
(`vm restore`, `vm test --restore`) renews the guest's lease over
PowerShell Direct before waiting for the agent; if an agent wait times
out anyway, compare the guest's IP (`Get-VMNetworkAdapter`) with the
host's `vEthernet (Default Switch)` subnet first, because that is the
usual cause.

**The agent, or what it launches, is tied to a terminal you are using.**
An agent started from your own Windows Terminal is attached to it, and so
is everything it starts. Start it from a shell with a hidden console of
its own, for example from PowerShell:
`Start-Process powershell.exe -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile','-Command',"& 'target\debug\verbatim-agent.exe' --bind-address 127.0.0.1 --port 44001 *> agent.log"`,
and stop it afterwards by the process ids of the agent and of that shell,
so that the scenarios never touch or close a terminal of yours.

**Binding a port around 47600 fails with "address already in use".** On
at least one development machine every port in that band refused binds
while neither `Get-NetTCPConnection` nor
`netsh interface ipv4/ipv6 show excludedportrange` showed anything holding
it, almost certainly a reservation by security software. So
`verbatim_agent::protocol::DEFAULT_PORT` is 44001. If 44001 is blocked on
a machine, override it everywhere consistently: `verbatim-agent.exe
--port <N>`, and the matching endpoint wherever the suite or `xtask vm`
expects one (`xtask/src/vm/mod.rs`'s `AGENT_PORT` for the VM path),
because a mismatch looks like an unreachable agent.

**Gestures and status queries succeed, but nothing is heard and focus
never moves.** A locked screen or an active screensaver makes `SendInput`
inject nothing, with no error anywhere in the chain; the control plane
still answers `Ok`, since sending input is fire-and-forget.
`Initialize-VerbatimHarness.ps1` disables the screensaver and locking by
machine-wide policy, but a manual Win+L or a plain (not Enhanced Session)
RDP disconnect locks the workstation again. Check that the console sits
at an unlocked desktop before looking anywhere else, because nothing
else reports it.

**Launched applications never take the foreground, and the scenarios
that launch them fail.** Attention moves only when a window becomes the
foreground, so on a desktop with no foreground window
(`GetForegroundWindow` returns null, and `SendInput` injects nothing), or
with the foreground held by a cloaked window such as the Start search
host's `Windows.UI.Core.CoreWindow`, nothing the suite launches is
spoken. Before a local run, check that `GetForegroundWindow` returns a
window and that `DwmGetWindowAttribute` with `DWMWA_CLOAKED` (14) reads 0
for it; if not, click into a real window on the session's console first.

**Windows' foreground lock keeps launched applications behind.** A newly
launched application is refused the foreground unless the process that
allows it may set the foreground itself: Windows raises its foreground
event but keeps the previous window in front, and raises no second event
if the application is given the foreground later, so Verbatim, like NVDA,
says nothing. The agent may allow a launch the foreground
(`AllowSetForegroundWindow`) only while it injected the last input, so
typing or clicking on the machine yourself, or another agent's or test's
`SendInput`, takes the right away, and the foreground lock never expires
on the development machine (`SPI_GETFOREGROUNDLOCKTIMEOUT` reads
2147483647 ms). The rules:

- The harness never relies on that right for a window it needs in front.
  `Scenario::launch_titled` (mockapp, the Windows Forms text box, Notepad's
  documents), `Scenario::launch_console` and
  `Scenario::launch_owning_window` (Windows Terminal) open the window
  minimized and inactive (`SW_SHOWMINNOACTIVE`) and then restore it and
  set it as the foreground, which works whatever input came last.
- The console host is launched without the right (the launch's
  `withhold_foreground`), because with it the window took the foreground
  by itself before its shell had started, and the harness's own
  bring-forward was then a repeat Verbatim did not announce.
- File Explorer (`Scenario::open_folder`) is launched without it and is
  not opened minimized, because a folder window the shell opens minimized
  has no keyboard focus in its file list once restored, so arrow keys say
  nothing in NVDA or Verbatim. The shell opens it in front by its own
  right, or restored and inactive, when it is brought forward as below.
- A Settings page (`Scenario::open_settings_page`) opens minimized and
  inactive and is brought forward like the harness's own windows.
- A window that opens restored and not in front, as msinfo32's does
  whatever show state it is given, is brought forward by the agent's
  `SetForeground`: it minimizes the window without activating another
  (`SW_SHOWMINNOACTIVE`, since `SW_MINIMIZE` activates the next window in
  Z order, once the taskbar, whose Start button Verbatim then announced),
  waits on window events until it is minimized, then restores it and sets
  it as the foreground, since a window restored from minimized takes the
  foreground at once. The minimize and restore are posted to the window's
  thread, so a hung window cannot block the agent, because synchronous
  `ShowWindow` calls failed `windows_terminal_up_typing` in four of six
  runs; `SetForeground` then waits for the window to be in front.
- Every `SetForeground` logs a "bring forward steps" line at info, with
  the window's title and when each step finished, so a slow switch can be
  explained. A switch takes about 150 to 300 ms and has taken 1.4 s, so a
  scenario that switches while speech plays allows that long.
- The harness never injects a key to take the foreground, and nobody
  taps keys by hand or from a script to fix a run, because a window that
  refuses the foreground is a failure the test must report. If the right
  was lost by your own typing, run a scenario that sends keys first, such
  as `lock_key_announcements`.

**Notepad opens with tabs from its previous session, or a "Cannot find
the file" dialog.** Windows 11 Notepad keeps the tabs of a window that
was closed, rather than whose tabs were closed, and opens them again next
time beside a harness document. The harness closes its documents as tabs,
never by closing Notepad's window, and closes every harness tab (named
`verbatim-e2e-`) it finds, including ones an earlier run left. A restored
tab whose file was deleted shows "Cannot find the file", which blocks
closing tabs until dismissed. A restored tab of your own keeps Notepad
running after the harness tab closes, and the Notepad scenarios then fail
saying so: close those tabs, or set Notepad to start a new session,
before running them, because the harness never closes what is not its
own.

**Crash dumps.** Any of Verbatim's own processes that exits unexpectedly
during a scenario fails it, read from the exits the agent records for
Verbatim's job. To keep a dump too, run
`vm\scripts\Enable-VerbatimCrashDumps.ps1` once from an elevated
PowerShell (Dickson runs it himself; the harness never does): it sets
Windows Error Reporting's LocalDumps keys for `verbatim.exe`,
`verbatim-outpost.exe`, and `verbatim-synth-host.exe`, writing dumps into
`C:\ProgramData\Verbatim\CrashDumps`, and the suite copies every dump
written during a scenario into its artifacts. Verbatim and the
synthesizer host get minidumps; an outpost gets a full-memory dump with
handle data and thread information (DumpType 0, CustomDumpFlags 0x1826),
100 to 300 megabytes each, at most 10 kept, so the heap behind a crash
inside UI Automation's own code can be read in cdb. The dumps come from
Windows Error Reporting rather than the outpost, because a fail-fast ends
a process without running any handler of its own. Windows holds the
crashed process until its dump is written, so the dump is complete when
the suite sees the exit. A machine set up before outpost dumps became
full ones needs the script run again.

**Counting outpost crashes.** After each scenario the suite writes a line
to stderr, for passing and failing scenarios alike, naming each outpost
crash it saw, with its pid, its exit code (the exception code, such as
`0xC0000005` for an access violation or `0xC0000409` for a fail-fast) and
its dump, followed by the running total for the test process ("in this
test run so far: 1 in 20 scenario(s)"); a full suite runs in one test
process, so its last line is the suite's total. Each scenario's
`summary.txt` keeps an `outpost_crashes` count and one `outpost_crash`
line each, and `cargo xtask vm test`, which runs each scenario in its own
process, prints them under each scenario and then the run's total. To
count across separate runs, such as twenty runs of one scenario with
`--exact`, read the archived summaries under
`target/e2e-artifacts/history/<scenario>`, because no single process
sees them all.

**Stray processes or windows survive a failed run.** Cleanup closes what
a scenario opened, by process id or window, and fails the run when
something will not close; `Scenario`'s `Drop` does the same after a
panic, and ends Verbatim by its process id when it will not quit. The
rules, each so that nothing of yours is touched:

- Nothing is ever ended by its image name, and nothing without
  `verbatim-e2e-` in its name is ever deleted.
- Notepad is opened only when no Notepad window is open, on a harness
  document whose title holds `verbatim-e2e-`, so its window and process
  are the scenario's own; its tab is closed with Control+W, saved first
  when changed, then Notepad's exit is awaited and the document, and the
  folders made for Explorer and terminal windows, deleted.
- The agent records every top-level window when a scenario starts, and
  cleanup's last step waits, on window events and up to the close
  timeout, for every shown window not among those to be gone; one still
  open fails the run with its title, class, process id and program, and
  is left open to see. Titles alone are not enough because a window can
  lose its title, as File Explorer windows do once their folders are
  deleted.
- When the test process is killed outright (Ctrl+C, a CI cancellation),
  no cleanup runs; the agent notices its connection end and keeps the
  abandoned scenario's windows. The next launch then closes Notepad
  harness tabs as tabs, ends every process the agent launched that still
  runs by its own handle (`EndLaunched`), prints and closes by handle each
  window the abandoned scenario left open, failing only if one will not
  close, closes harness windows by title, deletes harness files and
  folders in `target/e2e-stage`, and minimizes the desktop. Closing by
  handle is safe because only windows opened during the abandoned
  scenario are closed. Restarting the agent forgets the recorded
  windows, leaving only the title sweep.
- A launch that fails after opening its Notepad document closes that tab
  and deletes the document before reporting.
- Minimizing sends the taskbar's Minimize All, waits for the taskbar to
  handle it, then minimizes directly, without activating another window,
  each window still restored, because Minimize All has left a just-opened
  window restored.
- A stray guest-side process is also cleared by restoring a checkpoint
  (`cargo xtask vm restore`).

**A gesture pressed just after launch does nothing, though the control
plane answers `Ok`.** The control server starts before `verbatim-app`'s
GUI thread is up, and before the focus listener and the outpost reading
Verbatim's own windows are ready, so an early gesture's menu is never
shown or never seen. Wait for `ready` in
`StatusInfo`: `Scenario::launch` passes a named event in
`VERBATIM_READY_EVENT`, which Verbatim sets once it is ready, and a
hand-rolled script can do the same or check `Status`, rather than
pausing for a fixed time, because a fixed pause is either too short or
wasted.

**Speech frames Verbatim wrote never reach the suite.** The cause is
unknown. Read the agent log's `control tunnel closed` line first: it
gives each direction's byte total and why the tunnel ended, telling a
closed pipe from a stop and from an error.

**A provider probe answers "no" after about three seconds, and the focus
is read through MSAA, spoken as "pane", or announced twice.** The
outposts and the focus listener probe with the raw probe, one
`WM_GETOBJECT` that waits at most 3 seconds, logged at debug as `raw
probe` with its outcome (a timeout, a refusal, a gone window, or the
window's answer) and then `arbitration probed` with its `elapsed_ms`. A
"no" after about 3 seconds is the outcome `TimedOut`: the window's thread
did not answer in time because the application was busy, starting up or
hung. Look at the application, not at UIA, because the raw probe makes no
UIA connection, so neither UIA fault that gives
`UiaHasServerSideProvider` a slow false "no" (`docs/uia-bugs.md`, "A new
connection fails while another process's remote operation is queued" and
"UiaHasServerSideProvider says no for a new conhost window while another
process has an event handler") reaches it.

**`measure_focus_explorer` times out on File Explorer's "Working on
it..." placeholder.** File Explorer sometimes gives the focus to the
placeholder and raises no later focus, so the wait for the item times
out with no fault of Verbatim's (`docs/roadmap.md`, M4, "Follow-ups from
M4.1"). Record the run and repeat it, because the measure is valid only
when the item is focused.

**A window's title-bar information takes seconds during a parallel
mockapp run.** The Desktop Window Manager does work for every top-level
window made or destroyed anywhere in the session (on the development VM
about 30 ms for a hidden one, 200 ms for a visible one, and 110 to 120 ms
for every desktop made) and does it in turn, so anything that waits on
it, such as `WM_GETTITLEBARINFOEX`, which the MSAA proxy sends, can wait
seconds while `cargo test -p mockapp` runs in parallel (up to 13 s
measured, against 35 ms idle). Reusing desktops per worker thread did not
measurably shorten the waits. No test times or tightly bounds such a read
(`docs/testing.md`, "Waits are for evidence"), because it would fail from
load elsewhere in the session rather than from its own code.

**Minimizing a console after a flood takes 30 to 45 seconds.** The
Desktop Window Manager's backlog on the development VM; the failure it
causes in `conhost_leave_flood` is in [the known failures](../flaky-tests.md).

**A scenario run soon after a flood fails its setup with "no window
titled ... opened within 30s", the foreground being that very window,
marked "hidden, not responding".** It is the same backlog: the console
host makes its window hidden and already titled, and its window thread
waits on the manager's API port (`\Sessions\<n>\Windows\DwmApiPort`)
before its first `ShowWindow`, for more than 30 seconds after a flood;
Windows Terminal's window does not appear in that time either. The
hidden window can hold the foreground, and the harness waits for a shown
window. It affects any `conhost_` or `windows_terminal_` scenario run
straight after `conhost_leave_flood`, `conhost_full_history_flood` or
`windows_terminal_full_history_flood`, recording on or off. The harness
does not wait for the manager (Dickson, 2026-10-26); instead, before a
scenario's setup, wait until the counter `\Process(dwm)\% Processor Time`
reads under 130 percent, for example with
`(Get-Counter '\Process(dwm)\% Processor Time').CounterSamples.CookedValue`,
because `Get-Process` cannot read the manager's protected process.

**`cargo xtask trace scenario` and `cargo xtask measure` refuse to start
while `VERBATIM_E2E_VERBATIM_EXE` is set.** Name the other build with
`--verbatim-exe <path>` instead, because the commands once discarded the
variable silently, so a comparison of two builds measured the current
build in both arms.
