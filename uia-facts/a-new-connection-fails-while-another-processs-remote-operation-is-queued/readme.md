# A new connection fails while another process's remote operation is queued

## Fact

While a remote operation from process A waits to be evaluated in an application, process B's new connection by window handle to any window of that application, or its walk up from an element it holds into a window of that application from another process, answers wrongly with no error: `UiaHasServerSideProvider` answers "no" after about 3 seconds, and `ElementFromHandleBuildCache` returns UIA's window proxy after about 2 seconds, in place of the application's own element (`books/dev/src/reference/uia-bugs.md`, the fault of that name). It happens to a client that connected before, to a window on the application's idle UI thread, and when B's connection was sent before A's operation. Not affected: B's calls on elements it holds, which are only delayed, B's walk up within one process, B's own remote operation, B's MSAA calls, the raw probe, and A's own lookups and probes during its own operation. A remote operation waiting for a busy UI thread delays every other client's call into the process, to any of its UI threads, until the thread answers. A second form needs no remote operation: two processes' new connections waiting in the busy application while a third process has a call held there, one of them answering otherwise than with the application's element in some trials. The console host does not show the fault.

## What Verbatim relies on

The rule that only an application's own outpost connects to it or sends it a remote operation (`books/dev/src/explanation/design/listener-without-connections.md`, section 2; the comment in `crates/verbatim-uia-rops/src/operation.rs`). If the fault went away the rule is merely unneeded; if the spared set shrank, Verbatim breaks, since the outpost makes both remote operations and lookups into its own application (A's own lookups), and the listener reads held elements while outposts run operations. The second form is accepted in decision 3 of the same design.

## Method

The provider program P has windows 0 and 2 on UI thread 0 and window 1 on UI thread 1, with `UseComThreading`, and answers MSAA's `OBJID_CLIENT` with its UIA provider (`--bridge`). A is a second copy of the program that holds window 0's element and has set UIA up in P with one remote operation; B is the program itself. A queued operation: P's UI thread 0 busy until released, A's remote operation reading window 0's name sent, and P's log showing a message reaching the busy thread after it was sent. B's call follows; a call that sends the window a message (a lookup, a probe, an MSAA call, the raw probe) is released once P logs that message, any other after 100 ms, a deliberate pause, since nothing marks a request waiting in UIA's channel. Every trial has a new P and a new A. Both timeouts are set in each client: 2,000 ms for connections, 30,000 ms for transactions, so that no held call times out.

The default run checks what Verbatim relies on:

- `collision`: B's `ElementFromHandleBuildCache` and B's `UiaHasServerSideProvider` of window 0, each in a trial of its own, and each with nothing queued, the control. The answer is classified by its provider description, by Verbatim's proxy rule (`verbatim_uia::served_only_by_uia_proxies`).
- `spared`: during the queued operation, each in a trial of its own: B's name read of an element it holds; A's own lookup and probe; B's remote operation, its first into P, reading window 0's name; B's `AccessibleObjectFromWindow` and its name, through UIA's bridge; B's raw probe (`verbatim_uia::raw_probe`); B's raw-view walk up from window 2, made window 0's child, into window 0.

The other cases run only when named, their claims kept here and in the documents:

- `again`: B's probe of window 0 once before the operation, then again during it (`e9`).
- `whole-process`: B's probe of window 1, on the idle UI thread (`e16`).
- `order`: B's lookup sent first and logged at the busy thread, then A's operation (`e31`).
- `walk-up-across`: a second provider process's window made window 0's child; B's walk up from it (`e24`).
- `delay`: B's read of window 1's node, on the idle thread, during the queued operation, and with nothing queued (`e28`, `e28c`).
- `second-form`: 42 trials, a new P each: a third client X, which has set UIA up in P, has its name read held in P's provider; then B and a fourth client D, which has connected to nothing, look window 0 up at once; released 300 ms later, as the original released it. A trial counts when either lookup answered otherwise than with P's own element.
- `conhost`: on a desktop of the program's own, where A and B are clients started there: a console host started directly (`conhost.exe cmd.exe`), fresh in each trial (`e35b`); its window's thread, found among the console host's threads, suspended; A's operation, then 5 ms later B's lookup, then the thread resumed 300 ms later, as `e35` stalled it; and the provider program's UI thread suspended the same way (`e35c`).
- `getfocus`, on the unlocked desktop: window 0 shown and in front; B's `GetFocusedElementBuildCache` during the queued operation (`e20`).

Not tried by any case: walks down into a child window's provider (`e25`, `e26`, `e26b`), since the provider program's windows are hidden and UIA's window proxy lists only visible child windows; the brief window with an idle UI thread (`e21`); the walk down from the desktop root (`e33`); event registrations and connections made once evaluation has begun (`e14`, `e17`); and `e18`'s two connections without an operation. Their claims stay as the documents state them.

Judged: deterministic outcomes (the classification, the probe's answer, the name), relations (a read answered after the release, or before it), and the second form as a rate, 3 of 42. The times are printed for trends.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `e1` to `e36`: the probe "no" after about 3,047 ms; the proxy after about 2,049 ms (`e11`), 2,300 to 2,337 ms after a 300 ms stall (`e33`); every connection and the whole process exposed (`e9`, `e16`, `e28d`); either order (`e31`, 6 of 6); the walk up into another process (`e24`, 6 of 6, about 2,045 ms); the spared set (`e10`, `e12`, `e23`, `e25` to `e27`, `e29`, `e34`); the delay to other clients (`e28`, 9 of 9 to the end of the busy period; `e28c` under 1.1 ms); conhost not exposed (`e35`, `e35b`), the provider stalled the same way exposed (`e35c`, 5 of 5); `GetFocusedElement` 3 of 3 correct (`e20`) | 2026-10-17 and 2026-10-18 | Build 26200 | The collide tool, its patches and outputs, in git in the commit before the one that moved them here | Deterministic and relations |
| The second form: 3 of 42 trials, the failing lookup after 2,306.6 to 2,318.6 ms | 2026-10-29 | Build 26200 | `xproc.rs`'s `second_form` (`experiments/cross-process-calls-study-program/`), its outputs in git in the same commit | A rate |
| Verbatim's exposure before it kept its own calls apart: about 1 in 25 Notepad openings and 3 in 100 Windows Terminal launches read the first focus through MSAA; one outpost probe waited 3,008 ms | 2026-10-16 and 2026-10-17 | Build 26200 | `recorded/notepad/`, `recorded/wt-launch-slow/`, `recorded/runs-wt-section20-traced.txt` | History, no program (Dickson, 2026-11-04) |
| First uia-facts run, default: the lookup the proxy, 2,030.3 and 2,040.1 ms; the probe false, 3,013.4 and 3,033.2 ms; the controls the provider's own element and true; every spared call as recorded | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Pass |
| First uia-facts run, named: `again` true then false; `whole-process` false after 3,004.1 ms; `order` the proxy; `walk-up-across` the proxy after 2,106.2 ms; `delay` held to the release, the control 0.2 ms; `conhost` the console host's own element 3 of 3, the provider program the proxy 3 of 3 | 2026-11-06 | As above | As above | Pass |
| First uia-facts run, `second-form`: 8 of 42, the failing connections after about 2,300 ms | 2026-11-06 | As above | As above | Inconclusive: the intervals of 8 of 42 and 3 of 42 overlap |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-new-connection-fails-while-another-processs-remote-operation-is-queued.exe`, which runs `collision` and `spared` and takes over nothing. The other cases run when named, for example `--case again --case whole-process --case order --case walk-up-across --case delay --case second-form --case conhost`; `conhost` runs on a desktop of its own. `--case getfocus` needs the unlocked desktop and shows the provider's window in front.

## Debugger guide

Why the connection answers wrongly rather than failing is not known (`books/dev/src/explanation/windows/uia.md`, "Not known"). To watch it, run `--case collision --pause`: each trial prints the process ids of P, A and this program and waits for Enter. Attach cdb to this program (B) with Microsoft's public symbols (`.sympath srv*`), and to P. In B, `bp UIAutomationCore!UiaHasServerSideProvider` and, once it is hit, `bp user32!SendMessageTimeoutW` and `bp KERNELBASE!WaitForSingleObjectEx`; `kb` at each shows where the 3 seconds are spent after the window has answered. In P, `~*kb` after the release shows the I/O thread evaluating A's operation. No stacks are recorded yet; the first run under the debugger records them here.

## Evidence kept

`recorded/notepad/`, `recorded/wt-launch-slow/` and `recorded/runs-wt-section20-traced.txt`: the Notepad and Windows Terminal launches that met the fault in Verbatim, kept as the history of Verbatim's exposure that `books/dev/src/reference/uia-bugs.md` cites.
