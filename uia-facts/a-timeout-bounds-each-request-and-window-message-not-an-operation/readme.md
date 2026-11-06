# A timeout bounds each request and window message, not an operation

## Fact

An operation made of several requests and window messages waits for each of them, each on its own bound, and re-sends nothing (`books/dev/src/reference/uia-bugs.md`, the limitation of that name; `books/dev/src/explanation/windows/uia.md`, "Timeouts and cancelling"). A failed read of a search's start runtime id ends the search, and with connection recovery on its `Find` request is sent only when the waits before it ended about 3.1 s or more after the search began. So E-TT's search, its provider holding the condition's test, both timeouts at 1 s and the start fetched at a 10-second connection timeout, failed after 13,208 ms, four waits of which one was the connection timeout in force at the fetch; a search queued behind another client's held read failed after one wait, 1,083 to 1,106 ms; with the window thread stalled, both timeouts at T before the fetch and recovery on, it failed after 332, 618 and 1,545 ms at T of 100, 200 and 500 ms without sending the `Find`, and after 4,207 ms at 1,000 ms with it. E3's held `HasKeyboardFocus` read, never released, failed after 22,111 ms at the default transaction timeout and 1,092 ms at 1 second, which [a call waits its timeout from its own start](../a-call-waits-its-timeout-from-its-own-start/readme.md) checks. With recovery on, the search skips its `Find` at T of 900 ms, its waits ending by 2.78 s, and sends it at 1,000 ms, about 3.1 s; why recovery skips it is not known. The re-read of a search's start runtime id, when the start was fetched 600 ms or more before and not at 300 ms, is [UIA answers some calls in the client](../uia-answers-some-calls-in-the-client/readme.md)'s.

## What Verbatim relies on

Only the explanation in `crates/verbatim-supervisor/src/retire.rs` and `crates/verbatim-uia/src/bounded.rs`, which run operations of many calls with a deadline of their own on a thread that can be abandoned; the general rule is checked by [a call waits its timeout from its own start](../a-call-waits-its-timeout-from-its-own-start/readme.md).

## Method

Every case runs against the provider program with one UIA window (`WS_OVERLAPPEDWINDOW`, so UIA's window and non-client proxies take part, as with mockapp's window), each trial against a provider and client object of its own, so no trial's timed-out call reaches the next through connection recovery.

- `ett`: the window's element fetched with the connection timeout at 2,000 ms and the transaction timeout at 1,000 ms; the provider holds the root's `GetPropertyValue`; the connection timeout set to 1,000 ms; then `FindFirstBuildCache` over the descendants for the name `First`, caching the name. The connection timeout is set explicitly, since Verbatim's own is now 2 seconds; E-TT set 10,000 ms, and 2,000 ms, still twice the call's 1,000 ms, shows the same four waits with less waiting.
- `queued-search`: another client object's name read of node 1 held; a `FindFirst` for `First` from the window's element, fetched a second before (a deliberate pause: a search re-reads its start's runtime id when the start was fetched 600 ms or more before, not at 300 ms); both timeouts 1,000 ms.
- `stall-sweep`: the window's UI thread busy; both timeouts at T set before the window's element is fetched; `FindFirst` for `Second`. T of 100, 200, 500 and 1,000 ms, and with `--recovery` also 900 ms, the last timeout whose search skips its `Find`, each judged against its record. After the release, whether the provider walked the tree for the search's `Find`, watched for 2 s: the walk follows a release within milliseconds when it comes, so its absence over 2 s is the evidence that no `Find` was sent.

Judged: the HRESULT (deterministic), each search's total (a band, the sum of its waits' bands), whether the `Find` was sent (deterministic).

## Client settings

Event coalescing changes no outcome, and connection recovery changes the `stall-sweep` case's (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both). The documents state the stall sweep with recovery on, so its judged command carries `--recovery`; the program judges each setting against its own record.

- Connection recovery on, as Verbatim's client has it: at 100, 200 and 500 ms the search waits three bounds and fails without sending its `Find`, and from 1,000 ms it waits four and sends it; between, it sent no `Find` up to 900 ms (1,853, 2,193, 2,475 and 2,791 ms at 600 to 900 ms) and sent it from 1,000 ms (4,207, 4,613 and 5,021 ms at 1,000 to 1,200 ms); 900 ms is judged. `target\debug\a-timeout-bounds-each-request-and-window-message-not-an-operation.exe --recovery --case stall-sweep`
- Connection recovery off, the default: the search waits four bounds and sends its `Find` at every timeout: 435, 823, 2,085 and 4,204 ms at 100, 200, 500 and 1,000 ms, and 2,515 to 5,032 ms at 600 to 1,200 ms. `target\debug\a-timeout-bounds-each-request-and-window-message-not-an-operation.exe --case stall-sweep`

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT's search 13,208 ms (13,195 to 13,211 later); the queued search 1,083 to 1,106 ms; the stall sweep 332, 618, 1,545 and 4,207 to 4,209 ms, the `Find` sent only at 1,000 ms | 2026-10-10 and 2026-11-02 | Build 26200 and 26200.9711 | E-TT in the mockapp experiments, in git at b0a1989, `recorded/e-tt-rerun.txt`; the parallel-calls experiment's readme, in git at fdfb63ae, "A search's timeouts" (`search-timeouts.ps1`) | `UIA_E_TIMEOUT`; bands of the waits' sums |
| E3: 22,111 ms at the default, 1,092 ms at 1 s | 2026-10-10 | Build 26200 | E3 in the mockapp experiments, in git at b0a1989 | Checked now by [a call waits its timeout from its own start](../a-call-waits-its-timeout-from-its-own-start/readme.md) (`waits`), which holds a read at the same timeouts |
| First uia-facts run, `ett`: `UIA_E_TIMEOUT` 5 of 5, 13,206 ms (13,196 to 13,220) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | c84ab856 | Pass |
| First uia-facts run, `queued-search`: `UIA_E_TIMEOUT` 5 of 5, 1,087 ms (1,082 to 1,094) | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `stall-sweep`: `UIA_E_TIMEOUT` 5 of 5 at each T; 329, 620, 1,545 and 4,208 ms; the `Find` sent only at 1,000 ms, 5 of 5 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e3`: `UIA_E_TIMEOUT` 5 of 5 each; 22,139 ms (22,135 to 22,146) at the default, 1,091.7 ms (1,084.1 to 1,093.1) at 1 s | 2026-11-05 | As above | As above | Pass; the case is removed, since call-waits' `waits` checks the same |
| First uia-facts run, `stall-sweep`, 600 to 1,200 ms: at T of 600, 700, 800 and 900 ms the search failed after 1,847, 2,181, 2,480 and 2,778 ms without sending the `Find`; at 1,000, 1,100 and 1,200 ms after 4,204, 4,614 and 5,032 ms with it; 3 of 3 each | 2026-11-05 | As above | As above | The answer: the `Find` is skipped when the waits before it end by 2.78 s and sent when they end at about 3.1 s or later. That narrows the edge recorded before, skipped by 2.6 s and sent from 3.1 s, without explaining it. 900 ms is judged since, with `--recovery` |
| uia-facts run of the cut program, every setting: `ett` at a 2,000 ms fetch `UIA_E_TIMEOUT` after 5,211.3 ms; `queued-search` 1,084.3 ms; `stall-sweep` 436.5, 822.0, 2,101.6 and 4,196.5 ms with recovery off, the `Find` sent at every timeout | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ff23f81a | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-timeout-bounds-each-request-and-window-message-not-an-operation.exe`, or one case with `--case`. No desktop, application or debugger.

## Evidence kept

`recorded/e-tt-rerun.txt`: the output of E-TT's rerun at b0a19893 on 2026-11-02 for its cases that connect inside the hold, each failing or returning after the 10-second connection timeout then in force. `recorded/search-timeouts.txt`: the original traces and timings of the searches.
