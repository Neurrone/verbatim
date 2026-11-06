# The raw probe answers 0x10000 for a window with a UIA provider

## Fact

A window with a UIA provider of its own answers `WM_GETOBJECT` with wParam 0 and lParam `UiaRootObjectId`, sent sign-extended as UIA sends it, with exactly `0x10000`, and a window without one answers 0, in agreement with `UiaHasServerSideProvider` (`books/dev/src/reference/uia-bugs.md`, "UiaHasServerSideProvider says no for a new conhost window while another process has an event handler", its workaround). Microsoft documents no meaning for the answer.

## What Verbatim relies on

`verbatim_uia::raw_probe` (`crates/verbatim-uia/src/probe.rs`), which sends that message and takes exactly `0x10000` as a UIA provider, and every probe the listener and the outposts make (`books/dev/src/explanation/design/listener-without-connections.md`, decision 2). If the value changed, every UIA window would be judged MSAA, or the reverse, and read through the wrong backend.

## Method

The kept CI tests, `crates/mockapp/tests/arbitration.rs`, check the probe against mockapp's two backends. The program checks it, through Verbatim's own `verbatim_uia::raw_probe`, and `UiaHasServerSideProvider` beside it, on targets mockapp cannot stand for:

- `targets`: the provider program's UIA window, its window with its own `IAccessible` and no UIA provider, and its plain Win32 window; and a console host started directly (`conhost.exe cmd.exe`) on a desktop of the program's own, probed once its window has answered a `WM_NULL`, so that it has pumped and no new window's fault applies. Judged: each target's pair of answers against its expected pair (deterministic); a disagreement between the two probes is a difference.

The 88-window sweep, which compared the two probes on the applications open at the time, stays the recorded history of an unnamed sweep; File Explorer's child windows, which answered `0xffffffff80040201`, are not run, since no fact program starts File Explorer.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The raw probe agreed with `UiaHasServerSideProvider` on every one of 88 windows that answered 0 or `0x10000`; 3 File Explorer child windows answered `0xffffffff80040201` | 2026-10-18 | Build 26200 | `e39-classify.ps1` in the probe's experiments, in git at fdfb63ae | History |
| First uia-facts run: the UIA window `0x10000` and true; the `IAccessible` window and the plain window 0 and false; the console host `0x10000` and true | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider.exe`. The tests: `cargo test -p mockapp --test arbitration`.

## Evidence kept

None: the program's output is the evidence.
