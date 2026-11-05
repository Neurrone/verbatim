# A client process's events run on one worker

## Fact

The event work of a client process runs on one work-item thread at a time, in order; two client objects in one process were seen sharing it, and that every client object of a process shares one worker, however many there are, is inferred. Four events in a row ran on four different thread ids, so the thread is not kept between events; that it ends when its queue is empty is inferred. A handler runs on the worker, not on a UI thread, so whatever it waits for, a slow handler or a slow query inside it, delays every later event that worker delivers (inferred). Whether UIA drops events when the client's handler itself is slow is not known (`books/dev/src/explanation/windows/uia.md`, "Events into the client", "Predicting outcomes" and "Not known").

## What Verbatim relies on

D9 as amended on 2026-11-02 (`books/dev/src/explanation/architecture.md`), whose stated reason is the shared worker and the flood, and each outpost holding its own application's registrations. If the worker became per client object, D9's reason would be wrong.

## Method

The client is the program itself, its client objects from support's `uia::client()`. The provider program holds B1 and B2, UIA windows on UI threads of their own, and C, a third. An event is a name change of a window's node 1, renamed so that the handler can tell events apart; every handler registers name changes on its window's subtree, caching the name. A handler's slowness is a deliberate sleep inside it. Each case starts a new provider.

- `shared`: 2 and then 8 client objects; the first object's handler takes 1 s on B1's event, the others are registered on B2, whose event is raised 100 ms after B1's. Whether every other object hears B2's event only after the first object's handler returns: judged for 2 objects against the record, new for 8, which narrows the original's 1 to 8 to its two ends.
- `threads`: one handler; four events 800 ms apart, each awaited, as the original's events came at least 800 ms apart; after each, the program asks every 50 ms for 750 ms whether the event's thread has ended. Then four events at four fresh client objects, 800 ms apart, and four events raised at once. Judged: the distinct thread ids of the four events apart (deterministic, against 4); that every handler runs on a thread other than the registering one (deterministic). New: when each thread ended, and the thread ids of four events at once.
- `slow-handler`: one client object; a handler that takes 0.5 s on B1's event, and B2's event raised 50 ms later. New: whether B2's event waits for the handler.
- `slow-query`: one client object; a handler that reads node 1's name of C, whose provider holds the read until released 1 s later, and B2's event raised 100 ms after the read reached C. New: whether B2's event waits for the release.
- `drops`: a handler that takes 0.2 s on every event, and 20 name changes of one node raised at once. New: whether some are never delivered, with the count delivered.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Two client objects shared the worker: an event on one waited behind the other's focus tracking | 2026-10-18 | Build 26200 | `h3`'s client variants, kept with [the hung-window fact](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md) | A relation |
| Four events on four thread ids, the handler on `WorkItemQueue::StaticThreadProc` | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Events and releases", `results/ops-event-stacks.txt` | Deterministic, 4 |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-client-processs-events-run-on-one-worker.exe`, or one case with `--case`. No desktop or application.

## Evidence kept

None of its own: the original's stacks stay with `experiments/parallel-uia-calls/` until that folder moves.
