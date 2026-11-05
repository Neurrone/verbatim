# A registration that times out reports success

## Fact

`AddEventHandlerGroup` on a window whose provider does not answer returns success after the connection timeout, though the provider never answered (`books/dev/src/reference/uia-bugs.md`, the fault of that name). Such a registration then delivers nothing until about 2 seconds after its call returned (1,981 to 2,001 ms), or until the stall ends if that is later, and every event raised in that gap is lost, not delivered late. The registration's own request runs as the stall ends: the provider is advised of it once, and with no event raised nothing else reaches it for 6 seconds. The gap silences every registration the process has on that window, one made before the stall on another client object included, and no registration on another window or in another process. A removal during the gap fails at once with `UIA_E_TIMEOUT` and leaves the registration in place; after the gap it succeeds. That events raised between a focus change and a registration made for the new focus are not heard is inferred (`books/dev/src/explanation/windows/uia.md`, "Events into the client").

## What Verbatim relies on

`timed_out` in `crates/verbatim-uia/src/subscribe.rs`, which treats a registration slower than `CONNECTION_TIMEOUT` as timed out and logs it at warn, the only sign, and `Follower::close`'s second removal of a group a move failed to remove. If registrations began to fail honestly, the follower's error path would run instead, which is not wrong; if the wait stopped following the connection timeout, the warning would misfire. The gap's figures are used only by an unbuilt proposal.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/a_registration_that_times_out_reports_success.rs`, check that the registration returns success while its provider is held and that a read on a held provider fails: `a_group_registered_while_its_provider_is_held_reports_success` and `a_read_while_its_provider_is_held_fails`. The program covers how long the registration waits and what follows it, which needs timing and pauses a test may not make.

Every registration is an event handler group holding UIA's focus event, subtree scope, the sender's name cached, on the provider program's window, as e57 and e58 made it. Each focus event is raised on node 5, renamed `ev<i>` first, so the handler names the event it heard. A stall is the window's UI thread busy until released, taking no message. The stall's release 200 ms after the registration returns, the event offsets, the 2 s of listening after the last event and the 6 s of silence are deliberate waits, since the fact is about time; the removal arms watch 15 s for the handler's release.

- `ett`: E-TT's registration, both timeouts 1,000 ms, the provider holding `FragmentRoot`. 5 trials.
- `connection`: a registration during a stall at connection 500 ms and transaction 5,000 ms, and the reverse. 5 trials each.
- `e57`: connection 2,000 ms, transaction 5,000 ms; a focus event every 50 ms from the stall's end to 2,500 ms after it. 5 trials.
- `e58-silence`: as `e57` with no event; the provider's log 6 s after the release. 3 trials.
- `e58-scope`: two provider programs P and Q. Before the stall of P, group A on P and group C on Q from the main client object, and group D on P from a second client process; during the stall, group B on P from another client object. Events on P every 100 ms from the stall's end to 3,000 ms, on Q every 100 ms from 50 to 2,950 ms. 3 trials.
- `e58-remove`: as `e57`, the group removed 500 ms after the release (`remove`), 3,000 ms after it (`remove-late`), or with no stall (`remove-nostall`); events at 1,000 and 4,000 ms. 3 trials each.
- `between`: a focus event and, at once, a name change on the focused node; the client, on hearing the focus, registers for the node's name changes; then a second name change. 3 trials.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-TT: success after 1,089 ms, held in `FragmentRoot` | 2026-10-10 | Build 26200 | `experiments/mockapp-experiments-2026-10-10/` at b0a1989 | `ok`; band 1,000 to 1,200 ms |
| The wait follows the connection timeout: 532 to 551 ms at 500 ms, 2,190 to 2,226 at 2,000, 5,514 to 5,541 at 5,000 (e57); 540 to 549 and 5,453 to 5,504 ms (the CI test) | 2026-10-18 | Build 26200 | e57, `recorded/e57/`; the CI test | Bands from the connection timeout |
| e57: events up to 1,750 ms after the stall's end lost, from 1,800 ms heard, 30 of 30; none heard late; none lost after the first heard | 2026-10-18 | Build 26200 | `recorded/e57/` (180 trials) | All lost up to 1,500 ms, all heard from 2,300 ms |
| e58: one advise at the release; nothing for 6 s with no event; A loses B's events, C and D lose none; `remove` `UIA_E_TIMEOUT`, handler kept 15 s, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeed, handler freed, neither event heard | 2026-10-18 | Build 26200 | `recorded/e58/` (5 trials each) | Deterministic |
| First uia-facts run, `ett`: `ok` 5 of 5, 1,085 ms (1,084 to 1,089) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | The commit that added this folder | Pass |
| First uia-facts run, `connection`: `ok` 5 of 5; 535 ms (532 to 539) at 500 ms; 5,521 ms (5,519 to 5,523) at 5,000 ms | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e57`: `ok` 5 of 5 after 2,195 ms (2,191 to 2,201), pass; but every event was heard, the first raised as the stall ended, 200 to 202 ms after the return: events up to 1,500 ms lost, differ; from 2,300 ms heard, pass; none late, pass; none lost after the first heard, pass | 2026-11-05 | As above | As above | Differ, raised below |
| First uia-facts run, `e58-silence`: one advise within 20 ms of the release 3 of 3, pass; nothing for 6 s 3 of 3, pass | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `e58-scope`: A lost no event, so "A loses B's events" differs; C and D lost none, pass | 2026-11-05 | As above | As above | Differ, raised below |
| First uia-facts run, `e58-remove`: `remove` succeeded and freed the handler, neither event heard, differ; `remove-late` heard the 1,000 ms event, differ; `remove-nostall` as recorded, pass | 2026-11-05 | As above | As above | Differ, raised below |
| First uia-facts run, `between`: the focus heard, the registration made, the name change before it not heard, the one after it heard, 3 of 3 | 2026-11-05 | As above | As above | New: the inference holds |

### The differing gap, raised

Against the provider program, a registration that timed out heard every event from the stall's end. The registration's own behaviour matched e57 and e58 exactly (success, the wait on the connection timeout, one advise at the release, the silence after it), but there was no gap. The removal and scope arms differ only as consequences: with no gap, a removal 500 ms after the release succeeds, and A has nothing to lose. So the gap is not reproduced by this setup, and this run does not show that UIA dropped the gap.

What differs between the setups, any of which may matter, is not established:

- e57 and e58 ran against mockapp on a desktop of the test's own (`harness::run_isolated`); the provider program's windows are hidden windows on the user's desktop.
- mockapp stalled with its `stall-until-release` command; the provider program is busy inside a posted request's window procedure. Both take no message.
- The provider program's root always answers `IRawElementProviderAdviseEvents`; mockapp did so only under `--count-registrations`, which e58's `silent` and `onset` cases used and still met the gap.
- The machine's Windows revision then was not recorded beyond build 26200.

Until a run settles which, the entry in `books/dev/src/reference/uia-bugs.md` keeps its statement, with this run noted, and the gap's figures stay e57's and e58's. Settling it is the follow-up: run the `e57` arm with the provider program on a desktop of its own, as mockapp's harness does.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-registration-that-times-out-reports-success.exe`, or one arm with `--arm`. No desktop, application or debugger; it starts a second copy of itself for D. About 4 minutes. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_group_registered_while_its_provider_is_held_reports_success`, and the same for `a_read_while_its_provider_is_held_fails`.

## Evidence kept

`recorded/e57/` and `recorded/e58/`: e57's and e58's outputs, one line per trial, named after the case (`recorded/e57/explore/` holds the 66 exploratory trials that found e57's conditions). E-TT's rerun output is with [the timeout-bounds folder](../a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md), whose search it times.
