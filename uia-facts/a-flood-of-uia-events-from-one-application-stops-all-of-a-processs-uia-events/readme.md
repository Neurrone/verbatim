# A flood of UIA events from one application stops all of a process's UIA events

## Fact

When one application raises UIA events at about 10,000 a second, name changes, or text changed events with event coalescing off, a client process registered for them delivers the first 83 to 110 and then no event to any of its handlers, the other applications' included, for at least 90 seconds, its work-item thread using one processor core throughout. Another process's handlers are not affected. Text changed events with coalescing on kept being delivered, 2,529 and 2,641 of 30,000. At 2,000 to 5,000 a second some of another application's events were never delivered. Whether coalescing dropped those, and whether the worker's work grows with the square of its backlog, are not known; that selective registration keeps a flood from the worker, while a flood of desktop-wide events such as notifications still fills it, is inferred (`books/dev/src/reference/uia-bugs.md`, the fault of that name; `books/dev/src/explanation/windows/uia.md`, "Events into the client", "Predicting outcomes" and "Not known").

## What Verbatim relies on

D9 as amended on 2026-11-02 (`books/dev/src/explanation/architecture.md`) and each outpost holding its own application's registrations. If the flood stopped, D9's reason would be wrong; if a flood began stopping other processes, the per-application isolation would fail.

## Method

Flooding windows are UIA windows of the provider program raising events on their UI thread at a set pace. B is a UIA window whose node 1 is renamed `P<k>` as a probe. Each client is a listening process of its own, its registrations on one client object, warmed with one event of each flood before it starts, as `q2` warmed its handlers. A client's sender names are read by its worker when it takes the event from its queue, so probes are counted, not told apart.

- `stops`: on a desktop of its own, where the notification flood's window can be shown, since a desktop-wide registration hears only shown windows' events. Three windows each flood 30,000 events at 10,000 a second for 3 s: name changes, text changes and notifications. Four clients: one registered for the name flood and B, one for the text flood and B, one for desktop-wide notifications and B, and one for B alone. B is renamed every 250 ms until 90 s after the flood. Judged: the name-flooded client hears nothing from the flood's end to 90 s after (a relation) and uses at least 0.8 of a core over those 90 s (a relation); the text-flooded client the same with coalescing off, and keeps hearing with it on; B's own client hears every probe. The flood events heard, for trends; the notification-flooded client, new.
- `rates`: one client registered for a window's name changes and B's, flooded at 2,000 and then 5,000 a second for 3 s; B renamed every 100 ms until the flood has drained, every flood event heard or none for 2 s, or 30 s after the flood. Judged: some probes never heard (a relation, inconclusive while the flood is still arriving). The `q2_thr` record was taken with coalescing on.
- `backlog`: bursts of 250, 500, 1,000 and 2,000 name changes raised back to back, each at a client of its own: the events it hears and its processor time in the 3 s after. New.

## Client settings

To be filled from the settings check.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `q2`: at about 10,000 a second, 83 to 110 of 30,000 delivered; the other process's probes 1.5 to 1.6 ms; text with coalescing on 2,529 and 2,641 of 30,000 | 2026-11-01 | Build 26200 | `recorded/q2_m_*.out` and `.log` | The relations; the counts for trends |
| `q2_thr`: with coalescing on, at 2,000, 3,000 and 5,000 a second every flood event delivered, the last 2.9, 5.2 and 10.4 s after; 28, 50 and 103 probes never delivered | 2026-11-01 | Build 26200 | `recorded/q2_thr_one_c1.out` and `.log` | Some probes never heard |
| `q2long`: 95 and 96 delivered; none of 349 probes in 90 s; processor time 3.2 s at the flood's end and 90.1 s 88.7 s later | 2026-11-01 | Build 26200 | `recorded/q2long_*.out` and `.log` | None heard for 90 s; at least 0.8 of a core |
| The work-item thread about 2 s after the flood in `WorkItemQueue::GetDuplicatedEventWorkItemCount`, `SavedEvent::GetWorkItemInfo`, `ProtobufHelper::GetEventArgs` | 2026-11-01 | Build 26200 | `recorded/stacks/` | The frames, by the guide below |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events.exe`, or one case with `--case`. No desktop or application; `stops` takes about two minutes and keeps four processor cores busy, each stopped client spinning one until the run ends.

## Debugger guide

The work-item thread's dumps (`q2long`'s `stacks/`):

1. Run `--case stops --pause`. It prints each client's process id and waits for Enter.
2. Press Enter; about 2 s after the flood ends, take the name-flooded client's stacks with a non-invasive attach: `cdb -pv -p <name changes> -y srv*C:\symbols*https://msdl.microsoft.com/download/symbols -c "~*k 30; qd"` (the Windows SDK's `cdb.exe` or the WinDbg app's `cdbX64.exe`).
3. Expect the thread started at `WorkItemQueue::StaticThreadProc` in `WorkItemQueue::ThreadProc`, `WorkItemQueue::GetDuplicatedEventWorkItemCount`, `SavedEvent::GetWorkItemInfo` and `ProtobufHelper::GetEventArgs`, with `ProtobufHelper::GetObj`, or `GetAutomationId` and `GetWstring`, below.

## Evidence kept

- `recorded/q2_m_*.out` and `.log`, `recorded/q2_thr_one_c1.out` and `.log`, `recorded/q2long_*.out` and `.log`: the raw outputs and logs. Cited: the counts delivered, the probes' delays and losses, and the processor time.
- `recorded/stacks/`: the two thread dumps. Cited: the work-item thread's frames.
