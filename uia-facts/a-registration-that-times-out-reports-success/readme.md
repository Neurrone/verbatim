# A registration that times out reports success

## Fact

`AddEventHandlerGroup` on a window whose provider does not answer returns success after the connection timeout, whatever the transaction timeout, though the provider never answered (`books/dev/src/reference/uia-bugs.md`, the fault of that name). With the client's connection recovery on, as Verbatim's and NVDA's clients have it, such a registration then delivers nothing until about 2 seconds after its call returned, and every event raised in that gap is lost, not delivered late; with recovery off there is no gap. The registration's own request runs as the stall ends: the provider is advised of it once, and with no event raised nothing else reaches it for 6 seconds. The gap silences every registration the process has on that window, one made before the stall on another client object included, and no registration on another window or in another process. A removal during the gap fails at once with `UIA_E_TIMEOUT` and leaves the registration in place; after the gap it succeeds. Events raised between a focus change and a registration made for the new focus are not heard (`books/dev/src/explanation/windows/uia.md`, "Events into the client").

## What Verbatim relies on

`timed_out` in `crates/verbatim-uia/src/subscribe.rs`, which treats a registration slower than `CONNECTION_TIMEOUT` as timed out and logs it at warn, the only sign, and `Follower::close`'s second removal of a group a move failed to remove. If registrations began to fail honestly, the follower's error path would run instead, which is not wrong; if the wait stopped following the connection timeout, the warning would misfire. The gap's figures are used only by an unbuilt proposal, which `books/dev/src/reference/uia-bugs.md` describes.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/a_registration_that_times_out_reports_success.rs`, check that the registration returns success while its provider is held and that a read on a held provider fails: `a_group_registered_while_its_provider_is_held_reports_success` and `a_read_while_its_provider_is_held_fails`. The program covers how long the registration waits and what follows it, which needs timing and pauses a test may not make.

Every trial runs in a client process of its own (the program's `trial` role), as e57 and e58 ran each trial in a test process of its own, so no other trial's registration is in the process. Its client object is made as Verbatim's is (`create_client` in `crates/verbatim-uia/src/client.rs`) and as e57's was: event coalescing and connection recovery on. Every registration is an event handler group holding UIA's focus event, subtree scope, the sender's name cached, on the provider program's window, as e57 and e58 made it. Each focus event is raised as e57 raised it through mockapp's `set-name` and `focus`: the next of nodes 5 to 8 in turn is renamed `ev<i>` with a name change raised, then the tree's focus moves to it and its focus event is raised, so the handler names the event it heard. A stall is the window's UI thread busy until released, taking no message. The stall's release 200 ms after the registration returns, the event offsets, the 2 s of listening after the last event and the 6 s of silence are deliberate waits, since the fact is about time; the removal arms watch 15 s for the handler's release.

- `ett`: E-TT's registration, both timeouts 1,000 ms, the provider holding `FragmentRoot`. 5 trials.
- `connection`: a registration during a stall at connection 500 ms and transaction 5,000 ms, and the reverse. 5 trials each.
- `e57`: connection 2,000 ms, transaction 5,000 ms; a focus event every 50 ms from the stall's end to 2,500 ms after it. 5 trials.
- `e57-no-recovery`: `e57` with the client's connection recovery off, the gap's cause. 3 trials.
- `e58-silence`: as `e57` with no event; the provider's log 6 s after the release. 3 trials.
- `e58-scope`: two provider programs P and Q. Before the stall of P, group A on P and group C on Q from the trial's client object, and group D on P from a second client process; during the stall, group B on P from another client object. Events on P every 100 ms from the stall's end to 3,000 ms, on Q every 100 ms from 50 to 2,950 ms. 3 trials.
- `e58-remove`: as `e57`, the group removed 500 ms after the release (`remove`), 3,000 ms after it (`remove-late`), or with no stall (`remove-nostall`); events at 1,000 and 4,000 ms. 3 trials each.
- `between`: a focus event and, at once, a name change on the focused node; the client, on hearing the focus, registers for the node's name changes; then a second name change. 3 trials.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT: success after 1,089 ms, held in `FragmentRoot` | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989 | `ok`; band 1,000 to 1,200 ms |
| The wait follows the connection timeout: 532 to 551 ms at 500 ms, 2,190 to 2,226 at 2,000, 5,514 to 5,541 at 5,000, and the same at 2,000 with the transaction timeout at 500 and 20,000 ms (e57); 540 to 549 and 5,453 to 5,504 ms (the CI test) | 2026-10-18 | Build 26200 | e57, `recorded/e57/` (`m_*`, `t_*`); the CI test | Bands from the connection timeout |
| e57: at connection 2,000 ms and transaction 5,000 ms, released 200 ms after the return, events up to 1,750 ms after the stall's end lost and from 1,800 ms heard; none heard late; none lost after the first heard | 2026-10-18 | Build 26200 | `recorded/e57/m_in_cto2000.out`, `m_out_cto2000.out` (10 trials) | All lost up to 1,500 ms, all heard from 2,300 ms |
| The original e57, built at e04bdd8a against mockapp's `small.json` on a desktop of its own: the same, events up to 1,750 ms lost and from 1,800 ms heard, the registration returning after 2,193 to 2,207 ms; the same with the stall not posting (`E57_POSTED=0`) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | `recorded/e57-original-2026-11-05/` (5 and 2 trials) | As above |
| e58: one advise at the release; nothing for 6 s with no event; A loses B's events, C and D lose none; `remove` `UIA_E_TIMEOUT`, handler kept 15 s, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeed, handler freed, neither event heard | 2026-10-18 | Build 26200 | `recorded/e58/` (5 trials each) | Deterministic |
| First uia-facts run, `ett`: `ok` 5 of 5, 1,096.3 ms (1,085.0 to 1,099.3) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 14cb91d7 with the client made as Verbatim's (the commit after it) | Pass |
| First uia-facts run, `connection`: `ok` 5 of 5; 541.1 ms (535.1 to 543.4) at 500 ms; 5,525.4 ms (5,514.4 to 5,538.3) at 5,000 ms | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e57`: `ok` 5 of 5 after 2,200.4 ms (2,198.2 to 2,210.5); every event up to 1,500 ms after the stall's end lost, every one from 2,300 ms heard, none late, none lost after the first heard, 5 of 5; the last lost raised 1,951 to 1,953 ms and the first heard 2,001 to 2,002 ms after the return | 2026-11-05 | As above | As above | Pass; the edge for trends |
| First uia-facts run, `e57-no-recovery`: every event raised from the stall's end heard, the first raised 201 ms after the return, 3 of 3 | 2026-11-05 | As above | As above | This record |
| First uia-facts run, `e58-silence`: one advise within 20 ms of the release 3 of 3; nothing for 6 s 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-scope`: A lost exactly B's events, 3 of 3; C and D lost none, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-remove`: `remove` `UIA_E_TIMEOUT`, the handler kept, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeeded, the handler freed, neither event heard; 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `between`: the focus heard, the registration made, the name change before it not heard, the one after it heard, 3 of 3 | 2026-11-05 | As above | As above | This record |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-registration-that-times-out-reports-success.exe`, or one arm with `--arm`. No desktop, application or debugger; it starts a copy of itself for each trial, and another for D. About 5 minutes. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_group_registered_while_its_provider_is_held_reports_success`, and the same for `a_read_while_its_provider_is_held_fails`.

## Evidence kept

These are raw outputs, one line per trial, kept whole; the fields each is cited for are named.

- `recorded/e57/m_in_*` and `m_out_*`: e57 at connection timeouts of 500, 2,000 and 5,000 ms (transaction timeout 5,000 ms), in-process and from a second process. Cited: each line's `registered ok <ms>`, the registration's outcome and wait; and in `m_in_cto2000.out` and `m_out_cto2000.out`, each event's `heard` or `not heard`, the gap.
- `recorded/e57/t_in_*` and `t_out_*`: e57 at a connection timeout of 2,000 ms with the transaction timeout at 500 and 20,000 ms. Cited: `registered ok <ms>`, the wait following the connection timeout whatever the transaction timeout.
- `recorded/e57-original-2026-11-05/`: the original e57 run on this machine on 2026-11-05: `m_in_cto2000.out` (5 trials) and `x_noposted.out` (2 trials, `E57_POSTED=0`). Cited: whole lines, the wait and the gap.
- `recorded/e58/silent.out`: Cited: `advises`, one advise at the release, and the empty log after the backlog.
- `recorded/e58/scope.out`: Cited: the `A:` and `B:` fields, the same events lost, and the `C:` and `D:` fields, none lost.
- `recorded/e58/remove.out`, `remove-late.out` and `remove-nostall.out`: Cited: the removal's outcome, the handler's release, and the two events heard or lost.

E-TT's rerun output is with [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md), whose search it times.
