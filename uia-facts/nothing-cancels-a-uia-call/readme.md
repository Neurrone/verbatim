# Nothing cancels a UIA call

## Fact

`CoCancelCall` cancels a COM call outstanding on a thread, and a UIA client call is not one: it waits in UIA's own channel (`ChannelBasedClientConnection::WaitForResponse`, on `WaitOnAddress`, with no COM frame on its stack). Every cancel answers `RPC_E_NO_CONTEXT` and changes nothing, for property reads, text calls, `FindAll`, remote operations and new connections, with cancellation enabled or not: 5,614 of 5,614 over 203 trials. A held call returns at the provider's answer or at one of UIA's own timeouts. Abandoning a call, leaving its thread waiting and making later calls on new threads, only frees the client's wait: every later call into the stuck process waits for the release, and the first answer after it comes within 2 ms (`books/dev/src/explanation/windows/uia.md`, "The parts" and "Timeouts and cancelling").

## What Verbatim relies on

`crates/verbatim-uia/src/bounded.rs` and the outpost watchdog, which bound calls by abandoning their thread. If `CoCancelCall` began to work, nothing breaks; Verbatim is only more conservative than needed.

## Method

The caller is a thread of the multithreaded apartment that has called `CoEnableCallCancellation`, holding one client object and the elements it reads, fetched before each trial; the main thread cancels with `CoCancelCall(caller's thread id, 0)`, as NVDA's watchdog does. Each trial uses a new provider program with one UIA window, connection timeout 2,000 ms (500 ms for `connection`), transaction timeout 30,000 ms.

Kinds, each a case: `property` (`CurrentName` of node 1), `text` (`GetText(-1)` of node 3's document range), `findall` (`FindAll` of the window's descendants), `remote` (a remote operation reading node 3's name, through `verbatim-uia-rops`) and `connection` (`ElementFromHandle` of the new provider's window). For each kind, each of: the call's first provider method held (`hold *`, or for `connection` `hold GetObject` inside the window's `WM_GETOBJECT`), cancelled once at once or every 50 ms, released at 500 ms; or the UI thread busy for 500 ms, cancelled once the call has reached it (the busy thread logs the call's message arriving), once or every 50 ms. `connection` holds or stalls 800 ms against its 500 ms connection timeout, so the timeout comes first. The hold and the 50 ms pace are deliberate, the 50 ms the original's pace. `property` then makes the read once more, its UI thread stalled, on a caller that never enabled cancellation, cancelled once.

- `abandon`: node 1's name read held for 1,000 ms; later reads of node 2 at 250, 500 and 750 ms on new threads (`new`), or on the stuck read's own thread once it has failed on a 500 ms transaction timeout, while the provider is still stuck (`same`); released at 1,000 ms.

Judged for each kind and condition on its own: every cancel's answer (a relation), the call's outcome (deterministic), that it returned only at the release or the stall's end (a relation), and for `connection` that it failed at the connection timeout before the release (a band).

## Client settings

Event coalescing changes no outcome, and connection recovery changes one, `abandon`'s `same` (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both; `abandon` again on 2026-11-06 after its stuck read was given a timeout shorter than the hold). The program judges each setting against its own record.

- Connection recovery off, the default: the later reads on the stuck read's thread wait for the release and then succeed. `target\debug\nothing-cancels-a-uia-call.exe --case abandon`
- Connection recovery on, as Verbatim's client has it: once the stuck read has timed out, each later read made on its thread into the same application fails at once, in 0.1 ms, with `UIA_E_TIMEOUT`, before the release. `target\debug\nothing-cancels-a-uia-call.exe --recovery --case abandon`

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Every cancel `RPC_E_NO_CONTEXT`, 5,614 of 5,614 over 203 trials; held calls returned 0.0 to 2.9 ms after the release; new connections failed `UIA_E_TIMEOUT` before the release | 2026-10-30 | Build 26200 | The `CoCancelCall` experiment of 2026-10-30, run at 5797c075; its outputs are in `recorded/` | Deterministic; band 1,984 to 2,350 ms at a 2,000 ms timeout; 484 to 625 ms at the 500 ms the case now sets |
| Abandoning: every later call waited for the release; the first answer after it within 2 ms either way | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Abandoning a stuck call" | A relation; the 2 ms for trends |
| First uia-facts run, `property`, `text`, `findall`, `remote`: every cancel `RPC_E_NO_CONTEXT` (294 cancels in 12 trials each), every call `ok`, each returning only at the release or the stall's end | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 1682f8cb | Pass |
| First uia-facts run, `connection`: every cancel `RPC_E_NO_CONTEXT`; `UIA_E_TIMEOUT` 12 of 12, each before the release; 2,008.5 ms (1,999.8 to 2,022.2) | 2026-11-05 | As above | As above | The band inconclusive; the rest pass |
| First uia-facts run, `property`, cancellation never enabled: `RPC_E_NO_CONTEXT` 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `abandon`: every later read `ok` and only after the release, on new threads and on the same thread; the first answer after the release 0.8 ms (new threads) and 0.5 ms (same thread) | 2026-11-05 | As above | As above | Pass; the times for trends |

One trial's failure came at 1,999.8 ms, a fifth of a millisecond before the 2,000 ms timeout, with the median inside the band. A new connection's wait is a window message's (`SendMessageTimeoutW`), whose timer is the system's, not this program's clock.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\nothing-cancels-a-uia-call.exe`, or one kind or case with `--case`. No desktop or application; the guide needs cdb.

## Debugger guide

Where a UIA client call waits, the original `e_stacks`:

1. Run `target\debug\nothing-cancels-a-uia-call.exe --case property --pause`. At each trial's pause it names the provider and client processes and waits for Enter, before the provider method is held.
2. Press Enter, and within the 2.5 s hold take the client's stacks with a non-invasive attach, as the original did: `cdb -pv -p <client pid> -y srv*C:\symbols*https://msdl.microsoft.com/download/symbols -c "~*k 30; qd"`.
3. Expect the caller thread's frames, from the top: `ntdll!NtWaitForAlertByThreadId`, `ntdll!RtlWaitOnAddress`, `KERNELBASE!WaitOnAddress`, `UIAutomationCore!ChannelBasedClientConnection::WaitForResponse`, `ChannelBasedClientConnection::DoCallWithReturn`, `CUIAutomationBlockingCore::DoCallWithReturn`, `LocalUiaNodeProxy::RealCrossProcess_GetPropertyValues`, `CAutomationElement::get_CurrentName`. For `--case findall`, `UiaNodeTraverser::Traverse` and `FindingVisitor::Enter` in place of the last.
4. It matches when no `combase` or `rpcrt4` frame is on the caller's stack: the wait is UIA's own, so the cancel finds no COM call. An invasive debugger changes how the client waits (`books/dev/src/explanation/windows/uia.md`, "Timeouts and cancelling"), so attach with `-pv`.

## Evidence kept

`recorded/`: the original outputs, one file per kind (`e_property.txt`, `e_text.txt`, `e_findall.txt`, `e_remote_op.txt`, `e_new_connection.txt`), the control (`e_disabled_control.txt`), the cost of cancellation (`e_cost.txt`), the exit check (`e_exit.txt`), the focus read that could not be aimed (`e_focus.txt`) and the stacks (`e_stacks.txt`).
