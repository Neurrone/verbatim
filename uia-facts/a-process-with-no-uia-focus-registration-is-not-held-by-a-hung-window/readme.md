# A process with no UIA focus registration is not held by a hung window

## Fact

A focus `WinEvent` naming a window that does not answer UIA holds the UIA events of a process with a focus registration, and of no other: in a process with no focus registration of any kind, another application's name and text changes, a name read on another thread and an MSAA `WinEvent` hook all answered at once during the same stall, 0 to 1 ms in `e_cross`'s outpost stand-in, and another application's name change in 1 ms (P4's `N_B`) (`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events", "Not affected" and "Workarounds that worked").

## What Verbatim relies on

Outposts holding no focus registration, so that a hang holds only the listener (`books/dev/src/explanation/design/focus-pipeline.md`, section 12.3; the module comment of `crates/verbatim-listener/src/listener.rs`). If a process with no focus registration began to be held, every outpost's events would stop during any hang.

## Method

The hung-window scene of support's `uia_facts_support::hung`, on a desktop of the program's own, as the originals ran: provider program P1 holds T, a plain Win32 window on UI thread 0, the window made to hang, and M, a window with its own `IAccessible`; provider program P2 holds B, a UIA window. The client is `e_cross`'s outpost stand-in, a listening process of its own (`uia_facts_support::listen`) with no focus registration: B's name and text changes, and an out-of-context MSAA hook on P1. T's UI thread is stalled 0.9 s, past two 250 ms waits, a deliberate length; a focus `WinEvent` naming T is raised from P1's control thread; 100 ms later B's name and text change, and 50 ms after those a name-change `WinEvent` naming M and a name read of B on another thread of the stand-in.

- `outpost`: judged: each of the four within 100 ms of its raise or its start, not held (a relation); a hold is at least one connection timeout, 250 ms or more.

The hold of a client with a focus registration, Verbatim's listener's included, is [the hold's own program](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md)'s.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| P4's `N_B` 1 ms | 2026-10-18 | Build 26200 | `../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/recorded/p4_run1.out`, `p4_run2.out` | Within 100 ms |
| `e_cross`: the outpost stand-in 0 to 1 ms | 2026-10-10 | Build 26200 | The mockapp experiments, in git at b0a1989 | Within 100 ms |
| First uia-facts run, in the hold's program's `who`: the outpost stand-in's name, text, read and hook 0.1 to 2.7 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |
| The hold's program's `stand-ins`: the outpost stand-in 0.1 to 2.4 ms | 2026-11-06 | As above | e7394449 | Pass |
| This program's first run, every setting: the outpost stand-in's name, text, read and hook 0.0 to 2.1 ms | 2026-11-06 | As above | ff23f81a | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-process-with-no-uia-focus-registration-is-not-held-by-a-hung-window.exe`. It needs no desktop or application; its processes run on a desktop of their own.

## Evidence kept

None of its own: P4's outputs stay with [the hold's folder](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md), which cites them for its own figures too.
