# Experiment: the listener's read below another process's window

This experiment settles risk 14 and decision 13 of `docs/design/listener-without-connections.md` before section 10, step 6 is built. It ran on Dickson's x64 machine (Windows 11, build 26200) on 2026-10-19, in a debug build at d441c6be, against mockapp on a desktop of its own (`harness::run_isolated`), with no other UIA client on that desktop.

## The question

When the listener reads a focus in a child window whose parent window belongs to another process, UIA's client side sends the parent `WM_GETOBJECT` with `UiaRootObjectId` and a nonzero wParam, the message a UIA connection begins with, each time a listener call returns the child window's element (decision 13; the boundary test in `crates/mockapp/tests/call_counts.rs`). If the parent's process has another process's remote operation queued at that moment, as an outpost's would be, does the listener's read wait, and for how long, and does it fail or return anything different, compared with the same read with nothing queued?

## Method

`boundary-collision.patch` applies to d441c6be and adds a scratch test binary, `crates/mockapp/tests/boundary_collision.rs`, with three small changes it needs: its `[[test]]` entry in mockapp's `Cargo.toml`; `MockApp::stdin_clone` in the tests' `common` module, so a timer thread can send `release`; and an instrument in mockapp, `log_get_object`, which appends every `WM_GETOBJECT` a window takes, with the time it was taken (microseconds since the Unix epoch), its wParam and lParam, to `getobject-<pid>.log` in `MOCKAPP_GETOBJECT_LOG_DIR` when that is set. The instrument only logs; no mockapp command or behavior was added. The patch was removed from the working tree after the runs.

Processes in each trial, all new for the trial:

- P, the parent: one mockapp.
- C, the child: mockapp over `counts.json` on the UIA backend; its window is made a child of P's top-level window with `SetParent` from the test process, as in the boundary test, and its `First` button focused (`set-focus first`).
- The listener: the test process, reading the focus in C with `OutpostUnderTest::listener_read`, the listener's reader at HEAD, by the classic path (`focus_ancestry_classic`) or the remote one, its UIA timeouts Verbatim's 5 seconds unless said.
- O, the outpost of P's application: the test binary run again as a separate process. It looks up P's UIA window by handle, then runs a first remote operation there, reading the element's name, which sets UIA up in P, before anything is held.

The two cases:

1. P's top-level window has no UIA provider: `boundary_win32_host.json` on the `win32` backend, a plain Win32 window holding a UIA child window ("Embedded") on the same thread, as the Settings frame has no provider. O's operations go into the UIA child window, so they are queued in P's process while C's parent window has no provider. P's thread is held by mockapp's `hold`: O's plain `Name` read of the UIA child window, made on a thread of its own, is held inside P's provider (`held GetPropertyValue` on P's output).
2. P's top-level window has a UIA provider: `small.json` on the `uia` backend, as a WinUI host of WebView2 has. P's thread is stalled by `stall-after-focus-read 1`: O's plain `HasKeyboardFocus` read of P's root is answered, and P prints `stall started`; P's thread then takes no message until `release`.

The arms, run interleaved trial by trial for each case and read path:

- control: nothing held, nothing queued; 10 trials.
- queued-short: P's thread held; O sends a second remote operation into P (the name read again); then the listener reads; P is released 300 ms after the read begins; 10 trials.
- busy-short: as queued-short without O's second operation; 10 trials.
- queued-long and busy-long: as above, but P is released only when the listener's read returns, with a 15-second cap that was never reached; 5 trials each.

A second invocation set the listener process's connection timeout to 2 seconds, decision 4's value (`BC_CTO=2000`; the setting is process-wide, so it ran as a process of its own): control, queued-short and busy-short 5 trials each, queued-long 3, for each case and path.

Commands, from the repo root with the patch applied: `cargo test -p mockapp --test boundary_collision --no-run`, then the binary it names with `--exact boundary_collision` and the environment `BC_OUT` (the output file), `BC_LOG_DIR` (the instrument's folder), `BC_TRIALS=10` (the long arms run half as many), and for the second invocation `BC_CTO=2000 BC_TRIALS=5 BC_ARMS=control,queued-short,busy-short,queued-long`. `python -I summary.py main.txt` prints `main-summary.txt`, and the same for `cto2.txt`.

Files: `main.txt` and `cto2.txt` hold every trial: O's lookup and first operation, the evidence of what P was doing, the listener's read time, call counts and outcome, the release time, O's answers with their times relative to the read's start, P's hit counters since before the hold (O's calls included), and every `WM_GETOBJECT` P took, timed from the read's start (negative before it). `main-summary.txt` and `cto2-summary.txt` group them by case, path and arm. In the summaries, "released at the cap" means the release came at its set time, before the read returned.

## What was in progress in P when the listener's messages were sent

Case 2, queued arms: P printed `stall started` after O's plain read was answered; O then printed `op sent`; P then printed `stall message arrived`, the first message reaching its stalled thread; only then did the listener's read begin (O's `op sent` 0.07 to 0.5 ms before it). Nothing else was sent to P meanwhile, so O's operation had reached P's stalled thread and was waiting there. In the busy arms of case 2, `stall message arrived` came only after the read began, from the listener's own message.

Case 1, queued arms: P printed `held GetPropertyValue`, O's plain read held on P's thread; O printed `op sent` before the read began, and O's operation finished only 1.0 to 11 ms after the release in every trial. That the operation was waiting in P's process, rather than elsewhere, is inferred from that timing and from O's first operation having run in P; mockapp's `hold` gives no evidence of a message reaching the thread.

In every held arm, P's log shows no `WM_GETOBJECT` taken between the hold and the release, and the listener's messages taken within milliseconds after the release, so they were pending at P's thread during the hold.

## Results, at Verbatim's 5-second timeouts

Read times are from the read's start; "the chain" is the listener's ancestors, Settings and then C's window, the same as the boundary test's.

Case 1, the parent without a provider:

- Classic. Control: 4.9 to 8.3 ms (median 6.1), the chain in 10 of 10, 6 UIA calls and 1 window message, P taking 2 `WM_GETOBJECT`s with nonzero wParam. Queued-short: 308.9 to 327.4 ms (median 315.9), released at 304.1 to 322.5 ms, the chain in 10 of 10, the same calls. Busy-short: 307.9 to 320.8 ms (median 315.8), the chain in 10 of 10.
- Remote. Control: 12.8 to 16.1 ms (median 14.1), the chain in 10 of 10, 3 UIA calls and 1 window message. Queued-short: 321.7 to 340.7 ms (median 332.3), released at 303.0 to 316.0, the chain in 10 of 10. Busy-short: 323.2 to 335.8 ms (median 328.3), the chain in 10 of 10.
- So with the parent held for 300 ms, the read waits for the hold and ends 3 to 37 ms after the release, alike with and without O's operation queued; its result and calls are the control's.

Case 2, the parent with a provider:

- Classic. Control: 6.6 to 9.5 ms (median 7.9), the chain in 10 of 10, 6 UIA calls and 1 window message. Queued-short: 5,310.9 to 5,325.7 ms (median 5,319.3), released at 301.3 to 311.0 ms, the chain in 10 of 10, the same calls. Busy-short: 311.0 to 322.6 ms (median 317.3), the chain in 10 of 10.
- Remote. Control: 14.6 to 19.2 ms (median 16.2), the chain in 10 of 10, 3 UIA calls and 1 window message. Queued-short: 5,322.6 to 5,344.8 ms (median 5,333.3), released at 301.3 to 315.1, the chain in 10 of 10. Busy-short: 317.4 to 339.2 ms (median 324.4), the chain in 10 of 10.
- So with O's operation queued, the read waits the hold and then about 5 seconds more, in 20 of 20 trials, and still returns the control's chain with the control's calls; with only the hold, it ends a few milliseconds after the release.
- What P took (`UiaRootObjectId` messages from the read's start, N for a nonzero wParam, Z for zero): control and busy-short "N Z N Z", 20 of 20 each; queued-short "N N N Z", 20 of 20, the last N taken 5,005.7 to 5,011.3 ms after the release (classic) and 5,015.0 to 5,028.2 ms (remote). The first N is taken at the release; the second, also at the release, is O's operation's (queued-long, in which the listener's messages are never taken, shows exactly one N after the release); then nothing for one timeout, then "N Z". So the listener's first message is answered at the release, the zero-wParam message that follows it in the control never comes, and the read goes on only one connection timeout later.
- With a provider, the listener's read also reaches P's provider: in the controls, P counted 4 `WM_GETOBJECT`s, 12 `ProviderOptions`, 4 `HostRawElementProvider` and 4 `Navigate` calls per read, by either path, where the parent without a provider counted 2 `WM_GETOBJECT`s and nothing else.

Long holds, both cases and both paths (5 trials of each arm):

- Queued-long and busy-long alike: the read returned after 5,000.9 to 5,016.4 ms with the focus dropped ("it has no window above it, so NVDA makes no object for it"), after 1 UIA call and no window message, in 40 of 40. The nearest-window lookup timed out waiting for P's thread. P never took the listener's message: in busy-long P took no `UiaRootObjectId` message at all after the release, and in queued-long only O's.

O's operation: it returned the application's own element's name ("Embedded" in case 1, "Mockapp Events Fixture" in case 2) in every trial of every arm, 1.0 to 11 ms after the release. The listener's messages did not make it fail.

## Results, with the listener's connection timeout at 2 seconds

- Case 2, queued-short: classic 2,306.7 to 2,325.9 ms (median 2,324.7), remote 2,333.9 to 2,346.6 ms (median 2,341.7), the chain in 10 of 10, P's messages again "N N N Z" with the last N 2,003.7 to 2,041.4 ms after the release. Busy-short: 312.2 to 328.3 ms.
- Case 1, queued-short: 304.5 to 353.3 ms, the chain in 10 of 10; busy-short 314.3 to 331.9 ms.
- Queued-long, both cases and paths: the focus dropped after 2,002.3 to 2,016.0 ms, 12 of 12.
- Controls as at 5 seconds, the chain in 20 of 20; O's operation answered correctly in every trial.

So both the extra wait and the drop follow the connection timeout.

## Conclusion

Established, in mockapp:

- (a) The listener's read waits for the parent's UI thread whenever the parent is busy, whether or not a remote operation is queued there, and with or without a provider: the messages UIA's client side sends the parent are sent messages, taken only when that thread pumps.
- When the parent's thread is busy for longer than the listener's connection timeout, the read gives up after that timeout and drops the focus as having no window above it, in both cases and by both paths, with or without a queued operation (40 of 40 at 5 seconds, 12 of 12 at 2 seconds). This is not the collision; it is a dependence of the listener's read on another process's UI thread.
- When the parent has a provider and another process's remote operation is queued there, the read waits one further connection timeout after the parent's thread is free: about 5,000 ms at Verbatim's 5 seconds and about 2,000 ms at 2 seconds, in 30 of 30 trials, against a few milliseconds after the release with only the parent busy (30 of 30). This is the collision's first form, taking place inside the listener's read.
- (b) In that case the read still returns the control's chain, with the control's calls, in 30 of 30: no proxy and no error reach the listener's result, because its chain stops at the child's window and uses nothing the parent's connection would have given. O's operation is not disturbed.
- When the parent has no provider, a queued operation in its process makes no difference: the read ends a few milliseconds after the release, as with the parent only busy (30 of 30 against 30 of 30, across both timeouts).
- With a provider, the listener's read makes provider calls in the parent's process (12 `ProviderOptions`, 4 `HostRawElementProvider`, 4 `Navigate` per read, besides 4 `WM_GETOBJECT`s), so decision 13's "two messages" understates what UIA's client side does for such a parent: it does connect to it.

Inferred, not measured:

- That the extra wait is spent on the connection begun by the listener's first message (in the nearest-window lookup): its answer was taken at the release and the follow-up message of the control never came, but the listener's own calls were not timed one by one.
- That a real WinUI host of WebView2, or `ApplicationFrameHost`, behaves as mockapp's windows do; neither was measured.
- That today's build, which sends the same messages, has the same delays and drops.

## Consequence for step 6

Step 6 makes the classic read the listener's only read; both paths behave alike here, so step 6 neither causes nor removes these effects. Two things follow for the design:

- A focus below a parent window with a provider can be read about one connection timeout late when the parent's application is busy with an outpost's remote operation, which under decision 7 is the host's own outpost; the result is right but late, by about 2 seconds at decision 4's timeout.
- A focus below any parent window of another process, the Settings frame included, is dropped when the parent's UI thread is busy for longer than the connection timeout.

Both come from the listener holding, or being handed, the element of a window whose parent is another process's. Recommended: the listener stops its walks one step lower, at the last element below the child window's own element, and takes the nearest window from a local call (the event's window, or `GetAncestor` from it) rather than from UIA, measured with this experiment's harness and the boundary test for the parent's count of `WM_GETOBJECT`s, to be zero; if UIA still sends the messages when it hands over the event's sender itself, the alternative is accepting both effects and recording them in decision 13 and risk 14 with these figures.
