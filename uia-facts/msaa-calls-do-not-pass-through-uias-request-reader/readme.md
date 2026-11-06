# MSAA calls do not pass through UIA's request reader

## Fact

An MSAA call does not go through UIA's I/O thread: with a UIA call stuck in a provider off the UI thread, MSAA calls into the same process answered in 1.2 to 2.6 ms, through oleacc's window proxy and through UIA's MSAA bridge alike, while UIA calls waited (`books/dev/src/explanation/windows/uia.md`, "MSAA beside UIA"). For a window with no `IAccessible` of its own, oleacc's proxy reads the name by sending `WM_GETTEXTLENGTH` and `WM_GETTEXT`. Sent messages slow down with more sending threads, to 0.52 to 0.78 times one thread's rate, even when each thread sends to a UI thread of its own; that the contention is in the window manager is inferred. An MSAA call into a busy UI thread waits the whole busy time, with no bound up to 20 seconds.

## What Verbatim relies on

The hung-window workaround's claim that MSAA keeps working, and the outposts' MSAA reads of other processes' windows (`books/dev/src/explanation/design/listener-without-connections.md`, sections 4.3 and 4.4). A change would slow those reads behind UIA's.

## Method

- `stuck`: the provider program without `UseComThreading` and with `--bridge`: window 0 a UIA window, window 1 one with its own `IAccessible`, window 2 a plain window, all on UI thread 0, and window 3 a UIA window on UI thread 1. Window 0's node 1 read is held on UIA's I/O thread until the MSAA reads have answered and 200 ms more (a deliberate length), so the UIA read is still waiting at the release. Meanwhile `accName` on each window's `OBJID_CLIENT` object, got before the hold: UIA's MSAA bridge (window 0), the window's own `IAccessible` (window 1), oleacc's window proxy (window 2); and a UIA read into window 3.
- `messages`: oleacc's `accName` on a plain window, the window's messages counted by the provider's log.
- `busy-sweep`: oleacc's `accName` on a plain window while its UI thread is busy for 20 s; judged to wait the whole busy time, to within a system tick, and then answer.
- `sent-rate`: 20,000 `WM_GETTEXT`s sent by 1, 4 and 8 threads of this process, each thread to a plain window on a UI thread of its own.

The `processes` case, threads of one process against client processes, printed only for trends, is no longer run; its first run stays in the records below.

Judged: whether each MSAA call answered before the release and the UIA read after it (relations); the message counts (deterministic); whether more threads send fewer messages a second than one (a relation); that the busy-sweep read waits the whole busy time (a relation). The times and rates are printed for trends.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| MSAA during a stuck UIA call: 1.2 to 2.6 ms; UIA waits | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "MSAA calls while a UIA call is stuck" (`msaa-while-stuck.ps1`) | Relations; the time for trends |
| oleacc's `accName` sends `WM_GETTEXTLENGTH` and `WM_GETTEXT` | 2026-11-02 | Build 26200.9711 | The same, "Why MSAA slowed with more threads" (`msaa-path.ps1`) | (1, 1) |
| Sent messages at 0.52 to 0.78 times one thread's rate | 2026-11-02 | Build 26200.9711 | The same | Slower than one thread; the ratio for trends |
| First uia-facts run, `stuck`: each of the three `accName`s answered during the hold, 3 of 3 (the bridge 0.9 ms, the window's own `IAccessible` 0.2 ms, oleacc's proxy 0.1 ms); the UIA read waited for the release, 3 of 3 | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | c84ab856 | Pass |
| First uia-facts run, `messages`: (1, 1) in 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `busy-sweep`: answered after 1,001, 5,001, 15,000 and 60,000 ms | 2026-11-05 | As above | As above | The answer: an MSAA call into a busy UI thread waits the whole busy time, with no bound up to 60 s; the case now runs 20 s alone and states no bound up to 20 s |
| First uia-facts run, `sent-rate`: slower than one thread 3 of 3 at 4 and at 8 threads; the rate 0.51 at 4 threads (0.49 to 0.53) and 0.51 at 8 (0.50 to 0.51) | 2026-11-05 | As above | As above | Pass; the ratio for trends |
| First uia-facts run, `processes`: 8 threads of one process 802 to 810 ms, of two processes 810 to 822 ms | 2026-11-05 | As above | As above | For trends |
| uia-facts run of the cut program: `stuck` each `accName` answered during the hold (0.2 to 1.1 ms) and the UIA read waited; `messages` (1, 1); `busy-sweep` answered after 20,003 ms; `sent-rate` slower than one thread at 4 and 8, 0.45 and 0.48 | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ff23f81a | Pass; the times and rates for trends |

The `stuck` times are lower than the original's; they are printed for trends only.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\msaa-calls-do-not-pass-through-uias-request-reader.exe`, or one case with `--case`. No desktop, application or debugger.

## Evidence kept

`recorded/msaa-path.txt` and `recorded/msaa-while-io-thread-stuck.txt`: the parallel-calls experiment's MSAA outputs.
