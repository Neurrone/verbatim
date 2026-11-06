# UIA answers some calls in the client

## Fact

Some calls make no request to the application (`books/dev/src/explanation/windows/uia.md`, "Making a connection"): `Cached*` reads; `GetRuntimeId` after the first read of an element, and on an event's sender inside its callback; `CompareElements` of elements whose runtime ids UIA has read, with one application stalled; the parent of a top-level element, the desktop; and the parts of a window that UIA's own proxies build, such as its title bar. `GetFocusedElement` does not go to an application of the client's choosing: its requests go to the process with the focus.

UI Automation's copy of an element's runtime id lapses (`books/dev/src/reference/uia-bugs.md`, the fault of that name): a call that needs it after a pause fetches it from the provider again, so a cold registration costs what a warm one does plus a lapsed `GetRuntimeId` read (9 provider calls against 3 on mockapp's `First`), and a move of a registration to the element it already follows, free when warm, makes provider calls when the copy has lapsed. A `GetRuntimeId` read with no pause makes no provider call, and one after a pause of 1 second makes provider calls. The lapse is not a fixed lifetime: with pauses of 250 to 400 ms taken in turn, a move after a longer pause finds the copy warm where one after a shorter pause found it lapsed, so no pause below about a second can be relied on to stay warm, which is why the workaround reads the runtime id just before the call. The copy is kept per element. Reading the runtime id just before the call keeps it warm: a registration and a move after a pause of 1 second, past the lapse, cost the warm calls. No provider call comes after a registration, a removal or a read returns, except `AdviseEventRemoved`, which comes after the removal returns. A search re-reads its start's runtime id when the start was fetched 600 ms or more before, and not at 300 ms.

## What Verbatim relies on

D13's contract that the listener's callbacks make no cross-process call (the module comment of `crates/verbatim-listener/src/listener.rs`: building a fact is local memory reads, plus `GetRuntimeId`, a local read on a cached element); the listener's comparison with the current focus (`compare_elements` in `crates/verbatim-listener/src/focus_read.rs`); the follower's group comparison (`crates/verbatim-uia/src/subscribe.rs`); and the lapse's workaround, reading the runtime id just before a registration. If any of these calls began calling the provider, a hung application would block the listener's callbacks and every focus with them.

## Method

The kept CI test `uia_event_registrations_cost_exactly` (`crates/mockapp/tests/call_counts.rs`) pins the warm registration costs, the runtime id read first. The program covers what needs pauses or a second application, which a test may not make.

Every count is of the provider program's own calls (its log's `call` entries) that start between a call's start and its return, against one UIA window (`WS_OVERLAPPEDWINDOW`, so UIA's proxies build its title bar); the client process's timeouts are 2,000 and 5,000 ms.

- `no-call`: a `CachedName` read after `BuildUpdatedCache`; a second `GetRuntimeId`; `GetRuntimeId` of a name-change event's sender inside the callback; `CompareElements` of node 1 of two provider programs, both runtime ids read, one program's UI thread busy; the raw view parent of the window's element; the names of the window's title bar and its parts.
- `focus-elsewhere`: `GetFocusedElement` with another process in front.
- `focus-here`: `GetFocusedElement` with the provider's window brought to the foreground. Needs the unlocked desktop.
- `lapse`: node 1, registered as Verbatim's follower registers it, with raw UIA calls on the program's own client object, so the flags reach it: an event handler group holding a property-changed handler for Verbatim's focus properties (`FOCUS_PROPERTIES`) on the element alone, added with `AddEventHandlerGroup`; a move to the element already followed is `CompareElements` of the followed element and a fresh reference to it, as the follower decides it. The pauses are deliberate, since the fact is about them. The warm and cold registration (cold after 1 s) and a lapsed `GetRuntimeId` read alone; a `GetRuntimeId` read with no pause, and the lapsed read after 1 s, which the cold registration's count already took; the copy kept per element; moves after pauses of 250, 300, 350 and 400 ms in turn, three at each, judged that a longer pause found the copy warm where a shorter one found it lapsed, which no fixed lifetime allows, the count lapsed at each pause printed with it; the workaround, a registration and a move each after a pause of 1 s with the runtime id read just before.
- `late-calls`: a registration, a read and a removal, the log watched for the whole 500 ms after.
- `search-reread`: a `FindFirst` for node 6 from node 1, a fragment with a runtime id of its own, fetched 0, 300, 600, 700 and 1,000 ms before; the provider `GetRuntimeId` calls of node 1 itself during the search.
- `operations`: each operation of "What one request is" once, its provider calls printed for trends; it drives [the requests guide](../requests-per-client-operation/readme.md) and [the channel guide](../the-uia-channel-and-the-provider-io-thread/readme.md).

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both; `focus-here` on the unlocked desktop, passing under each).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| No request for cached reads, a second `GetRuntimeId`, the desktop parent and the proxies' parts; `GetFocusedElement` 6 requests into mockapp in front, none otherwise | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "Requests per client operation" (`ops.ps1`) | 0 calls |
| `CompareElements`: 10 of 10 "different" in under 0.1 ms, no call in either application, one stalled | 2026-10-29 | Build 26200 | The `CompareElements` experiment of `docs/design/cross-process-calls-study.md`, `recorded/compareelements.txt` | (false, 0 calls) |
| The lapse: 9 against 3; per element; warm with the workaround; `AdviseEventRemoved` late | 2026-11-03 | Build 26200 | `books/dev/src/reference/uia-bugs.md`, from experiments whose code was not kept | As each claim |
| First uia-facts run, `no-call`: 0 provider calls in 3 of 3 for the cached read, the second `GetRuntimeId`, the sender's `GetRuntimeId` in its callback, `CompareElements` (answered "different" within 100 ms with one application busy), the desktop parent and the title bar with its 5 parts | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 5ad6b886 | Pass |
| First uia-facts run, `focus-elsewhere`: neither the provider's element nor any provider call, 3 of 3 | 2026-11-05 | As above | c84ab856 | Pass |
| First uia-facts run, `focus-here`: the provider's element, with provider calls, (true, true) in 3 of 3 | 2026-11-05 | As above | dab71a73 | Pass |
| First uia-facts run, `lapse`, costs: warm 8, cold 14, a lapsed `GetRuntimeId` read 6, 3 of 3; cold equals warm plus the lapsed read, 3 of 3 | 2026-11-05 | As above | c84ab856 | The counts this record; the relation passes |
| First uia-facts run, `lapse`, per element: First's copy lapsed after reading First's name, another button's, the window's, and after a registration on another button, 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| Second uia-facts run, `lapse`, with the pauses above: costs (8, 14, 6) and cold equals warm plus the lapsed read; warm with no pause and cold after 1 s; First's copy lapsed after each of the four touches; with the runtime id read just before, 4 of 4 registrations at the warm cost and 4 of 4 moves with no provider call; 1 run each | 2026-11-05 | As above | The commit that adds this row | Pass |
| First uia-facts run, `late-calls`: `AdviseEventRemoved` after the removal returned, 3 of 3; no other provider call after any of the three calls returned, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `search-reread`: reads of the start's own runtime id during the search: 0 after 0 and 300 ms, 1 after 600, 700 and 1,000 ms, 3 of 3 each | 2026-11-05 | As above | 5ad6b886 | 0 and 1,000 ms this record; 300, 600 and 700 ms for trends |
| First uia-facts run, `operations`: provider calls per operation, each including any lookup of its node: `CurrentName` 3; a walker's first child 0, next sibling 16, step to the root 18; `FindFirst` 47 and with a cache request 62; `FindAll` 503 and with a cache request 848; `BuildUpdatedCache` 15; `ElementFromHandle` 13; a remote operation 6; adding and removing a handler 9 | 2026-11-05 | As above | c84ab856 | For trends |
| uia-facts run of the cut `lapse`, every setting: costs (8, 14, 6), cold equal to warm plus the lapsed read, the lapsed read making provider calls; with the runtime id read just before, the registration after 1 s at the warm cost and the move with no provider call | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ff23f81a | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\uia-answers-some-calls-in-the-client.exe`, or one case with `--case`. Every case but `focus-here` needs no desktop; `focus-here` brings the provider's window to the foreground, so it needs the unlocked desktop and takes the focus for a moment. The test: `cargo test -p mockapp --test call_counts -- --exact uia_event_registrations_cost_exactly`.

## Evidence kept

`recorded/compareelements.txt`: the `CompareElements` experiment's output, one line per comparison.
