# The connection timeout applies to the whole process

## Fact

Both UIA timeouts are settings of the client process, not of a client object: the value any client object in the process set last applies to every client object's calls, and a client object made later that sets nothing does not reset it (`books/dev/src/reference/uia-bugs.md`, the fault of that name). A read on a client object that set 500 ms waited 10,013 ms after another client object set 10,000 ms, and 508 ms the other way round.

## What Verbatim relies on

`set_process_timeouts` inside `ensure_ready` (`crates/verbatim-uia/src/client.rs`), which sets both timeouts once per process, and every deadline built from `CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT` (`crates/verbatim-listener/src/focus_read.rs`, `crates/verbatim-outpost/src/outpost/read.rs`, `crates/verbatim-supervisor/src/retire.rs`). If the timeouts became per client object, Verbatim's other client objects would run on UIA's defaults (a 20-second transaction timeout) and those deadlines would abandon calls still waiting.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/the_connection_timeout_applies_to_the_whole_process.rs`, read the timeouts back: `timeouts_set_through_one_client_are_read_by_another` (E1), `the_first_client_in_a_process_has_verbatims_timeouts` and `making_a_client_keeps_the_timeouts_another_set`. The program shows what no test can, since it is a timed wait: that the value read back is the one a call waits by.

Each arm runs 5 trials, a new provider program each, its one UIA window's UI thread busy until released. Client object A, on the main thread, sets the connection timeout; client object B, made on another thread, sets another; then A reads the busy window (`ElementFromHandle`, then `CurrentName`), which fails at the connection timeout in force.

- `other-longer`: A sets 500 ms, B then 10,000 ms (`h5_main500_other10000`).
- `other-shorter`: A sets 10,000 ms, B then 500 ms (`h5_main10000_other500`).

Judged: the HRESULT (deterministic) and the wait (a band from B's timeout).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E1: a client object made first read 1,000 and 1,000 after another set both | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989, now the CI tests | The tests |
| `h5_main500_other10000`: 10,013 ms; `h5_main10000_other500`: 508 ms; `h5_default` 2,014 ms; `h5_main500` 514 ms | 2026-10-18 | Build 26200 | `experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/`, `h5_*.out` | `UIA_E_TIMEOUT`; bands 10,000 to 11,550 and 500 to 625 ms |
| First uia-facts run, `other-longer`: `UIA_E_TIMEOUT` 5 of 5, 10,013 ms (10,010 to 10,019) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | The commit that added this folder | Pass |
| First uia-facts run, `other-shorter`: `UIA_E_TIMEOUT` 5 of 5, 506.9 ms (504.1 to 513.8) | 2026-11-05 | As above | As above | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\the-connection-timeout-applies-to-the-whole-process.exe`, or one arm with `--arm`. No desktop, application or debugger. About 55 seconds. The tests: `cargo test -p mockapp --test uia_bugs -- --exact timeouts_set_through_one_client_are_read_by_another`, and the same with the other two names.

## Evidence kept

None of its own: `h5`'s outputs stay with the hung-window experiments until they move.
