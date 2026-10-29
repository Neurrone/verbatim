# Experiment: a process-scoped hook takes events by the window's owner

This experiment serves `docs/design/cross-process-calls-study.md`. An outpost's `WinEvent` hook is scoped to its application's process id (`EventThread::hook`, `crates/verbatim-outpost/src/event_thread.rs`), and its worker acts on the window an event names with no check of who serves that window. The review of 2026-10-29 asked what happens when the application raises an event naming another process's window with `NotifyWinEvent`. Two questions: which process-scoped hook receives such an event, and what an outpost does with an event naming another process's window.

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=hook`. A and B are mockapps on `small.json`, MSAA backend, hidden; R is a helper process.

1. The scope. The test process sets an out-of-context hook for `EVENT_OBJECT_FOCUS` to `EVENT_OBJECT_NAMECHANGE` scoped to one process id, R's, then B's. R calls `NotifyWinEvent` with each of the two events, naming B's window, `OBJID_CLIENT`, child 0. The hook records every event it receives with its window, object and thread; the test pumps messages for 300 ms after each.
2. The outpost. An outpost for A (`OutpostUnderTest`) is handed an MSAA focus fact naming B's window, object 1 (B's first node), as A's own hook would deliver it, 5 times with A's window read as the foreground and 5 with B's; the test lets it settle and counts B's provider calls and messages.

## Results

`results.txt`.

- The hook scoped to R, the process that raised the events, received nothing (0 of 2 events). The hook scoped to B, the window's owner, received both, with B's window and `OBJID_CLIENT`, and with a thread that was not R's raising thread.
- The outpost for A, with A's window in front: no call into B and nothing reported, 5 of 5 (the acceptance test drops a focus in a window unrelated to the foreground before any call). With B's window in front: the outpost sent B the raw probe (1 `WM_GETOBJECT` with `UiaRootObjectId`, wParam 0, the first time) and made MSAA calls into B (`accParent` 2, `get_accChild` 1, `get_accRole` 1, `get_accState` 1, `accFocus` 1, and the acquisition's `WM_GETOBJECT`), 5 of 5, and then reported nothing.

## Conclusion

- A process-scoped hook takes an event by the process that owns the event's window, as `GetWindowThreadProcessId` names it, not by the process that raised it. So `NotifyWinEvent` from one application naming another's window reaches the window owner's outpost, not the raiser's: the review's case does not arise in that form.
- An outpost does call into whatever window an event it is handed names. The case that remains is a window whose owner by `GetWindowThreadProcessId` is not the process that serves it: a console window, owned by its console's client while its console host serves it. A console client that has an outpost of its own, such as a windowed program that also has a console, would by this result receive its console window's events in its own hook and act on them in the console host. That is inferred from the measured scoping and the console's owner rule, not run.
