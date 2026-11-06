# UiaHasServerSideProvider says no for a new conhost window while another process has an event handler

## Fact

`UiaHasServerSideProvider` on a conhost window moments after it is created, while another process has any UIA event handler registered, answers "no" in some trials, after about 3 seconds, though the window answers UIA's request; the raw probe answers `0x10000` (`books/dev/src/reference/uia-bugs.md`, the fault of that name). Beside an idle `ToolTipOpened` handler, the "no" came only when a second process looked the same new window up by handle at the same moment, as `e38`'s script did. On a window that does not pump, the call answers "no" after about 5 seconds.

## What Verbatim relies on

Nothing in its code: the raw probe replaced the call (`verbatim_uia::raw_probe`), and Verbatim's workaround is right whether or not the fault remains.

## Method

Each trial probes a new console window, a console host started directly (`conhost.exe cmd.exe`), with `UiaHasServerSideProvider` as soon as the window is found; the console host is the target because the claim is about its windows.

- `tooltip`, on the unlocked desktop, run only when named: `e38-tooltip` ported step for step from the collide tool of `collide-tool-e38.patch` (in git at fdfb63ae, applied onto f9fb32f7), in `src/e38.rs`. Each of 60 trials starts three client processes, T, U and R, each a `CUIAutomation8` client object made as `IUIAutomation` on its main thread in the multithreaded apartment, with no setting changed, so `--recovery` and `--coalescing` do not reach them, as they did not reach the original's. T fetches the desktop root and registers one `ToolTipOpened` handler over its subtree with no cache request; U and R make their client objects and connect to nothing. The program then injects a zero mouse move and allows any process to take the foreground, starts `conhost.exe cmd.exe /d /k` suspended, titled by `lpTitle`, in a job that kills it on close, resumes it, and spins on `FindWindowW` for its window; at once it tells R and U the window and sends R's raw probe, U's `UiaHasServerSideProvider`, R's lookup by handle (`ElementFromHandleBuildCache` with the name, process id and provider description cached) and U's, in that order, waits for every answer, waits 1.5 s, closes the console and waits 0.5 s. Judged: the count of "no" as a rate against `e38`'s 7 of 30 and against the rerun of `e38-tooltip`'s 3 of 30; 60 trials tell a rate of 3 to 7 in 30 from none, since a run of 60 would miss it fewer than 1 time in 500; R's raw probe `0x10000` every time.
- `focus-handler`: on a desktop of the program's own, a trial of its own structure: U and R started, the console host started by a third process there and found by its title, U's probe and R's raw probe at once, beside a fourth process there holding one idle desktop-wide focus handler; 30 trials. Judged as a recorded absence (below), against its first run's 0 of 30.
- `no-handler`, run only when named: as `focus-handler` with no other process holding a handler, judged as a recorded absence against its first run's 0 of 30.
- `stall`, run only when named: the provider program's window, its UI thread busy until the call returns. Judged: "no" (deterministic), after a band from 5,000 ms, a value UIA sets.

What made the port show the fault where the program's own structure did not: R's lookup by handle, sent right after R's raw probe and running beside U's `UiaHasServerSideProvider`. Without it the port and the program's own structure answered "no" in 0 of 60 and 0 of 30 in every run; with it the port answered "no" in 4 of 60, each after about 3,035 ms. So beside the idle handler, the "no" came with a second process's new connection to the same new console window at the same moment.

Not run: Windows 11 Notepad's 3-second "no" while it starts (`docs/nvda/uia.md`) and File Explorer's `DirectUIHWND` probes, which no case tries; their claims stay as the documents state them.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `e38`: 7 "no" in 24 trials at 0 ms and 4 in 96 at later delays; at 0 ms a full listener stand-in 4 of 30, a passive one 10 of 30, an idle tooltip handler 7 of 30; no other handler 0 of 110 (`e38`) and 0 of 180 (`e37`); "no" after 3,022 to 3,058 ms; the raw probe `0x10000` in 360 of 360 | 2026-10-18 | Build 26200 | The probe's experiments (`e37`, `e38`, `e39`), in git at fdfb63ae | Rates |
| `UiaHasServerSideProvider` on a window that does not pump: "no" after 5,001 to 5,014 ms | 2026-10-29 | Build 26200 | The focus pipeline rework's measurement of the probe under a stall, in git at fdfb63ae | A band |
| First uia-facts run: beside an idle focus handler on the program's desktop 0 of 30; with no handler 0 of 30; the raw probe `0x10000` 60 of 60; the stalled window "no" after 5,028.5 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Focus handler new: 0 of 30; no handler inconclusive, 30 trials could miss it; the stall passes |
| Dickson's desktop runs of `tooltip`: 0 of 30 each time; the raw probe `0x10000` 30 of 30 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 28b23338 | Differs from 7 of 30 |
| `e38-tooltip` rerun with the collide tool of `collide-tool-e38.patch` at f9fb32f7, 30 trials at 0 ms on the unlocked desktop: `UiaHasServerSideProvider` no in 3 of 30, after 3,029.3 to 3,039.3 ms; the raw probe `0x10000` each time | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | `recorded/e38-tooltip-rerun.out` | 3 of 30 lies within the record's interval: the fault remains |
| `tooltip`, `e38-tooltip` ported without R's lookup by handle: 0 of 60 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | the port before 0dd81ebf | Differs: the lookup is what the program's own structure lacked |
| `tooltip`, `e38-tooltip` ported step for step, 60 trials a run on the unlocked desktop, a first run under each of the four flag combinations, which do not reach its client objects: "no" in 4, 4, 1 and 1 of 60, 10 of 240 in all, each after 3,031 to 3,052 ms; R's raw probe `0x10000` every time | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 0dd81ebf | Against the rerun's 3 of 30, pass or inconclusive; against `e38`'s 7 of 30, inconclusive or differs |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler.exe`, which runs `focus-handler` on a desktop of its own. `--case no-handler --case stall` run the others that need no desktop; `--case tooltip` needs the unlocked desktop and opens 60 console windows on it, about 2.5 minutes.

## Evidence kept

`recorded/e38-tooltip-rerun.out`: the 30 trials of `e38-tooltip` rerun with its own tool at f9fb32f7 on 2026-11-06, one `=== run` line before each, `U`'s `done probe` line giving each answer and its time.

`experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/kept-runs/`, the evidence of the `conhost_two_windows` inference the fault's entry cites, stays there.
