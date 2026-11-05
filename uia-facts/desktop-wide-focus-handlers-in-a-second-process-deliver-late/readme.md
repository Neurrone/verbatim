# Desktop-wide focus handlers in a second process deliver late

## Fact

When two or more processes each hold a desktop-wide `AddFocusChangedEventHandler`, and focus changes come from a window UIA reads through its proxy, every process after the first receives the focus events late: with one process at most 9 to 10 ms after the raise, with two the second at most 682 and 1,112 ms, with four 670 to 2,677 ms for processes 2 to 4; at a 500 ms pace the late processes waited about 1,000, 1,500 and 2,000 ms. The same number of handlers in one process are never late. Focus `WinEvent`s for a real window without the keyboard focus reach no desktop-wide handler at all. The cause is not known (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

Nothing: Verbatim holds one desktop-wide focus handler, in the listener.

## Method

Every process runs on a desktop of its own (`uia_facts_support::desktop`), as P2 ran, where no window has the keyboard focus, so a desktop-wide focus handler receives only the focus events UIA derives from a focus `WinEvent`. M is the provider program's MSAA window: its object has two children, `First` and `Second`, and reports itself and each child focused, as mockapp's MSAA objects did. M's UI thread moves its focus from one child to the other and raises the focus `WinEvent` naming it, 10 times. Each client is a listening process holding desktop-wide focus handlers, each caching the sender's name, started in turn. A delivery's lateness is the time from the latest raise before it to its arrival; the program listens 3 s after the last raise.

- `processes`: 1, 2 and 4 processes, one handler each, focus every 150 ms (P2c).
- `one-process`: 2 and 4 handlers in one process (P2c's `one` layout).
- `slow`: 4 processes, focus every 500 ms (P2b).
- `plain`: the provider program's plain Win32 window, which has no `IAccessible` of its own and no keyboard focus, named by the 10 focus `WinEvent`s, with 4 processes and a process with 4 handlers (P2d, which used charmap's dialog).
- `threading`: 2 processes, the second registering on a thread of a single-threaded apartment, toward the cause (new).

Judged: the first process's deliveries within 100 ms (a relation; P2 measured 9 to 15 ms); every later process's worst delivery at least 300 ms later than the first's (a relation; P2's were 670 ms or more); the events each process hears in the end (deterministic).

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| P2c: one process at most 9 and 10 ms; two, the first at most 10 and 13 ms, the second 682 and 1,112 ms; four, the first at most 15 and 10 ms, processes 2 to 4 670 to 2,677 ms; 2 and 4 handlers in one process at most 9 to 11 ms; every process heard all 10 | 2026-10-18 | Build 26200 | `recorded/p2c_*.out` | As the Method says |
| P2b: late waits of about 1,000, 1,500 and 2,000 ms at a 500 ms pace | 2026-10-18 | Build 26200 | `recorded/p2b_two-slow.out`, `recorded/p2b_four-slow.out` | Later than the first by 300 ms |
| P2d: charmap's dialog without the keyboard focus, 1, 2 and 4 processes and 4 handlers in one: no handler heard any | 2026-10-18 | Build 26200 | `recorded/p2d_*.out` | Deterministic, none |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\desktop-wide-focus-handlers-in-a-second-process-deliver-late.exe`, or one case with `--case`. It runs on a desktop of its own and takes over nothing.

## Evidence kept

- `recorded/p2c_*.out`: P2c's raw logs. Cited: each client's `ev` lines against the `focus` sends, the lateness and the count.
- `recorded/p2b_two-slow.out`, `recorded/p2b_four-slow.out`: P2b's. Cited: the late processes' waits.
- `recorded/p2d_r1.out`, `p2d_r2.out`, `p2d_r4.out`, `p2d_o4.out`: P2d's. Cited: no `ev` line.
