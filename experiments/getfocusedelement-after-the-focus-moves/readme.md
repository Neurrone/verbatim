# Experiment: GetFocusedElement after the focus moves

This experiment serves `docs/design/outpost-focused-element-study.md`, sections "How wide the race is" and "What losing it costs". The study infers from the code that when the system focus moves to another application between the outpost's check of the focus window and its call to `GetFocusedElementBuildCache`, the outpost's call connects into that other application. Experiment E5 (`e5_focus_read_in_held_application`, in `experiments/mockapp-experiments-2026-10-10/experiments.rs`) did not show this, because it injected a read by handle. This experiment checks the inference directly. It makes the race happen every time, by doing the check, the move and the call in that order on one thread.

## Setup

`getfocusedelement_after_the_focus_moves.rs` is the program, a mockapp test binary with no test harness, kept here and not built. `scratch-support.patch` adds its `[[test]]` entry to mockapp's `Cargo.toml`. It was built at 93691570 and has not been run yet.

The program starts two mockapp windows, A and B, both on the UIA backend with the repository's `tree.json`. They are shown on the interactive desktop (`--show`) and have distinct titles. Each window's focused node is set with `set-focus`, which raises no event: A's is `item1`, named "First", and B's is `item2`, named "Second". The program makes one UIA client (`verbatim_uia::Uia`) and the outpost's own node request: `Context::node_request` with `gates::node_gates` of the outpost's initial `Fetches::default()`, which turns every detail on. It makes the call through `Uia::focused`, the outpost's production `FocusedElementReader`, so the call uses the cache request the outpost uses, with the provider description added for the proxy rule.

Each round of the moved case does the following:

1. Brings A forward and waits for evidence that it has the focus. The program brings a window forward the way the end-to-end agent's `set_foreground` does (`crates/verbatim-agent/src/desktop.rs`): it minimizes the window without activating another, restores it, and calls `SetForegroundWindow`. It waits on window events (foreground, minimize and focus `WinEvent`s, out of context) until `GetGUIThreadInfo` names A's window as the focus window and `GetForegroundWindow` names it too.
2. Runs the outpost's check for A. The check's functions are private to `verbatim-outpost`, so the program repeats the same local calls, citing `focused_control_in` in `crates/verbatim-outpost/src/outpost/read.rs` and the helpers in `crates/verbatim-outpost/src/outpost/window.rs` at 93691570:
   - `focus_window`: `GetGUIThreadInfo(0)`, the focus window or else the active window;
   - `control_window`: the focus window must be served by A's process (`verbatim_uia::served_by`);
   - `event_window_unreadable`: `IsWindow`, no ghost window (`GhostWindowFromHungWindow`), and not `IsHungAppWindow`.

   Two steps of the check are left out: the hidden-frame test, which only concerns Core's hidden window, and the backend verdict (`window_uses_uia`), which comes before the call in either case.
3. Brings B forward and waits, the same way, for evidence that B's window has the focus.
4. Reads both windows' provider call counts (mockapp's hit counters, by method, `common::read_hits`) and their `WM_GETOBJECT` counts (`common::read_get_objects`). Then it makes the outpost's call and reads both windows' counts again.

The control case does the same, except that step 3 is replaced by a check that A still has the focus.

The program runs 20 rounds of the moved case and then 20 rounds of the control case. Each call prints a `RESULT` line with these fields:

- the window that had the focus just before the call;
- the processes that the element's cached provider description names, as A, B or another process;
- the element's cached name;
- the call's error, if it failed;
- the calls each window counted during the call, by method;
- the microseconds from the check to the call's return, and the call's own duration;
- the description itself.

At the end the program prints one `SUMMARY` line per case. The outpost's request does not cache the process id property, so the program takes the process id from the cached provider description, which names the provider's process (`[pid:<n>,`, as in `experiments/focus-pipeline-rework-measurements/provider-description.txt`). Using the description keeps the request exactly the outpost's.

mockapp's counters are kept per window, not per node, and A and B are separate processes. So when B's counters rise during the call, B's provider was called. The deciding figures are B's element in the cache (B's process and the name "Second") together with a rise in B's `GetFocus`, and, when UIA opens a connection, B's `WM_GETOBJECT` for `UiaRootObjectId`.

Another UIA client on the machine, such as NVDA, Narrator, a running Verbatim or an inspection tool, would call into B when the focus moves. That would add calls to B's counts that are not the program's. So nothing else may run during the experiment. The control case shows what B's counters do when the program does not call into B.

## Rerunning

Start at 93691570 or later. Apply `scratch-support.patch`, copy the program to `crates/mockapp/tests/`, build it with `cargo test -p mockapp --test getfocusedelement_after_the_focus_moves --no-run`, and run it on an unlocked interactive desktop. Nothing else may use the desktop while it runs. Like the harness's `launch_titled`, it does not need the foreground right, because a window that is restored from minimized takes the foreground whatever input came last (`docs/tooling/troubleshooting.md`, "Windows' foreground lock keeps launched applications behind"). If a window still refuses the foreground, the program fails with the step that got no evidence.

## Results

Run on 2026-10-29 on the development machine's console, Windows 11 25H2, debug build, no other UIA client running; the full output is `results.txt`.

- Focus moved to B after the check: in 20 of 20 rounds the call returned B's element ("Second", B's process in its provider description), and B's provider was called each time, A's never. Each round B answered `WM_GETOBJECT` with `UiaRootObjectId` (UIA's connection request), `GetFocus`, and about 70 provider calls in all (`ProviderOptions` 14, `GetPatternProvider` 14, `GetPropertyValue` 22, among others).
- Control, focus left on A: in 20 of 20 rounds the call returned A's element and B was never called.
- From the check to the call's return: 24.0 ms at the fastest, 27.9 ms at the median and 40.1 ms at the slowest, the experiment's own move of the focus included.

So when the focus moves to another application between the outpost's check and its `GetFocusedElementBuildCache`, the call connects into that application and returns its element: shown here with the order forced. How often the order happens unforced is measured by the scratch build's `focused_read` line (`experiments/focus-pipeline-rework-measurements/`).
