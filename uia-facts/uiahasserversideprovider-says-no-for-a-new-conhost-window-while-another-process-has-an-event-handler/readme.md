# UiaHasServerSideProvider says no for a new conhost window while another process has an event handler

## Fact

`UiaHasServerSideProvider` on a conhost window moments after it is created, while another process has any UIA event handler registered, answers "no" in some trials, after about 3 seconds, though the window answers UIA's request; the raw probe answers `0x10000` (`books/dev/src/reference/uia-bugs.md`, the fault of that name). On a window that does not pump, the call answers "no" after about 5 seconds.

## What Verbatim relies on

Nothing in its code: the raw probe replaced the call (`verbatim_uia::raw_probe`), and Verbatim's workaround is right whether or not the fault remains.

## Method

A trial, as `e38` ran it: U and R, two client processes that have made their client objects and connected to nothing, are started; a third process starts a console host directly (`conhost.exe cmd.exe`) with a title of its own, finds its window by class and title in a spin, visible or not, as `e37` and `e38` found it, reports it, and on the unlocked desktop brings it to the front; U is told the window and calls `UiaHasServerSideProvider` on it, and R sends the raw probe at the same moment. The console host is the target because the claim is about its windows. Each rate runs 30 trials, as `e38`'s arms did.

- `focus-handler`: on a desktop of the program's own, beside a third process there holding one idle desktop-wide focus handler. Judged as the count of "no" against its first run's 0 of 30. A `ToolTipOpened` handler over the root of a desktop of the program's own fails to register (`0x8007029C`), so `e38`'s own handler runs only in `tooltip`.
- `no-handler`, run only when named: on the program's desktop, with no other process holding a handler. Judged as the count of "no" against its first run's 0 of 30, since `e38`'s 0 of 110 is a rate of zero that no run passes.
- `stall`, run only when named: the provider program's window, its UI thread busy until the call returns. Judged: "no" (deterministic), after a band from 5,000 ms, a value UIA sets.

R's raw probe is judged to answer `0x10000` every time. No case checks the fault itself, beside an idle `ToolTipOpened` handler: the claim stands on `e38`'s record (7 of 30) and on `e38-tooltip` rerun with its own tool at its commit on 2026-11-06, 3 of 30 (`recorded/e38-tooltip-rerun.out`). Not run: Windows 11 Notepad's 3-second "no" while it starts (`docs/nvda/uia.md`) and File Explorer's `DirectUIHWND` probes, which no case tries; their claims stay as the documents state them.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `e38`: 7 "no" in 24 trials at 0 ms and 4 in 96 at later delays; at 0 ms a full listener stand-in 4 of 30, a passive one 10 of 30, an idle tooltip handler 7 of 30; no other handler 0 of 110 (`e38`) and 0 of 180 (`e37`); "no" after 3,022 to 3,058 ms; the raw probe `0x10000` in 360 of 360 | 2026-10-18 | Build 26200 | The probe's experiments (`e37`, `e38`, `e39`), in git at fdfb63ae | Rates |
| `UiaHasServerSideProvider` on a window that does not pump: "no" after 5,001 to 5,014 ms | 2026-10-29 | Build 26200 | The focus pipeline rework's measurement of the probe under a stall, in git at fdfb63ae | A band |
| First uia-facts run: beside an idle focus handler on the program's desktop 0 of 30; with no handler 0 of 30; the raw probe `0x10000` 60 of 60; the stalled window "no" after 5,028.5 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Focus handler new: 0 of 30; no handler inconclusive, 30 trials could miss it; the stall passes |
| Dickson's desktop runs of `tooltip`, by default and with each client setting: 0 of 30 each time; the raw probe `0x10000` 30 of 30 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 28b23338 | Differs from 7 of 30 |
| `e38-tooltip` rerun with the collide tool of `collide-tool-e38.patch` at f9fb32f7, 30 trials at 0 ms on the unlocked desktop: `UiaHasServerSideProvider` no in 3 of 30, after 3,029.3 to 3,039.3 ms; the raw probe `0x10000` each time | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | `recorded/e38-tooltip-rerun.out` | 3 of 30 lies within the record's interval: the fault remains |
| `tooltip` with U's client made before the console, R's raw probe from a process of its own sent at the same moment, the console started by a third process and found by its title before it is visible, and brought to the front: 0 of 30 in each of three runs; `focus-handler` and `no-handler` 0 of 30 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 7319ed67 | The `tooltip` case was removed; the claim stands on the rerun above |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler.exe`, which runs `focus-handler` on a desktop of its own. `--case no-handler --case stall` run the others that need no desktop; 

## Evidence kept

`recorded/e38-tooltip-rerun.out`: the 30 trials of `e38-tooltip` rerun with its own tool at f9fb32f7 on 2026-11-06, one `=== run` line before each, `U`'s `done probe` line giving each answer and its time.

`experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/kept-runs/`, the evidence of the `conhost_two_windows` inference the fault's entry cites, stays there.
