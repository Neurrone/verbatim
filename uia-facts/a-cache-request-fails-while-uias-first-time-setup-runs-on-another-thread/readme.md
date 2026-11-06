# A cache request fails while UIA's first-time setup runs on another thread

## Fact

In a process where UIA has not been used yet, while UIA's first-time setup runs on one thread, `CreateCacheRequest` on another fails with `E_FAIL`: with six threads making client objects and cache requests at once in a fresh process, five fail nearly every time, and a race of six threads in each of four fresh processes meets a failure every time. A cache request made 50 ms later succeeds (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

`ensure_ready` (`crates/verbatim-uia/src/client.rs`): every client creation and the provider probe first wait for UIA's first-time setup, which creates a client and builds a cache request once, under a lock, on a thread of its own. If the fault were fixed, the wait is harmless.

## Method

The kept CI test, `clients_created_at_once_in_a_fresh_process_all_build_cache_requests` in `crates/mockapp/tests/uia_bugs/a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread.rs`, shows that the workaround holds. The program shows the fault itself, with no workaround.

- `race`: 50 runs. Each run starts 4 fresh client processes at once (the `race` role). In each, six threads of the multithreaded apartment wait at a barrier, then each makes a client object with support's `uia::client()` and calls `CreateCacheRequest`; a thread whose call failed calls it again 50 ms later. A run fails when any of its 24 first calls failed, as the CI test fails.

Judged: failed runs as a rate against 50 of 50; the failure's HRESULT and the retries' outcome (deterministic); the most threads of six that failed in one process, for trends.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Six threads at once in a fresh process: five failed nearly every time; a call 50 ms later succeeded | 2026-10-03 | Build 26200 | Commit 7c402534 | Trend; deterministic |
| The CI test with the workaround removed: failed 50 of 50 runs with `E_FAIL` (0x80004005) | 2026-10-18 | Build 26200 | `books/dev/src/reference/uia-bugs.md` | A rate, 50 of 50 |
| First uia-facts run, `race`: 50 of 50 runs failed, `E_FAIL`; at most 5 of 6 threads in a process; every retry 50 ms later `ok` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread.exe`. No desktop or application. The test: `cargo test -p mockapp --test uia_bugs -- --exact clients_created_at_once_in_a_fresh_process_all_build_cache_requests`.

## Evidence kept

None of its own: commit 7c402534.
