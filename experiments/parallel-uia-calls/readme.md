# Experiment: concurrent UIA calls into one application are served one at a time

Question: when a client sends several UI Automation calls (and MSAA calls) into one application at the same time from different client threads or client processes, does the application serve them in parallel or one at a time; does parallelising queries to one application gain anything; and when one call is slow or stuck, do other calls into the same application (other elements, other windows, other UI threads, other client processes) wait? Each question was asked with ordinary UIA calls, with UIA remote operations, and with the two mixed.

Run on 2026-11-02 on Dickson's machine: a QEMU virtual machine (Q35), AMD Ryzen 7 5700G, 12 logical processors, Windows 11 25H2 build 26200.9711, x64, on the interactive console desktop with no other UIA client running (NVDA and Verbatim closed). Repository at c71e8253. The program and mockapp were release builds, except mockapp's debug build for the debugger captures, for its symbols. Edge 156.0.4314.58, Firefox 159.0, Windows Notepad 11.2607.14.0, VS Code 1.141.1.

## Answer

- An application serves UIA requests one at a time, across all its UI threads and for all client processes together. The provider work of two calls never overlapped in any target. Parallel client threads gain at most about 1.3 to 2 times, and only for very cheap calls: the overlap is the client's and the transport's own time, not the application's. For calls whose cost is in the provider (a 5 ms provider call, a large FindAll, a text read), the gain is 1.0 to 1.5 times.
- When one call is in the application, a second call into that application (another element, another window, another UI thread of it, a remote operation, from another thread or another client process) waits until the first call's provider work ends. A call made of many requests lets other calls in between them. A single long call, such as one remote operation or one GetText of a large document, does not.
- A FindAll with a cache request is many requests. The client sends one Find request, and the application's UIA walks the whole subtree inside it, in one cross-apartment call that runs the walk on the UI thread. Then the client sends one request per found element for its cached properties, and releases. On mockapp's 24-node tree that was 81 requests, and 315 on its 102-node tree. A name read and a remote operation are one request each. A cheap call from another thread gets in between these requests, and its longest wait is the Find request.
- The MSAA calls that slowed with more client threads, in Notepad and Character Map, never reach the application as COM calls. Their `OBJID_CLIENT` object is oleacc's own proxy in the client, which reads the name by sending `WM_GETTEXTLENGTH` and `WM_GETTEXT` to the window. Plain cross-thread `SendMessage` slows down in the same way with more sending threads, even when each sending thread has a receiving UI thread of its own. So the contention is not in the application, oleacc or UIA; by inference, it is in the window manager, which every sent message passes through. In VS Code and mockapp's MSAA backend, which sped up, the object is the application's own `IAccessible`, and the calls are COM calls.
- An MSAA call does not wait for UIA's I/O thread. While a UIA call is stuck on that thread and the UI thread is free, MSAA calls into the same process answer in 1.2 to 2.6 ms, through oleacc's window proxy and through UIA's own MSAA bridge alike, while UIA calls into the process's other windows wait. An MSAA call into a busy UI thread waits, as a UIA call does. While one UI thread is stuck with a UIA call queued on it, an MSAA call into the process's other UI thread answers, but a UIA call into that thread waits.
- When the application's UI thread is stuck with a call queued on it, every other call into that process waits. A new connection (ElementFromHandle) to any of its windows fails with `UIA_E_TIMEOUT` after the 2-second connection timeout, even a window on an idle UI thread. Calls left waiting fail with `UIA_E_TIMEOUT` after about 22 seconds with UIA's default 20-second transaction timeout, because the client waits the timeout less 200 ms in steps that each last about 110 ms, then 205 ms more ("Timeouts" below). A call into another process is not affected.
- The cause, seen in the debugger in Notepad and in the shell's explorer.exe: the application's `UIAutomationCore` has one I/O thread (`OverlappedIOManager::IoThreadProc`). It reads every incoming request and makes the provider call itself, synchronously. For a provider in an STA, that is a cross-apartment COM call into the UI thread, so the reading thread waits on that UI thread and reads no other request.
- Remote operations are supported in every application measured, and they queue in the same way.
- A provider process has exactly one I/O thread. The first `UiaReturnRawElementProvider` the application makes, on its UI thread inside the first client's `WM_GETOBJECT`, creates it; no thread exists before. 8 client threads in one process, 5 client processes, 2 UI threads and 8 windows did not add a second. It takes one request, makes the provider call, writes the response, and only then takes the next, and a client process reads its replies on one I/O thread of its own.
- Several client threads speed up cheap calls because part of each call happens outside the provider process's I/O thread: the client's own work, and its own I/O thread reading the reply. The provider process serves one call at a time on its I/O thread, and for an STA provider also on its UI thread, the two in turn. That serial time per call, measured as the interval between provider entries under load, predicts the speed-ups, 5 to 14 percent high. For an STA provider it is about half of a whole call, so 2 threads keep it busy and the speed-up stops near 2.
- Windows Terminal's text provider runs on its XAML UI thread, not on UIA's I/O thread, so with that thread suspended its text calls wait, unlike the synthetic `nocom` mode. All its windows share one UI thread.
- AccessKit's Windows adapter is served on UIA's I/O thread, off the UI thread, one call at a time. A busy UI thread holds none of its reads, but Invoke and new connections still need the UI thread, and a new connection to the busy window held the client's other calls until it timed out. A slow action handler holds every call into the process.
- Most client operations are one to a few requests, and every element the client releases is one more request, sent by the releasing thread at once; the provider process batches releases, the client does not. A search or a subtree cache is one Find or BulkFetch request inside which the provider's UIA walks the tree. A cached property read, and a repeated `GetRuntimeId` or `CompareElements`, send none ("Requests per client operation" below).
- An event travels from the provider process to the client on the same pipe as the replies, written by the thread that raised it, the provider's UI thread here. The client's I/O thread reads it, and the handler runs on a UIA worker thread (`WorkItemQueue`), which first fetches the cache request's properties with requests of its own.
- The channel is a named pipe, `\\.\PIPE\UIA_PIPE_<provider pid>_<hex>`, which the client creates and the provider's I/O thread opens. Requests, replies, releases and events all pass on it.
- A client call's transaction timeout ends only the client's wait. The provider process's I/O thread stays in the provider call until it returns, then delivers the requests of calls that have already failed.
- A search into a held or stalled window is a few separate waits, each on its own timeout, not one timeout per provider call. E-TT's 13.2 seconds under a 1-second transaction timeout was a held `GetPropertyValues` request (1.1 s), two window messages from the client's own proxies of the window (1.0 s, then 10 s, the connection timeout in force when the window's element was fetched), and the `Find` request queued behind the held call (1.1 s). The client re-sent nothing. A search whose first request is the root's `GetRuntimeId` fails after that one timeout ("A search's timeouts" below).
- N threads of one client process were as fast as N client processes, or faster. No calling thread waited on another in the client, and 8 client processes were slower than 8 threads.
- Abandoning a stuck call and making later calls on a new thread does not get answers sooner. Every later call into the stuck process waits for the release, or fails after its own 22-second timeout, on any thread. The first answer after the release comes within 2 ms in both forms; the queued requests here were cheap name reads, and a backlog of costly ones would delay it more (inferred, not measured). What differs is that a single thread does not start its later calls until the stuck one fails. A pending new connection held the other calls of its own client process until its 2-second timeout.

## Already established by earlier experiments

- Calls into two different applications are independent: E-concurrency (the mockapp experiments of 2026-10-10, in git at commit b0a1989; focus pipeline section 11). This was confirmed here: two File Explorer folder windows that turned out to be in two explorer.exe processes did not delay each other (1 to 10 ms, `results/explorer.txt`).
- A remote operation queued behind a busy UI thread delays every client's calls into the process, on any of its UI threads, held elements included (`e28`). With the busy thread and no call queued on it, calls into the process's idle thread answered within 1.1 ms (`e28c`). A pending connection, which is a window message that has not reached UIA's channel yet, holds no other call (`e30`). These are in `experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/`. Calls on held elements into the busy thread are delayed to its end (`e10` there).
- A UIA client call waits in UIA's own channel, not in a COM call (`uia-facts/nothing-cancels-a-uia-call/`). Both timeouts apply to the whole client process (`uia-facts/the-connection-timeout-applies-to-the-whole-process/`).

New here:
- Throughput figures for concurrent calls.
- The plain-call form of `e28`: any call waiting in the application, not only a remote operation, holds every other call, from the same client process or another.
- A slow provider call compared with a busy UI thread.
- Failure of a new connection to an idle thread's window while a call is queued.
- Real applications across frameworks.
- The mechanism, from stacks.
- The requests one FindAll, name read and remote operation make, counted on both sides.
- The path MSAA calls take, why some slowed with more threads, and MSAA calls beside a stuck UIA call.
- What creates the I/O thread, how many there are, and the order in which it takes requests.
- One call split into its parts on one clock, and the serial share that predicts the speed-ups.
- Windows Terminal and AccessKit.
- Later calls beside a stuck call, on its thread and on new threads.
- The requests and provider calls of each client operation, events and releases included.
- How the transaction timeout is waited, and that it does not reach the provider.
- The channel's kind and name.
- Threads of one client process against client processes.

## Method

`src/main.rs` and `src/serve.rs` are one standalone program, `puia`. It is built outside the workspace, as the other experiments' prototypes are: copy `Cargo.toml.txt` to `Cargo.toml` in a scratch folder, make both of its paths absolute, and run `cargo build --release`. It depends on the workspace's `verbatim-uia-rops` by path, for remote operations. Every client thread is in the MTA and has its own `CUIAutomation8`. One shared client was also measured, on mockapp: it made no difference (`results/mockapp-bench-shared-client.txt`). The elements held for the reads are 16 descendants spread over the window's subtree. Elements under a title bar are left out, because UIA's own proxies answer those in the client without calling the application.

The calls:
- `name`: `CurrentName` of a held element.
- `findall`: `FindAllBuildCache` over the subtree, caching name and control type.
- `fromhandle`: `ElementFromHandle`, then `CurrentProcessId`.
- `text`: the text pattern's `DocumentRange().GetText(200)`.
- `textall`: the text pattern's `GetText(-1)`.
- `msaa`: `accName` and `accChildCount` on `AccessibleObjectFromWindow(OBJID_CLIENT)`.
- `rop`: a remote operation that reads the name.
- `ropslow`: one remote operation that reads the bounding rectangle 1,500 times. This is one round trip whose provider work is long, and it stays inside UIA's 10,000-instruction limit.
- `msaaname` and `msaacount`: the two halves of `msaa`, alone.
- `msaanew`: `AccessibleObjectFromWindow(OBJID_CLIENT)` made anew, then `accName`.
- `wmtext` and `wmnull`: `SendMessageW` of `WM_GETTEXT` and of `WM_NULL` to the window, with no oleacc.

The subcommands:
- `bench`: K calls on one thread, compared with the same K split evenly over N threads that start together, for N of 1, 2, 4 and 8. Each N has its own pool of worker threads, made before timing. Each N gets one warm-up run, then 20 timed runs (15 for the synthetic provider). The result is the median and 90th percentile of the time until the last thread finishes. Speed-up is the median at N = 1 divided by the median at N. With `hwnds=<list>`, worker thread w calls into the list's window w instead of the one window.
- `pair`: call A, 10 times on one thread, while call B runs back to back, 2 ms apart, from another thread or from another client process (`xb=1`). B targets another element of the same window, so B is served by the same UI thread. A alone and B alone are measured for comparison.
- `hung`: blocks the target, then starts a stuck call into it. 200 ms later, probes start together on threads of their own and in client processes of their own. Each probe's client and elements are made before the block. The target is blocked in one of these ways:
  - `SuspendThread` on the window's UI thread, for 4 seconds, as `e35c` did.
  - mockapp's `hold` or `stall`.
  - The synthetic provider's busy UI thread, or its slow name read.
- `serve`: the synthetic provider process, modelled on the collide tool's server. It has three top-level windows: two on one UI thread and one on a second UI thread. Each answers `UiaRootObjectId` with a server-side provider made on its window's thread, which is an STA. There are three threading modes:
  - `sta`: `UseComThreading` with a non-agile object.
  - `agile`: `UseComThreading` with an agile object.
  - `nocom`: no `UseComThreading`.

  With `msaa=bridge`, it also answers `OBJID_CLIENT` with `UiaReturnRawElementProvider`, so an MSAA client gets UIA's MSAA bridge over COM instead of oleacc's window proxy.

  Its name read counts how many calls are in it at once, and how many arrive off the window's own thread. It can be made to sleep in the name read, once or every time, or to wait in `CoWaitForMultipleHandles` while dispatching incoming calls ("pump"). Its UI thread can be made busy without pumping. It logs each name read's `QueryPerformanceCounter` values at entry and exit, with its thread, for `parts`.
- `threads pid=<pid>`: every thread of a process, with its start address as module and offset and the processor cycles it has used. UIA's I/O thread starts at `uiautomationcore.dll+0x162f60`, `OverlappedIOManager::StaticIoThreadProc`, on this build.
- `parts`: one call split into its parts. N client threads make K calls between them, each call timed on `QueryPerformanceCounter`, which every process shares. Around each run it reads the cycles of every thread of the target and of the client's own UIA threads, converted to time with the client thread's measured cycles per microsecond. With the synthetic provider it also reads the provider-call log, so it gives the time before and after the provider call and, under load, the interval between provider entries: the serial time per call. With `isolated`, it adds single calls 20 ms apart with their UTC times, to find in a context-switch trace.
- `abandon`: a stuck call, then later calls at set times after the block. With `mode=same`, every call is made on the stuck call's thread, each once the one before has returned and not before its time. With `mode=new`, the stuck call is left on its thread and each later call is made at its time on a new thread. One client object and all elements are made before the block, so only the thread differs.

- `ops hwnd=<h> target=mock|serve` (`src/ops.rs`): one client operation at a time, named on its standard input, each marked for a debugger with `OutputDebugString` and, against mockapp, followed by mockapp's provider-call counters (`crates/mockapp/src/hits.rs`, read with window messages). `async <tag> <op>` runs one on a new thread; `transaction <ms>` and `connection <ms>` set UIA's timeouts, and `recovery` and `coalesce` the client's connection recovery and event coalescing.
- `ett hwnd=<h> trans=<ms> conn=<ms> conn0=<ms> conn1=<ms> recovery=0|1 op=ett_find|stall_find` (`src/ops.rs`): one search on a new thread with a new client made as E-TT's was (`held_transaction` in `crates/mockapp/tests/experiments.rs` at b0a19893): connection timeout `conn0` (10 s), connection recovery and event coalescing on, transaction timeout `trans`, the connection timeout `conn1` when given, then the window's element fetched with `ElementFromHandleBuildCache`. On `go` it sets the connection timeout to `conn`, as E-TT's `within(1 s)` did, and searches: `ett_find` is E-TT's `FindFirstBuildCache` over the descendants for the name "First", `stall_find` the `FindFirst` for "Second" of the test removed in commit 31d462fc.
- `xbench` and `worker` (`src/xproc.rs`): N callers as N threads of one client process or as N client processes of one thread each, timed the same way, with each worker's UIA I/O thread's cycles.

The synthetic provider also has an invoke pattern, whose `Invoke` raises `Invoke_Invoked` as a button does. `ops.json` is mockapp's `tree.json` with a text node, `doc`, and automation ids on the nodes the operations use.

The call `invoke` is the invoke pattern's `Invoke` on a held element. `PUIA_TEXT_CLASS` names the class of the element whose text pattern `text` and `textall` use, where the first element with one is not the one wanted: in Windows Terminal it is `TermControl`, since the tab's title, a `TextBlock`, comes first.

`akapp/` is a minimal AccessKit provider, built outside the workspace the same way (copy `akapp/Cargo.toml.txt` to `Cargo.toml` beside a copy of `akapp/src`): `accesskit_windows` 0.35.2 with AccessKit 0.25.2, one hidden window per UI thread, each a root and 8 buttons. Invoking button 1 runs the application's action handler, which can be made to sleep, and which counts how many actions are in it at once and how many run off the window's thread. Its UI thread can be made busy as the synthetic provider's can.

The run scripts:
- `run-serve.ps1` and `run-serve-xproc.ps1`: the synthetic provider.
- `run-mockapp.ps1`: mockapp.
- `run-real.ps1`: one application. Throughput, FindAll beside a cheap call, and the suspended arms.
- `run-pairs.ps1`: the slow-A matrix.
- `stacks.ps1`: the debugger capture.
- `wpf-window.ps1` and `winforms-window.ps1`: the WPF and Windows Forms targets.

The debugger scripts attach cdb invasively, with breakpoints that print a marker, and sometimes a stack, and go on; `docs/tooling/debugging.md` explains the technique. They read `PUIA_SCRATCH`, the scratch folder holding `build\target\release\puia.exe` and mockapp's debug build in `mtarget\debug`, and write their logs there.
- `dbg.psm1`: attaching, breaking in with `DebugBreakProcess` and detaching, and a probe process to drive.
- `requests.ps1`: one call's requests on each side, client and mockapp, for `findall`, `name` and `rop`.
- `msaa-path.ps1` and `msaa-path-mockapp.ps1`: the path of one `msaa` call, with the client's outgoing calls and the serving thread's stack.
- `msaa-while-stuck.ps1`: MSAA calls while a UIA call is stuck in the synthetic provider.
- `serving-thread.ps1`: which thread of an application serves a UIA call.
- `wt-serving-thread.ps1`: the same for Windows Terminal's text and name reads.
- `iothread-count.ps1`: what creates the I/O thread, and how many there are.
- `iothread-sequence.ps1`: the order in which the I/O thread takes requests.
- `parts.ps1`: `parts` on the synthetic provider in each mode.
- `run-akapp.ps1`: the AccessKit provider.
- `run-abandon.ps1`: the abandonment arms.
- `ops.ps1` and `ops-count.py`: each operation's requests on both sides and its provider calls; the counter splits the target's log at each operation's counter reset and the client's at its marker.
- `transport.ps1`: the channel's handles, with cdb on both processes before the first connection.
- `timeouts.ps1`: a stuck call and one queued behind it under a set timeout, with mockapp's counters before the hold and after the release.
- `search-timeouts.ps1`: one `ett` search into mockapp (`small.json`) under `hold` or `stall`, with cdb on mockapp or on the client when asked. `blocked-summary.py` prints one line per wait of a `cargo xtask trace waits --blocked` report, with the UIA, oleacc and mockapp frames it waited on and was readied from.
- `run-xbench.ps1` and `client-stacks.ps1`: threads against processes, and the stacks of one loaded client process.

Every raw line is in `results/`, with the scratch folder's path redacted.

## Results by framework

The targets, and what each stands for:
- Synthetic: a raw UIA provider (`serve`).
- mockapp: an STA UIA provider, and on its MSAA backend an STA `IAccessible`.
- Win32 classic dialog: Character Map.
- WinUI 3 XAML: Windows Notepad, whose stacks show `Microsoft.UI.Xaml`.
- UWP XAML in a CoreWindow: Settings.
- DirectUI with XAML islands: File Explorer.
- Chromium: Edge.
- Gecko: Firefox.
- Electron: VS Code, started with a user data folder and an extensions folder of its own in the scratch folder, so it was a separate process tree, with a 400-line file open.
- WPF, .NET Framework 4.8: a window made from Windows PowerShell 5.1.
- Windows Forms, .NET Framework 4.8: a form made the same way.
- Windows Terminal 1.24.12741.0, a XAML island: the end-to-end harness's portable copy (`crates/verbatim-e2e/src/windows_terminal.rs`) copied to the scratch folder, so it ran as a process of its own, with its settings deleted, a shell that had printed 3,000 lines, and a second window. Its text pattern is its terminal control's (`TermControlUiaProvider` under system XAML's `TermControlAutomationPeer`).
- AccessKit: `akapp`.

The two script-made windows use the frameworks' own providers. Only the windows' contents are scripted.

### Throughput: best speed-up over N = 2, 4 and 8

The speed-ups for each target, by call:

- Synthetic, `sta`: `name` with no delay, 1.74 times. A 5 ms sleep in the provider, 1.01. A 5 ms pump, 1.00.
- Synthetic, `agile`: 1.98, 1.01 and 1.00.
- Synthetic, `nocom`: 4.92, 1.02 and 1.00.
- mockapp: `name` 1.85, `findall` 1.31, `fromhandle` 1.21, `text` 1.88, `msaa` 2.35, `rop` 1.52.
- Notepad: `name` 1.63, `findall` 1.23, `fromhandle` 1.28, `text` 1.83, `msaa` 0.66, `rop` 1.31.
- File Explorer: `name` 1.93, `findall` 1.28, `fromhandle` 1.20, `text` 1.69, `msaa` 0.71, `rop` 1.32.
- Settings: `name` 1.77, `findall` 1.40, `fromhandle` 1.48, `text` 1.55, `msaa` 1.13, `rop` 1.28.
- Character Map: `name` 1.45, `findall` 1.21, `fromhandle` 1.10, `text` 1.81, `msaa` 0.52, `rop` 1.19.
- Edge: `name` 1.66, `findall` 1.49, `fromhandle` 1.28, `text` 1.27, `msaa` 1.92, `rop` 1.46.
- Firefox: `name` 1.64, `fromhandle` 1.18, `text` 1.54, `msaa` 1.88, `rop` 1.43. `findall` was not measured.
- VS Code: `name` 1.66, `findall` 1.48, `fromhandle` 1.26, `text` 1.15, `msaa` 1.84, `rop` 1.52. `name` and `rop` are from a second run a minute after the first, whose N = 1 runs met elements that went stale while the window was new.
- WPF: `name` 1.42, `findall` 1.11, `fromhandle` 1.26, `text` 1.14, `msaa` 1.52, `rop` 1.16.
- Windows Forms: `name` 1.24, `findall` 1.14, `fromhandle` 1.19, `msaa` 1.64, `rop` 1.41. It has no text pattern.
- Windows Terminal: `name` 1.70, `findall` 1.16, `fromhandle` 1.27, `text` 1.51, `msaa` 0.60, `rop` 1.24.
- AccessKit: `name` 6.28, `invoke` 1.79, `fromhandle` 1.37, `rop` 1.78.

Notes on these figures:
- The speed-up did not grow past N = 2 in any real application with an STA provider. AccessKit's name read, served off the UI thread, grew to 6.28 at N = 8, as the synthetic `nocom` mode did.
- The synthetic provider's counters showed at most one call inside the provider at any time, in every mode and at every N. They showed 0 of 480 calls off the UI thread in `sta` and `agile` mode. In `nocom` mode, all 480 calls ran off the UI thread, but still one at a time.
- MSAA in Notepad, Explorer and Character Map was slower with more threads (0.52 to 0.71 times). "Why MSAA slowed with more threads" below has the cause.
- Per-call costs and 90th percentiles are in each target's `results/<target>.txt`.

### Slow call A and cheap call B on the same UI thread

This is the median of B while A runs, against B alone; full figures are in `results/*-pairs.txt`. Every row held for B from another thread and from another process (`xb0` and `xb1`), and for B as a name read and as a remote operation.

When A is one long call, B waits for A to end. B's median while A runs is close to A's remaining time:

- Edge, A one GetText of a 20,000-paragraph page, 222 to 225 ms: B 220 to 222 ms, against 0.15 to 0.34 ms alone.
- Notepad, A one GetText of a 4.3 MB file, 50 to 52 ms: B 48 to 50 ms.
- WPF, A `ropslow`, 228 to 232 ms: B 227 to 240 ms.
- Windows Forms, A `ropslow`, 166 to 168 ms: B 162 to 165 ms.
- File Explorer, A `ropslow`, 122 ms: B 117 to 124 ms.
- Settings, A `ropslow`, 35 to 36 ms: B 33 to 34 ms.
- Edge, A `ropslow`, 6.3 to 6.8 ms: B 4.2 to 5.0 ms.
- Notepad, A `ropslow`, 4.7 to 4.9 ms: B 3.0 to 3.5 ms.
- Character Map, A `ropslow`, 5.5 to 5.9 ms: B 3.6 to 4.0 ms.
- Firefox, A `ropslow`, 2.6 to 2.8 ms: B's median 0.6 ms, but its 90th percentile and maximum were close to A's length.
- Windows Terminal, A `ropslow`, 35 to 37 ms: B 33 to 34 ms. A `textall`, 2.6 to 2.9 ms, is two requests (the document range, then its text): B 0.5 to 0.8 ms.
- AccessKit, A an Invoke whose action handler sleeps 200 ms, 202 ms: B 199 to 200 ms, for a name read into the same window and into the window on the other UI thread, from this process and another (`results/akapp.txt`). An Invoke as B waited 200 to 400 ms.

When A is a FindAll with a cache request, it is many requests from the client (see "One call's requests" below), and B gets in between them. B's median rose from 0.16 to 0.4 ms at most. B's maximum was one request of A: Character Map 0.9 ms, Firefox 1.3, Windows Terminal 1.4 to 2.6, Notepad 4.6 to 7.9, Explorer 5.5 to 7.7, VS Code 6.3 to 7.4, Settings 12 to 13, Windows Forms 26 to 27, WPF 35 to 46, and Edge's 20,000-element page 664 to 696 ms.

### A UI thread suspended, or a provider held, for 4 seconds

The stuck call was a name read or a remote operation. Every probe below returned only once the thread was resumed or the call released, 4,001 to 4,051 ms after the probes started. The probes covered both plain reads and remote operations, from other threads and from another client process. This held in 2 or 3 trials of each arm.

- Notepad: another element; the other Notepad window, which is on another UI thread of the process.
- File Explorer: another element; the folder process's XAML island window, on another UI thread (`results/explorer-island.txt`).
- Settings, Character Map, WPF, Windows Forms, Firefox and VS Code: another element.
- Windows Terminal: another element; its second window, which is on the same UI thread; and with a text call stuck (`text` or `textall`), text calls into the terminal control from other threads and another process, a name read, and a text call into the second window (`results/windows-terminal-text-suspended.txt`). So its text calls do not answer while its UI thread is stopped, unlike the synthetic `nocom` mode's calls.
- Edge: another element; the browser's top-level window. Both are on the browser UI thread, so Chromium answers these calls on its UI thread.
- mockapp under `hold`: another element; a child window. Under `stall`: the same. On the MSAA backend under `hold`, the same object, a new object, and another client process all waited too.
- Synthetic provider, `sta` and `agile`: another window on the same thread and the window on the other UI thread, for both a busy UI thread and a slow provider call. The same held for client processes of their own (`results/serve-*-xproc.txt`).

What a busy UI thread does depends on where UIA calls the provider:
- In the synthetic provider's `nocom` mode, UIA calls the provider off the UI thread, so a busy UI thread delayed nothing (1 to 9 ms). A slow provider call still held every other call into the process, the other UI thread's window and other client processes included.
- A provider that waits while pumping COM calls (`pump`) held the others exactly as a sleeping one did.
- Several calls queued behind the stuck one were answered in turn: FIFO order was seen in `results/serve-*-hung.txt`, where a probe into the other UI thread came after the same-window probe that was queued before it.

New connections: in every application, a new connection (`fromhandle`) to the stuck window, or to another window of the process on an idle UI thread, failed with `UIA_E_TIMEOUT` (0x80131505) after 2,165 to 2,208 ms. This held from this process and from another. In the synthetic `nocom` mode with a busy UI thread, it succeeded.

Left stuck for 60 seconds (Notepad, `results/stacks-notepad.txt.hung.txt`): the stuck call and every queued probe, including the one into the idle thread's window and the one from another process, failed with `UIA_E_TIMEOUT` at 21.9 to 22.1 seconds. Calls into mockapp under `hold`, with no debugger attached, failed at the same 22.15 s; "Timeouts" below explains the figure.

### Where the waiting happens

`results/stacks-notepad.txt`, `results/stacks-explorer-shell.txt` and `results/stacks-vscode.txt` are every thread's stack (cdb, non-invasive attach), taken while calls waited on a suspended UI thread. In each, one thread of the application had this stack, from the top:

1. `combase!MTAThreadDispatchCrossApartmentCall`
2. `UIAutomationCore!ComInvoker::CallTarget`
3. `UIAutomationCore!ReleaseCollection::DispatchReleases`
4. `UIAutomationCore!ProcessIncomingBatchedMessages`
5. `UIAutomationCore!ChannelBasedServerConnection::OnDataIdle`
6. `UIAutomationCore!ReadWriteChannelInfo::Service`
7. `UIAutomationCore!OverlappedIOManager::IoThreadProc`

So the thread was waiting on a COM call into the suspended STA. The suspended explorer thread was itself inside `UIAutomationCore!ReleaseOnCorrectContext_Callback`, dispatched by COM's window procedure. In VS Code the suspended thread was the browser's main thread, `CrBrowserMain`, which also serves UIA calls: `results/vscode-serving-thread.txt` has a name read's provider callback, `InvokeElementMethodOnCorrectContext_Callback`, running on `CrBrowserMain`, dispatched by COM's window procedure, after the I/O thread read the request. Each capture has one `IoThreadProc` thread.

Measured: that I/O thread was blocked on the suspended UI thread. In every capture it was delivering element releases at the time, so a release a client sends is queued on the UI thread like a call.

Measured in mockapp: the I/O thread read every request a client sent, 81 of 81 and 315 of 315 ("One call's requests" below). Measured in the synthetic provider ("UIA's I/O thread" below): it is the only one in the process, it takes requests from every client thread and process one at a time, each only after the response before it is written, and in `nocom` mode it makes the provider call itself. That explains every result above:
- Provider work never overlaps.
- One stuck call holds every other UI thread and every client process.
- A `nocom` provider runs on that thread, off the UI thread, still one call at a time.
- Calls are answered in FIFO order.

### One call's requests

`results/findall-requests.txt` counts one call's requests on both sides, with the debugger attached to the client and to mockapp (UIA backend). The client counted `ChannelBasedClientConnection::DoCallWithReturn`, the server `ChannelBasedServerConnection::OnData` on its I/O thread, and mockapp its own `Navigate` and `GetPropertyValue`.

- `name`: 1 request, with 1 property read.
- `rop`: 1 request, with 1 navigation and 2 property reads in it.
- `findall` on the 24-node tree: 81 requests, each read by the server's I/O thread. They were 2 reads of the root's runtime id and properties, 1 node from the provider entry point, 1 Find, 1 more property read, 1 read of properties and patterns, 23 `BulkFetch` requests (one per found element below the root, each reading name and control type), and 52 releases, which the server delivered in batches.
- `findall` on the 102-node tree: 315 requests, 101 of them `BulkFetch` and 208 releases.

The Find request holds the whole walk. On the 24-node tree it held 81 of the 83 navigations and 51 of the 100 property reads; on the 102-node tree it held all 311 navigations. The client's stack for it is `FindingVisitor::CallRemoteAndMergeState` under `UiaNodeTraverser::Traverse`. The server's stack for a navigation inside it is `UiaNodeTraverser::Traverse` under `RemoteUiaNodeStub::Incoming_Find`, on mockapp's UI thread, dispatched by COM's window procedure. So the walk is one cross-apartment call from the I/O thread into the UI thread, and the UI thread walks the tree itself.

That is why a cheap call B from another thread got in between FindAll's steps: each `BulkFetch` and each release batch is a separate request on the I/O thread, and B's request is read between them. B cannot get in during the Find request, which is why B's longest wait beside a FindAll grew with the size of the tree, up to 696 ms beside Edge's 20,000-element page.

### Why MSAA slowed with more threads

`results/msaa-path.txt` has one `msaa` call's path, with the debugger on both sides.

In Character Map and Notepad, `AccessibleObjectFromWindow(OBJID_CLIENT)` gave the client oleacc's own proxy (`CClient`, wrapped in `AccWrap_Annotate`). Its `accName` sent `WM_GETTEXTLENGTH` and `WM_GETTEXT` to the window, from `oleacc!HrGetWindowName`. The application answered them on its UI thread: in Character Map's dialog procedure, and in Notepad's WinUI windowing code. `accChildCount` made no call into the application, and took 0.3 to 0.4 microseconds. No UIA request and no COM call was made. mockapp's UIA backend gave the same proxy.

In mockapp's MSAA backend and in VS Code, the object was the application's own `IAccessible`, and each method was a COM call (`combase!NdrExtpProxySendReceive`). mockapp answered it on its UI thread, dispatched by COM's window procedure.

Split into parts, with K = 64 and 20 runs (best speed-up over N = 2, 4 and 8):
- Character Map: `msaaname` 0.78, `wmtext` 0.73, `wmnull` 0.63.
- Notepad: `msaaname` 0.62, `wmtext` 0.60, `wmnull` 0.69.

So a bare cross-thread `SendMessage` slows down with more sending threads, as `accName` does. The synthetic provider, with 8 windows, separates the receiving side (speed-ups at N = 2, 4 and 8):
- Each client thread sending to its own window on its own UI thread: `wmnull` 0.73, 0.44 and 0.43; `wmtext` 0.44, 0.67 and 0.41.
- Each client thread sending to its own window, all 8 on one UI thread: `wmnull` 0.58, 0.56 and 0.59; `wmtext` 0.62, 0.60 and 0.59.
- Every client thread sending to one window, in two runs: `wmnull` 0.48 to 0.78; `wmtext` 0.53 to 1.06.

Measured: more sending threads slow sent messages down even when no two of them share a receiving thread. Inferred: the contention is in the window manager's handling of sent messages, which every sender in the session shares. It was not seen directly, since a user-mode debugger does not show the kernel. COM calls (Edge, Firefox, VS Code, WPF, Windows Forms, mockapp's MSAA backend) did not slow down; of these, only VS Code's and mockapp's paths were checked in the debugger.

### MSAA calls while a UIA call is stuck

`results/msaa-while-io-thread-stuck.txt` uses the synthetic provider with windows 0 and 1 on one UI thread and window 2 on another. Window 0's name read sleeps 4 seconds, as the stuck UIA call. The probes are `msaanew` and plain UIA name reads into windows 1 and 2, from this process and another. Two trials each:

- `nocom`, so the stuck call sleeps on UIA's I/O thread and the UI thread is free, with oleacc's window proxy: MSAA calls into windows 1 and 2 answered in 1.2 to 2.6 ms. UIA calls into the same windows waited 3,800 ms, until the stuck call ended.
- `nocom` with UIA's MSAA bridge (`msaa=bridge`): the same, 1.6 to 2.3 ms. The bridge (`UIAutomationCore!AccessibleProxy`) answered on the window's UI thread, dispatched by COM's window procedure, not on the I/O thread.
- `sta` with the bridge, so the stuck call sleeps on the UI thread of windows 0 and 1, and the I/O thread waits on it: an MSAA call into window 2, on the other UI thread, answered in 1.6 to 2.0 ms, while a UIA call into window 2 waited 3,800 ms. MSAA calls into window 1 waited for its UI thread.
- `sta` with the bridge, and the UI thread of windows 0 and 1 busy for 4 seconds with no call queued: calls into window 2 answered, UIA in 0.4 to 1.2 ms and MSAA in 1.9 to 2.4 ms. Calls into window 1 waited.

So an MSAA call waits only for the thread that answers it, never for UIA's I/O thread.

### UIA's I/O thread

`results/iothread-count-mockapp.txt` and `results/iothread-count-serve.txt` start mockapp, and the synthetic provider with 8 windows on 2 UI threads, attach cdb before any client connects, and list the target's threads by start address after each step (`puia threads`, no debugger involved).

- Before any client: no thread of the process started in `UIAutomationCore`, though mockapp had loaded it.
- Created at the first connection, on the UI thread that received the first `WM_GETOBJECT`, from the application's `UiaReturnRawElementProvider`. The creating stack, from the top: `KERNELBASE!CreateRemoteThreadEx`, `KERNEL32!CreateThreadStub`, `OverlappedIOManager::AddChannel_AnyThread`, `OverlappedIOManager::AddWorkQueueChannel`, `ChannelBasedServerConnectionManager::SetupConnectionEvents`, `UiaReturnRawElementProvider`, then the window procedure's `WM_GETOBJECT` handler. The new thread started in `OverlappedIOManager::StaticIoThreadProc`. In the synthetic provider it was made by the UI thread of windows 4 to 7, the first window connected.
- One per process: after one client process, then 8 client threads in another process each connected to a window of its own (all 8 windows, on both UI threads), then 3 more client processes beside the first, the count stayed at one, and the debugger saw one I/O thread start and no other thread creation from `UIAutomationCore`. `OverlappedIOManager::Init` ran on all 30 connections, but only the first created a thread. The I/O thread's cycle count grew with every client's calls, so it served them all.
- Windows Terminal also had one I/O thread, and one other thread started in `UIAutomationCore`, at `NavigationHelperProxyOneCore::s_CoreUIThreadProc`, whose work was not examined.

`results/iothread-sequence.txt` shows the order. A name read into window 0 sleeps 1 second in the provider; 50 and 100 ms after it starts, name reads into window 1 (the same UI thread) and window 2 (the other UI thread) start on threads of their own. cdb marks each request the I/O thread takes (`ChannelBasedServerConnection::OnData`), each provider entry, each cross-apartment call and each response written, with the debugger's clock:

- `nocom`: the I/O thread took the first request, entered the provider at 51.930 and wrote its response at 52.931. Only then did it take the second request, sent 950 ms earlier, enter the provider and write the response, then take the third. All on the I/O thread.
- `sta`: the same order, with each provider entry on the window's UI thread, reached by a cross-apartment call from the I/O thread (`ComInvoker::CallTarget` on both threads). The third request, into the idle UI thread, was taken only after the second's response.

So the I/O thread takes one request, makes the provider call, writes the response, and only then takes the next. The calls returned at 1,002, 1,004 and 1,007 ms (`nocom`), in that order.

The client side has one I/O thread too. In the context-switch trace below, every reply was read by the client process's own `OverlappedIOManager::IoThreadProc` thread, which then woke the calling thread (`ChannelBasedClientConnection::SetResponseInfo`). The calling thread writes its request itself (`ReadWriteChannelInfo::BlockingWrite` under `ChannelBasedClientConnection::DoCallWithReturn`).

Not measured: whether a process ever gets a second I/O thread under conditions not tried here, such as after every client has disconnected.

### Windows Terminal's text provider

Calls into the terminal control's text provider run on Windows Terminal's XAML UI thread. With 64 calls of `textall` (the whole buffer's text, 2.8 ms each) under a context-switch trace, the UI thread had 847 CPU samples, 391 of them in `XamlUiaTextRange::GetText` and 243 in `UiaTextRangeBase::GetText` (Microsoft.Terminal.Control.dll), under `CManagedTextRangeProvider::GetText` and `CUIATextRangeProviderWrapper::GetText` (Windows.UI.Xaml.dll), `RemotePatternStub::TextRange_GetText`, and COM's window procedure (`ThreadWndProc`). UIA's I/O thread had 575 samples and none in Microsoft.Terminal.Control.dll (`results/wt-text-provider-thread.txt`). The debugger shows the same path: the I/O thread makes the cross-apartment call, and XAML's text range wrapper runs on the UI thread (`results/wt-serving-thread.txt`).

So Windows Terminal is an STA provider like the others, and its throughput, pairs and suspended arms above behave as theirs do. Its text calls answer only when the UI thread does.

### Why several threads speed up cheap calls

`results/parts.txt` splits one name read into the synthetic provider into parts (`puia parts`, K = 64, 15 runs, medians; times in microseconds per call). The processor time of each thread is its cycles converted at 3,755 to 3,779 cycles a microsecond, measured on the client thread. It includes the thread's kernel time, so its wake-ups.

At N = 1, `sta`:
- The whole call: 89.8.
- Before the provider call, from the client's start: 44.3. After it, until the client returns: 44.7. The provider call: 0.1.
- Processor time: the calling thread 24.3, the client's own UIA I/O thread 10.5, the provider's I/O thread 33.6, its UI thread 23.4. Together 91.8, about the whole call, so little of a call is spent waiting for a processor.

At N = 1, `nocom`: the whole call 41.7; before 18.3, after 22.7; calling thread 19.4, client I/O thread 7.1, provider I/O thread 16.7, with the provider call on it.

`results/parts-traced.txt` follows one `sta` call across the four threads in a context-switch trace, in which every part costs about 2.5 times as much. Of the call's 246: the client wrote the request at 24; the provider's I/O thread ran from 24 and posted the call to the UI thread at 84; the UI thread ran from 84 and released the I/O thread at 140; the I/O thread wrote the response at 177 and waited again at 193; the client's I/O thread read it and woke the calling thread at 212; the call returned at 246. So the provider process held the call from 24 to 193, about 69 percent of it, and the rest was the client's: the calling thread before sending and after waking, and its I/O thread. The I/O thread waited 43 of those 169 on the UI thread, and took no other request meanwhile.

The serial time per call is the time the provider's I/O thread is held per call: its own processor time, and for an STA provider the UI thread's, since the I/O thread waits for it. Under load it is the interval between provider entries, and it is shorter than at N = 1, because the I/O thread finds the next request waiting instead of blocking and waking for each:

- `sta`, the mean interval at N = 2, 4 and 8: 52.0, 47.5 and 50.8. The I/O and UI threads' processor time per call at the same N: 50.6, 46.6 and 50.1.
- `agile`: 49.9, 48.4 and 54.8.
- `nocom`: 25.1, 15.3 and 11.5. The I/O thread's processor time: 13.6, 12.4 and 10.6. At N = 2 and 4 the interval is longer than the processor time, so the I/O thread was not yet busy all the time.

The speed-up predicted at N is the time per call at N = 1 (the run's wall time over K) over the interval at N, and the measured one is the ratio of wall times:
- `sta`: predicted 1.86, 2.04 and 1.91; measured 1.76, 1.89 and 1.79.
- `agile`: predicted 2.00, 2.06 and 1.82; measured 1.89, 1.96 and 1.73.
- `nocom`: predicted 1.83, 3.00 and 3.99; measured 1.71, 2.71 and 3.51.

The prediction is 5 to 14 percent above the measurement in every case; by inference, each run's first and last calls, which have no other call to overlap with, account for the difference. So the measured serial share accounts for the speed-ups.

Why the gain stops at 2: for an STA provider the serial part is about half of a call (about 48 of 97), the I/O thread and the UI thread working in turn. With two client threads, one call's client-side half runs while the other's is in the provider process, and the provider process is already busy all the time: the interval at N = 2 is already the minimum. More threads only queue: at N = 8 each call took 392, about 8 times the interval. In `nocom` the serial part is only the I/O thread's, about a quarter of a call under load, so the gain went on to 3.5. The client process's own I/O thread reads every reply too, so it is serial as well, but at 4 to 10 per call it is not the limit here.

### AccessKit

`results/akapp.txt` measures `akapp`. AccessKit's adapter gives UIA `ProviderOptions_ServerSideProvider` alone, with agile objects (`accesskit_windows` `node.rs`), so UIA calls it on its I/O thread, as in the synthetic `nocom` mode.

- Which thread: the debugger put AccessKit's `PlatformNode::GetPropertyValue`, its `Invoke`, and the application's action handler on UIA's I/O thread, under `ChannelBasedServerConnection::OnData`. The action counters showed 2,560 of 2,560 actions off the UI thread.
- At the same time: the action counters' largest number of actions in the handler at once was 1, at N = 1, 2, 4 and 8. AccessKit also wraps the handler in a mutex. No name read was answered during a slow action: a name read B, from another thread or process, into the same window or the other UI thread's, waited 199 to 200 ms beside a 200 ms action. Only the action handler was counted; that two calls are never in AccessKit's other code at once follows from the one I/O thread, and is inferred.
- Throughput: the name read sped up 1.92, 4.16 and 6.28 at N = 2, 4 and 8, as in `nocom`. `invoke` sped up at most 1.79 and took 645 per call alone: each Invoke is two requests, a `SetFocus` (AccessKit's focus action) and then the `Invoke` (its click), seen in the debugger.
- A busy UI thread for 4 s, no call queued on it: name reads into the busy window's process answered in 1.0 to 3.1 ms, from another thread and another process, into either window. A new connection to the busy window failed with `UIA_E_TIMEOUT` at 2.0 s, and the calls from this process that started with it, a name read, a remote operation and a new connection to the other UI thread's window, answered only then. By inference, the I/O thread was sending `WM_GETOBJECT` for the new connection, as it does for the Invoke here; no stack was taken of it. An Invoke into the busy window waited for the busy period's end, though AccessKit's action runs off the UI thread: its stacks (`results/akapp-stacks-busy-invoke.txt`) show the I/O thread in `HwndUtils::SendWmGetObject`, sending `WM_GETOBJECT` to the window from `GetNodeFromProviderEntryPoint`, so that request of the Invoke needs the UI thread and holds the I/O thread meanwhile.
- A slow action, 4 s: every call into the process waited for it, the other UI thread's window and other client processes included, and new connections failed at 2.2 s, as with a slow `nocom` provider call.

### Abandoning a stuck call

`results/abandon.txt` (`run-abandon.ps1`, 2 trials of each arm and form) compares two forms of one query thread per application. In the first, every call is made on the stuck call's client thread, one after another. In the second, the stuck call is abandoned on its thread and each later call is made on a new thread at its time. The later calls were set for 1, 3, 5, 8 (a new connection to the idle UI thread's window), 25, 26 and 41 seconds after the block, the block lasting 40 seconds. All times are from the block. The two trials of each arm agreed to within 30 ms, except where noted.

The synthetic provider's STA, its UI thread busy 40 s with no pumping, the stuck call a name read into its window:
- One thread: the stuck call failed with `UIA_E_TIMEOUT` at 22.1 s. The next call started then, waited, and answered 0.4 to 0.8 ms after the release at 40 s, within its own 22 seconds. The calls set for 3 to 26 s started only then and answered within 2.6 ms of the release. So no call failed after the stuck one, but none started before 22 s.
- New threads: the calls started at 1, 3 and 5 s, into the busy window, its thread's other window and the other UI thread's window alike, each failed at its own start plus 22.1 s. The new connection at 8 s failed at 10.2 s, after the 2-second connection timeout. The calls started at 25 and 26 s, after the stuck call had timed out while the provider was still held, waited and answered 1.2 to 1.7 ms after the release.
- After the release, the provider process served the queued requests of calls that had already failed, then the waiting ones: the first successful answer came 0.4 to 0.8 ms after the release on one thread, and 1.2 to 1.7 ms after it on new threads.

mockapp under `hold`, released at 40 s, the stuck call a name read: the same. On one thread the next call answered 0.3 to 0.4 ms after the release. On new threads, calls at 1 and 3 s failed at their start plus 22.1 s (the remote operation's failure is reported as `E_FAIL` by `verbatim-uia-rops`), the new connection at 5 s failed at 7.2 s, and the call at 25 s answered 1.4 ms after the release in one trial and 0.4 ms before the recorded release in the other, the release command having reached mockapp before its time was taken.

A provider that serves off the UI thread (`nocom`):
- Its UI thread busy 40 s: every call answered at once in both forms, the stuck one included, in 0.1 to 3.8 ms, and the new connection to the idle thread's window in 0.9 to 27 ms. Nothing was held.
- Its provider call itself slow, 40 s, so UIA's I/O thread is held: exactly as the busy STA. On one thread the next call answered 0.5 ms after the release; on new threads the calls at 1, 3 and 5 s failed at their start plus 22.1 s, the new connection failed at 2.2 s, and the calls at 25 and 26 s answered 1.1 to 1.7 ms after the release.

A stuck call that is a pending connection: `ElementFromHandle` of the busy STA window, which waits in the window's message queue, not in UIA's channel:
- The stuck connection failed at 2.0 s, the connection timeout. Where a later call went to the busy UI thread (a name read on an element held from before, at 1 s), that call was queued there and held the process as a busy STA does: on new threads every call made before 25 s failed at 22.1 s, and on one thread the calls after it waited for the release.
- With no later call into the busy thread (`sta-pending-connection-only`, a 12-second block, calls into the other UI thread's window at 0.5, 1, 1.5, 3 and 13 s): on one thread, the calls started when the connection failed at 2.0 s and all answered in 0.2 to 0.7 ms, the busy thread notwithstanding. On new threads, the name read and remote operation started at 0.5 and 1 s waited until the connection failed and answered at 2.0 s. So a pending connection held the other calls of its own process, though `e30` found it holds no other process's call. The new connection started at 1.5 s and the name read at 3 s then waited until the release at 12 s, in both trials.
- `results/abandon-pending-connection-stacks.txt` repeats that form and takes every stack at 6 s. The provider's I/O thread was in `ReleaseCollection::DispatchReleases`, making a cross-apartment call into the busy UI thread (`ComInvoker::CallTarget`, `combase!MTAThreadWaitForCall`): it was delivering element releases into the busy thread, and the 3-second name read waited behind them in the client (`ChannelBasedClientConnection::WaitForResponse`) until the release. In that trial the 1.5-second connection answered at 2.0 s. Which releases these were, and why the single-thread form did not meet them, was not established.

So one query thread per application and a new thread per call after abandonment give the same answers at nearly the same times. A provider held in its UI thread or its I/O thread answers nobody until the release, whichever client thread asks. A new thread only lets later calls start sooner, and each then fails on its own timeout, 22.1 s for a call and 2.2 s for a new connection, instead of waiting behind the stuck call.

### Requests per client operation

`results/ops-requests.txt` counts each operation on both sides (`ops.ps1`), cdb on the client and on the target, as "One call's requests" did: the client's `ChannelBasedClientConnection::DoCallWithReturn`, named by the UIA function that sent it, and the target's `ChannelBasedServerConnection::OnData` on its I/O thread. The provider calls are mockapp's own counters, or for the synthetic provider its COM entry points counted by cdb. Each operation ran twice in a row; the figures are the second, steady-state run, and the first differs where noted. In every operation the target's I/O thread read every request the client sent to it.

mockapp, `ops.json`, the operations on the list `list1` (two items, a parent window, siblings) unless named:
- Raw view walker, first child and next sibling: 2 requests, a `Navigate` and the `Release` of the element passed over. The first call of the first child also read the element's runtime id (3). Provider calls: 2 `Navigate`, 1 `GetPropertyValue`, 1 `GetRuntimeId`, and 11 or 12 of `ProviderOptions`, `HostRawElementProvider` and `FragmentRoot`, which UIA makes around every node it handles.
- Raw view walker, parent: 4 requests, a `Navigate` and 3 `Release`s; 3 provider `Navigate`s and a `WM_GETOBJECT`, since the parent is the window's root.
- Control view walker: the same requests, the parent with one more `GetPropertyValues` (5), and one more provider `GetPropertyValue` for each node it returns.
- With a cache request (`...BuildCache`): first child and next sibling 4 (`Navigate`, `BulkFetch`, 2 `Release`s); parent 5 in the raw view and 6 in the control view, with `GetPropertiesAndPatterns` in place of `BulkFetch`.
- FindFirst of a node 3 levels down (`tab2`): 7 requests: `GetPropertyValues`, `RealRemoteNodeFromProviderEntryPoint`, 1 `Find`, 4 `Release`s (the first run also a `GetRuntimeId`). The `Find` held the walk's 338 provider calls, 63 of them `Navigate`. FindFirstBuildCache: 9, one more `BulkFetch` and `Release`.
- FindAll without a cache request, the whole 31-element subtree: 34 requests, 1 `Find`, 2 `GetPropertyValues`, 1 entry point and 30 `Release`s; FindAllBuildCache in the same run: 84, with 24 `BulkFetch`es and 54 `Release`s, as before. Both made 86 provider `Navigate`s.
- BuildUpdatedCache of 6 properties on one element: 2 requests (`BulkFetch`, `Release`), 5 provider `GetPropertyValue`s and a `BoundingRectangle`. With subtree scope, Name and ControlType: 4 requests (`BulkFetch` and 3 `Release`s) for the list and its 2 items, 9 provider `Navigate`s and 11 `GetPropertyValue`s.
- A Current property read (`CurrentName`): 1 request (`GetPropertyValues`), provider `GetPropertyValue`, `HostRawElementProvider` and `FragmentRoot` once each.
- A Cached property read: no request and no provider call, confirmed.
- `GetRuntimeId`: 1 request the first time, none the second. `CompareElements` of two references to the element: 2 `GetRuntimeId` requests the first time, none after. So the client keeps an element's runtime id once read.
- `ElementFromHandle`: 5 requests (entry point, `GetPropertyValues`, 3 `Release`s), 2 `WM_GETOBJECT`s.
- `ElementFromPoint` at the window's centre, then the element's process id and name: 9 requests (entry point, `DrillForPointOrFocus`, 3 `GetPropertyValues`, `GetPropertiesAndPatterns`, 3 `Release`s), one `ElementProviderFromPoint`, 4 `WM_GETOBJECT`s.
- `GetFocusedElement`, mockapp the foreground window with its text node focused: 6 requests (entry point, `DrillForPointOrFocus`, 4 `Release`s), one provider `GetFocus` and 4 `WM_GETOBJECT`s. With another window in the foreground, the taskbar by the focus event's name, the same 5 requests went to another process, and mockapp read none.
- `GetCurrentPattern` (text): 2 requests, `GetPatternProvider` and the `Release` of the pattern object.
- The toggle pattern's `Toggle` on a held pattern object (mockapp has no invoke pattern): 12 requests, `Toggle` 1, `SetFocus` 2, `Navigate` 2, `GetPropertyValues` 3, `Release` 4; the provider took one `SetFocus` before its `Toggle`.
- Text: `DocumentRange` 2 requests (the call and the `Release` of the range object); `GetText` on a held range 1; `Move` on a held range 1, or 3 with a `Clone` first and its `Release`; `ExpandToEnclosingUnit` 1. Each made exactly one provider call of its name.

The synthetic provider (`sta`), a window's own provider with no fragments:
- `CurrentName` 1 request, 1 provider `GetPropertyValue`. Cached read none. BuildUpdatedCache of 6 properties 1 request (`GetPropertiesAndPatterns`), 5 provider `GetPropertyValue`s.
- `GetRuntimeId` 1 request with no provider call, then none. `ElementFromHandle` 5 requests, as mockapp.
- Walkers from the top-level element: the parent, the desktop, sent no request; the first child, UIA's own title bar proxy, one `GetRuntimeId` the first time; the next sibling 3 requests, none of which reached the synthetic provider; where they went was not identified.
- FindAll of the subtree (7 elements, most of them UIA's proxies) 11 requests; with a cache request 12.
- `GetCurrentPattern` (invoke) 2 requests. `Invoke` 2 requests (`GetPropertyValues`, `Invoke`) and one provider `Invoke`.

### Events and releases

`results/ops-event-stacks.txt` has the stacks; the counts are in `results/ops-requests.txt`.

Adding and removing a handler goes from the client to the provider process, as requests:
- `AddAutomationEventHandler` on the root, subtree scope: 2 requests, a `GetRuntimeId` and `EntireProcessAddEvent`; mockapp's provider took `GetEmbeddedFragmentRoots` and `FragmentRoot`. Removing it: 2 (`GetRuntimeId`, `RemoveEvent`). A property changed handler: the same.
- `AddFocusChangedEventHandler`: 27 requests, 6 to 8 of them `EntireProcessAddEvent`; mockapp read 7 of the 27, and the rest went to other processes, which were not identified. Removing it sent 6 `RemoveEvent`s, one of them to mockapp. Inside the add call, on the registering thread, UIA delivered a focus event for the element then focused (the taskbar), from `ClientEventManager::RegisterHandler`.

An event's delivery goes from the provider process to the client:
- mockapp raised it on its UI thread (`UiaRaiseAutomationEvent` from its command handler), and on that thread UIA wrote it to the client (`ChannelBasedServerConnection::SendEventData` under `RemoteEventCallbackProxy::CrossProcess_OnTentativeEvent`), a 149-byte write on the pipe that carries the replies (`results/transport.txt`). It was not handed to the I/O thread.
- The client's I/O thread read it (`ChannelBasedClientConnection::OnData`). The handler then ran on a thread that starts in `WorkItemQueue::StaticThreadProc`, under `SavedEvent::OnMessage`, a different thread id for each of the 4 events. Before calling the handler, that thread fetched the event's cache request from mockapp: an entry point request and a `BulkFetch`, then 6 `Release`s, 8 requests for one selection event.
- With no handler registered, the synthetic provider's `Invoke` raised its event and no event was sent (`SendEventData` 0); with one, one was sent.

An element's release is a request of its own, sent at once by the thread that releases the element (`CAutomationElement::Release`, then `LocalUiaNodeProxy::RealCrossProcess_Release` and `DoCallWithReturn`), with no reply read for it. Dropping 30 elements held from a FindAll sent 23 requests, one for each element mockapp serves; the other 7 are UIA's own proxies in the client. mockapp's I/O thread read each of the 23 separately and delivered them in 11 batches (`ProcessIncomingBatchedMessages`). So the provider process batches releases; the client does not. One element dropped alone: 1 request, 1 batch. Every walker step above includes the release of the element it passed over.

### Timeouts

`results/timeouts.txt` (`timeouts.ps1`): mockapp under `hold`, call A a name read, call B a name read of the same element 200 ms later on another thread, no debugger attached. The connection timeout was UIA's default 2 s except where set.

- Transaction timeout 2 s: A failed with `UIA_E_TIMEOUT` after 2,190 ms of its own, B after 2,198 ms of its own. 5 s: 5,516 and 5,519 ms. 20 s, the default: 22,158 and 22,153 ms. Each call failed on its own timeout from its own start; B's queuing behind A did not change it. Two runs at 2 s, one of them traced, one at 2 s with a 0.5-second connection timeout (2,203 and 2,201 ms), and two at 5 s, one traced, agree within 15 ms, so the connection timeout plays no part.
- Why 1.1 times the timeout: a context-switch trace of the 2-second run shows call A's thread blocked 18 times in `ChannelBasedClientConnection::WaitForResponse` on `WaitOnAddress`, each wait 108 to 111 ms and each ended by a timer's expiry, then once for 205 ms in `RequestResponseCoordinator::WaitForResponse`, then failing: 18 waits of about 110 ms plus 205 ms is the 2,197 ms measured. The 5-second run: 48 such waits and one of 205 ms, 5,512 ms. So the client waits the timeout less 200 ms as a count of steps, each of which lasts about 110 ms, then 205 ms. At 20 s that gives about 198 steps and 22.1 s, which is the 22.1 s seen throughout this experiment. Inferred, not observed: that each step is a 100 ms wait lengthened to seven 15.6 ms timer ticks; a bare `WaitOnAddress` of 2,000 or 5,000 ms in the same client process took 2,000 to 2,013 and 5,005 ms, so the system's wait itself does not add 10 percent.
- Under an invasive debugger the client waited differently: each call read its timeout (`ChannelBasedClientConnection::GetTimeout` returned 2,000), then waited once, `WaitOnAddress` with 2,000 ms, in `RequestResponseCoordinator::WaitForResponse`, and failed after 2,022 ms; at 20 s after 20,013 ms. Why the debugger changes the path was not established.
- The provider side: during the 2-second run, mockapp's I/O thread was blocked 4,060 ms in its cross-apartment call (`ComInvoker::CallTarget`, `combase!MTAThreadWaitForCall`), from A's request until the release at 4 s, though both client calls had failed at 2.2 and 2.4 s. So the client's timeout does not reach the provider process; its I/O thread stays in the provider call until the call returns, here until the release at 4, 12 and 30 s.
- After the release, mockapp's counters rose by 2 `GetPropertyValue`s, 2 `HostRawElementProvider`s and 2 `FragmentRoot`s in every run: A's held call finished, and B's request, which had failed seconds before, was still delivered to the provider and answered.
- A FindFirst as B under a 1-second timeout failed after one timeout, 1,106 ms, and after the release the counters showed only A's calls; the request it waited on needed no call into mockapp's own provider. "A search's timeouts" below names that request and explains E-TT's 13.2 seconds.

### A search's timeouts

`results/search-timeouts.txt` (`search-timeouts.ps1` and `timeouts.ps1`): why a search under a 1-second transaction timeout failed after 13.2 s in E-TT, after 1.1 s when queued behind another call, and after three or four timeouts with a stalled window thread. The client is E-TT's (`ett` above), its search started 0.3 s after the window's element was fetched, and the target is mockapp with `small.json`. The client's waits are from context-switch traces with no debugger attached, read with `blocked-summary.py`. The provider's requests are from the same traces, as the wakes of mockapp's I/O thread with the client's frames that woke it, and from cdb on mockapp.

E-TT, mockapp under `hold`, so mockapp held the search's first provider call: the search failed with `UIA_E_TIMEOUT` after 13,211, 13,208 and 13,211 ms untraced, and 13,195 ms traced. The search's thread waited four times, in this order:
- The `GetPropertyValues` request from `FindingVisitor::Enter`, the condition's test on the root, which mockapp held: 8 waits of 110 ms and one of 205 ms, 1,085 ms, then the request failed. The search went on.
- `SendMessageTimeoutW` of `CB_GETCOMBOBOXINFO` to mockapp's window, from `BasicHwndUtils::GetComboBoxInfoTimeout` under `HwndProxy::Navigate`, the client's own proxy for the root's window: 1,010 ms. Its timeout argument was 1,000, the transaction timeout.
- `SendMessageTimeoutW` of `WM_GETOBJECT` for object id -5, from oleacc's `AccessibleObjectFromWindowTimeout` under `DoNonClientNavigation` and `MsaaNonClientProxyRoot::Navigate`, the client's non-client proxy: 10,013 ms, with a timeout argument of 10,000. That is the connection timeout in force when the root was fetched, not the 1,000 ms set for the call. With 4 s when the client was made, the search took 7,202 ms. With 2 s it took 5,207 ms, and with 10 s when the client was made but 3 s set just before the fetch, 6,201 ms. The ops client, which fetches its root at the default 2 s, took 5,207 ms for the same search.
- The `Find` request from `FindingVisitor::CallRemoteAndMergeState`, queued behind the held call: 1,075 ms, then the search failed.

1,085 + 1,010 + 10,013 + 1,075 is 13,183 ms of the 13,195 traced. So the 13.2 s is not twelve timeouts. The client re-sent nothing and fetched no element: it sent mockapp two requests and the window two messages, and each waited out its own timeout. The messages wait on the window's thread, which the held call blocked, not behind UIA's I/O thread. A cdb on the client counted the same requests and messages, with one more `GetRuntimeId` that the attach's delay caused (below). Connection recovery off: 13,208 ms.

What mockapp did, from the trace: its I/O thread was woken once, by the client's `GetPropertyValues` write, and stayed in its cross-apartment call for 16,019 ms, until the release. Then it read the `Find`, without waiting, since the request was already in the pipe. The window thread ran the whole walk, 53 provider calls by cdb (ProviderOptions 19, GetPropertyValue 11, Navigate 10, HostRawElementProvider 7, GetRuntimeId 3, FragmentRoot 3), and the response went to a client that had failed 3 seconds before. Neither window message reached mockapp's window procedure after the release: cdb on `wnd_proc` logged mockapp's own messages and no `WM_GETOBJECT` or `CB_GETCOMBOBOXINFO`. The next request was the root's release when the client exited.

The FindFirst queued behind a held name read (`timeouts.ps1`, the ops client, ops.json): B's only request was the root's `GetRuntimeId`, from `ElementFinder::Find` before the walk. It waited one timeout behind A's call, 1,083 ms, and its failure ended the search. The ops client had fetched its root long before, so the search asked for the root's runtime id again. After the release, mockapp read that request and answered it with no provider call. So a failed runtime id read ends a search, and a failed condition test does not.

The stalled-thread setup of the test removed in commit 31d462fc (`a_search_waits_a_timeout_for_each_call_its_stalled_provider_does_not_answer`): mockapp's window thread took no message (then `stall-until-release quiet`, now `stall <ms>`). The search was a `FindFirst` over the descendants for "Second", with both timeouts set to T before the element's fetch, on verbatim-uia's client with connection recovery on. The search waited the same three times: `GetPropertyValues` (T in steps of about 110 ms, then 205 ms), `CB_GETCOMBOBOXINFO` (T), and `WM_GETOBJECT` (T, the connection timeout at the fetch). At T of 100, 200 and 500 ms it then failed without sending the `Find`, after 332, 618 and 1,545 ms. At 1,000 ms it also sent the `Find` and waited for it, and failed after 4,207 to 4,209 ms. That is the test's three timeouts at 100, 200 and 500 ms and four at 1,000 ms. At 500 ms mockapp's I/O thread read one request, at 1,000 ms two, and it stayed in the first until the stall ended.

When the `Find` is sent: with connection recovery on, the client sent it when the three waits ended 3.1 s or more after the search began, and not when they ended by 2.6 s. Written as transaction timeout, connection timeout at the call and connection timeout at the fetch, it sent the `Find` at 1,000, 1,000 and 1,000; 1,500 three times; 2,000, 2,000 and 500; 1,000, 500 and 1,000; and 500, 500 and 3,000 ms. It did not at 100, 200, 500 and 700 ms throughout; 1,000, 1,000 and 500; 500, 500 and 1,000; and 500, 1,000 and 500 ms. The transaction timeout set the first two waits and the connection timeout at the fetch the third, in every run. With connection recovery off it sent the `Find` in both arms tried: 1,000 ms with 500 ms at the fetch failed after 3,697 ms, and 500 ms after 2,087 ms. Why recovery skips the `Find` was not established.

The root's runtime id: a search that started 0.3 s after the root's fetch sent no `GetRuntimeId`. Searches that started 1.6 s or more after it, behind a debugger's attach, sent one, and mockapp runs it on the window thread: with cdb on mockapp, its stub ran only when the stall ended. With the window thread stalled and 300 ms, 600 ms, 1 s or 3 s more between the fetch and the search, the search failed after one timeout, 1,087 to 1,091 ms, as it does when that request waits; so from about 0.65 s after the fetch, by inference, the search sends it. How long the client keeps the runtime id from a fetch was not measured more closely.

### The transport

`results/transport.txt` (`transport.ps1`): cdb on mockapp and on a client before the client's first connection, with breakpoints on the file, pipe and ALPC calls that print each handle's type and name (`!handle <h> f`).
- The client's calling thread, inside `ElementFromHandle`, created a named pipe, `\\.\PIPE\UIA_PIPE_6268_dcaa6add` (6268 is mockapp's process id), in `ChannelBasedClientConnectionManager::CreatePipeToProvider`, then waited in `ConnectNamedPipe`. Before creating it, the same thread waited on two named events, `UIA_CONNECT_EVENT_6268_dcaa6add` and `UIA_NOBODY_THERE_EVENT_6268_dcaa6add`, in `ProviderEntryPoint::ConnectUsingHwnd`.
- mockapp's I/O thread opened the same name with `CreateFileW`, in `ChannelBasedServerConnectionManager::OnEventSignal` under `ListenEventInfo::Service`, and its `IoThreadProc` waited on a named event `UIA_PIPEREADY_EVENT_6268_dcaa6add` among its handles.
- Both ends' handle is of type File, with no name `!handle` can show, the pipe's ends. Every request the client sent was a `WriteFile` on it from the calling thread (`ReadWriteChannelInfo::BlockingWrite`), and the event worker thread's requests too; every reply and the event were read by the client's I/O thread with `ReadFile` (`ReadWriteChannelInfo::Service`). On mockapp's side the I/O thread read every request and wrote every response on its handle, and the UI thread wrote the event.
- The ALPC calls traced in both processes (71 in the client, 15 in mockapp) were not UIA's channel: the first three of each were theme services or COM's RPC binding; the rest were only counted.

### Threads of one client against client processes

`results/xbench.txt` (`run-xbench.ps1`): K calls split over N callers, as N threads of one client process or as N processes of one thread, 15 runs (10 for the 5 ms arms), the speed-up over one thread of one process.

- Name reads, synthetic `sta`: threads 1.35, 1.35 and 1.26 at N = 2, 4 and 8; processes 1.26, 1.21 and 0.78.
- Name reads, synthetic `nocom`: threads 1.14, 1.25 and 1.28; processes 1.02, 0.99 and 0.76.
- Name reads, mockapp: threads 1.47, 1.50 and 1.46; processes 1.40, 1.30 and 1.04.
- A 5 ms provider sleep (synthetic, both modes) and mockapp's 5 ms delay on each provider call: 0.91 to 1.01 in every form.

So spreading callers over processes gained nothing over threads, and 8 processes were slower than 8 threads in every cheap arm. The client's own I/O thread used 32,000 to 38,000 cycles a call with one thread and with 8 threads of one process (about 8.5 to 10 microseconds at the 3,760 cycles a microsecond measured in "Why several threads speed up cheap calls"), and 47,000 to 55,000 a call summed over 8 processes, each of which has an I/O thread of its own.

In the client, 5 snapshots of a process with 8 threads making name reads into the synthetic provider (`sta`) found the calling threads in `ChannelBasedClientConnection::WaitForResponse` on `WaitOnAddress` 39 times of 40, and once formatting a string in UIA; none waited on a lock, and the client's I/O thread was idle in `IoThreadProc`'s `WaitForMultipleObjects` in all 5. So no serialization in the client was seen before or after the transport: the calls waited for their replies. A lock held for less than the snapshots could catch is not ruled out.

## Frameworks not covered

- Qt: no Qt application is installed.
- conhost: this machine's default terminal hands every new console to Windows Terminal, so no conhost window could be opened without changing that setting.
- WPF and Windows Forms on .NET 8 or 10: the runtimes are installed, but no application uses them. The .NET Framework 4.8 versions were measured.
- Java Swing, through the Java Access Bridge: none installed.

## Limitations

- Each suspended arm ran 2 trials (3 for the synthetic provider and mockapp), and each pair 10 iterations of A. The mechanism makes the outcome deterministic, and no trial disagreed.
- The machine is a virtual machine, and absolute figures are from it.
- `CoWaitForMultipleHandles` in the pump arm waited with a timer that rounds to the 15.6 ms tick, which is why it took 15.8 ms a call. The figures compare arms, not that delay.
- The debugger's non-invasive attach suspends every thread of the target until it detaches. On the first capture, symbol downloads kept the shell's explorer.exe frozen for about eight minutes. That capture's call timings after the resume, about 22 seconds, are the debugger's, not the application's. `stacks.ps1` says so.
- Under the debugger's invasive attach, every UIA call into VS Code, and into Windows Terminal, failed with `UIA_E_ELEMENTNOTAVAILABLE` (0x80040201), the root element's included, while the same probe without the debugger answered. Their serving-thread stacks are from such failed calls, which reached the provider first; Windows Terminal's thread is confirmed without the debugger by the CPU samples. Why the attach makes them fail is not known.
- The request counts are mockapp's. Another provider can answer differently; the FindAll pairs above fit the same pattern in every application.
- File Explorer's MSAA path was not examined, so as not to attach a debugger to the shell.
- Not measured: whether UIA ever starts a second I/O thread under conditions other than those counted.
- The parts of a call are processor times from cycle counts, which include the thread's kernel time and, in a virtual machine, any time the host took the processor from it. The context-switch trace that splits one call in time made every part about 2.5 times as long, so it gives the order and the shares, not the untraced durations.
- AccessKit's concurrency was counted in the application's action handler only; its other code was observed on the I/O thread in the debugger, one call at a time, not counted.
- The abandonment arms ran 2 trials each. The pending connection's later calls on new threads differed between runs (a new connection answered at 2.0 s in one and at the release in two), and the cause of the releases that held the I/O thread was not established.
- The requests per operation were counted once (each operation twice in a row) on mockapp and the synthetic provider, under breakpoints; another provider can make UIA send other requests, and a breakpoint run's times are the debugger's.
- An invasive debugger on the client changes how UIA waits for a reply (one wait of the whole timeout instead of steps), so the timeout figures are from runs without one, and the steps' 100 ms nominal length is inferred.
- Where `AddFocusChangedEventHandler`'s requests that did not reach mockapp went, and what `GetFocusedElement` and a top-level next sibling reached in other processes, was not identified.
- Why connection recovery skips a search's `Find`, and how long the client keeps a fetched element's runtime id, are bounded by the runs in "A search's timeouts", not explained. Each search arm ran 1 to 4 times. With cdb attached to mockapp before the client started, the stalled search returned no element and no error, after 2,025 ms at 1 s and 1,010 ms at 500 ms. That was not seen without the debugger and was not investigated.
- The thread-against-process comparison used mockapp's debug build, and the client stacks under load are 5 snapshots of one run.
