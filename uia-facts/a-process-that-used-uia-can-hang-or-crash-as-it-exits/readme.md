# A process that used UIA can hang or crash as it exits

## Fact

A process that has connected to UIA providers as a client sometimes hangs at full processor use, or crashes with an access violation, as it exits normally and runs its DLLs' process-detach code: `UIAutomationCore.dll`'s detach code destroys its telemetry object and walks a corrupt list of per-provider connection records. It happened in about 1 to 3 percent of runs of the arbitration tests (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

Its processes end themselves with `TerminateProcess` once shut down, so the detach code never runs: outposts and the listener (`books/dev/src/explanation/architecture.md`, section 1), and the mockapp test runner (`crates/mockapp/tests/common/harness.rs`).

## Method

- `exit`: 150 child processes (the `exiter` role), one after another, where the design set 300, to keep the run short: one hang among them passes against the recorded 1 in 100, and none is inconclusive; `--trials 2` runs 300, the fewest at which none differs (0.99 to the 300th power is below 0.05). Each makes UIA's first-time setup on its main thread, starts six provider programs of its own and, from a thread each, connects as a UIA client to each of a provider's three windows (one with a UIA provider, one with its own `IAccessible`, one plain), reads its name, searches its subtree and registers a name-change handler, which stays registered. Each child then ends its providers, as the mockapp tests' providers ended before their test process, and ends normally with `std::process::exit`, running every DLL's detach code. Six threads each connected to a mockapp of their own hung 1 in 40 in the original's run. Only children that report their work done count. A child that has not ended 10 s after its work is a hang, and one that ends with an access violation a crash; a child that fails in any other way makes the claim inconclusive, with its outcome among the evidence. Judged as a rate against 1 in 100, the arbitration tests' rate after the first-time setup fix.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| About 1 to 3 percent of the arbitration tests' runs; 2 hangs in 100 runs after waiting 12 s for the providers to go; 1 in 40 before the first-time setup fix and 1 in 100 after; six threads each with a mockapp of its own 1 in 40, one thread 0 in 40 | 2026-10-03 | Build 26200 | Commits 1f69859e and fed532be | A rate, 1 of 100 |
| First uia-facts run, `exit`: 0 of 150 by default and with both settings; 1 of 150, a crash, with `--recovery` and with `--coalescing` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Inconclusive with none, pass with one, against 1 in 100 |
| `exit`, the children one after another, only access violations and hangs counted: 2 of 150, both hangs | 2026-11-06 | As above | d0a8d430 | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-process-that-used-uia-can-hang-or-crash-as-it-exits.exe`. No desktop or application.

## Evidence kept

None: commits 1f69859e and fed532be.
