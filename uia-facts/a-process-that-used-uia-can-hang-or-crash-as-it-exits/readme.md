# A process that used UIA can hang or crash as it exits

## Fact

A process that has connected to UIA providers as a client sometimes hangs at full processor use, or crashes with an access violation, as it exits normally and runs its DLLs' process-detach code: `UIAutomationCore.dll`'s detach code destroys its telemetry object and walks a corrupt list of per-provider connection records. It happened in about 1 to 3 percent of runs of the arbitration tests. It needs connections from several threads at once to several provider processes: from one thread, from two, or from six threads to one provider process, no child of 230 faulted, while three or more threads, each connecting to provider processes of its own, faulted in 1 to 5 percent of children, even when each only read a window's name. A proper teardown before the exit, every handler removed, every UIA object released and COM left on every thread, does not prevent it (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

Its processes end themselves with `TerminateProcess` once shut down, so the detach code never runs: outposts and the listener (`books/dev/src/explanation/architecture.md`, section 1), and the mockapp test runner (`crates/mockapp/tests/common/harness.rs`). A proper teardown does not prevent the fault, so ending without the detach code is what avoids it.

## Method

Each child process (the `exiter` role) makes UIA's first-time setup on its main thread, starts provider programs of its own and, from threads of its own, connects as a UIA client to their windows and reads each window's name, as its setup says. It then ends its providers, as the mockapp tests' providers ended before their test process, and ends normally with `std::process::exit`, running every DLL's detach code. Children run one after another. Only children that report their work done count. A child that has not ended 3 s after its work is a hang, since the detach code spins without end once it hangs, and one that ends with an access violation a crash; a child that fails in any other way makes the claim inconclusive, with its outcome among the evidence.

- `exit`: 150 children (Dickson, 2026-11-06) of the setup with the most faults a minute among the candidates: four provider programs with a UIA window each, a thread each, connecting and reading the name, the clients, elements and cache requests left in place. Judged as a rate against that setup's 8 of 230; 90 children would already make none differ.
- `candidates`, run only when named, 230 children of each setup: the first build's, six provider programs of three windows each (a UIA window, one with its own `IAccessible`, a plain one), a thread each, every connection reading, searching its subtree and registering a name-change handler; then that setup with one step removed or changed at a time, and smaller and larger ones. 230 children show a setup with none below the first build's rate of about 1 in 75. The rate and the faults a minute are printed as new.
- `clean`, run only when named: `exit`'s setup torn down before the exit: each thread removes its handlers, releases every element, cache request, handler and client and leaves COM, and the main thread does the same, the providers ended after the teardown in one run and before it in another, 90 children each, the fewest at which none separates the clean rate from 8 of 230. Judged against the dirty rate: differ when apart from it, pass when no difference shows.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| About 1 to 3 percent of the arbitration tests' runs; 2 hangs in 100 runs after waiting 12 s for the providers to go; 1 in 40 before the first-time setup fix and 1 in 100 after; six threads each with a mockapp of its own 1 in 40, one thread 0 in 40 | 2026-10-03 | Build 26200 | Commits 1f69859e and fed532be | A rate, 1 of 100 |
| First uia-facts run, `exit`: 0 of 150 by default and with both settings; 1 of 150, a crash, with `--recovery` and with `--coalescing` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Inconclusive with none, pass with one, against 1 in 100 |
| `exit`, the children one after another, only access violations and hangs counted: 2 of 150, both hangs | 2026-11-06 | As above | 93fed1a0 | Pass |
| `candidates`, 230 children each, a 10 s hang limit: the first build's setup 6 (3.4 faults a minute); no search 6; no registration 8; connect and read only 6; one thread 0; the UIA window alone 11; one provider and one thread 0; connect and read the UIA window alone 8 (5.9 a minute); the same with two providers and threads 0; one provider with six UIA windows and six threads 0; one provider's window with six threads each to it 0; four providers and threads 8 (6.6 a minute); six providers and three threads 3; twelve providers and threads 5 | 2026-11-06 | As above | This change's commit | New; four providers and threads chosen |
| `exit` with four providers and threads: 2 of 150, both hangs, in 22 s | 2026-11-06 | As above | This change's commit | Pass against 8 of 230 |
| `clean`: the providers ended after the teardown 2 of 90, before it 3 of 90, all hangs | 2026-11-06 | As above | This change's commit | Pass: no difference from the dirty rate |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-process-that-used-uia-can-hang-or-crash-as-it-exits.exe`, which runs `exit`; `--case candidates` takes about 20 minutes and `--case clean` about 35 seconds. No desktop or application.

## Evidence kept

None: commits 1f69859e and fed532be.
