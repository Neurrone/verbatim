# A new connection to a stuck application times out

## Fact

When an application's UI thread is stuck with a UIA call queued on it, a new connection (`ElementFromHandle`) to any of its windows fails with `UIA_E_TIMEOUT` after the connection timeout, 2.0 to 2.2 seconds at its 2-second default, even to a window whose own UI thread is idle (`books/dev/src/explanation/windows/uia.md`, "Making a connection" and "Predicting outcomes"). A lookup with the provider description cached shows, by stall kind, either UIA's proxies alone or the timeout: the proxy-only answer is recorded for the collision (`books/dev/src/reference/uia-bugs.md`, "A new connection fails while another process's remote operation is queued") and the timeout for this stall.

## What Verbatim relies on

The proxy rule (`books/dev/src/reference/crates/verbatim-uia.md`, `window_element`), which refuses a lookup answered by UIA's proxies alone by reading the cached provider description, and the focus-now answer at an outpost's start (`books/dev/src/explanation/design/listener-without-connections.md`, decision 8). If the description's form changed, a proxy's answer would be taken as the application's own and read as "pane". The program classifies descriptions with that rule itself, which `uia_facts_support::uia::served_only_by_uia_proxies` re-exports from `verbatim-uia`.

## Method

The provider program with two UIA windows on two UI threads, `UseComThreading`; the client process's connection timeout 2,000 ms, transaction timeout 30,000 ms.

- `connection`, a new provider each: window 0's node 1 fetched on a thread of its own; UI thread 0 made busy until released; that thread's name read queued on it; after 200 ms (a deliberate wait, since nothing marks a request queued on a busy thread), `ElementFromHandle` of window 1, on the idle thread, then of window 0. Judged: the HRESULT (deterministic) and each wait (a band from the connection timeout).
- `description`: `ElementFromHandleBuildCache` caching the provider description. `queued`: as above, window 1. `busy`: UI thread 0 busy with nothing queued, window 0. `held`: a name read held in window 0's provider (`hold GetPropertyValue`), window 1. Judged: each stall kind against its record, a timeout.

## Client settings

With connection recovery on, as Verbatim's client has it, a new connection made right after one into the same application failed fails at once, in 0.2 ms: `target\debug\a-new-connection-to-a-stuck-application-times-out.exe --recovery --case connection`.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `UIA_E_TIMEOUT` after 2.0 to 2.2 s, to any window, an idle UI thread's included | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "A UI thread suspended, or a provider held, for 4 seconds" (`run-mockapp.ps1`'s suspended cases) | `UIA_E_TIMEOUT`; band 1,984 to 2,350 ms |
| First uia-facts run, `connection`: `UIA_E_TIMEOUT` 5 of 5 to each window; idle thread's window 2,195.9 ms (2,184.0 to 2,199.4), busy thread's 2,199.1 ms (2,195.7 to 2,201.9) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | c84ab856 | Pass |
| First uia-facts run, `description`: `queued` timeout 3 of 3 (pass); `busy` timeout 3 of 3 (new); `held` timeout 3 of 3 (new) | 2026-11-05 | As above | As above | `queued` pass; the others recorded as the answer |

So on this machine, for all three stalls, the lookup times out; it does not return UIA's proxies alone. The proxy-only answer is the collision's alone (`books/dev/src/reference/uia-bugs.md`).

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-new-connection-to-a-stuck-application-times-out.exe`, or one case with `--case connection` or `--case description`. It needs no desktop, application or debugger; its windows stay hidden.

## Evidence kept

`recorded/mockapp-hung.txt`: the parallel-calls experiment's suspended cases.
