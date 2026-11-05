# A registration that times out reports success

## Fact

`AddEventHandlerGroup` on a window whose provider does not answer returns success after the connection timeout, whatever the transaction timeout, though the provider never answered (`books/dev/src/reference/uia-bugs.md`, the fault of that name). The registration's own request runs as the stall ends: the provider is advised of it once, and with no event raised nothing else reaches it for 6 seconds. Once the stall ends the registration delivers every event raised; a registration made before the stall on another client object, one on another window and one on the same window from another process lose none; and a removal 500 ms after the stall's end succeeds. Events raised between a focus change and a registration made for the new focus are not heard (`books/dev/src/explanation/windows/uia.md`, "Events into the client").

## What Verbatim relies on

`timed_out` in `crates/verbatim-uia/src/subscribe.rs`, which treats a registration slower than `CONNECTION_TIMEOUT` as timed out and logs it at warn, the only sign, and `Follower::close`'s second removal of a group a move failed to remove. If registrations began to fail honestly, the follower's error path would run instead, which is not wrong; if the wait stopped following the connection timeout, the warning would misfire.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/a_registration_that_times_out_reports_success.rs`, check that the registration returns success while its provider is held and that a read on a held provider fails: `a_group_registered_while_its_provider_is_held_reports_success` and `a_read_while_its_provider_is_held_fails`. The program covers how long the registration waits and what follows it, which needs timing and pauses a test may not make.

Every trial runs in a client process of its own (the program's `trial` role), as e57 and e58 ran each trial in a test process of its own, so no other trial's registration is in the process. Every registration is an event handler group holding UIA's focus event, subtree scope, the sender's name cached, on the provider program's window, as e57 and e58 made it. Each focus event is raised as e57 raised it through mockapp's `set-name` and `focus`: the next of nodes 5 to 8 in turn is renamed `ev<i>` with a name change raised, then the tree's focus moves to it and its focus event is raised, so the handler names the event it heard. A stall is the window's UI thread busy until released, taking no message. The stall's release 200 ms after the registration returns, the event offsets, the 2 s of listening after the last event and the 6 s of silence are deliberate waits, since the fact is about time; the removal arms watch 15 s for the handler's release.

- `ett`: E-TT's registration, both timeouts 1,000 ms, the provider holding `FragmentRoot`. 5 trials.
- `connection`: a registration during a stall at connection 500 ms and transaction 5,000 ms, and the reverse. 5 trials each.
- `e57`: connection 2,000 ms, transaction 5,000 ms; a focus event every 50 ms from the stall's end to 2,500 ms after it. 5 trials.
- `e58-silence`: as `e57` with no event; the provider's log 6 s after the release. 3 trials.
- `e58-scope`: two provider programs P and Q. Before the stall of P, group A on P and group C on Q from the trial's client object, and group D on P from a second client process; during the stall, group B on P from another client object. Events on P every 100 ms from the stall's end to 3,000 ms, on Q every 100 ms from 50 to 2,950 ms. 3 trials.
- `e58-remove`: as `e57`, the group removed 500 ms after the release (`remove`), 3,000 ms after it (`remove-late`), or with no stall (`remove-nostall`); events at 1,000 and 4,000 ms. 3 trials each.
- `between`: a focus event and, at once, a name change on the focused node; the client, on hearing the focus, registers for the node's name changes; then a second name change. 3 trials.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT: success after 1,089 ms, held in `FragmentRoot` | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989 | `ok`; band 1,000 to 1,200 ms |
| The wait follows the connection timeout: 532 to 551 ms at 500 ms, 2,190 to 2,226 at 2,000, 5,514 to 5,541 at 5,000 (e57); 540 to 549 and 5,453 to 5,504 ms (the CI test) | 2026-10-18 | Build 26200 | e57, `recorded/e57/`; the CI test | Bands from the connection timeout |
| e58: one advise at the release; nothing for 6 s with no event; C and D lose none; with no stall, the removal succeeds and frees the handler | 2026-10-18 | Build 26200 | `recorded/e58/` (5 trials each) | Deterministic |
| First uia-facts run, `ett`: `ok` 5 of 5, 1,091.5 ms (1,090.8 to 1,095.6) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | FIXCOMMIT | Pass |
| First uia-facts run, `connection`: `ok` 5 of 5; 541.0 ms (538.0 to 545.6) at 500 ms; 5,521.9 ms (5,511.7 to 5,529.5) at 5,000 ms | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e57`: `ok` 5 of 5 after 2,197.7 ms (2,190.1 to 2,203.5); every event raised from the stall's end heard, each within 100 ms of its raise, 5 of 5; the first heard was raised 201 ms after the return, as the stall ended | 2026-11-05 | As above | As above | The wait passes; the delivery is this record |
| First uia-facts run, `e58-silence`: one advise within 20 ms of the release 3 of 3; nothing for 6 s 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-scope`: A and B lost no event, 3 of 3; C and D lost none, 3 of 3 | 2026-11-05 | As above | As above | C and D pass; A and B are this record |
| First uia-facts run, `e58-remove`: `remove` succeeded and freed the handler, neither event heard; `remove-late` succeeded and freed the handler, the 1,000 ms event heard; `remove-nostall` succeeded and freed the handler, neither event heard; 3 of 3 each | 2026-11-05 | As above | As above | `remove-nostall` passes; the others are this record |
| First uia-facts run, `between`: the focus heard, the registration made, the name change before it not heard, the one after it heard, 3 of 3 | 2026-11-05 | As above | As above | This record |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-registration-that-times-out-reports-success.exe`, or one arm with `--arm`. No desktop, application or debugger; it starts a copy of itself for each trial, and another for D. About 5 minutes. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_group_registered_while_its_provider_is_held_reports_success`, and the same for `a_read_while_its_provider_is_held_fails`.

## Evidence kept

`recorded/e57/`: e57's outputs at connection timeouts of 500, 2,000 and 5,000 ms, in-process (`m_in_*`) and from a second process (`m_out_*`), one line per trial, which time the registration's wait. `recorded/e58/`: e58's `silent`, `scope` and `remove-nostall` outputs, one line per trial. E-TT's rerun output is with [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md), whose search it times.
