# A provider continues when a client is killed in the middle of its call

## Fact

A provider process goes on serving UIA when a client process is ended with `TerminateProcess` while the client's call is held in the provider: once released, the held call returns, by the provider's own log, after the client has ended, and the provider then answers its own requests and a new client's read. This held for a property read, a text range's `GetText` and a remote operation, with and without `ProviderOptions_UseComThreading`, and for a client holding event handlers ended while the provider was still raising events back to back, the kill coming before the raise had answered (2026-11-06). Whether the provider delivered events to the ended client, rather than raising them, the trials do not show. `books/dev/src/explanation/architecture.md`, section 1, names killing a UIA client in the middle of a call as the leading suspect in a crash of a Windows Terminal it was reading, inside `UIAutomationCore.dll`, on 2026-10-08; the provider program, a tree this program controls, does not crash that way. The parts of Windows Terminal's provider this program cannot stand for are under "Method".

## What Verbatim relies on

The clean shutdown of outposts and the listener, the 21-second `SHUTDOWN_LIMIT` before a child is killed through its job, and M6.2's kill order, which rest on that suspicion (`books/dev/src/explanation/architecture.md`, section 1). If a killed client did crash its provider, the clean shutdown is what keeps Verbatim from crashing applications; that it does not crash the provider program shows only that the suspicion is not a general property of UIA.

## Method

The provider program P serves window 0. A client process K, a second copy of this program, holds window 0's element, node 1 and node 3's document range. P holds K's call in its provider until released; this program ends K with `TerminateProcess` while the call is held, waits for K to end, then releases the call. P's log has to show the held call ending after the release, and P then has to answer within 5 s both its own request line and a name read of node 2 from this program's own client object; a trial whose P has ended is recorded with its exit code, and one that answers neither within 5 s as hung. Each trial is a new P and a new K. Every case runs 20 trials with P's COM threading and 20 without it, where P's calls run on UIA's I/O thread, as Windows Terminal's text provider declares `ServerSideProvider` alone.

- `read`: node 1's name (`GetPropertyValue`) held.
- `text`: node 3's document range's `GetText` held.
- `operation`: a remote operation reading the window's name, its `GetPropertyValue` held.
- `events`: no call held; K holds a name-change handler on the window while P raises 2,000 name changes back to back on its UI thread, K ended 20 ms after they are asked for, a deliberate offset, since nothing marks K's handler running; a trial counts only when the kill came before P had answered the raise, so while it was still raising.
- `text-and-events`: 20 trials without COM threading, putting together what Windows Terminal's provider does beside a client's call: K holds name-change and notification handlers and its `GetText` is held, while P raises 2,000 notifications back to back on its UI thread, and K is ended 20 ms after they are asked for, the kill before P answered the raise and the held call ending after the release, by P's log.

Windows Terminal's source (microsoft/terminal, `src/types/ScreenInfoUiaProviderBase.cpp`, `src/types/UiaTextRangeBase.cpp`, `src/cascadia/TerminalControl/TermControlAutomationPeer.cpp`) differs from the provider program in ways a Win32 provider cannot reproduce: its calls arrive through system XAML's automation peer, which wraps each text range in a second COM object (`XamlUiaTextRange`); its range methods take the terminal's lock, which its output thread also takes; and its events are queued to the XAML UI thread with `Dispatcher().RunAsync` from the output side. The program reproduces the threading and the events beside a held call; the XAML layer and the lock it does not.

Judged: each case's count of trials in which P continued, as a rate against the first run's 20 of 20; every other outcome is printed as evidence.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| A Windows Terminal crashed inside `UIAutomationCore.dll` while Verbatim was reading it, killing a UIA client in the middle of a call the leading suspect | 2026-10-08 | Build 26200 | `books/dev/src/explanation/architecture.md`, section 1 | History, no program |
| First uia-facts run: P continued in 20 of 20 trials of every case, with and without COM threading, and in 20 of 20 of `text-and-events` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | New: continues |
| Rerun with the held call's end required in the provider's log and the kill before the raise answered: the provider continued in 20 of 20 trials of every case, each held call returning after its client ended, each kill among the events | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 7319ed67 | Pass, against 20 of 20 |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call.exe`, or one kind with `--case`. No desktop, application or debugger.

## Evidence kept

None: the program's output is the evidence.
