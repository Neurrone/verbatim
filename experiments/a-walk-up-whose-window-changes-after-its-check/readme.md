# Experiment: a walk up whose window changes after its check

This experiment serves `docs/design/cross-process-calls-study.md`. The classic walk up a focus's ancestors, the listener's read (`verbatim_uia_rops::focus_ancestry_classic`) and the outpost's (`Uia::ancestors`), checks before each step whether the current window's parent, or a top-level window's visible owner, is served by another process (`verbatim_uia::parent_across_process_boundary`, local calls), and only then steps (`GetParentElementBuildCache`). The question: when the window hierarchy changes between the check and the step, does the step go into the other process, and what does that cost?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=walk_race`. Each trial has new processes: P, mockapp on `small.json`, UIA backend, hidden; C, mockapp on `counts.json`, UIA backend, hidden, a top-level window with `First` focused.

The test process walks up from the focus in C with `focus_ancestry_classic`, given the listener's query (its two cache requests, the nearest window C's, no time limit). The query's `before_call` hook, which the walk calls before each call it makes, is called after the boundary check and before the step; at the third step, the one from C's window's own element, it changes the hierarchy, resets P's counters, and the step follows. So the order is forced: check, change, step.

Arms, 5 trials each:

- `control`: no change; the third step reaches the desktop's root.
- `reparented`: C made P's child with `SetParent` in the hook.
- `reparented-busy`: as `reparented`, and P's window thread then stalled for 1 second (`stall 1000`) before the step.
- `owner-shown`: C owned by P from the start (`GWLP_HWNDPARENT`), P hidden, so the check finds no visible owner; P shown (`SW_SHOWNOACTIVATE`) in the hook.

## Results

`results.txt`.

- `control`: P took nothing, 5 of 5; the chain `Settings`, then C's window; 2.5 to 5.3 ms.
- `reparented` and `owner-shown`: the step went into P, 10 of 10. P took 1 `WM_GETOBJECT` with `UiaRootObjectId` and a nonzero wParam (a connection), 1 with wParam 0, and 43 provider calls (`GetPropertyValue` 21, `GetPatternProvider` 11, `ProviderOptions` 7, `HostRawElementProvider` 2, `Navigate` 2): its element was read and filled with the ancestor request. The chain was still `Settings`, then C's window, 10 of 10: the check after the step (`stepped_across`) found the parent another process's and ended the walk without it. The walk took 11.5 to 15.1 ms.
- `reparented-busy`: the same calls in P, and the walk took 1,012.6 to 1,013.5 ms, its step 1,007.0 to 1,007.9 ms after the change: it waited for P's thread, 5 of 5.

## Conclusion

The check and the step are two steps, and a change between them makes the step a connection into the other process, with its element read: measured with the order forced, for a reparented child and for an owner shown. The result is still right, since the check after the step drops the parent, but the step waits on the other process's UI thread, and is the walk into another process that `docs/uia-bugs.md`'s first entry shows failing like a new connection while a remote operation is queued there (`e24`). How often a window is reparented or its owner shown in the instant between the two is not measured; nothing logs it.
