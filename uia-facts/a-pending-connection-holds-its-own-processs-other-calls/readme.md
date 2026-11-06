# A pending connection holds its own process's other calls

## Fact

A pending connection, one whose `WM_GETOBJECT` waits in a busy window's message queue, holds the other calls of its own client process until it fails at the connection timeout (`books/dev/src/explanation/windows/uia.md`, "Making a connection"). A process's first connection to the application holds no other process's calls. A connection made by a process already connected to the application holds other processes' calls into the application too, until it fails (new, 2026-11-06). Which releases held the I/O thread in the original measurement is not known.

## What Verbatim relies on

D13's listener making no connections, and `crates/verbatim-uia/src/bounded.rs` and the outpost watchdog abandoning threads. If a stuck application held calls from another process, the isolation would fail. The new answer bears on it: an outpost, connected to its own application, that leaves a new connection pending there while a window's thread is busy holds the listener's reads into that application until the connection fails, for up to one connection timeout. Dickson accepts this as a known cost of D13 (2026-11-06; `books/dev/src/explanation/architecture.md`, D13).

## Method

The provider program has window 0 on UI thread 0 and window 1 on UI thread 1. This program, B, and a second client process, C, each hold window 1's node 1, fetched first. UI thread 0 is made busy until released; a client connects to window 0 with `ElementFromHandle`, a connection left pending, at a 500 ms connection timeout, short since the claim is the hold's relation to it; once the provider logs the connection's message reaching the busy thread, the reads are made. The busy thread is released once the reads have answered, or 5 s after the connection failed. A read is judged held when it answers no sooner than the connection timeout less a tick (15.6 ms) from the connection's start, and answered when it answers within half of it.

- `connected`: B, which holds window 1's node, makes the connection, with `UseComThreading`; B's read and C's read of their node of window 1, on the idle thread.
- `first`: the connection is the first of a third client process, F, which has connected to nothing; C reads meanwhile, as `e30` ran it.
- `connected-nocom`, run only when named: as `connected`, the provider without `UseComThreading`, the reads going to window 0's own node 1, served on UIA's I/O thread.

Judged: the connection's failure (deterministic, `UIA_E_TIMEOUT`) and its time (a band from the 500 ms timeout); B's read held (a relation); C's read beside F's first connection answered, and beside B's connection held, each deterministic, the second against its first run's answer.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| A pending connection held the other calls of its own process until its 2-second timeout (`sta-pending-connection-only`); the I/O thread delivering element releases into the busy thread meanwhile | 2026-11-02 | Build 26200 | the parallel-calls experiment, in git at fdfb63ae (`run-abandon.ps1`), in git at fdfb63ae | A relation |
| `e30`: a pending first connection beside a third client's remote operation and name read, which answered in about 1 ms and 0.3 ms, 12 of 12 | 2026-10-17 | Build 26200 | The collision experiments, in git at fdfb63ae | Answered |
| First uia-facts run, 3 trials each: the connection `UIA_E_TIMEOUT` after 507.1 to 516.6 ms; B's read held until it failed, 3 of 3; beside F's first connection, C answered, 3 of 3; beside B's connection, C held until it failed, 3 of 3, with and without `UseComThreading` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Pass; C beside B's connection new: held |
| Rerun with C's read beside B's connection judged against the first run: held, 1 of 1; beside F's first connection answered | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 7319ed67 | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-pending-connection-holds-its-own-processs-other-calls.exe`, which runs `connected` and `first`; `--case connected-nocom` runs the third. No desktop, application or debugger.

## Debugger guide

Which releases held the I/O thread is not known. Under `--pause`, the program prints nothing of its own yet; attach cdb to the provider program (its process id from `tasklist /fi "imagename eq uia-facts-provider.exe"`) while the connection is pending, about the first 500 ms of a trial at `--case connected`, and take `~*kb`: the I/O thread delivering element releases shows `UIAutomationCore!ReleaseCollection::DispatchReleases` under `combase!MTAThreadWaitForCall`, as the original's stacks at 6 s did. Set a longer connection timeout in the program to give the debugger time.

## Evidence kept

`recorded/abandon.txt` and `recorded/abandon-pending-connection-stacks.txt`: the parallel-calls experiment's abandonment runs, the pending connection's among them.
