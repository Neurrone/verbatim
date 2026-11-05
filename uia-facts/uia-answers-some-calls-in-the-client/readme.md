# UIA answers some calls in the client

## Fact

Some calls make no request to the application (`books/dev/src/explanation/windows/uia.md`, "Making a connection"): `Cached*` reads; `GetRuntimeId` after the first read of an element, and on an event's sender inside its callback; `CompareElements` of elements whose runtime ids UIA has read, with one application stalled; the parent of a top-level element, the desktop; and the parts of a window that UIA's own proxies build, such as its title bar. `GetFocusedElement` does not go to an application of the client's choosing: its requests go to the process with the focus.

UI Automation's copy of an element's runtime id lapses (`books/dev/src/reference/uia-bugs.md`, the fault of that name): a call that needs it after a pause fetches it from the provider again, so a cold registration costs what a warm one does plus a lapsed `GetRuntimeId` read (9 provider calls against 3 on mockapp's `First`), and a move of a registration to the element it already follows, free when warm, makes provider calls after 250 ms or more (30 of 30) and none after 0 or 100 ms (20 of 20). The lapse fits a periodic sweep, not an age: pauses of 0 to 190 ms were always warm, 1,000 and 3,000 ms always cold, and 210 to 400 ms alternated. The copy is kept per element. Reading the runtime id just before the call keeps it warm: 256 of 256 registrations and 50 of 50 moves over pauses of 0 to 3,000 ms. No provider call comes after a registration, a removal or a read returns, except `AdviseEventRemoved`, which comes after the removal returns. A search re-reads its start's runtime id when the start was fetched more than about 0.65 s before (inferred).

## What Verbatim relies on

D13's contract that the listener's callbacks make no cross-process call (the module comment of `crates/verbatim-listener/src/listener.rs`: building a fact is local memory reads, plus `GetRuntimeId`, a local read on a cached element); the listener's comparison with the current focus (`compare_elements` in `crates/verbatim-listener/src/focus_read.rs`); the follower's group comparison (`crates/verbatim-uia/src/subscribe.rs`); and the lapse's workaround, reading the runtime id just before a registration. If any of these calls began calling the provider, a hung application would block the listener's callbacks and every focus with them.

## Method

The kept CI test `uia_event_registrations_cost_exactly` (`crates/mockapp/tests/call_counts.rs`) pins the warm registration costs, the runtime id read first. The program covers what needs pauses or a second application, which a test may not make.

Every count is of the provider program's own calls (its log's `call` entries) that start between a call's start and its return, against one UIA window (`WS_OVERLAPPEDWINDOW`, so UIA's proxies build its title bar); the client process's timeouts are 2,000 and 5,000 ms.

- `no-call`, 3 runs each: a `CachedName` read after `BuildUpdatedCache`; a second `GetRuntimeId`; `GetRuntimeId` of a name-change event's sender inside the callback; `CompareElements` of node 1 of two provider programs, both runtime ids read, one program's UI thread busy; the raw view parent of the window's element; the names of the window's title bar and its parts.
- `focus-elsewhere`: `GetFocusedElement` with another process in front. 3 runs.
- `focus-here`: `GetFocusedElement` with the provider's window brought to the foreground. Needs the unlocked desktop. 3 runs.
- `lapse`: node 1 through Verbatim's own `Registration` (`verbatim_uia::Registration` with the focus-following property part, as the CI test makes it), the pauses deliberate since the fact is about them. The warm and cold registration (cold after 1,500 ms) and a lapsed `GetRuntimeId` read alone, 3 runs; the move to the element followed after pauses of 250 to 400 ms (30 trials) and of 0 or 100 ms (20 trials); the pause sweep by a `GetRuntimeId` read, 3 runs a pause; the copy kept per element, 3 runs each; the workaround over pauses of 0 to 3,000 ms (256 registrations, 50 moves).
- `late-calls`: a registration, a read and a removal, the log watched 500 ms after. 3 runs.
- `search-reread`: a `FindFirst` from the window's element fetched 0, 300, 600, 700 and 1,000 ms before; its provider `GetRuntimeId` calls.
- `operations`: each operation of "What one request is" once, its provider calls printed for trends; it drives [the requests guide](../requests-per-client-operation/readme.md) and [the channel guide](../the-uia-channel-and-the-provider-io-thread/readme.md).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| No request for cached reads, a second `GetRuntimeId`, the desktop parent and the proxies' parts; `GetFocusedElement` 6 requests into mockapp in front, none otherwise | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Requests per client operation" (`ops.ps1`) | 0 calls |
| `CompareElements`: 10 of 10 "different" in under 0.1 ms, no call in either application, one stalled | 2026-10-29 | Build 26200 | The `CompareElements` experiment of `docs/design/cross-process-calls-study.md`, `recorded/compareelements.txt` | (false, 0 calls) |
| The lapse: 9 against 3; the move's 6 calls after 250 ms or more, 30 of 30, none at 0 or 100 ms, 20 of 20; warm 0 to 190 ms, cold 1,000 and 3,000 ms, 12 of 12 each; 210 cold, 230 warm, 270 cold, 290 warm, 310 cold, 400 warm; per element; 256 of 256 and 50 of 50 with the workaround; `AdviseEventRemoved` late | 2026-11-03 | Build 26200 | `books/dev/src/reference/uia-bugs.md`, from experiments whose code was not kept | As each claim |
| First uia-facts run, `no-call`: 0 provider calls in 3 of 3 for the cached read, the second `GetRuntimeId`, the sender's `GetRuntimeId` in its callback, `CompareElements` (answered "different" within 100 ms with one application busy), the desktop parent and the title bar with its 5 parts | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | The commit that added this folder | Pass |
| First uia-facts run, `focus-elsewhere`: neither the provider's element nor any provider call, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `lapse`, costs: warm 8, cold 14, a lapsed `GetRuntimeId` read 6, 3 of 3; cold equals warm plus the lapsed read, 3 of 3 | 2026-11-05 | As above | As above | The counts new for this provider; the relation passes |
| First uia-facts run, `lapse`, moves: after 250, 300, 350 and 400 ms, provider calls in 15 of 30; after 0 or 100 ms, none in 18 of 20 | 2026-11-05 | As above | As above | Differ and inconclusive, raised below |
| First uia-facts run, `lapse`, sweep: warm after 0, 50 and 100 ms 3 of 3; after 150 and 190 ms the runs disagreed; cold after 1,000 and 3,000 ms 3 of 3; 210 warm, 230 warm, 270 cold, 290 warm, 310 cold, 400 warm | 2026-11-05 | As above | As above | Inconclusive below 200 ms, raised below; cold passes; the middle for trends |
| First uia-facts run, `lapse`, per element: First's copy lapsed after reading First's name, another button's, the window's, and after a registration on another button, 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `lapse`, workaround: 256 of 256 warm registrations; 50 of 50 free moves | 2026-11-05 | As above | As above | Pass (the run printed the moves as inconclusive through a rounding error in the Wilson interval at 50 of 50, fixed in the same commit) |
| First uia-facts run, `late-calls`: `AdviseEventRemoved` after the removal returned, 3 of 3; no other provider call after any of the three calls returned, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `search-reread`: one provider `GetRuntimeId` in the search at every pause, 0, 300, 600, 700 and 1,000 ms | 2026-11-05 | As above | As above | New, raised below |
| First uia-facts run, `operations`: provider calls per operation, each including any lookup of its node: `CurrentName` 3; a walker's first child 0, next sibling 16, step to the root 18; `FindFirst` 47 and with a cache request 62; `FindAll` 503 and with a cache request 848; `BuildUpdatedCache` 15; `ElementFromHandle` 13; a remote operation 6; adding and removing a handler 9 | 2026-11-05 | As above | As above | For trends |

### What differs, raised

The lapse's timing differs from the record in a way the record's own reading predicts. The record found the copy kept by a periodic sweep, not an age, with 290 and 400 ms warm beside 270 and 310 ms cold; so a move "after 250 ms or more" is cold only when the pause crosses a sweep. Here pauses of 250 to 400 ms gave a cold move in half the trials, and pauses of 150 and 190 ms were not always warm, so the sweep's phase, not the pause, decides. The record's "30 of 30" and "warm to 190 ms, 12 of 12" do not hold as rates on this run; the fault itself, and the workaround (256 of 256, 50 of 50), do. Settling the period is the follow-up: sweep the pause in 10 ms steps from a fixed phase.

The search re-read its start's runtime id at every pause, 0 ms included, where the inference was a re-read only after about 0.65 s. The start here is the window's element, whose provider gives no runtime id of its own (UIA takes it from the window's handle), so UIA may ask its provider each time; the inference came from a start inside mockapp's tree. Whether a fragment's start behaves as inferred is open.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\uia-answers-some-calls-in-the-client.exe`, or one arm with `--arm`. Every arm but `focus-here` needs no desktop; `focus-here` brings the provider's window to the foreground, so it needs the unlocked desktop and takes the focus for a moment. About 13 minutes, most of it the lapse's 256 registrations. The test: `cargo test -p mockapp --test call_counts -- --exact uia_event_registrations_cost_exactly`.

## Evidence kept

`recorded/compareelements.txt`: the `CompareElements` experiment's output, one line per comparison.
