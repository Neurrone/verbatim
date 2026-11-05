# A provider process serves one UIA request at a time

## Fact

A provider process's UIA has one I/O thread, which takes one request, makes the provider call, writes the response, and only then takes the next, first in, first out across every window, UI thread and client process (`books/dev/src/explanation/windows/uia.md`, "Inside the provider process" and "Where the provider call runs"). So provider work never overlaps; a slow call holds every call into the process from any client, thread or window, remote operations included, and no other process's call; a request into an idle UI thread waits behind an earlier one into a busy thread; a request sent 950 ms earlier waits until the response before it. With `UseComThreading`, a busy UI thread with nothing queued on it holds nothing (0.4 to 1.2 ms into the other UI thread), and a provider waiting in `CoWaitForMultipleHandles` holds the others as a sleeping one does. Without it, reads answer while the UI thread is busy, a new connection to the busy window fails at the connection timeout, and an `Invoke` waits for the UI thread. Whether connection recovery changes any of these is not known.

## What Verbatim relies on

D9's per-application outposts and its giving up of parallel queries within one application (`books/dev/src/explanation/architecture.md`, section 1). If UIA became parallel, nothing breaks; if a stuck application held calls into another, the isolation would fail.

## Method

The provider program has windows 0 and 1 on UI thread 0 and window 2 on UI thread 1, with `UseComThreading` unless an arm says otherwise; the client process's timeouts are 2,000 and 30,000 ms. Every element is fetched before a hold or a busy thread. A hold lasts 1,000 ms, a deliberate length; the order arm sends its second read 200 ms after the first, since nothing marks a request queued on a busy thread.

- `no-overlap`: 8 client threads make 50 name reads each across the windows; the provider's peak of calls in progress at once, in both threading modes. 3 runs each.
- `held`: window 0's node 1 read held, while probes run: node 2 of window 0 from another thread, window 2's node 1, a remote operation on window 0's node 3, a read from a second client process into window 1, and a read into a second provider process; and the provider's log shows when the waiting read started. 3 trials.
- `fifo`: UI thread 0 busy; a read queued into window 0, then a read into window 2; the provider's order of serving them, and whether the second waited for the release. 3 trials.
- `busy-only`: UI thread 0 busy with nothing queued; a read into window 2. 3 trials.
- `pumping`: window 0's node 1 read waiting in `CoWaitForMultipleHandles` until released; a read into window 2. 3 trials.
- `nocom`: the provider without `UseComThreading`, two windows on two UI threads, UI thread 0 busy: a read of window 0's node 1, a new connection to window 0, then its node 1's `Invoke`. 3 trials.
- `recovery`: `held`, `fifo`, `busy-only`, `pumping` and `nocom` again with connection recovery on in every client object, printed as new.

Judged: relations (each probe's reply before or after the release, by this program's clock; the waiting read's start after the held call's end, by the provider's log), orders and counts (deterministic), and the new connection's wait (a band).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| At most one call in the provider, every mode; a slow call holding every probe into the process from any thread, window or process, remote operations included; FIFO; the 950 ms request; 0.4 to 1.2 ms with nothing queued; the pumping wait holding; without `UseComThreading` reads answer, a new connection times out, `Invoke` waits | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/` (`serve.rs` with `run-serve.ps1`, `run-serve-xproc.ps1`, `iothread-sequence.ps1`, `run-pairs.ps1`) and the collision experiments' `e16` and `e28c` | Deterministic and relations |
| First uia-facts run, `no-overlap`: peak 1 in 3 of 3, with and without `UseComThreading` | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 1682f8cb | Pass |
| First uia-facts run, `held`: every probe into the process waited for the release, the other process's read answered during the hold, and the waiting read started only after the held call returned, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `fifo`: window 0's request served before window 2's, and window 2's read waited for the release, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `busy-only`: answered 3 of 3, 0.2 ms | 2026-11-05 | As above | As above | Pass; the time for trends |
| First uia-facts run, `pumping`: the read into window 2 waited for the release, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `nocom`: the read answered, the new connection failed `UIA_E_TIMEOUT` after 2,014 ms (2,012 to 2,017), the `Invoke` waited for the UI thread, 3 of 3 | 2026-11-05 | As above | As above | Pass |
| First uia-facts run, `recovery`: every outcome as without recovery; the new connection 2,013 ms (2,011 to 2,014) | 2026-11-05 | As above | As above | New: connection recovery changes none of these outcomes |

The 950 ms request is not a separate arm: `held`'s probes are sent during a 1,000 ms hold and answered only after it, by the log after the held call's end.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-provider-process-serves-one-uia-request-at-a-time.exe`, or one arm with `--arm`. No desktop, application or debugger; it starts a second copy of itself as the other client process. About 1 minute.

## Evidence kept

None of its own: the original outputs stay in `experiments/parallel-uia-calls/results/` until that folder's last facts move.
