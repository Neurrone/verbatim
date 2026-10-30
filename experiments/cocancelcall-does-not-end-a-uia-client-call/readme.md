# Experiment: CoCancelCall does not end a UIA client call

This experiment is the first item of `docs/roadmap.md`'s M6.1, brought forward by Dickson on 2026-10-30: whether `CoCancelCall` ends a UIA client call blocked in an application that does not answer, as `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/` showed it ends an `IAccessible` call. It ran on Dickson's x64 machine (Windows 11, build 26200) on 2026-10-30, in a debug build at 5797c075, against mockapp's UIA backend (`text.json`) on a desktop of its own (`harness::run_isolated`), with no other client on that desktop.

## Setup

`cocancel_uia.rs` is the scratch test binary that ran, kept here and not built. `scratch-support.patch` applies to 5797c075 and adds its `[[test]]` entry to mockapp's `Cargo.toml`; mockapp needed no new command, because its `hold` command already holds the next provider method of either backend, whatever the client, and `stall` blocks the window thread that serves every provider call (mockapp's UIA providers use COM threading, so their calls arrive in the window thread's apartment). The patch and the binary were removed from the working tree after the runs.

Rerunning: apply the patch at 5797c075, copy `cocancel_uia.rs` to `crates/mockapp/tests/`, and run `cargo test -p mockapp --test cocancel_uia -- --exact <name>`, the names as listed in its `main`. `e_stacks` needs `CDB` set to WinDbg's `cdb.exe` and downloads Microsoft's symbols. Each experiment prints `RESULT` lines and fails only when its setup does.

The caller is a thread in the multithreaded apartment, as the outpost's worker and the listener's reading threads are, that has called `CoEnableCallCancellation` (except in `e_disabled_control`). It holds one `CUIAutomation8` client, mockapp's window element, the text element "Notes" and its document range, and makes one call at a time. The test thread cancels with `CoCancelCall(caller's thread id, 0)`, as NVDA's watchdog does (`nvda/source/watchdog.py`, `_recoverAttempt`), either once or every 50 ms (NVDA's `RECOVER_ATTEMPT_INTERVAL`). Each kind of call ran in five arms:

- `control`: the call's first provider method held and released after 300 ms, no cancel, 3 trials.
- `hold-once` and `hold-repeat`: the first provider method held (mockapp prints `held <method>`, the evidence that the call is in progress); cancelled at once, once or every 50 ms, and released after 5 seconds if the call had not returned. 10 trials each.
- `stall-once` and `stall-repeat`: the window thread stalled for 3 seconds, the call made, cancelled 300 ms in, once or every 50 ms until it returned. 10 trials each.

After each trial: the provider calls mockapp counted for it, read 200 ms after the release or the stall's end, and 30 follow-up pairs, each the held element's `CurrentName` and a new `ElementFromHandle` of the window with its `CurrentName`.

Error codes below: `RPC_E_NO_CONTEXT` is 0x8001011E, which `CoCancelCall` answers when the thread has no COM call outstanding; `UIA_E_TIMEOUT` is 0x80131505.

## Where a UIA client call waits

`e_stacks.txt`. With a `CurrentName` and a `FindAll` held, the caller thread's stack (cdb, non-invasive) was, from the top: `ntdll!NtWaitForAlertByThreadId`, `RtlWaitOnAddress`, `KERNELBASE!WaitOnAddress`, `uiautomationcore!ChannelBasedClientConnection::WaitForResponse`, `ChannelBasedClientConnection::DoCallWithReturn`, `CUIAutomationBlockingCore::DoCallWithReturn`, `LocalUiaNodeProxy::RealCrossProcess_GetPropertyValues`, and the client's own method (`CAutomationElement::get_CurrentName`; for `FindAll`, `UiaNodeTraverser::Traverse` and `FindingVisitor::Enter`, so the client walks the tree itself, one cross-process call per step). No `combase` or `rpcrt4` frame is on it: UIA's client sends its calls over a channel of its own and waits on an address, not through a COM proxy, so the thread has no COM call for `CoCancelCall` to find, and it answered `RPC_E_NO_CONTEXT` both times.

## Results

Every cancel of every kind, 5,614 in all over 203 trials, answered `RPC_E_NO_CONTEXT` and changed nothing:

- A property read, `CurrentName` (`e_property.txt`; held at `FragmentRoot`, the first method UIA asks): held, the call returned only at the release, 5,000.5 to 5,063.4 ms after the first cancel, 20 of 20, with the name, 0.0 to 0.6 ms after the release; stalled, it returned at the stall's end, 3,000.3 to 3,004.2 ms after it was made, 20 of 20, with the name.
- A text pattern call, `GetText(-1)` on the document range (`e_text.txt`; held at `GetText`): the same, the text returned at the release or the stall's end, 40 of 40.
- `FindAll` over the descendants (`e_findall.txt`; held at the first `GetPropertyValue`): the same, all 7 elements returned at the release or the stall's end, 40 of 40.
- A remote operation reading the text element's name (`e_remote_op.txt`; held at `ProviderOptions`): the same, the operation succeeded at the release or the stall's end, 40 of 40.
- Across the text, `FindAll` and remote operation arms: held, 5,001.4 to 5,062.4 ms after the first cancel and 0.0 to 2.9 ms after the release; stalled, 3,000.5 to 3,003.7 ms after the call was made.
- The first call on a new connection, `ElementFromHandle` on a new mockapp each trial (`e_new_connection.txt`; held inside mockapp's `WM_GETOBJECT` handler, `held GetObject`): the call failed with `UIA_E_TIMEOUT` 2,000.6 to 2,015.0 ms after the first cancel, before the release, 20 of 20; stalled, `UIA_E_TIMEOUT` 2,000.5 to 2,015.1 ms after it was made, while mockapp was still stalled, 20 of 20. That is UIA's connection timeout (2 seconds, its default) ending the window message, not the cancel; the same timeout ends the call uncancelled.
- The keyboard focus, `GetFocusedElement` (`e_focus.txt`): not measured. On the test's own desktop no window is the foreground, so UIA answered without any call into mockapp and nothing could be held; the run failed at its first hold. The stacks above show the transport every element call uses, so a focus read through a provider is inferred to behave as the property read does.
- The control, a caller that did not enable cancellation (`e_disabled_control.txt`): the same `RPC_E_NO_CONTEXT`, 3 of 3, and on an idle caller as well, so enabling cancellation made no difference to what a cancel reaches.

A call waiting in UIA's channel (shown by the stacks for a property read and `FindAll`, and inferred for the text call and the remote operation, which answer the cancel alike) and the new connection, which waits for its window message (where on the client's side was not captured), are told apart by how they end, the first at the provider's answer and the second at the connection timeout, but not by the cancel: neither is a COM call on the caller's thread.

### Afterwards

- The client and the element: every follow-up call succeeded in every trial of every kind, 30 of 30 each, the slowest pair 0.8 to 12.1 ms. In the new connection's cancelled trials the timed-out lookup had returned no element, so only the client's 30 lookups ran there, and all succeeded.
- Later calls and faults: no follow-up failed and no call returned an error but the new connection's timeouts. `e_exit.txt`: 10 child processes that made 10 held name reads each, cancelled every 50 ms for 500 ms, and then exited normally (with DLL detach code, which the test runner otherwise skips), and 10 that made the same reads uncancelled: all 20 exited with code 0, none hung. The known exit fault in `UIAutomationCore.dll` (`docs/uia-bugs.md`, "A process that used UIA can hang or crash as it exits") occurs in 1 to 3 runs in 100, so 20 runs cannot show that cancels leave it unchanged, only that they did not make it common.
- The call in the provider: since no call was cancelled, each ran in mockapp as in the control, the same provider calls counted (for `FindAll`, 27 `WM_GETOBJECT`, 23 `ProviderOptions`, 9 `Navigate` and the rest, in every arm). For the new connection whose client had timed out, the provider still answered the rest of the connection after the release (`ProviderOptions` 2, `HostRawElementProvider` 1, `Navigate` 1), work nobody read; stalled, mockapp counted nothing, as the timed-out window message was not delivered.

### Cost

`e_cost.txt`. 1,000 `CurrentName` reads through one element, cancellation disabled and enabled in turn, 6 rounds: disabled 92.7 to 116.0 ms (median 106.7), enabled 91.7 to 132.4 ms (median 100.8). No cost is measurable: a UIA call makes no COM call on the thread, so the per-call cost the MSAA experiment found (about 12 microseconds on each COM call) applies to a thread's MSAA calls only. `CoEnableCallCancellation` took 2.6 to 10.4 microseconds.

## Answer

No. `CoCancelCall` does not end any UIA client call measured: a property read, a text pattern call, `FindAll`, a remote operation, or the first call on a new connection. UIA's client makes its cross-process calls over its own channel and waits on an address, not through COM, so the cancel finds no COM call (`RPC_E_NO_CONTEXT`), with or without cancellation enabled, once or every 50 ms. A held call returns when the provider answers, or when one of UIA's own timeouts ends it (the 2-second connection timeout for a new connection; the transaction timeout, 20 seconds by default, for a call on a held element, not reached here). The client and its elements stay usable, and nothing faulted. This confirms the inference in `docs/uia-bugs.md` drawn from NVDA #20170, whose watchdog cannot cancel these calls either and abandons the thread making them.

## What it means for the outpost and the listener

- Abandonment stays for every UIA call: the listener's reading threads and the outpost's worker cannot be freed by cancelling, so `READ_DEADLINE`, the bound on an application's abandoned reads (`ABANDONED_LIMIT`), `WatchedRead`'s claim and the late-result discard all stay, and the focus pipeline rework's publication at the bound by the watchdog (`docs/design/focus-pipeline-rework.md`) remains the only way to meet the bound while a UIA call is stuck.
- Owed work and abandonment in the outpost: a worker stuck in a UIA call cannot be cancelled back to attempt what it owes, so the watchdog's commit of the published focus, with what is still owed, and the abandoned worker's discarded hand-over are needed as designed.
- MSAA cancellation, if built, would reach only the outpost worker's `IAccessible` calls; its cost would fall only on those COM calls, and a worker stuck in a UIA call would still be abandoned.
