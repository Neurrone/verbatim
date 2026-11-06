# A new process's focus events reach the desktop-wide handler late or not at all

## Fact

A newly started application, a File Explorer folder window in its own process or the Settings app, moves its focus while a desktop-wide `AddFocusChangedEventHandler` is registered in another process. The `WinEvent` hook of that process sees the focus move, and UI Automation delivers the matching focus events late, or none of them. (`books/dev/src/reference/uia-bugs.md`, the fault of that name.) The program checks a tree it controls, the provider program, where none of 11 starts had a late or missing focus.

## What Verbatim relies on

The recovery, not a workaround: the application's outpost waits a second from the `WinEvent` focus copy for its UIA focus and otherwise reads the focus itself (`books/dev/src/explanation/design/listener-without-connections.md`, decision 11).

## Method

The program checks a tree it controls, the provider program, since the end-to-end runs that met the fault used File Explorer and the Settings app, which the program does not start (Dickson, 2026-11-06). On the unlocked desktop, it holds one desktop-wide focus handler, caching the sender's process id, and an out-of-context hook for focus `WinEvent`s from every process, as Verbatim's listener holds both.

- `provider`: the provider program started 11 times, its window brought to the foreground and its focus moved to node 5 with its focus event raised, then ended. The end-to-end runs that met the fault recorded no rate; 11 starts would show one met in 1 start of 4, an assumed rate, with probability 0.95.

For each start it takes the first focus `WinEvent` from the new process, waiting up to 2 s since the program brings the window to the front itself, and waits up to 5 s for a UIA focus whose sender is in that process. The UIA focus is late when it comes more than 1 s after the `WinEvent`, the second Verbatim's outpost waits, and missing when none comes within 5 s. A start with no focus `WinEvent` has nothing to measure from: the case stops there, inconclusive. The fault was never measured as a rate, so the counts are printed as new.

The count of late and missing focus is judged against the first desktop runs' 0 and 0 of 11.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| None of a new folder window's items, or none of its first 50 to 130, in some `measure_repeat_explorer` runs; the Settings `CoreWindow`'s focus `WinEvent`s with no UIA focus in 14.5 s in `settings_system_page` | 2026-10 | Build 26200 | End-to-end runs, in prose (`books/dev/src/explanation/design/listener-without-connections.md`, decision 2, risks 2 and 13) | New |
| First run of `provider`: 0 late and 0 missing of 11 starts, the UIA focus 21 to 36 ms after the focus `WinEvent` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | cdb6b4d6 | New: none in 11 starts |
| Dickson's desktop runs at 8e4c9d50: 0 late and 0 missing of 11 in each, the UIA focus 11 to 41 ms after the focus `WinEvent` | 2026-11-06 | As above | Dickson's desktop runs | New: none in 44 starts |
| Dickson's desktop runs: 0 late and 0 missing of 11 starts each time, the UIA focus 12 to 59 ms after the `WinEvent` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 28b23338 | The record: 0 of 11, judged as that count |
| Dickson's desktop runs: 0 late and 0 missing of 11 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | e21eeccc | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all.exe`. It needs the unlocked desktop and takes it over: the provider program is started 11 times in front.

## Evidence kept

None: the fault was met in end-to-end scenarios, which kept no record of it.
