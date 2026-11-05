# A call waits its timeout from its own start

## Fact

A UIA call on an element already obtained waits the transaction timeout, and each call's wait starts at its own start: a call queued 200 ms behind a stuck one failed 2,198 ms after its own start under a 2-second timeout, not when the first did (`books/dev/src/explanation/windows/uia.md`, "Timeouts and cancelling"). The client waits the timeout less 200 ms as a count of waits that each last about 110 ms, then one of 205 ms, so a 2-second timeout fails after about 2.2 s, 5 seconds after 5.5 and the 20-second default after 22.1; E-TT's single reads at a 1-second timeout failed after 1,082 to 1,095 ms. With the system timer at 1 ms the wait is the same, so the extra tenth is not the timer's tick. The provider side never learns of the timeout: after a hold it still answers the calls that already failed. A remote operation is one transaction, so the timeout bounds it as a whole.

## What Verbatim relies on

The listener's read deadline, which adds `TRANSACTION_TIMEOUT` (`crates/verbatim-listener/src/focus_read.rs`), the outpost's walk budget (`crates/verbatim-outpost/src/outpost/read.rs`), the supervisor's 20-second retirement bound (`crates/verbatim-supervisor/src/retire.rs`), and `crates/verbatim-uia/src/bounded.rs`'s abandonable thread for operations of several calls. If waits lengthened, or a remote operation's bound changed, those deadlines would fire early or late.

## Method

The kept CI test `a_stalled_provider_holds_execute_until_the_transaction_timeout` (`crates/mockapp/tests/remote_ops.rs`) shows that a remote operation is bounded as one transaction. The program covers plain calls, which needs timed waits a test may not make.

Every case runs against the provider program with one UIA window, the elements fetched before the provider holds node 1's `GetPropertyValue`, the connection timeout 2,000 ms unless named.

- `waits`: `CurrentName` of node 1, held, at transaction timeouts of 2,000, 5,000 and 20,000 ms. Then E-TT's reads with both timeouts at 1,000 ms: `CurrentName` and `CurrentHasKeyboardFocus` of node 1, held.
- `own-start`: at 2,000 ms, node 1's read held on one thread and, 200 ms later (a deliberate offset), node 2's read on another; the second read's wait from its own start, and, after the release, whether the provider log shows node 2's read served (watched for up to 5 s).
- `timer`: the 2,000 ms wait with the system timer at 1 ms (`timeBeginPeriod(1)`).

Judged: the HRESULT (deterministic), each wait (a band from its timeout), the late service (a relation). `timer`, an open question whose first run is its record, is judged by the same band as `waits`.

## Client settings

Event coalescing changes no outcome, and connection recovery changes one, E-TT's second read (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both). The documents state the default's outcome, which the default command judges.

- Connection recovery off, the default: each of E-TT's held reads fails after one timeout, `CurrentName` and `CurrentHasKeyboardFocus` alike. `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --case waits`
- Connection recovery on, as Verbatim's client has it: the first held read still fails after one timeout, but a read made right after one into the same application timed out fails at once, `CurrentHasKeyboardFocus` in 0.1 ms. `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --recovery --case waits`

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| A held read at 2, 5 and 20 s: 2,190, 5,516 and 22,158 ms; the queued read 2,198 ms from its own start; the provider answering both reads after the release | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Timeouts" (`timeouts.ps1`, the trace read by `blocked-summary.py`) | `UIA_E_TIMEOUT`; bands 2,000 to 2,350, 5,000 to 5,800 and 20,000 to 23,050 ms |
| E-TT's single reads at 1 s: `CurrentName` 1,095 ms, `CurrentHasKeyboardFocus` 1,092 ms; 1,088 and 1,082 ms on 2026-11-02 | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989; the rerun in [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md)'s `recorded/e-tt-rerun.txt` | Band 984 to 1,200 ms |
| First uia-facts run, `waits`, E-TT's reads: `UIA_E_TIMEOUT` 5 of 5 each; `CurrentName` 1,092 ms (1,090 to 1,094), `CurrentHasKeyboardFocus` 1,093 ms (1,088 to 1,095) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 1682f8cb | Pass |
| First uia-facts run, `own-start`: both reads `UIA_E_TIMEOUT` 5 of 5; the queued read 2,203 ms (2,196 to 2,204) from its own start; node 2's read served after the release 5 of 5 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `waits`: `UIA_E_TIMEOUT` 5 of 5 at each timeout; 2,197.8 ms (2,190.9 to 2,199.6), 5,517.6 ms (5,515.3 to 5,527.6) and 22,133 ms (22,124 to 22,136) | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `timer`: `UIA_E_TIMEOUT` 5 of 5, 2,201.2 ms (2,199.3 to 2,207.4) with the system timer at 1 ms | 2026-11-05 | As above | As above | This record: the wait is unchanged, so the 1.1 times is not the system timer |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-call-waits-its-timeout-from-its-own-start.exe`, or one case with `--case`. No desktop or application; the guide needs cdb, and the trace needs the main workspace's `cargo xtask trace`. The test: `cargo test -p mockapp --test remote_ops -- --exact a_stalled_provider_holds_execute_until_the_transaction_timeout`.

## Debugger guide

The slices, in a context-switch trace:

1. In an elevated console, from the repository root, run `cargo xtask trace start`, then `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --case waits`, and `cargo xtask trace stop` after its first 2-second trials (`books/dev/src/how-to/measuring.md`, "The trace commands themselves").
2. Run `cargo xtask trace waits <trace> --symbol-server --blocked a-call-waits-its-timeout-from-its-own-start.exe:*` and read the client's calling thread. Expect it blocked 18 times in `UIAutomationCore!ChannelBasedClientConnection::WaitForResponse` on `WaitOnAddress`, each wait 108 to 111 ms and ended by a timer's expiry, then once for about 205 ms in `RequestResponseCoordinator::WaitForResponse`, then failing: about 2,197 ms. At 5 s, 48 such waits. It matches when the count of waits is the timeout less 200 ms divided by about 110 ms.
3. The provider side: the provider program's I/O thread, or its UI thread under `UseComThreading`, blocked from the held request until the release, in its cross-apartment call (`UIAutomationCore!ComInvoker::CallTarget`, `combase!MTAThreadWaitForCall`), long after both client calls failed.

The one whole wait under cdb:

1. Run `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --case waits --pause`, attach invasively, `cdb -p <client pid>` with Microsoft's symbols, `bp UIAutomationCore!ChannelBasedClientConnection::GetTimeout "gu; r rax; g"` and `g`, then press Enter.
2. Expect `GetTimeout` to return the transaction timeout (2,000), then one `WaitOnAddress` of that length in `RequestResponseCoordinator::WaitForResponse`, failing after about 2,022 ms; at 20 s, about 20,013 ms. An invasive debugger changes how the client waits, which is why the figures above come from runs without one.

## Evidence kept

None of its own: the trace and timings stay in `experiments/parallel-uia-calls/results/timeouts.txt` until that folder's last facts move.
