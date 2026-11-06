# A call waits its timeout from its own start

## Fact

A UIA call on an element already obtained waits the transaction timeout, and each call's wait starts at its own start: a call queued 200 ms behind a stuck one failed 2,198 ms after its own start under a 2-second timeout, not when the first did (`books/dev/src/explanation/windows/uia.md`, "Timeouts and cancelling"). The client waits the timeout less 200 ms as a count of waits that each last about 110 ms, then one of 205 ms, so a 2-second timeout fails after about 2.2 s, 5 seconds after 5.5 and the 20-second default after 22.1; E-TT's single reads at a 1-second timeout failed after 1,082 to 1,095 ms. With the system timer at 1 ms the client waits about the timeout itself, 1,001.5 to 1,028.5 ms at 1 second. The provider side never learns of the timeout: after a hold it still answers the calls that already failed. A remote operation is one transaction, so the timeout bounds it as a whole. With the client's connection recovery on, a read made right after one into the same application timed out fails at once, from another thread and through another client object of the process alike, and reads go on failing at once until about 2 seconds after the application answers again.

## What Verbatim relies on

The listener's read deadline, which adds `TRANSACTION_TIMEOUT` (`crates/verbatim-listener/src/focus_read.rs`), the outpost's walk budget (`crates/verbatim-outpost/src/outpost/read.rs`), the supervisor's 20-second retirement bound (`crates/verbatim-supervisor/src/retire.rs`), and `crates/verbatim-uia/src/bounded.rs`'s abandonable thread for operations of several calls. If waits lengthened, or a remote operation's bound changed, those deadlines would fire early or late.

## Method

The kept CI test `a_stalled_provider_holds_execute_until_the_transaction_timeout` (`crates/mockapp/tests/remote_ops.rs`) shows that a remote operation is bounded as one transaction. The program covers plain calls, which needs timed waits a test may not make.

Every case runs against the provider program with one UIA window, the elements fetched before the provider holds node 1's `GetPropertyValue`, the connection timeout 2,000 ms unless named.

- `waits`: `CurrentName` of node 1, held, at transaction timeouts of 2,000 and 5,000 ms. Then E-TT's reads with both timeouts at 1,000 ms: `CurrentName` and `CurrentHasKeyboardFocus` of node 1, held. The 20-second default's 22.1 s is not run, since a 22-second wait is past the program's limit; its claim stays in the documents, from the original's record, and is not tried.
- `own-start`: both timeouts 1,000 ms, node 1's read held on one thread and, 200 ms later (a deliberate offset), node 2's read on another; the second read's wait from its own start, and, after the release, whether the provider log shows node 2's read served (watched for up to 5 s). Each trial has a provider and a client object of its own, so that under connection recovery one trial's timed-out reads do not fail the next at once.
- `timer`: the 1,000 ms wait with the system timer at 1 ms (`timeBeginPeriod(1)`). The program reads the resolution in effect (`NtQueryTimerResolution`) and prints it, for `waits` too; when it is not 1 ms or finer, the claim is inconclusive.
- `mark`: both timeouts 1,000 ms; node 1's read held until it times out; then, the provider still held, node 1 read from another thread through the same element, and through an element another client object of this process fetched before the hold, in turn; then the provider released, and node 1 read every 10 ms until a read succeeds, watched up to 10 s. Connection recovery's mark, by which a read made right after one into the same application timed out fails at once: whether it reaches another thread and another client object, and how long it lasts after the application answers again.

Judged: the HRESULT (deterministic), each wait (a band from its timeout), the late service (a relation). `waits` is judged by the band of the timeout and a tenth, from 1.055 times the timeout to 1.15 times it plus 50 ms, and `timer` by the band of the timeout itself, from the timeout less a tick to 1.055 times it, so that the two are told apart: at 1 s the bands meet at 1,055 ms, 26.5 ms above the longest wait recorded with the timer at 1 ms (1,028.5 ms) and 26.3 ms below the shortest without it (1,081.3 ms), each more than a tick of the default timer (15.6 ms). `mark`'s answers were new on their first runs and are its record: each of the two reads failing at once or not (deterministic, for the setting in use), and, with recovery on, the mark's length after the release (a band by the same rule from the first run's 2,004 ms, since it looks set by UIA rather than by the machine); with recovery off, the first read after the release succeeding.

## Client settings

Event coalescing changes no outcome, and connection recovery changes E-TT's second read and `mark` (checked on 2026-11-05, and for `mark` on 2026-11-06, by running every case by default and once each with `--recovery`, `--coalescing` and both). The documents state the default's outcome, which the default command judges.

- Connection recovery off, the default: each of E-TT's held reads fails after one timeout, `CurrentName` and `CurrentHasKeyboardFocus` alike; in `mark` each read waits its own timeout, and the first read after the release succeeds. `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --case waits --case mark`
- Connection recovery on, as Verbatim's client has it: the first held read still fails after one timeout, but a read made right after one into the same application timed out fails at once, `CurrentHasKeyboardFocus` in 0.1 ms; in `mark` the reads from another thread and through another client object fail at once too, and reads fail at once until about 2 seconds after the release. `target\debug\a-call-waits-its-timeout-from-its-own-start.exe --recovery --case waits --case mark`

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| A held read at 2, 5 and 20 s: 2,190, 5,516 and 22,158 ms; the queued read 2,198 ms from its own start; the provider answering both reads after the release | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "Timeouts" (`timeouts.ps1`, the trace read by `blocked-summary.py`) | `UIA_E_TIMEOUT`; bands 2,000 to 2,350, 5,000 to 5,800 and 20,000 to 23,050 ms |
| E-TT's single reads at 1 s: `CurrentName` 1,095 ms, `CurrentHasKeyboardFocus` 1,092 ms; 1,088 and 1,082 ms on 2026-11-02 | 2026-10-10 | Build 26200 | the mockapp experiments, in git at b0a1989; the rerun in [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md)'s `recorded/e-tt-rerun.txt` | Band 984 to 1,200 ms |
| First uia-facts run, `waits`, E-TT's reads: `UIA_E_TIMEOUT` 5 of 5 each; `CurrentName` 1,092 ms (1,090 to 1,094), `CurrentHasKeyboardFocus` 1,093 ms (1,088 to 1,095) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | c84ab856 | Pass |
| First uia-facts run, `own-start`: both reads `UIA_E_TIMEOUT` 5 of 5; the queued read 2,203 ms (2,196 to 2,204) from its own start; node 2's read served after the release 5 of 5 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `waits`: `UIA_E_TIMEOUT` 5 of 5 at each timeout; 2,197.8 ms (2,190.9 to 2,199.6), 5,517.6 ms (5,515.3 to 5,527.6) and 22,133 ms (22,124 to 22,136) | 2026-11-05 | As above | As above | Pass |
| uia-facts run at the cut setups, recovery off: `waits` `UIA_E_TIMEOUT` at 2,000 and 5,000 ms, E-TT's reads after one timeout; `own-start` at 1,000 ms both `UIA_E_TIMEOUT`, the queued read one timeout from its own start, served after the release | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ff23f81a | Pass |
| uia-facts runs of `timer`: at 1,000 ms 1,001.5 to 1,028.5 ms in 7 runs, where the same read with the timer left alone waited 1,091.0 to 1,092.2 ms; the program before this cut, at 2,000 ms, 2,009.6 ms (2,008.2 to 2,016.4) where its first run waited 2,201.2 ms and an earlier run that day 2,188.4 ms | 2026-11-06 | As above | ff23f81a and 5ee36a07 | The record of `timer`: about the timeout itself, judged by its band |
| The two bands apart: `waits` 2,190.1 ms at 2,000 ms and 5,515.7 ms at 5,000 ms, in the band of the timeout and a tenth; `timer` 1,012.0 and 1,004.8 ms at 1,000 ms, in the band of the timeout itself | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ac4e3f44 | Pass |
| First runs of `mark`: recovery on, the reads from another thread and through another client object `UIA_E_TIMEOUT` at once, 0.1 to 0.2 ms, 4 of 4; the first read to succeed 2,002 to 2,005 ms after the release, 192 to 193 failing first, 4 of 4; recovery off, each read `UIA_E_TIMEOUT` after one timeout and the first read after the release succeeding | 2026-11-06 | As above | ff23f81a | The answers, now the record |
| `mark` judged against that record, every setting: recovery on, the first read to succeed 1,999 and 2,003 ms after the release; recovery off, at once | 2026-11-06 | As above | 3bda30b6 | Pass |
| `timer` with the resolution read: 1.0000 ms in effect, the held read 1,002.8 to 1,005.4 ms at 1,000 ms in 4 runs, inside the band of the timeout itself; `waits` at the default 15.625 ms 2,187.8 to 2,199.1 ms at 2 s; `own-start` with `--recovery --trials 2`, a provider per trial, 1,093.1 to 1,094.6 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 7319ed67 | Pass: at a confirmed 1 ms the wait is the timeout itself, so the removal of the claim that the tenth is not the timer's tick stands |

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

`recorded/timeouts.txt`: the parallel-calls experiment's trace and timings of `timeouts.ps1`.
