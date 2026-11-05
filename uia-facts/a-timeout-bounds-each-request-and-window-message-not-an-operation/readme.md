# A timeout bounds each request and window message, not an operation

## Fact

An operation made of several requests and window messages waits for each in turn, each on its own bound, and re-sends nothing (`books/dev/src/reference/uia-bugs.md`, the limitation of that name; `books/dev/src/explanation/windows/uia.md`, "Timeouts and cancelling"). A search's waits, in order: the start element's runtime id when read again (its failure ends the search); the condition's test on the start element (its failure is passed over); `CB_GETCOMBOBOXINFO` to the window, on the transaction timeout; oleacc's `WM_GETOBJECT`, on the connection timeout in force when the start element was fetched; and the `Find` request, which with connection recovery on is sent only when the waits before it ended 3.1 s or more after the search began. So E-TT's search, its provider holding the condition's test, both timeouts at 1 s and the start fetched at a 10-second connection timeout, failed after 13,208 ms; a search queued behind another client's held read failed after one wait, 1,083 to 1,106 ms; with the window thread stalled, both timeouts at T before the fetch and recovery on, it failed after 332, 618 and 1,545 ms at T of 100, 200 and 500 ms without sending the `Find`, and after 4,207 ms at 1,000 ms with it. E3's held `HasKeyboardFocus` read, never released, failed after 22,111 ms at the default transaction timeout and 1,092 ms at 1 second. Why recovery skips the `Find`, and where its edge lies, are not known. The re-read of a search's start runtime id after about 0.65 s is [UIA answers some calls in the client](../uia-answers-some-calls-in-the-client/readme.md)'s.

## What Verbatim relies on

Only the explanation in `crates/verbatim-supervisor/src/retire.rs` and `crates/verbatim-uia/src/bounded.rs`, which run operations of many calls with a deadline of their own on a thread that can be abandoned; the general rule is checked by [a call waits its timeout from its own start](../a-call-waits-its-timeout-from-its-own-start/readme.md).

## Method

Every arm runs against the provider program with one UIA window (`WS_OVERLAPPEDWINDOW`, so UIA's window and non-client proxies take part, as with mockapp's window).

- `ett`: the window's element fetched with the connection timeout at 10,000 ms and the transaction timeout at 1,000 ms; the provider holds the root's `GetPropertyValue`; the connection timeout set to 1,000 ms; then `FindFirstBuildCache` over the descendants for the name `First`, caching the name. 5 trials. The connection timeout is set explicitly, since Verbatim's own is now 2 seconds.
- `queued-search`: another client object's name read of node 1 held; a `FindFirst` for `First` from the window's element, fetched a second before (a deliberate pause, past the 0.65 s after which a search re-reads its start's runtime id); both timeouts 1,000 ms. 5 trials.
- `stall-sweep`: the window's UI thread busy; both timeouts at T set before the window's element is fetched; connection recovery on; `FindFirst` for `Second`. T of 100, 200, 500 and 1,000 ms, 5 trials each. After the release, whether the provider walked the tree for the search's `Find`, watched for 2 s: the walk follows a release within milliseconds when it comes, so its absence over 2 s is the evidence that no `Find` was sent.
- `e3`: E3's `CurrentHasKeyboardFocus` of node 1, held until it fails, at transaction timeouts of 20,000 and 1,000 ms. 5 trials each.
- `recovery-edge`: the stall sweep's search at T of 600 to 1,200 ms in 100 ms steps, 3 trials each, toward the edge of recovery's skip (Not known).

Judged: the HRESULT (deterministic), each search's total (a band, the sum of its waits' bands), whether the `Find` was sent (deterministic). What reaches the provider after E-TT's release is printed for trends: the window messages that timed out never reach the window procedure, so the order of the waits rests on the original trace.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT's search 13,208 ms (13,195 to 13,211 later), in four waits 1,085, 1,010, 10,013 and 1,075 ms; the queued search 1,083 to 1,106 ms; the stall sweep 332, 618, 1,545 and 4,207 to 4,209 ms, the `Find` sent only at 1,000 ms | 2026-10-10 and 2026-11-02 | Build 26200 and 26200.9711 | E-TT in `experiments/mockapp-experiments-2026-10-10/` at b0a1989, `recorded/e-tt-rerun.txt`; `experiments/parallel-uia-calls/`, "A search's timeouts" (`search-timeouts.ps1`) | `UIA_E_TIMEOUT`; bands of the waits' sums |
| E3: 22,111 ms at the default, 1,092 ms at 1 s | 2026-10-10 | Build 26200 | E3 in `experiments/mockapp-experiments-2026-10-10/` at b0a1989 | Bands 20,000 to 23,050 and 1,000 to 1,200 ms |
| First uia-facts run, `ett`: `UIA_E_TIMEOUT` 5 of 5, 13,206 ms (13,196 to 13,220); after the release the provider received the condition's test, the `Find`'s walk and a `WM_GETOBJECT` for `UiaRootObjectId`, no `CB_GETCOMBOBOXINFO` and no `WM_GETOBJECT` for -5 | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 1682f8cb | Pass; the order for trends |
| First uia-facts run, `queued-search`: `UIA_E_TIMEOUT` 5 of 5, 1,087 ms (1,082 to 1,094) | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `stall-sweep`: `UIA_E_TIMEOUT` 5 of 5 at each T; 329, 620, 1,545 and 4,208 ms; the `Find` sent only at 1,000 ms, 5 of 5 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e3`: `UIA_E_TIMEOUT` 5 of 5 each; 22,139 ms (22,135 to 22,146) at the default, 1,091.7 ms (1,084.1 to 1,093.1) at 1 s | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `recovery-edge`: at T of 600, 700, 800 and 900 ms the search failed after 1,847, 2,181, 2,480 and 2,778 ms without sending the `Find`; at 1,000, 1,100 and 1,200 ms after 4,204, 4,614 and 5,032 ms with it; 3 of 3 each | 2026-11-05 | As above | As above | New: the `Find` is skipped when the waits before it end by 2.78 s and sent when they end at about 3.1 s or later, which narrows the recorded edge (not by 2.6 s, sent from 3.1 s) without explaining it |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-timeout-bounds-each-request-and-window-message-not-an-operation.exe`, or one arm with `--arm`. No desktop, application or debugger. About 6 minutes.

## Evidence kept

`recorded/e-tt-rerun.txt`: the output of E-TT's rerun at b0a19893 on 2026-11-02 for its arms that connect inside the hold, each failing or returning after the 10-second connection timeout then in force. The traces of the search's waits stay in `experiments/parallel-uia-calls/results/search-timeouts.txt` until that folder's last facts move.
