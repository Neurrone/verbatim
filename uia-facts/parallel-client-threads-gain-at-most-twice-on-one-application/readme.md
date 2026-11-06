# Parallel client threads gain at most twice on one application

## Fact

For a provider on its UI thread, UIA's I/O thread and the UI thread, working in turn, take about half of each call's time, and the client's threads the rest; so two client threads already keep the provider busy, and parallel client threads gain at most about 2 times on cheap calls and 1.0 to 1.5 times on calls whose cost is in the provider (`books/dev/src/explanation/windows/uia.md`, "Where the provider call runs"). One client object shared by every thread made no difference (`books/dev/src/explanation/windows/uia.md`, "Calls from the client").

## What Verbatim relies on

No figure; D9 gives up parallel queries within one application (`books/dev/src/explanation/architecture.md`, section 1).

## Method

The provider program with one UIA window served on its UI thread (`UseComThreading`). A bench makes K name reads of nodes 1 to 16 on one thread, then the same K split evenly over N threads that start together. The threads are made, with their client objects and elements, before a warm-up run and 3 timed runs, and live through them, so a run times the reads alone; each run starts every thread together and ends when the last is done, and an N's time is the median.

- `gains`: first cheap calls, K = 1,000 plain name reads, at N of 2 and 8; then costly ones, K = 16, each read's `GetPropertyValue` made 5 ms slow while the window keeps pumping (`slow 5 GetPropertyValue`), at N of 2 and 4; each 5 ms wait lasts a system timer tick or more, and the read's other provider calls stay cheap. A progress line names each bench as it starts.

Each N's bound is the serial-share bound of its own runs: one call's time on one thread divided by the provider's serial time per call under that load, the time from entry to exit of every provider call per read, by the provider's log, during which no other call into the provider runs. The provider's log sees only the provider's own calls and cannot see UIA's I/O thread, so for cheap calls, whose time is mostly that thread's, the bound is far above the speed-up; for costly calls, whose time is the provider's, it measures the right thing, within 0.01 of the speed-up. Both are printed for trends, since a judged claim with no margin would differ from noise alone. Judged: each N's speed-up against the fact's own figures, at most 2 times for cheap calls and at most 1.5 for costly ones, whose speed-up at N of 2 could not pass 2 in any case (a relation). The speed-ups, the bounds and the serial shares are printed for trends.

- `shared`: the cheap 8-thread bench, K = 1,000, with one client object's elements for every thread against a client object each, printed.

The `processes` case, threads of one process against client processes, printed only for trends, is no longer run; its first run stays in the records below.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-05 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Speed-ups: synthetic provider 1.74 (STA) to 1.98 (agile) on cheap calls, 1.01 with a 5 ms provider call; mockapp 1.31 to 2.35; real applications 1.1 to 1.9; the bound 5 to 14 percent above the speed-up | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "Throughput" and "Why several threads speed up cheap calls" (`run-xbench.ps1`, `parts.ps1`, `run-mockapp.ps1`) | At most 2 times on cheap calls and 1.5 on costly ones |
| First uia-facts run, `gains`, cheap calls: 1.67, 1.71 and 1.72 at 2, 4 and 8 threads (a call 0.104 ms); its bounds and shares came from the gap between call entries, which equals the speed-up by construction, so they are not kept | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 5ad6b886 | At most 2 times: pass |
| First uia-facts run, `gains`, costly calls: 1.00, 1.00 and 1.04 at 2, 4 and 8 threads (a call 47.5 ms) | 2026-11-05 | As above | As above | At most 1.5: pass |
| uia-facts run with the bound from the provider's busy time, `gains`: cheap calls 1.78 and 1.75 at 2 and 8 threads (bounds 23.4 and 21.2, serial shares 0.04 and 0.05; a call 0.101 ms); costly calls 1.00 at 2 and 4 threads (bounds 1.01, serial shares 0.99; a call 47.5 ms) | 2026-11-06 | As above | The commit that adds this row | At most 2 and 1.5: pass; the bounds for trends |
| First uia-facts run, `processes`: medians, threads 250, 263 and 261 ms against processes 240, 248 and 238 ms at 2, 4 and 8; at 8, threads as fast or faster in 2 of 7 runs | 2026-11-05 | As above | As above | For trends |
| First uia-facts run, `shared`: 252 ms against 240 ms | 2026-11-05 | As above | As above | For trends |
| uia-facts run of the cut program, every setting: cheap calls 1.72 and 1.78 at 2 and 8 threads (a call 0.108 ms), costly calls 1.00 at 2 and 4 threads (a call 15.8 ms, `GetPropertyValue` alone slowed); `shared` 60 ms against 63 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | ff23f81a | At most 2 and 1.5: pass; the rest for trends |

The costly calls' share is about 1.00, not about half, because the slow provider call is the whole call. The cheap calls' share is far below half because the provider's log sees only the provider's own calls; the half the fact states includes UIA's I/O thread, which the log cannot see.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe`, or one case with `--case`. No desktop or application; the guide needs cdb.

## Debugger guide

1. Run `target\debug\parallel-client-threads-gain-at-most-twice-on-one-application.exe --case gains --pause` and press Enter at its pause; the cheap calls' 8-thread runs come before the costly calls.
2. During an 8-thread run, take the client's stacks non-invasively, five times, a few hundred milliseconds apart: `cdb -pv -p <client pid> -y srv*C:\symbols*https://msdl.microsoft.com/download/symbols -c "~*k 20; qd"`.
3. Expect, of the 8 reading threads' 40 stacks, nearly all (39 of 40 in the original) waiting for their replies, with frames from the top `ntdll!NtWaitForAlertByThreadId`, `ntdll!RtlWaitOnAddress`, `KERNELBASE!WaitOnAddress`, `UIAutomationCore!ChannelBasedClientConnection::WaitForResponse`, `ChannelBasedClientConnection::DoCallWithReturn`; none waiting on a lock. A thread writing its request shows `ReadWriteChannelInfo::BlockingWrite` under `ChannelBasedClientConnection::DoCallWithReturn`; the client's one I/O thread reads every reply in `OverlappedIOManager::IoThreadProc` and wakes the caller through `ChannelBasedClientConnection::SetResponseInfo`.
4. It matches when the waits are on replies, not on each other: the gain is bounded in the provider, not by a lock in the client.

## Evidence kept

`recorded/`: the parallel-calls experiment's throughput outputs (`xbench.txt`, `parts.txt`, `parts-traced.txt`, `mockapp-bench.txt` and `mockapp-bench-shared-client.txt`).
