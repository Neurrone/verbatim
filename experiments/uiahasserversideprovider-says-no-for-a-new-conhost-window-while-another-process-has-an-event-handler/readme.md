# Experiments: UiaHasServerSideProvider says no for a new conhost window while another process has an event handler

These experiments establish the fault recorded in `docs/uia-bugs.md` under [UiaHasServerSideProvider says no for a new conhost window while another process has an event handler](../../docs/uia-bugs.md#uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler). They ran on 2026-10-18 on Dickson's x64 machine (Windows 11, build 26200), debug builds, with neither NVDA nor Verbatim running. They use the collide tool described in [the previous fault's experiments](../a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md), extended for each experiment.

Terms used here:

- The probe is `UiaHasServerSideProvider`; a "no" is its false answer after about 3 seconds for a window that has a provider.
- The raw probe is one `SendMessageTimeoutW(hwnd, WM_GETOBJECT, 0, UiaRootObjectId)`, `UiaRootObjectId` sign-extended as UIA sends it; an answer of `0x10000` means the window has a provider.
- `efhbcd` is `ElementFromHandleBuildCache` with the name, process id and provider description cached, so one call shows whose provider the element has: conhost's own ("Microsoft Console Host Window", conhost.exe) or UIA's MSAA proxy.
- U is the client process that probes and then calls `efhbcd`; R is a separate process that sends the raw probe at the same moment and then `efhbcd`; V is one process that sends only the raw probe and then `efhbcd`. L is a focus listener stand-in, T an idle client with one tooltip handler, O an outpost stand-in.
- The delay of a trial is the time from the moment the orchestrator found the new console's window (`FindWindowW` on its title) to the call.

## The trigger seen in the end-to-end suite

`kept-runs/` holds three failing runs of `conhost_two_windows` from 2026-10-17 (08-04-30, 08-12-20 and 08-13-47), each with Verbatim's listener and outpost logs, Core's flight recorder and the timeline. In each, the failing probe came from a spare outpost newly given the second console's conhost (outpost-spare-5 or spare-6), sent 1 to 3 ms after the window's foreground event, and answered no after about 3 s. In 08-12-20 and 08-13-47 the listener made no UIA call on that conhost while the probe was pending; in 08-04-30 it probed and read the focus about 125 ms after the probe started and got its answers at once. 08-13-47 already had the per-application lock. So a queued remote operation is not needed for this failure.

## e37: probing a console window while the console host starts

The tool is `collide-tool-e37.patch`, the full diff against main at f9fb32f7: the e33 tool plus the orchestrator steps `console` (starts `conhost.exe cmd.exe /d /k` with the title given as `lpTitle` and no standard handles, as the end-to-end agent launches a console, in a job that kills both when closed, then spins on `FindWindowW` for the `ConsoleWindowClass` window with that title) and `closeconsole`, the client command `efhbcd`, and wait steps whose prefix may have several words. `e37/run-e37.py` runs every trial as its own `collide run` with a fresh console and fresh client processes, each of which had created its UIA client and connected to nothing, the sets interleaved trial by trial; `e37/e37-summary.py` writes `e37-summary.txt`. The actual delay was within 0.4 ms of the one asked for in all but 7 trials (2.2 to 10.3 ms late).

- The window: `FindWindowW` found it 17.8 to 55.3 ms after `CreateProcessW` returned, not yet visible in 189 of 190 trials. A probe sent at once waits for conhost to start pumping: at 0 ms the first call took 17.0 to 39.0 ms, against 3.3 to 11.1 ms in the controls.
- `e37-parallel`, 60 trials (12 at 0 ms, 48 spread over 0 to 100 ms): R's raw probe and U's probe at the same moment. The raw probe answered `0x10000` in all 60 (3.1 to 29.6 ms) and the probe true in all 60 (8.5 to 39.0 ms); each process's `efhbcd` returned conhost's own element in all 60, U's in 0.9 to 4.9 ms and R's in 7.3 to 30.4 ms.
- `e37-rawfirst`, 60 trials, same delays: V sent the raw probe, then the probe, then `efhbcd`: `0x10000` (3.1 to 26.1 ms), true (8.4 to 24.1 ms) and conhost's own element (0.9 to 4.4 ms) in all 60.
- `e37-alone`, 60 trials, same delays: U alone. True in all 60 (11.4 to 38.6 ms) and conhost's own element (0.9 to 71.1 ms).
- Controls at 2,000 ms, 5 trials each of the parallel and raw-first shapes: all 10 correct.

Result: with no other UIA client running, 180 early probes never answered no. The hypothesis that probing during conhost's start alone causes the fault is not confirmed. In 4 parallel trials the script's wait for R's `efhbcd` timed out because R's line had been consumed by an earlier wait; every call had completed by then.

## e38: an early console probe with the scenario's other UIA clients present

The tool is `collide-tool-e38.patch`, the full diff against main at f9fb32f7: the e37 tool plus `e38.rs` and these steps:

- `collide listen <tag> <title prefix> [mode]`: a focus listener built from Verbatim's own code: `verbatim_uia::Registration` over the desktop with the listener's parts (the focus handler, then one group on the root with selection, menu and notification parts), a global out-of-context WinEvent hook, and a reading thread that, for each UIA focus event from a console window whose title has the prefix, calls the probe and `console_reports_formatting` once per window, then the listener's remote focus read (`verbatim_uia_rops::focus_ancestry`). Mode `passive` makes the same registrations and hook and reads nothing.
- Client commands `spare` (a registration over nothing, as a spare outpost holds), `hookpid <pid>` (an out-of-context WinEvent hook on that process, as an outpost installs when given an application), `outpost` (`ElementFromHandleBuildCache` on the target, a registration with text events over its subtree, and the hook on its conhost) and `root`.
- Orchestrator step `fgright`, so that each console takes the foreground as in the scenario; the console step also reports when the window became visible and the foreground window, and sets `$CONPID`.

Every trial is one `collide run` with fresh consoles and processes (`e38/run-e38.py`; `e38/e38-summary.py` writes `e38-summary.txt`, marking a trial odd for a no, a proxy, a raw answer other than `0x10000`, a call over 1 s, or a call with no answer before the trial ended). The second console took the foreground a median 9.9 ms after its window was found and became visible a median 21.7 ms after; a raw probe sent at 0 ms was answered a median 25.3 ms later, so at 0 ms the probes reach conhost before its window thread first pumps.

Phase 1, 40 trials per set, delays as in e37 (8 at 0 ms, 32 over 0 to 100 ms):

- `e38-alone` (e37-parallel with the foreground, no other client): 0 of 40.
- `e38-listener` (L listening before the console starts): U's probe answered no after 3,022 to 3,036 ms in 5 of 40.
- `e38-outpost` (console A open with O attached; U behaves as the failing spare did, hooking B's conhost and then probing): no after 3,028 and 3,039 ms in 2 of 40. In a third trial (delay 27 ms) U's probe answered true but R's `efhbcd` had not answered 1.57 s later, when the trial closed the console.
- `e38-both`: no after 3,035 to 3,046 ms in 4 of 40.

Of these 11 noes, 7 came in the 24 trials at 0 ms and 4 in the 96 at other delays (0.3, 7.6, 46.2 and 77.3 ms).

Phases 2 and 3, every trial at 0 ms, to find the smallest trigger:

- `e38-alone-zero`: 0 of 30.
- `e38-listener-zero`: no in 4 of 30; in a fifth, U's probe was true but U's `efhbcd` returned the MSAA proxy after 2,012 ms while L's probe, console check and remote focus reads ran (the previous fault, which the lock covers).
- `e38-listener-passive` (L registers and hooks, reads nothing): no in 10 of 30.
- `e38-tooltip` (T in place of L: one idle `ToolTipOpened` handler over the root's subtree): no in 7 of 30.
- `e38-tooltip-nohandler` (T fetches the root and registers nothing): 0 of 40.
- `e38-listener-uonly` (full L, U alone, no R): no in 2 of 30. `e38-passive-uonly`: 4 of 40.
- `e38-rawpath` (passive L; V sends only the raw probe and then `efhbcd`; no process calls the probe): 0 of 40, the raw probe `0x10000` in 22.3 to 30.6 ms and conhost's own element in 12.4 to 29.5 ms every time.

Across the 38 trials with a no (3,022 to 3,058 ms), U's own `efhbcd` straight after it returned conhost's own element in 1.2 to 2.5 ms in all 38; in the 32 with R, R's raw probe answered `0x10000` (0.1 to 42.9 ms) and R's `efhbcd` returned conhost's own element (9.9 to 35.9 ms) in all 32. Every raw probe in e38, 360 in all, answered `0x10000`.

## e39: what the probes send, and the lookup as the probe

The tool is `collide-tool-e39.patch`, the full diff against main at f9fb32f7: the e38 tool plus `collide-hook`, a scratch DLL whose `WH_CALLWNDPROC` and `WH_CALLWNDPROCRET` hooks log every `WM_GETOBJECT` and registered message a hooked thread receives, with the full 64-bit lParam and the result; the orchestrator step `hook <pid | hwnd:h>`; and the client commands `rawprobe2 [lparam]` (`SendMessageTimeoutW` with `SMTO_ABORTIFHUNG`, `SMTO_BLOCK` and `SMTO_ERRORONEXIT`, 3,000 ms), `lookup` (`ElementFromHandleBuildCache` with the outpost's node request, plus the provider description and process id; every e39 client is set up through `verbatim_uia::Uia::new`, so it has Verbatim's process-wide settings, including the 5-second connection timeout) and `classify <pid>...`. `e39/run-e39.py` runs the sets, each trial a fresh `collide run`; `e39-msg-summary.py`, `e39-summary.py` (written to `e39-summary-zero.txt` and `e39-summary-settled.txt`) and `e39-classify-summary.py` (`e39-classify-summary.txt`) summarise them. The trigger, where named, is e38's passive L, running before the console starts.

Experiment 1, the messages (hook logs in `e39/hooklogs/`):

- Against collide's server window (`e39-msg-server`), each call from a fresh client: the probe and the lookup send the same sequence: a `WM_GETOBJECT` with a nonzero wParam (a per-process value that rises by one per call) and lParam `0xffffffffffffffe7`, answered `0x10000`; a `WM_GETOBJECT` with wParam 0 and the same lParam, answered `0x10000`; then two `UiaServerConnection_HookMessage` (a registered message, wParam 3, lParam a counter). A lookup after the probe in the same process repeats all four. The raw probe sends exactly one `WM_GETOBJECT`, wParam 0; the zero-extended `0x00000000ffffffe7` was also answered `0x10000` by this server.
- Against conhost under the trigger (`e39-msg-console`, 60 trials at 0 ms, U probes then looks up; hooks installed just after the calls are sent): every logged `UiaRootObjectId` request was sign-extended. Each trial also showed L's own first request to the new window about 3.5 ms after U's. The probe answered no after 3,028 to 3,050 ms in 6 of 60. In the 5 of those whose first request was logged, conhost received U's first request 24 to 31 ms after the window was found and answered `0x10000` in about 3.5 ms, as in the yes trials, with the same order and count of requests. So the no is not an unanswered or refused `WM_GETOBJECT`: the window answered, and UIA's client waited afterwards. The lookup straight after returned conhost's own element in 1.6 to 2.5 ms.
- `e39-msg-console-hookfirst`, 60 trials: installing the hooks before the probe moved the probe to about 18 ms after the window was found, and gave no no in 60.

Experiment 2, three ways to decide, under the trigger at 0 ms, 120 trials each (times are probe plus lookup):

- `e39-lookup`, the lookup alone: conhost's own element in 113; UIA's MSAA proxy (its process id the console's client, not conhost) after 5,030 to 5,043 ms in 7. Otherwise 27 to 47 ms, median 38.5.
- `e39-uia`, the probe then the lookup: yes and conhost's own element in 120 of 120. Sum median 39.2 ms (30 to 55).
- `e39-raw`, the raw probe then the lookup: `0x10000` and conhost's own element in 120 of 120. Probe median 24.7 ms, lookup median 20.9, sum median 45.6 (37 to 56).
- On a console 2 s old (`-settled`, 20 each): the lookup alone 9.1 ms median; probe then lookup 10.2; raw probe then lookup 9.6. All correct.

Experiment 3, classification (`e39-classify.ps1` opens Notepad, winver, the Mouse control panel, a console, a Windows Forms form like the text_box scenario's (`e39-winforms.ps1`), Edge with a fresh profile, a File Explorer window and Settings; with the shell's own windows, 88 windows; Windows Terminal is left out because a new terminal window would be hosted by the user's own terminal process). The probe said yes on 30. The raw probe answered `0x10000` on exactly those 30 and 0 on 55; on 3 File Explorer child windows (two `Button` windows and "Shell Preview Extension Host") it answered `0xffffffff80040201` where the probe said no, so only `0x10000` may count as yes. The lookup disagreed with both probes on 11 File Explorer child windows (`CtrlNotifySink`, `NamespaceTreeControl`, `ScrollBar` and `SHELLDLL_DefView`): the window answers `UiaRootObjectId`, yet UIA's element for it has an MSAA or non-control proxy as its main provider.

## The leak measurement

The leak judge of `design-raw-probe.md` (section 6, judge 3, and its amendments), as `docs/design/listener-without-connections.md` section 10 step 1 decided it: a one-off measurement, not a test, of the raw probe as built (`verbatim_uia::raw_probe`, 2026-10-19), with no change to mockapp. A scratch example in `verbatim-uia`, not kept, found mockapp's window by its title and called `raw_probe` on it a given number of times in one process, or `has_server_side_provider` for the control; a PowerShell script started mockapp (`--fixture small.json --backend uia`, its window hidden) and read its `HandleCount` and `PrivateMemorySize64` from `Get-Process` before the first probe and after each batch. Debug build, this machine.

- Every raw probe answered `0x10000` (`RawProbe::Provider`), 20,301 in all over four runs; 1,000 took 47 to 63 ms in one process, where 1,000 of `UiaHasServerSideProvider` took 825 ms.
- Handles: 116 before any probe, 165 after the first (UIA's server-side setup in mockapp, its threads going from 5 to 9), 165 after 1,000, 166 after 1,100, and 166 through 12,000.
- Private bytes: 1.64 MB before, 1.93 to 1.98 MB after the first probe, then about 2.3 KB more a probe, in steps of 176 to 291 KB a hundred, to 4.19 to 4.26 MB after 1,000, then unchanged after 1,100, 2,000, 3,000 and 12,000. The growth had stopped by 1,000 probes in each of the four runs, and between 900 and 1,000 in the run read every 100 probes.
- The control, `UiaHasServerSideProvider` on a fresh mockapp: handles 116, then 186 after one probe, 194 after 1,000, 195 after 2,000 and 3,000; private bytes 1.53 MB, then 2.09, 2.21, 2.24 and 2.24 MB.

Established: the raw probe leaks no handle and nothing without bound in the probed process over 12,000 probes of one window. Not established: what keeps the 2.2 MB, whether its bound is per window or per process, and whether it is ever released. The inference is that UIA's `UiaReturnRawElementProvider`, given a request with wParam 0 that no connection follows, keeps the provider it was handed in a table of about 1,000 entries, which mockapp, creating a new root provider for each request, fills. The listener keeps a "yes" for the window's lifetime, so it probes a window with a provider once.

## Established and inferred

Established, on this machine: the probe from a fresh client on a just-created console window answers no after about 3 s when another process has any UIA event handler registered, even one idle tooltip handler, and never did with none (0 of 110 in e38, 0 of 180 in e37). No remote operation, listener read, WinEvent hook, outpost registration or processor load is needed. The rate is highest when the call reaches conhost before its window thread first pumps, and it happened up to 77.3 ms later. Both UIA calls send the same requests, and conhost answered them at once in the no trials. The lookup alone meets the same fault (the MSAA proxy after about 5 s, 7 of 120). The raw probe agreed with the probe on every window that answered 0 or `0x10000`, and a lookup after it was right in 120 of 120 at 0 ms. The rate varies between runs: 0 of 120 in `e39-uia` against 6 of 60 with hooks and 4 of 40 in e38's comparable set.

Inferred, not shown: that the kept runs' failures are this fault; that the no and the 5-second proxy are UIA's client waiting for the second half of a connection after a concurrent first connection from another process (L's request arrives 3.5 ms after U's in every trial, including those that succeed); and that the raw probe's lookup is safe because it starts after conhost has pumped.

## Files

- `collide-tool-e37.patch`, `collide-tool-e38.patch`, `collide-tool-e39.patch`: each the full diff of the tool against main at f9fb32f7.
- `e37/`, `e38/`, `e39/`: each set's script (`.txt`), output (`.out`), runner and summary, and in `e39/hooklogs/` the hook DLL's log for each trial of experiment 1, named by set, trial and process id.
- `design-raw-probe.md`: the proposal to replace the probe with the raw probe, as written on 2026-10-18 with its amendments after e39; Dickson decided it for the focus listener only (`docs/design/listener-without-connections.md`, decision 2), built in its section 10, step 1. Its section 6, judge 5, describes the mockapp test that would reproduce this fault.
- `kept-runs/`: the three failing `conhost_two_windows` runs above, as the end-to-end suite kept them.

## Rerunning

In a scratch worktree of f9fb32f7, apply the set's patch, build `collide` (and for e39 `collide-hook`) into `target\collide`, and run the set's `run-e3N.py` with `python -I`; each script's `EXE` line names the built executable. The consoles open on the current desktop and take the foreground (e38 and e39), so these take over the desktop while they run.
