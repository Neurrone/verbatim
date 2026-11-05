# Parallel client threads gain at most twice on one application

## Fact

For a provider on its UI thread, UIA's I/O thread and the UI thread, working in turn, take about half of each call's time, and the client's threads the rest; so two client threads already keep the provider busy, and parallel client threads gain at most about 2 times on cheap calls and 1.0 to 1.5 times on calls whose cost is in the provider (`books/dev/src/explanation/windows/uia.md`, "Where the provider call runs"). One client object shared by every thread made no difference (`books/dev/src/explanation/windows/uia.md`, "Calls from the client").

## What Verbatim relies on

No figure; D9 gives up parallel queries within one application (`books/dev/src/explanation/architecture.md`, section 1).

## Method

The provider program with one UIA window served on its UI thread (`UseComThreading`). A bench makes K name reads of nodes 1 to 16 on one thread, then the same K split evenly over N threads that start together, N of 2, 4 and 8. The threads are made, with their client objects and elements, before a warm-up run and 7 timed runs, and live through them, so a run times the reads alone; each run starts every thread together and ends when the last is done, and an N's time is the median.

- `cheap`: K = 4,000 plain name reads.
- `costly`: K = 100, every provider call made 5 ms slow while the window keeps pumping (`slow 5`); each 5 ms wait lasts a system timer tick or more.

Each N's bound is the serial-share bound of its own runs: one call's time on one thread divided by the provider's serial time per call under that load, the median interval between provider call entries. Judged: each N's speed-up at or below its bound (a relation). When the provider's time is the whole call, as with the costly calls, the bound is 1.00 and equal to the speed-up within the clock's reach, so the relation is printed, not judged. The speed-ups and the serial shares are printed for trends.

- `processes`: the cheap bench as N threads of this process, each with its own client object, against N client processes of one thread each, both kept alive across the runs and timed the same way, from the signal to start to the last one's end. Printed for trends, run by run.
- `shared`: the cheap 8-thread bench with one client object's elements for every thread against a client object each, printed.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Speed-ups: synthetic provider 1.74 (STA) to 1.98 (agile) on cheap calls, 1.01 with a 5 ms provider call; mockapp 1.31 to 2.35; real applications 1.1 to 1.9; the bound 5 to 14 percent above the speed-up | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Throughput" and "Why several threads speed up cheap calls" (`run-xbench.ps1`, `parts.ps1`, `run-mockapp.ps1`) | At or below the bound |
| First uia-facts run, `cheap`: 1.67, 1.71 and 1.72 at 2, 4 and 8 threads, against bounds of 1.73, 1.79 and 1.79 (a call 0.104 ms; serial shares 0.58, 0.56 and 0.56) | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549 | FIXCOMMIT | Pass |
| First uia-facts run, `costly`: 1.00, 1.00 and 1.04 at 2, 4 and 8 threads, each bound 1.00 (a call 47.5 ms, serial shares 1.00) | 2026-11-05 | As above | As above | For trends: within the fact's 1.0 to 1.5 times |
| First uia-facts run, `processes`: medians, threads 250, 263 and 261 ms against processes 240, 248 and 238 ms at 2, 4 and 8; at 8, threads as fast or faster in 2 of 7 runs | 2026-11-05 | As above | As above | For trends |
| First uia-facts run, `shared`: 252 ms against 240 ms | 2026-11-05 | As above | As above | For trends |

The costly calls' share is 1.00, not about half, because the slow provider call is the whole call. Each 5 ms delay waits while the window pumps, so at 8 threads a call can begin inside another's delay, which fits the speed-up above 1.00 there.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe`, or one arm with `--arm`. No desktop or application; the guide needs cdb. About 4 minutes.

## Debugger guide

1. Run `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe --arm cheap --pause` and press Enter at its pause; the 8-thread runs come last.
2. During an 8-thread run, take the client's stacks non-invasively, five times, a few hundred milliseconds apart: `cdb -pv -p <client pid> -y srv*C:\symbols*https://msdl.microsoft.com/download/symbols -c "~*k 20; qd"`.
3. Expect, of the 8 reading threads' 40 stacks, nearly all (39 of 40 in the original) waiting for their replies, with frames from the top `ntdll!NtWaitForAlertByThreadId`, `ntdll!RtlWaitOnAddress`, `KERNELBASE!WaitOnAddress`, `UIAutomationCore!ChannelBasedClientConnection::WaitForResponse`, `ChannelBasedClientConnection::DoCallWithReturn`; none waiting on a lock. A thread writing its request shows `ReadWriteChannelInfo::BlockingWrite` under `ChannelBasedClientConnection::DoCallWithReturn`; the client's one I/O thread reads every reply in `OverlappedIOManager::IoThreadProc` and wakes the caller through `ChannelBasedClientConnection::SetResponseInfo`.
4. It matches when the waits are on replies, not on each other: the gain is bounded in the provider, not by a lock in the client.

## Evidence kept

None of its own: the original outputs stay in `experiments/parallel-uia-calls/results/` until that folder's last facts move.
