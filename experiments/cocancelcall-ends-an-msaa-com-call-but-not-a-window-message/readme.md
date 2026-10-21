# Experiment: CoCancelCall ends an MSAA COM call, but not a window message

This experiment tests the recommendation in the "MSAA calls" paragraph of `docs/design/phase6-design.md`, "Time limits that hold (2026-10-11)": that `CoEnableCallCancellation` on the outpost's worker and `CoCancelCall` from its watchdog, as NVDA's watchdog does, could end an MSAA call blocked in a hung application, so that the worker returns with an error instead of being abandoned. Dickson decided on 2026-10-21 to run the experiment and to build nothing in production on its account. It ran on Dickson's x64 machine (Windows 11, build 26200) on 2026-10-21, in a debug build at 0d1e7802, against mockapp's MSAA backend on a desktop of its own (`harness::run_isolated`), with no other client on that desktop.

## The question

Can an MSAA call blocked inside a hung application be ended from another thread with `CoCancelCall`, so that the call returns an error? Which of the outpost's MSAA calls can be ended that way, and which cannot? What does the outpost do with such a call today?

## Which of the outpost's MSAA calls are COM calls and which are window messages

The outpost makes MSAA calls only on its worker (`verbatim-ia2`, called from `outpost/read.rs`, `worker.rs`, `text_reads.rs`, `narrowing.rs`, `dialog_text.rs` and `text/edit.rs`). The listener makes none; its reading threads make UIA calls only.

- COM calls through a proxy into the application, which cancellation can reach: every `IAccessible` method on an object an application serves itself (`accName`, `accValue`, `accDescription`, `accKeyboardShortcut`, `accDefaultAction`, `accDoDefaultAction`, `accRole`, `accState`, `accLocation`, `accParent`, `accChildCount`, `accChild`, `accNavigate`, `accFocus`, `accSelection`), `AccessibleChildren` (which calls the object's methods), `WindowFromAccessibleObject` (which walks `accParent`), `IAccIdentity::GetIdentityString`, the `QueryInterface` for it, and the final `Release` of a proxy.
- Window messages: `AccessibleObjectFromWindow` sends `WM_GETOBJECT` to the window, and `AccessibleObjectFromEvent` does the same and then calls `accChild` (a COM call) for a nonzero child id. The messages `verbatim-ia2`'s `window::send` and the list view and edit control reads send with `SendMessageTimeoutW` (half a second, `SMTO_ABORTIFHUNG`) are bounded already.
- Calls on oleacc's own proxies for standard Win32 controls (buttons, list boxes and others whose window does not answer `WM_GETOBJECT`): the object lives in the outpost's process, and its methods read the control with window messages. Nothing about them is a COM call into the application.

## Setup

`cocancelcall.rs` is the scratch test binary that ran, kept here and not built. `scratch-support.patch` applies to 0d1e7802 and adds what it needs: its `[[test]]` entry in mockapp's `Cargo.toml`, `tracing-subscriber` as a development dependency of mockapp (for experiment 3's log capture, with the `Cargo.lock` line), and in mockapp's `src/hits.rs` a scratch switch: with `MOCKAPP_HOLD_PAST_GETOBJECT` set in mockapp's environment, the `hold` command passes over `WM_GETOBJECT` and holds the next provider method, so the COM part of an acquisition can be held. The patch and the binary were removed from the working tree after the runs.

The caller is a thread in the multithreaded apartment, as the outpost's worker is, that has called `CoEnableCallCancellation` unless said otherwise. It makes one call at a time; the test thread cancels it with `CoCancelCall(caller's thread id, timeout)`. Two ways of making mockapp's provider not answer, both on its window thread, which serves every call (its objects live in that single-threaded apartment):

- `stall <ms>`: the window thread sleeps, so a COM call waits queued for it and a window message waits unanswered. The cancel is made 300 ms after the call starts (a measured delay, since nothing in mockapp shows the call queued).
- `hold`: the next provider method announces itself (`held <method>`) and blocks inside the provider until `release`. The cancel is made as soon as the announcement is read, which is evidence that the call is in progress.

Rerunning: apply the patch at 0d1e7802, copy `cocancelcall.rs` to `crates/mockapp/tests/`, and run `cargo test -p mockapp --test cocancelcall -- --exact <name>`, the names as listed in its `main`; `e3_outpost_baseline` holds the COM call when `E3_HOLD_PAST_GETOBJECT` is set in the test's environment and the window message otherwise. Each experiment prints `RESULT` lines and fails only when its setup does. The output files were made by the file as it was at each run: the earlier runs (`e1-name-in-a-stall`, `e1-name-held`, `e1-first-call-held`, `e2-acquisitions-in-a-10s-stall`, `e2-hung-probe`, `e2-getobject-held`) printed fewer fields than the final file does, and `e2-getobject-held` ran `e2_from_event_child_held` before it set `MOCKAPP_HOLD_PAST_GETOBJECT`, so its hold took the `WM_GETOBJECT`.

Error codes below: `RPC_E_CALL_CANCELED` is 0x80010002, `CO_E_CANCEL_DISABLED` 0x80010140, and `RPC_E_NO_CONTEXT` 0x8001011E, which `CoCancelCall` answers when the thread has no COM call outstanding.

## Experiment 1: an `IAccessible` call through the proxy

`e1-name-in-a-stall.txt`. `accName(CHILDID_SELF)` on mockapp's root object, through a proxy that had already made one call; mockapp stalled for 4 seconds; cancelled 300 ms in, 10 trials for each cancel timeout. `CoCancelCall` answered `S_OK` in 0.1 ms or less in all 30.

- Timeout 0: the call returned `RPC_E_CALL_CANCELED` 0.1 to 0.3 ms after the cancel, 10 of 10.
- Timeout 1 second: `RPC_E_CALL_CANCELED` 1,001.3 to 1,012.2 ms after the cancel, 10 of 10.
- Timeout 2 seconds: `RPC_E_CALL_CANCELED` 2,000.3 to 2,015.9 ms after the cancel, 10 of 10.
- So the timeout is how long the caller goes on waiting for the server after the cancel; a server that does not answer is waited for that long. NVDA passes 0.
- The proxy afterwards: a second `accName` once the stall ended answered the name ("Mockapp Events Fixture") in 0.1 to 1.4 ms, 30 of 30. mockapp counted 2 `get_accName` calls in every trial: the cancelled call still ran in the application once its thread was free, and only its answer was discarded.
- The control, a caller that did not enable cancellation: `CoCancelCall` answered `CO_E_CANCEL_DISABLED` and the call waited out the 2-second stall and succeeded, 3 of 3. On an idle caller, `CoCancelCall` answered `RPC_E_NO_CONTEXT`.

`e1-name-held.txt`. The same call held inside the provider: `RPC_E_CALL_CANCELED` 0.0 to 0.1 ms after the cancel, 10 of 10; the second call after the release answered the name.

`e1-first-call-held.txt` and `e12-repeated-cancels.txt`. The first call made through a new proxy is two COM calls: oleacc's proxy calls `accParent` before the method asked for (`e1-name-held.txt`, "hits after first call": `accParent` 1, `get_accName` 1; later calls make no `accParent`). With a new proxy each trial and the `accParent` held:

- One cancel did not end the call: `CoCancelCall` answered `S_OK`, the `accParent` was ended, and the proxy went on to make the `accName`, which waited behind the held thread. A second cancel two seconds later ended that one: the call returned `RPC_E_CALL_CANCELED` after 2,003.6 to 2,010.2 ms, 10 of 10.
- Cancelling every 50 ms, as NVDA's watchdog does (`RECOVER_ATTEMPT_INTERVAL`): 2 cancels, the call returned `RPC_E_CALL_CANCELED` 51.3 to 62.0 ms after the first, 10 of 10.
- In every trial mockapp then counted 1 `accParent` and 1 `get_accName`: the cancelled `accName` ran after the release.

`e1-release-in-a-stall.txt`. The final `Release` of a proxy is a COM call too. Releasing a proxy while mockapp was stalled for 4 seconds waited out the stall, 4,000.2 to 4,000.7 ms, 10 of 10 (5 for an object obtained by `accChild`, 5 for a window's client object). Cancelled 300 ms in, it returned at 300.1 to 300.7 ms after one cancel, 10 of 10.

## Experiment 2: the acquisitions, and oleacc's in-process proxies

`e2-acquisitions-in-a-10s-stall.txt`, `e2-acquisitions-in-a-15s-stall.txt` and `e2-hung-probe.txt`. `AccessibleObjectFromEvent` for the client object at `CHILDID_SELF` and at child 1, and `AccessibleObjectFromWindow`, with mockapp stalled, 3 trials of each with a cancel 300 ms in and 3 without.

- `CoCancelCall` answered `RPC_E_NO_CONTEXT` in all 18 cancelled trials: no COM call was outstanding, since the call was waiting for the `WM_GETOBJECT` to be answered. The cancel changed nothing.
- Stalled for 10 seconds: every call returned at the stall's end, 10,000.3 to 10,006.2 ms, and succeeded, 18 of 18.
- Stalled for 15 seconds: 16 of 18 returned after 10,001.3 to 10,015.4 ms, while mockapp was still stalled and judged hung, and 2 (the first two trials) at the stall's end, 15,000.5 ms. All 18 succeeded, but with oleacc's own default client object rather than mockapp's: its name, read after the stall, was the window's title, not the provider's "Mockapp Events Fixture". So oleacc gives up on `WM_GETOBJECT` after about 10 seconds and hands back its default proxy for the window, with no error; at child 1, `AccessibleObjectFromEvent` then answered that object with child id 1.
- Windows' hung judgement did not end the wait: `IsHungAppWindow` was false 4 seconds into a 10-second stall and true at 6 seconds, and neither acquisition had returned by then (10,009.9 and 10,000.7 ms).

`e2-getobject-held.txt`. `AccessibleObjectFromEvent` at child 1 with mockapp holding the `WM_GETOBJECT` inside its window procedure: released after 300 ms it returned mockapp's child object (3 of 3); cancelled, `CoCancelCall` answered `RPC_E_NO_CONTEXT` and the call went on, returning at the release after 15 seconds in 2 trials and after 10,002.1 to 10,015.5 ms, still held, in 8, with an object and child id 1.

`e2-from-event-child-held.txt` and `e12-repeated-cancels.txt`. `AccessibleObjectFromEvent` at child 1 with the window message answered and its `accChild` held (the scratch switch set). Released after 300 ms, it returned the child's own object at child id 0, named "Original Name" (3 of 3). Cancelled once, `CoCancelCall` answered `S_OK` and the call returned 0.3 to 0.6 ms after it started, 20 of 20: oleacc took the cancelled `accChild` as no child object and answered the root object with child id 1, the form it gives for a simple child, with no error. In `e2-from-event-child-held.txt` the job then released the proxy it held from the previous trial, an object obtained by `accChild`, and that release waited for the held thread, 15,002.6 to 15,015.5 ms, since its cancel had already been spent on the `accChild`; in `e12-repeated-cancels.txt` the previous proxy was a window's client object and its release took 0.0 ms. Why the two releases differ was not established.

`e2-native-proxy-in-a-stall.txt`. A real Win32 push button in mockapp's window (`group_box.json`, "Apply"), whose object is oleacc's in-process proxy. `accName` on it during a 10-second stall waited out the stall (10,000.2 to 10,000.6 ms) and answered "Apply", 6 of 6, with and without a cancel; `CoCancelCall` answered `RPC_E_NO_CONTEXT`. The button's window was not judged hung when the call returned, as the stall had ended. So such a call is a window message sent from the outpost's process, which neither cancellation nor the hung judgement ends.

## Experiment 3: the outpost today

`e3-outpost-getobject-held.txt` and `e3-outpost-com-call-held.txt`. A real outpost in the test process (`OutpostUnderTest`, its time the system's), watching mockapp; `set-focus btn1`, then mockapp told to hold, then an MSAA focus fact on the button handed over as the listener would. The outpost's log was captured.

- With the `WM_GETOBJECT` of the worker's `AccessibleObjectFromEvent` held, and with its `accChild` held (6.6 ms after the fact was handed over), alike: the watchdog logged "the worker is abandoned and replaced" with the reason "a call passed its deadline" 10,000.5 and 10,000.6 ms after the fact, the outpost said nothing about the focus, and a ping answered 1 abandoned worker. After the release, the abandoned worker returned ("an abandoned worker returned; its result is discarded", 10,008 and 10,003 ms), a focus on the slider handed over then was reported 9.2 and 5.0 ms after the release, and a ping answered 0.
- With mockapp stalled for 12 seconds and a focus on the list handed over: the worker was abandoned at 10,000.4 and 10,000.5 ms, mockapp judged hung by then, nothing said, and the abandoned worker returned at the stall's end, 12,000.6 ms.

So today a call that does not return within `HANDLING_DEADLINE`, 10 seconds, costs the entry: nothing is said for it, a replacement worker takes the queue, and the abandoned thread ends when the application answers.

## Experiment 4: cost

`e4-cost.txt`. 1,000 `accName` calls that answer, through one proxy, with cancellation disabled and enabled alternately, 6 rounds: disabled 43.5 to 51.1 ms (median 47.8), enabled 53.9 to 68.6 ms (median 59.7). Enabling cancellation costs about 12 microseconds, a quarter, on every outgoing COM call the thread makes, in this debug build of the test (the cost is in the system's COM code). `CoEnableCallCancellation` itself took 2.0 to 11.4 microseconds.

## Answer

Yes for COM calls, no for window messages.

- An `IAccessible` call through the proxy, blocked in an application that does not answer, returns `RPC_E_CALL_CANCELED` at once when another thread calls `CoCancelCall` with a timeout of 0, queued or inside the provider (40 of 40), and the proxy works afterwards. The cancelled call still runs in the application later.
- One cancel ends one COM call. oleacc makes more than one COM call for some single calls and swallows a cancelled one: the first call through a new proxy (`accParent` first) and `AccessibleObjectFromEvent` with a child id (`accChild`, answered as a simple child when cancelled). A watchdog must cancel repeatedly until the call returns; every 50 ms ended them in 50 to 62 ms.
- The final `Release` of a proxy also blocks in such an application, and is cancellable.
- The acquisitions' `WM_GETOBJECT` and every call on oleacc's in-process proxies for standard controls are window messages: `CoCancelCall` finds no call to cancel, and Windows' hung judgement does not end them either. oleacc gives up on `WM_GETOBJECT` after about 10 seconds and answers its default client object for the window, with no error; the in-process proxy waited out a 10-second stall. These stay guarded by the outpost's hung-window check (an event from a window already judged hung is dropped before any call) and by abandonment.
- Cancellation costs about 12 microseconds on every COM call of a thread that has it enabled.

## What a production change would involve

- Where cancellation is enabled: the outpost's worker, the one thread that makes MSAA calls. The listener's reading threads make UIA calls only; UIA's own timeouts bound those, and whether `CoCancelCall` reaches a UIA client call was not tested. Enabling it on the worker puts about 12 microseconds on each of its COM calls, MSAA and UIA alike, on the event path; that is a cost `docs/performance.md` would have to state, against no wake-up and no thread saved while applications answer.
- The watchdog: at the entry's deadline, cancel the worker's call, and repeat every 50 ms, since one cancel ends only the COM call in flight and oleacc may make another; keep abandonment for a call that does not return, which covers every window message (the acquisitions and oleacc's in-process proxies), a cancel that comes between two calls, and a UIA call. The worker's `MOVED_ON_GRACE` abandonment, which mirrors NVDA's `_shouldRecoverAfterMinTimeout`, could cancel the same way.
- The cancelled error: every `verbatim-ia2` property read already maps a failed call to an absent value, so a cancelled read would let the entry go on with what it has. NVDA does not: its comtypes patch raises `CallCancelled` in place of `RPC_E_CALL_CANCELED` (`monkeyPatches/comtypesMonkeyPatches.py`), which the getters that catch only `COMError` let through, so the event's handling, or the whole core cycle, ends; its `accessibleObjectFromEvent` catches any exception and the event is dropped (`IAccessibleHandler/__init__.py`, `accessibleObjectFromEvent`; `docs/nvda/main-loop-and-watchdog.md`, "When an application stops responding"). Matching NVDA would mean the worker treating `RPC_E_CALL_CANCELED` as the end of the entry, with nothing said.
- What is then heard in a hung application: today, nothing for an entry whose call passes 10 seconds, the entry abandoned; NVDA, after 0.5 seconds when the user has left for another window or 10 seconds otherwise, likewise speaks nothing for the event whose call it cancels. With cancellation mapped to the end of the entry, what is heard stays the same and the worker's thread is kept, not abandoned, for every COM call; mapped to an absent value instead, a focus whose acquisition succeeded would be spoken without what the cancelled reads would have given, where NVDA speaks nothing. Either way, an acquisition stuck in its window message is not shortened, and after oleacc gives up on it the entry reads oleacc's default object for the window, a stand-in named by the window's title, as it does today.
