# Parallel client threads gain at most twice on one application

## Fact

For a provider on its UI thread, UIA's I/O thread and the UI thread, working in turn, take about half of each call's time, and the client's threads the rest; so two client threads already keep the provider busy, and parallel client threads gain at most about 2 times on cheap calls and 1.0 to 1.5 times on calls whose cost is in the provider (`books/dev/src/explanation/windows/uia.md`, "Where the provider call runs"). N threads of one client process were as fast as N client processes or faster, 8 processes slower than 8 threads, and one client object shared by every thread made no difference (`books/dev/src/explanation/windows/uia.md`, "Calls from the client").

## What Verbatim relies on

No figure; D9 gives up parallel queries within one application (`books/dev/src/explanation/architecture.md`, section 1).

## Method

The provider program with one UIA window served on its UI thread (`UseComThreading`). A bench makes K name reads of nodes 1 to 16 on one thread, then the same K split evenly over N threads that start together, N of 2, 4 and 8; each N has a warm-up run and 7 timed runs, and its time is the median until the last thread ends.

- `cheap`: K = 4,000 plain name reads.
- `costly`: K = 100, every provider call made 5 ms slow while the window keeps pumping (`slow 5`); each 5 ms wait lasts a system timer tick or more.

Each arm's bound is the serial-share bound of the same run: one call's time on one thread divided by the provider's serial time per call, the median interval between provider call entries in the 8-thread runs. Judged: the best speed-up at or below the bound (a relation). The speed-ups and the serial share are printed for trends.

- `processes`: the cheap bench as N threads of this process, each with its own client object, against N client processes of one thread each. Judged at N = 8 (a relation); the rest printed.
- `shared`: the cheap 8-thread bench with one client object's elements for every thread against a client object each, printed.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Speed-ups: synthetic provider 1.74 (STA) to 1.98 (agile) on cheap calls, 1.01 with a 5 ms provider call; mockapp 1.31 to 2.35; real applications 1.1 to 1.9; the bound 5 to 14 percent above the speed-up | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Throughput" and "Why several threads speed up cheap calls" (`run-xbench.ps1`, `parts.ps1`, `run-mockapp.ps1`) | At or below the bound |
| Threads against processes: N threads as fast or faster; 8 processes slower than 8 threads | 2026-11-02 | Build 26200.9711 | The same, "Threads of one client against client processes" (`results/xbench.txt`) | A relation at N = 8 |
| First uia-facts run, `cheap`: 1.79, 1.82 and 1.78 at 2, 4 and 8 threads; best 1.82 against a bound of 1.84 (a call 0.103 ms, serial 0.056 ms, a share of 0.54) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | The commit that added this folder | Pass |
| First uia-facts run, `costly` (K = 400): 1.00 at every N; bound 1.00 (a call 47.5 ms, serial 47.5 ms, a share of 1.00) | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `processes`: threads 233, 240 and 243 ms against processes 220, 226 and 220 ms at 2, 4 and 8 | 2026-11-05 | As above | As above | Differ, raised below |
| First uia-facts run, `shared`: 240 ms against 240 ms | 2026-11-05 | As above | As above | For trends: no difference |

The costly calls' share is 1.00, not about half, because the slow provider call dominates both the call and the serial time; the fact's "1.0 to 1.5 times" holds.

### Threads against processes, raised

On this run N threads of one process were 6 to 10 percent slower than N client processes, at every N, where the original found them as fast or faster. Not established why. The candidates: every reply to a client process is read by that process's one I/O thread (`books/dev/src/explanation/windows/uia.md`, "Calls from the client"), which N processes have N of; the original's client was a release build and this one a debug build, though UIA's own work is the same in both; and the original's processes were timed by their own clocks. The claim stays the explainer's until a rerun separates these, which is the follow-up: rerun `--arm processes` with a release build (`cargo build --release` in `uia-facts/`).

## How to rerun

`uia-facts/build.ps1`, then `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe`, or one arm with `--arm`. No desktop or application; the guide needs cdb. About 4 minutes.

## Debugger guide

1. Run `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe --arm cheap --pause` and press Enter at its pause; the 8-thread runs come last.
2. During an 8-thread run, take the client's stacks non-invasively, five times, a few hundred milliseconds apart: `cdb -pv -p <client pid> -y srv*C:\symbols*https://msdl.microsoft.com/download/symbols -c "~*k 20; qd"`.
3. Expect, of the 8 reading threads' 40 stacks, nearly all (39 of 40 in the original) waiting for their replies, with frames from the top `ntdll!NtWaitForAlertByThreadId`, `ntdll!RtlWaitOnAddress`, `KERNELBASE!WaitOnAddress`, `UIAutomationCore!ChannelBasedClientConnection::WaitForResponse`, `ChannelBasedClientConnection::DoCallWithReturn`; none waiting on a lock. A thread writing its request shows `ReadWriteChannelInfo::BlockingWrite` under `ChannelBasedClientConnection::DoCallWithReturn`; the client's one I/O thread reads every reply in `OverlappedIOManager::IoThreadProc` and wakes the caller through `ChannelBasedClientConnection::SetResponseInfo`.
4. It matches when the waits are on replies, not on each other: the gain is bounded in the provider, not by a lock in the client.

## Evidence kept

None of its own: the original outputs stay in `experiments/parallel-uia-calls/results/` until that folder's last facts move.
