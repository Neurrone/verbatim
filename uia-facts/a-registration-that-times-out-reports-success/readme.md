# A registration that times out reports success

## Fact

`AddEventHandlerGroup` on a window whose provider does not answer returns success after the connection timeout, whatever the transaction timeout, though the provider never answered (`books/dev/src/reference/uia-bugs.md`, the fault of that name). With the client's connection recovery on, as Verbatim's and NVDA's clients have it, such a registration then delivers nothing until about 2 seconds after its call returned (1,981 to 2,001 ms), or until the stall ends if that is later, and every event raised in that gap is lost, not delivered late; with recovery off there is no gap. The gap's end follows neither timeout, the next event does not bring it forward, and a registration from a second client process behaves alike. With no stall the registration returns at once and hears every event. The events raised in the gap reach the provider, which makes calls after each one; that they are lost in the registering client is inferred. The registration's own request runs as the stall ends: the provider is advised of it once, and with no event raised nothing else reaches it for 6 seconds. The gap silences every registration the process has on that window, one made before the stall on another client object included, and no registration on another window or in another process. A removal during the gap fails at once with `UIA_E_TIMEOUT` and leaves the registration in place; after the gap it succeeds. Events raised between a focus change and a registration made for the new focus are not heard (`books/dev/src/explanation/windows/uia.md`, "Events into the client").

## What Verbatim relies on

`timed_out` in `crates/verbatim-uia/src/subscribe.rs`, which treats a registration slower than `CONNECTION_TIMEOUT` as timed out and logs it at warn, the only sign, and `Follower::close`'s second removal of a group a move failed to remove. If registrations began to fail honestly, the follower's error path would run instead, which is not wrong; if the wait stopped following the connection timeout, the warning would misfire. The gap's figures are used only by an unbuilt proposal, which `books/dev/src/reference/uia-bugs.md` describes.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/a_registration_that_times_out_reports_success.rs`, check that the registration returns success while its provider is held and that a read on a held provider fails: `a_group_registered_while_its_provider_is_held_reports_success` and `a_read_while_its_provider_is_held_fails`. The program covers how long the registration waits and what follows it, which needs timing and pauses a test may not make.

Every trial runs in a client process of its own (the program's `trial` role), as e57 and e58 ran each trial in a test process of its own, so no other trial's registration is in the process. Its client object takes the run's `--recovery` and `--coalescing`; Verbatim's client (`create_client` in `crates/verbatim-uia/src/client.rs`) and e57's had both on, and the first uia-facts runs below were made with both on. Every registration is an event handler group holding UIA's focus event, subtree scope, the sender's name cached, on the provider program's window, as e57 and e58 made it. Each focus event is raised as e57 raised it through mockapp's `set-name` and `focus`: the next of nodes 5 to 8 in turn is renamed `ev<i>` with a name change raised, then the tree's focus moves to it and its focus event is raised, so the handler names the event it heard. A stall is the window's UI thread busy until released, taking no message. The stall's release 200 ms after the registration returns, the event offsets, the 2 s of listening after the last event and the 6 s of silence are deliberate waits, since the fact is about time; the removal cases watch 15 s for the handler's release.

- `ett`: E-TT's registration, both timeouts 1,000 ms, the provider holding `FragmentRoot`.
- `connection`: a registration during a stall at connection 500 ms and transaction 5,000 ms, and the reverse.
- `e57`: connection 2,000 ms, transaction 5,000 ms; a focus event every 50 ms from the stall's end to 2,500 ms after it. Then the same at connection timeouts of 500, 2,000 and 5,000 ms (transaction 5,000 ms) and transaction timeouts of 500 and 20,000 ms (connection 2,000 ms); with events every 10 ms from 1,700 to 1,850 ms after the stall's end; and with no stall, events from the return.
- `release-later`: the stall released 1,000, 1,700 and 3,000 ms after the return, and outlasting it by 4,000 and 10,000 ms at a 500 ms connection timeout and 6,000 ms at 2,000 ms.
- `next-event`: single events at 1,500 or 2,000 ms after the stall's end, and the pair at 1,000 and 2,500 ms.
- `second-process`: `e57` registered by a second client process (the `registrar` role), given the run's flags.
- `onset`: a focus event every 100 ms from the stall's end to 3,000 ms after it, and the provider's log: whether provider work follows each event raised in the gap.
- `e58-silence`: as `e57` with no event; the provider's log 6 s after the release.
- `e58-scope`: two provider programs P and Q. Before the stall of P, group A on P and group C on Q from the trial's client object, and group D on P from a second client process; during the stall, group B on P from another client object. Events on P every 100 ms from the stall's end to 3,000 ms, on Q every 100 ms from 50 to 2,950 ms.
- `e58-remove`: as `e57`, the group removed 500 ms after the release (`remove`), 3,000 ms after it (`remove-late`), or with no stall (`remove-nostall`); events at 1,000 and 4,000 ms.
- `between`: a focus event and, at once, a name change on the focused node; the client, on hearing the focus, registers for the node's name changes; then a second name change.

## Client settings

Event coalescing changes no outcome; connection recovery causes the gap (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both). The documents state the gap, so the gap's cases are judged with `--recovery`; the program judges each setting against its own record. The registration's success and its wait on the connection timeout (`ett`, `connection`, and `e57`'s first two claims), the removal with no stall, `e58-silence`, C and D of `e58-scope`, and `between` do not change.

- Connection recovery on, as Verbatim's and NVDA's clients have it: the gap, as the Fact says and the records below show. `target\debug\a-registration-that-times-out-reports-success.exe --recovery`, or one case, such as `--recovery --case e57`.
- Connection recovery off, the default: no gap. In `e57` every event raised from the stall's end is heard, the first 201 ms after the registration returned; at every timeout, released at any time, and from a second client process, the first event heard is the one at the stall's end and none is lost; the single events at 1,500 and 2,000 ms and the pair are all heard; in `onset` none is lost and nothing reaches the provider between the backlog and the first delivery; in `e58-scope` neither A nor B loses an event; in `e58-remove` the removal 500 ms after the release succeeds and frees the handler, and with it 3,000 ms after, the 1,000 ms event is heard. `target\debug\a-registration-that-times-out-reports-success.exe`, or `--case e57`.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT: success after 1,089 ms, held in `FragmentRoot` | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989 | `ok`; band 1,000 to 1,200 ms |
| The wait follows the connection timeout: 532 to 551 ms at 500 ms, 2,190 to 2,226 at 2,000, 5,514 to 5,541 at 5,000, and the same at 2,000 with the transaction timeout at 500 and 20,000 ms (e57); 540 to 549 and 5,453 to 5,504 ms (the CI test) | 2026-10-18 | Build 26200 | e57, `recorded/e57/` (`m_*`, `t_*`); the CI test | Bands from the connection timeout |
| e57: at connection 2,000 ms and transaction 5,000 ms, released 200 ms after the return, events up to 1,750 ms after the stall's end lost and from 1,800 ms heard; none heard late; none lost after the first heard | 2026-10-18 | Build 26200 | `recorded/e57/m_in_cto2000.out`, `m_out_cto2000.out` (10 trials) | All lost up to 1,500 ms, all heard from 2,300 ms |
| The original e57, built at e04bdd8a against mockapp's `small.json` on a desktop of its own: the same, events up to 1,750 ms lost and from 1,800 ms heard, the registration returning after 2,193 to 2,207 ms; the same with the stall not posting (`E57_POSTED=0`) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | `recorded/e57-original-2026-11-05/` (5 and 2 trials) | As above |
| e57: the gap's end the same at connection timeouts of 500, 2,000 and 5,000 ms and transaction timeouts of 500 and 20,000 ms; in 10 ms steps, 1,981 to 2,001 ms after the return; released 1,000 and 1,700 ms after the return, heard from 1,000 and 300 ms after the stall's end, and with no gap released 3,000 ms after or more; a single event at 1,500 ms never heard and at 2,000 ms heard; of the pair at 1,000 and 2,500 ms, the first lost; a second process alike; with no stall, the registration in 3 ms and every event heard | 2026-10-18 | Build 26200 | `recorded/e57/` (`m_*`, `t_*`, `f_*`, `h_*`, `K_*`, `s_*`, `c_*`) | As each claim |
| e58: one advise at the release; nothing for 6 s with no event; A loses B's events, C and D lose none; `remove` `UIA_E_TIMEOUT`, handler kept 15 s, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeed, handler freed, neither event heard | 2026-10-18 | Build 26200 | `recorded/e58/` (5 trials each) | Deterministic |
| First uia-facts run, `ett`: `ok` 5 of 5, 1,096.3 ms (1,085.0 to 1,099.3) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 1731dcd6 | Pass |
| First uia-facts run, `connection`: `ok` 5 of 5; 541.1 ms (535.1 to 543.4) at 500 ms; 5,525.4 ms (5,514.4 to 5,538.3) at 5,000 ms | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e57`: `ok` 5 of 5 after 2,200.4 ms (2,198.2 to 2,210.5); every event up to 1,500 ms after the stall's end lost, every one from 2,300 ms heard, none late, none lost after the first heard, 5 of 5; the last lost raised 1,951 to 1,953 ms and the first heard 2,001 to 2,002 ms after the return | 2026-11-05 | As above | As above | Pass; the edge for trends |
| First uia-facts run, `e57` with connection recovery off: every event raised from the stall's end heard, the first raised 201 ms after the return, 3 of 3 | 2026-11-05 | As above | As above | This record |
| First uia-facts run, `e57`, other timeouts: the last event lost 1,750 ms and the first heard 1,800 ms after the stall's end at every timeout, 3 of 3 each | 2026-11-05 | As above | 1d85c0a0 | Pass |
| First uia-facts run, `e57`, near the edge: the last lost 1,991 to 1,992 ms and the first heard 2,001 to 2,002 ms after the return, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `release-later`: (950, 1,000) at 1,000 ms, (250, 300) at 1,700 ms, and every event heard from the stall's end at 3,000, 4,000, 10,000 and 6,000 ms, 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `next-event`: the single event at 1,500 ms never heard, at 2,000 ms heard; of the pair, the first lost and the second heard; 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `second-process`: `ok`; (1,750, 1,800); lost, not late; 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e57`, no stall: the registration in 3.4 to 3.5 ms; every event heard within 100 ms, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `onset`: (1,700, 1,800); provider work after each event in the gap, 3 of 3; 997 log entries and no `WM_GETOBJECT` between the backlog and the first delivery | 2026-11-05 | As above | As above | Pass; the count for trends |
| First uia-facts run, `e58-silence`: one advise within 20 ms of the release 3 of 3; nothing for 6 s 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-scope`: A lost exactly B's events, 3 of 3; C and D lost none, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-remove`: `remove` `UIA_E_TIMEOUT`, the handler kept, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeeded, the handler freed, neither event heard; 3 of 3 each | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `between`: the focus heard, the registration made, the name change before it not heard, the one after it heard, 3 of 3 | 2026-11-05 | As above | As above | This record |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-registration-that-times-out-reports-success.exe`, or one case with `--case`. No desktop, application or debugger; it starts a copy of itself for each trial, and another for D. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_group_registered_while_its_provider_is_held_reports_success`, and the same for `a_read_while_its_provider_is_held_fails`.

## Evidence kept

These are raw outputs, one line per trial, kept whole; the fields each is cited for are named.

- `recorded/e57/m_in_*` and `m_out_*`: e57 at connection timeouts of 500, 2,000 and 5,000 ms (transaction timeout 5,000 ms), in-process and from a second process. Cited: each line's `registered ok <ms>`, the registration's outcome and wait, and each event's `heard` or `not heard`, the gap at each connection timeout; `m_out_*` from a second process.
- `recorded/e57/f_*`: e57 in 10 ms steps. Cited: the last event lost and the first heard, the edge.
- `recorded/e57/h_*` and `K_*`: the stall released 1,000, 1,700 and 3,000 ms after the return, and outlasting it. Cited: each event's `heard` or `not heard`.
- `recorded/e57/s_*`: single events and the pair. Cited: each event's `heard` or `not heard`.
- `recorded/e57/c_*`: the control with no stall. Cited: `registered ok <ms>` and every event heard.
- `recorded/e57/t_in_*` and `t_out_*`: e57 at a connection timeout of 2,000 ms with the transaction timeout at 500 and 20,000 ms. Cited: `registered ok <ms>`, the wait following the connection timeout whatever the transaction timeout, and each event's `heard` or `not heard`, the gap's end the same.
- `recorded/e57-original-2026-11-05/`: the original e57 run on this machine on 2026-11-05: `m_in_cto2000.out` (5 trials) and `x_noposted.out` (2 trials, `E57_POSTED=0`). Cited: whole lines, the wait and the gap.
- `recorded/e58/onset.out`: Cited: the events lost and heard, and the provider work after each raise.
- `recorded/e58/silent.out`: Cited: `advises`, one advise at the release, and the empty log after the backlog.
- `recorded/e58/scope.out`: Cited: the `A:` and `B:` fields, the same events lost, and the `C:` and `D:` fields, none lost.
- `recorded/e58/remove.out`, `remove-late.out` and `remove-nostall.out`: Cited: the removal's outcome, the handler's release, and the two events heard or lost.

E-TT's rerun output is with [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md), whose search it times.
