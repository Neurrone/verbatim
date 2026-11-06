# Elements cannot be passed to another process

## Fact

`CoMarshalInterface` of an `IUIAutomationElement` succeeds, but no other process can unmarshal it, whichever way it travels, while COM between the processes works; so the process that receives a UIA event is the only one that can read its sender. A sender stays usable in its own process for as long as the provider keeps the element, not for a fixed time: P3 read kept senders live after 0.6, 31 and 152 seconds (`books/dev/src/reference/uia-bugs.md`, the limitation of that name).

## What Verbatim relies on

The process that receives a UIA event is the only one that can read its element, which is why the listener judges and reads each focus itself (`books/dev/src/reference/uia-bugs.md`, the limitation's entry).

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/elements_cannot_be_passed_to_another_process.rs`, cover E-marshal and P3's removal and exit: `an_element_marshaled_for_another_process_cannot_be_unmarshaled_there`, `an_element_marshaled_for_another_apartment_cannot_be_unmarshaled_in_another_process`, `an_object_passed_to_another_process_works_until_a_call_passes_it_an_element` and `a_kept_sender_is_read_live_until_its_provider_removes_it`. They hold a sender briefly; the program covers P3's long pauses, which no test may make.

- `kept`: the program registers the focus event in an event handler group on the provider program's window, subtree scope, as P3's prototype did. The provider moves its focus to node 5 and raises the event; the program keeps the sender and reads it live (`CurrentName`, `CurrentHasKeyboardFocus`, `CurrentProcessId`) 0.6 and 28 s after the event. The pauses are deliberate: the fact is about how long a kept sender lasts. P3 read at 31 and 152 s; the program reads at 28 s, the longest pause its run time allows, which narrows the claim it checks to 28 s.

Judged: each read's outcome (deterministic).

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-marshal: `CoUnmarshalInterface` in the other process `E_FAIL` 3 of 3; the in-process form `E_UNEXPECTED` | 2026-10-10 | Build 26200 | the mockapp experiments, in git at f86da7f, now the CI tests | The tests |
| P3: live reads of kept senders at 0.6, 31 and 152 s succeeded, each in 0.2 to 1.3 ms; once the node was removed `UIA_E_ELEMENTNOTAVAILABLE` | 2026-10-18 | Build 26200 | `recorded/p3_held.out` | `ok` at each pause; the removal is the CI test's |
| First uia-facts run, `kept`: the kept sender's three reads `ok` at 0.6 and 28 s | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\elements-cannot-be-passed-to-another-process.exe`. No desktop or application. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_kept_sender_is_read_live_until_its_provider_removes_it`, and the same with the other three names.

## Evidence kept

- `recorded/p3_held.out`: P3's output. Cited: the `held` lines at 0.6, 31 and 152 s, each read's outcome and time.
