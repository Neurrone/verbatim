# The gap after a timed-out registration silences the process's registrations on that window

## Fact

With the client's connection recovery on, a registration that timed out during a stall delivers nothing for about 2 seconds after its call returned ([a registration that times out reports success](../a-registration-that-times-out-reports-success/readme.md)). That gap silences every registration the registering process has on that window, one made before the stall on another client object included, which loses the same events; a registration on another window in the same process, and one on the same window in another process, hear every event. A removal during the gap fails at once with `UIA_E_TIMEOUT` and leaves the registration in place: its handler is not freed and it delivers once the gap ends. Removed once it has delivered an event, it is removed at once and its handler released. The registration's own request runs as the stall ends: a provider that is told of registrations is advised of it then, once, and with no event raised nothing else reaches the application in the 3 seconds after the stall's end, so nothing marks the gap's end (`books/dev/src/reference/uia-bugs.md`, "A registration that times out reports success").

## What Verbatim relies on

`Follower::close`'s second removal of a group a move failed to remove (`crates/verbatim-uia/src/subscribe.rs`), and the unbuilt proposals `books/dev/src/reference/uia-bugs.md` describes, which need a safe time to remove a timed-out registration. If a removal during the gap began to succeed, the second removal would find nothing to remove, which is not wrong.

## Method

Every trial runs in a client process of its own (the program's `trial` role), as e58 ran each trial in a test process of its own. Its client object takes the run's `--recovery` and `--coalescing`; both timeouts are 500 ms, since the gap's end follows neither timeout. Every registration is an event handler group holding UIA's focus event, subtree scope, the sender's name cached, on the provider program's window, as e58 made it. Each focus event renames the next of nodes 5 to 8 in turn with a name change raised, moves the tree's focus to it and raises its focus event, so the handler names the event it heard. A stall is the window's UI thread busy until released, 200 ms after the registration returns. The waits are deliberate where the fact is about time: the 3 s of silence, the event offsets and the 0.5 s of listening after the last event; the removal cases wait for evidence instead.

- `e58-silence`: a registration during a stall and no event; the provider's log read 3 s after the release. The stall's backlog is the registration's own request, an unbroken run of calls on the window's root from the release, ended by the provider's advise of the focus event.
- `e58-scope`: two provider programs P and Q. Before the stall of P, group A on P and group C on Q from the trial's client object, and group D on P from a second client process; during the stall, group B on P from another client object. Events on P every 100 ms from the stall's end to 2,400 ms, on Q every 100 ms from 50 to 2,350 ms.
- `e58-remove`: the group removed 500 ms after the release, in the gap (`remove`); once it has heard one of the events raised every 100 ms from the release, past the gap (`remove-late`); or with no stall (`remove-nostall`). After the removal an event every 100 ms until the handler is freed or an event is heard, up to 3 s; once freed, one more event, listened for 0.5 s. A handler's release comes within milliseconds of a removal that succeeds, so a registration that delivers after a removal shows the removal left it in place.

Judged: the advise and the silence (deterministic); A's loss against B's (a relation), and C's and D's losses (deterministic); each removal's outcome, the handler's release and whether an event after the removal was heard (deterministic).

## Client settings

Event coalescing changes no outcome; connection recovery causes the gap (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both). The documents state the gap, so the gap's cases are judged with `--recovery`; the program judges each setting against its own record. `e58-silence`, and C and D of `e58-scope`, do not change.

- Connection recovery on, as Verbatim's and NVDA's clients have it: A loses exactly B's events; `remove` fails with `UIA_E_TIMEOUT`, its handler kept and an event after it heard. `target\debug\the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window.exe --recovery`
- Connection recovery off, the default: no gap. A and B lose no event, and `remove` succeeds, its handler freed and no event after it heard. `target\debug\the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window.exe`

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| e58: one advise at the release; nothing for 6 s with no event; A loses B's events, C and D lose none; `remove` `UIA_E_TIMEOUT`, handler kept 15 s, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` 3 s after the release and `remove-nostall` succeed, handler freed, neither event heard | 2026-10-18 | Build 26200 | `recorded/e58/` (5 trials each) | Deterministic |
| The first uia-facts runs, in the registration program at 09bbb0d7 and 1363fe22, recovery on: one advise within 20 ms of the release and nothing for 6 s, 3 of 3; A lost exactly B's events, C and D none, 3 of 3; `remove` `UIA_E_TIMEOUT`, the handler kept 5 s, the 1,000 ms event lost and the 4,000 ms one heard; `remove-late` and `remove-nostall` succeeded, the handler freed, neither event heard; 3 of 3 each | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | The registration program's records | Pass |
| This program's first run, recovery on: one advise ending the backlog, nothing for 3 s; A lost exactly B's events, C and D none; `remove` `UIA_E_TIMEOUT`, the handler not freed, an event after it heard; `remove-late` and `remove-nostall` `ok`, the handler freed, no event after heard | 2026-11-06 | As above | ff23f81a | Pass |
| This program's first run, recovery off: one advise, nothing for 3 s; A and B lost none, C and D none; every removal `ok`, the handler freed, no event after heard | 2026-11-06 | As above | ff23f81a | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window.exe --recovery` for the gap the documents state, or without `--recovery` for the outcome with connection recovery off; one case with `--case`. Under `--pause`, each trial's process prints its provider's and its own process ids once its provider is up, and waits for Enter here. No desktop, application or debugger; it starts a copy of itself for each trial, and another for D.

## Evidence kept

These are raw outputs, one line per trial, kept whole; the fields each is cited for are named. They moved here from the registration program's folder.

- `recorded/e58/silent.out`: Cited: `advises`, one advise at the release, and the empty log after the backlog.
- `recorded/e58/scope.out`: Cited: the `A:` and `B:` fields, the same events lost, and the `C:` and `D:` fields, none lost.
- `recorded/e58/remove.out`, `remove-late.out` and `remove-nostall.out`: Cited: the removal's outcome, the handler's release, and the two events heard or lost.

`recorded/e58/onset.out` stays with [the registration program](../a-registration-that-times-out-reports-success/readme.md), whose `e57` case judges the onset.
