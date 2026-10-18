# Experiments: desktop-wide focus handlers in a second process deliver late

These experiments establish the fault recorded in `docs/uia-bugs.md` under [Desktop-wide focus handlers in a second process deliver late](../../docs/uia-bugs.md#desktop-wide-focus-handlers-in-a-second-process-deliver-late). They are part P2 of the option (c) prototype experiments, run on 2026-10-18 on Dickson's x64 machine (Windows 11, build 26200), in a debug build, with the prototype described in [`../focus-events-cannot-be-limited-to-one-application/`](../focus-events-cannot-be-limited-to-one-application/readme.md), which also holds P2's cost figures.

Terms: a desktop-wide focus handler is `AddFocusChangedEventHandler` (the prototype's `desk:<cache>` spec). M is a mockapp with the MSAA backend, which UIA reads through its proxy, so each `focus <item>` command makes M raise a focus WinEvent from which UIA derives a focus event. A client process is one `focusproto client` process. Every process ran on the desktop `protoc`, where no window has the keyboard focus, so desktop-wide handlers receive only such derived focus events.

## P2c: one, two and four processes, focus every 150 ms (`p2c_desk.py`, `p2c_sum.py`)

M raised 10 focus events, alternating between two items, 150 ms apart; each client process held one desktop-wide handler with the process id cached (`procs` layout), or one process held two or four handlers (`one` layout). Two runs of each. `p2c_sum.py <tag>` prints, for each process, the number of events received and the longest time from the latest raise before an event's arrival to that arrival:

- One process (`procs1fast1`, `procs1fast2`): 10 events, at most 9 and 10 ms.
- Two processes (`procs2fast1`, `procs2fast2`): the first at most 10 and 13 ms; the second at most 682 and 1,112 ms.
- Four processes (`procs4fast1`, `procs4fast2`): the first at most 15 and 10 ms; processes 2 to 4 at most 834, 1,415 and 834 ms in the first run and 670, 2,677 and 670 ms in the second.
- Two and four handlers in one process (`one2fast*`, `one4fast*`): 20 and 40 events, at most 9 to 11 ms.

Every process received all 10 events in the end.

## P2b: two and four processes at a slower pace (`p2b_desk_two.py`)

The same with focus every 500 ms, two processes (`p2b_two-slow.out`) and four (`p2b_four-slow.out`). The late processes received their events in bursts, with waits of about 1,000, 1,500 and 2,000 ms; in `p2_cost`'s shorter collection window (the cost folder's `p2_cost.out`) some events had not arrived 0.5 s after the last raise.

## P2d: a real Win32 window (`p2d_real_win32.py`, `p2d_sum.py`)

The script started charmap on the same desktop and raised 10 focus WinEvents naming its dialog, which did not have the keyboard focus, with 1, 2 and 4 client processes (`p2d_r1`, `p2d_r2`, `p2d_r4`) and four handlers in one process (`p2d_o4`). No handler received any of them. Inferred: UIA checks a real window's focus before reporting a focus derived from a WinEvent, and mockapp's MSAA objects pass that check because they report themselves focused. So whether the fault happens with a real application needs real keyboard focus.

## Established and inferred

Established, on this machine: with two or more processes each holding a desktop-wide focus handler, every process after the first received proxied focus events up to 0.7 to 2.7 s late; the same number of handlers in one process were never late. The cause is not known, and no workaround was tried.

Inferred: that Verbatim does not meet this with itself, since only its listener holds a desktop-wide focus handler, and that beside another screen reader, which holds one too, it is exposed.

## Files

- `p2c_desk.py`, `p2c_sum.py` and the ten `p2c_*.out`; `p2b_desk_two.py`, `p2b_two-slow.out` and `p2b_four-slow.out`; `p2d_real_win32.py`, `p2d_sum.py` and the four `p2d_*.out`.
- `proto.py`: the prototype's shared helpers.

## Rerunning

Build the prototype as [its folder](../focus-events-cannot-be-limited-to-one-application/readme.md) says, set `BIN` in `proto.py` to the build's output folder, and run, for example, `python -I p2c_desk.py 0.15 2 procs procs2fast1` (pace in seconds, count, layout, tag) and then `python -I p2c_sum.py procs2fast1`. Every process runs on a desktop of its own.
