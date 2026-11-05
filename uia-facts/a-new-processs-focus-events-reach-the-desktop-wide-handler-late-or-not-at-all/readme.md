# A new process's focus events reach the desktop-wide handler late or not at all

## Fact

A newly started application, a File Explorer folder window in its own process or the Settings app, moves its focus while a desktop-wide `AddFocusChangedEventHandler` is registered in another process. The `WinEvent` hook of that process sees the focus move, and UI Automation delivers the matching focus events late, or none of them. The cause is not known (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

The recovery, not a workaround: the application's outpost waits a second from the `WinEvent` focus copy for its UIA focus and otherwise reads the focus itself (`books/dev/src/explanation/design/listener-without-connections.md`, decision 11).

## Method

On the unlocked desktop, the program holds one desktop-wide focus handler, caching the sender's process id, and an out-of-context hook for focus `WinEvent`s from every process, as Verbatim's listener holds both. It starts a fresh application 20 times each, ending it after each start:

- `provider`: the provider program, its window brought to the foreground and its focus moved to node 5 with its focus event raised;
- `notepad`: Notepad, `notepad.exe` in the system folder;
- `settings`: the Settings app, `ms-settings:`.

For each start it takes the first focus `WinEvent` from a process of that executable that had no window before, and waits up to 5 s for a UIA focus whose sender is in that process. The UIA focus is late when it comes more than 1 s after the `WinEvent`, the second Verbatim's outpost waits, and missing when none comes within 5 s. The fault was never measured as a rate, so each count is printed as new.

## Client settings

Not checked: the program needs the desktop and was not run by the build (see "How to rerun").

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| None of a new folder window's items, or none of its first 50 to 130, in some `measure_repeat_explorer` runs; the Settings `CoreWindow`'s focus `WinEvent`s with no UIA focus in 14.5 s in `settings_system_page` | 2026-10 | Build 26200 | End-to-end runs, in prose (`books/dev/src/explanation/design/listener-without-connections.md`, decision 2, risks 2 and 13) | New |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all.exe`, or one application with `--case`. It needs the unlocked desktop and takes it over for about two minutes: each application is started 20 times in front.

## Evidence kept

None: the fault was met in end-to-end scenarios, which kept no record of it.
