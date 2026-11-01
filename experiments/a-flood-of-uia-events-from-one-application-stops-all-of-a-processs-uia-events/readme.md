# Experiments: a flood of UIA events from one application stops all of a process's UIA events

These experiments establish the fault recorded in `docs/uia-bugs.md` under [A flood of UIA events from one application stops all of a process's UIA events](../../docs/uia-bugs.md#a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events). They ran on Dickson's x64 machine (Windows 11, build 26200) on 2026-11-01, for the process and thread layout study, with hidden windows only: no keys and no foreground changes.

Terms used here:

- A is a UIA mockapp on `text.json` that floods with mockapp's `flood <count> <interval_ms> name|text` (commit cb90020e): name changes of its root, or text changed events from its first text node, from its window thread. B is a UIA mockapp whose `btn1` is renamed `P<k>` or `L<k>` every 100 or 250 ms as a probe.
- `evprobe` (`evprobe/main.rs`, release build) is the client. Its handlers only read two cached properties and queue them to a consumer thread, so a handler costs the worker thread almost nothing. `qname:<hwnd>` and `qtext:<hwnd>` register a name change handler (`AddPropertyChangedEventHandlerNativeArray`) and a text changed handler (`AddAutomationEventHandler`) on a window's subtree, counted but not printed; `name:<hwnd>` is the same as `qname` with each event printed. Its `cpu` command reports the process's processor time and that of the thread its first handler ran on, UIA's work-item thread, here called the worker.
- Layout one: one process, D, holds `qname:A`, `qtext:A` and `name:B` on one client object. Layout two: DA holds A's, DB holds B's.
- Every client used Verbatim's timeouts (connection 2,000 ms, transaction 5,000 ms) and connection recovery on; `c0` and `c1` in a name mean coalescing (`IUIAutomation6::CoalesceEvents`) off and on.
- B's delay is the time from B's rename to its arrival in the process holding `name:B`.

## `q2`: the rate (`q2.py`)

Each run warms each handler with one event, then floods A at four paces in turn, 300 events at 10 ms (100 a second), 3,000 at 1 ms (1,000), 30,000 at 0.1 ms (10,000) and 10,000 back to back, probing B every 100 ms until A's received count has been still for a second. Outputs `q2_m_<layout>_<coalescing>_<kind>.out`, with logs; `q2_smoke` is a first run of the two slower paces.

- 100 a second: every event delivered; B's median delay 1.2 to 1.4 ms in all eight runs.
- 1,000 a second: every event delivered. In layout one B's median delay was 16.2 ms (max 43) for name changes with coalescing off and 40.9 ms (max 100) with it on, and 1.8 and 3.8 ms for text changes; in layout two 1.7 to 1.8 ms. The worker used 190 to 270 µs of processor time per event.
- About 10,000 a second, for 3 s: name changes in either layout and text changes with coalescing off: 83 to 110 of A's 30,000 events were delivered, the last of them about 2.9 s before the flood ended, and the worker had used 4.3 s of processor time by the end of the run, 4 to 5 s after the flood began. In layout one 40 of B's probes were not delivered by the end of the run, and the one delivered came in 2.7 to 49 ms; in layout two B's median delay stayed 1.5 to 1.6 ms. Text changes with coalescing on did not stop: 2,529 and 2,641 of A's events delivered, B's median delay 29.3 ms in layout one and 1.7 ms in layout two.
- Back to back (16,000 to 22,000 a second, for under a second): 0 to 2 of A's events delivered in every run; in layout one at most one of B's probes was delivered and 16 to 18 were not; in layout two B's median delay 1.6 to 1.7 ms.

## `q2_thr`: between 1,000 and 10,000 a second (`q2.py`, output `q2_thr_one_c1.out`)

Layout one, coalescing on, name changes, for 3 s at 2,000, 3,000 and 5,000 a second: every one of A's events was delivered, the last 2.9, 5.2 and 10.4 s after the flood ended. B's median delay was 12.2, 40.1 and 32.9 ms during the flood and 23.1, 30.6 and 42.2 ms after it, at most 100 to 101 ms; 28, 50 and 103 of B's probes were never delivered. Whether coalescing, which merges queued events of the same kind from one element, dropped them is not established.

## `q2long`: after the flood (`q2long.py`)

Layout one, name changes, 30,000 at 0.1 ms, then B probed every 250 ms and the worker's processor time read every 2 s. Outputs `q2long_one_c1_10k.out` (coalescing on, watched 90 s) and `q2long_one_c0_10k.out` (off), with logs. In the run with coalescing off the thread dump below took 31 s, during which the process was paused and the script waited, so that run watched only its first 2.3 s and 9 probes.

- 95 and 96 of A's events were delivered, all in the first 112 ms of the flood, and none after.
- None of B's probes was delivered: 349 in the 90 s after the flood with coalescing on, 9 with it off.
- The worker's processor time grew by about 2 s every 2 s throughout: 3.2 s at the flood's end, 90.1 s at 88.7 s after it.

A thread dump in each run, about 2 s after the flood (`stacks/`, a non-invasive cdb attach), caught the worker in `WorkItemQueue::ThreadProc`, `WorkItemQueue::GetDuplicatedEventWorkItemCount`, `SavedEvent::GetWorkItemInfo` and `ProtobufHelper::GetEventArgs`, decoding a saved event's arguments (`ProtobufHelper::GetObj` with coalescing on, `GetAutomationId` and `GetWstring` with it off). That this count decodes every queued event at each dequeue, so that the work grows with the square of the backlog, is inferred from the function names and the processor time, not shown.

## Files

- `evprobe/`: the probe's `main.rs` and its manifest as `Cargo.toml.txt`.
- `evc.py`: the shared helpers, also used by the hung-window series (`q1`, `q3`) in [`../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/2026-11-01/`](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/2026-11-01/readme.md).
- `q2.py` with `q2_*.out` and `q2_*.log`; `q2long.py` with `q2long_*.out` and `q2long_*.log`; `stacks/` with the two dumps.

## Rerunning

Copy `evprobe/` to a scratch folder outside the workspace, rename `Cargo.toml.txt` to `Cargo.toml`, move `main.rs` under `src/`, and run `cargo build --release`; build `mockapp` in the repository (`cargo build -p mockapp`, at or after cb90020e). `evc.py` expects the probe at `probe/target/release/evprobe.exe` beside it, and cdb and the symbol path as written there. Run a script with `python -I`, for example `python -I q2.py m_one_c1_name one 1 name 300 10 3000 1 30000 0.1 10000 0`.
