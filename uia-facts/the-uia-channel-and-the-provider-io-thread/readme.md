# The UIA channel and the provider I/O thread

## Fact

A UIA client reaches a provider process over a named pipe, `\\.\PIPE\UIA_PIPE_<provider pid>_<hex>`, which the client's calling thread creates inside its first connection to the process, after waiting on two named events (`UIA_CONNECT_EVENT_<pid>_<hex>` and `UIA_NOBODY_THERE_EVENT_<pid>_<hex>`), and which the provider process's I/O thread opens. Requests, replies, element releases and events all pass on it; the ALPC calls seen during a connection are theme services and COM's RPC binding, not UIA's channel. The provider process's I/O thread is created by its first `UiaReturnRawElementProvider` and starts in `OverlappedIOManager::StaticIoThreadProc`; there is one per process, after 30 connections from 5 client processes. It delivers element releases in batches (`ReleaseCollection::DispatchReleases`), as a cross-apartment call for a provider on its UI thread. Events do not pass through it: the thread that raises an event writes it to the pipe. Whether a process ever gets a second I/O thread, in a provider or in a client, and whether a new connection's work runs on it, are not known (`books/dev/src/explanation/windows/uia.md`, "The parts" and "Inside the provider process").

## What Verbatim relies on

No code. The observable outcomes, one request at a time, in order, held by a stuck call, are checked by [a provider process serves one UIA request at a time](../a-provider-process-serves-one-uia-request-at-a-time/readme.md) and the other request-reader folders.

## Method

A guide only: the internals are function names in `UIAutomationCore.dll`, which no program can judge without a debugger. The client is `uia-answers-some-calls-in-the-client.exe --case operations --pause`, which starts the provider program, pauses before its first connection, then makes each operation of "What one request is" one at a time, pausing before each and marking it for the debugger (`uia-facts op <name> begin` and `end`, by `OutputDebugString`). The provider is uia-facts' own provider program, one UIA window on an STA UI thread with `UseComThreading`.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The pipe `UIA_PIPE_6268_dcaa6add` made by the client's calling thread in `ChannelBasedClientConnectionManager::CreatePipeToProvider`, after `UIA_CONNECT_EVENT_6268_dcaa6add` and `UIA_NOBODY_THERE_EVENT_6268_dcaa6add`; opened by mockapp's I/O thread with `CreateFileW` | 2026-11-02 | Build 26200.9711 | `experiments/parallel-uia-calls/`, "The transport", `results/transport.txt` | The names' form and the functions |
| The I/O thread created from `UiaReturnRawElementProvider` on the first connection, starting in `OverlappedIOManager::StaticIoThreadProc`; one after 30 connections from 5 client processes | 2026-11-02 | Build 26200.9711 | The same, "UIA's I/O thread", `results/iothread-count-*.txt` | Exactly one thread |
| Releases delivered by the I/O thread in batches, `ReleaseCollection::DispatchReleases`: 23 requests for 30 elements, in 11 batches | 2026-11-02 | Build 26200.9711 | The same, "Events and releases" | The frame present |
| An event written to the pipe by the raising thread (`ChannelBasedServerConnection::SendEventData`), not the I/O thread | 2026-11-02 | Build 26200.9711 | The same | The writing thread |

The folder has no program, so nothing here has a first uia-facts run; a run by hand adds its row.

## How to rerun

Needs cdb (the Windows SDK's, `C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe`, or `CDB` set to it) and Microsoft's symbol server. Build with `uia-facts/build.ps1`, then follow the guide.

## Debugger guide

1. Start the client: `target\debug\uia-answers-some-calls-in-the-client.exe --case operations --pause`. It prints `pause: provider is process <p>` and `pause: client is process <c>`, and waits for Enter before connecting.
2. Attach to each, in its own console: `cdb -p <p>` and `cdb -p <c>`, then in each `.sympath srv*C:\symbols*https://msdl.microsoft.com/download/symbols` and `.reload`.
3. The pipe and the events. In the client: `bp KERNELBASE!CreateNamedPipeW "du @rcx; k 8; g"`, `bp KERNELBASE!ConnectNamedPipe "k 8; g"` and `bp KERNELBASE!OpenEventW "du @r8; g"`. In the provider: `bp KERNELBASE!CreateFileW "du @rcx; g"`. Then `g` in both and press Enter in the client's console.
   - Expect, in the client: two event names `UIA_CONNECT_EVENT_<p>_<hex>` and `UIA_NOBODY_THERE_EVENT_<p>_<hex>` under `ProviderEntryPoint::ConnectUsingHwnd`; then `\\.\PIPE\UIA_PIPE_<p>_<hex>` with these frames, from the top: `KERNELBASE!CreateNamedPipeW`, `UIAutomationCore!ChannelBasedClientConnectionManager::CreatePipeToProvider`, then the connection's frames up to `UIAutomationCore!CUIAutomation::ElementFromHandle`; then `ConnectNamedPipe` on the same thread.
   - Expect, in the provider: `CreateFileW` of the same pipe name, from `ChannelBasedServerConnectionManager::OnEventSignal` under `ListenEventInfo::Service`.
4. The I/O thread's creation. Repeat from step 1 with, in the provider before the first Enter, `bp KERNELBASE!CreateRemoteThreadEx "k 10; g"`. Expect one creation, its frames from the top: `KERNELBASE!CreateRemoteThreadEx`, `KERNEL32!CreateThreadStub`, `UIAutomationCore!OverlappedIOManager::AddChannel_AnyThread`, `OverlappedIOManager::AddWorkQueueChannel`, `ChannelBasedServerConnectionManager::SetupConnectionEvents`, `UiaReturnRawElementProvider`, then the provider program's `window_procedure`. `~*k 3` afterwards shows one thread starting in `OverlappedIOManager::StaticIoThreadProc`. To count I/O threads after many connections, run any fact program's case with many clients against the same provider, such as `parallel-client-threads-gain-at-most-twice-on-one-application.exe --case processes --pause`, and `!findstack UIAutomationCore!OverlappedIOManager::IoThreadProc` in the provider: one thread matches.
5. Releases. In the provider, `bp UIAutomationCore!ReleaseCollection::DispatchReleases "k 8; g"`, then press Enter until the operation `releasing the elements of a FindAll`. Expect frames from the top: `ReleaseCollection::DispatchReleases`, `ProcessIncomingBatchedMessages`, `ChannelBasedServerConnection::OnDataIdle`, `ReadWriteChannelInfo::Service`, `OverlappedIOManager::IoThreadProc`; with the provider on its UI thread, `ComInvoker::CallTarget` above it, a cross-apartment call.
6. Events. In the provider, `bp UIAutomationCore!ChannelBasedServerConnection::SendEventData "~.; k 8; g"`, and raise an event to a registered client: the operations case's handler operation registers one on node 1; `a-registration-that-times-out-reports-success.exe --case between --pause` raises events to its own. Expect the raising thread, the provider's UI thread, under `RemoteEventCallbackProxy::CrossProcess_OnTentativeEvent`, never the I/O thread; with no handler registered, no `SendEventData`.
7. Not known: whether a second I/O thread appears (step 4's `!findstack` after every client has disconnected, or in a client process with many client objects), and where a new connection's work runs (`bp UIAutomationCore!ProviderEntryPoint::ConnectUsingHwnd "~.; k; g"` in the client and the I/O thread's frames in the provider during step 3).

The result matches when the names and frames above appear; a renamed function is a change to note, not a failure, since symbols name code, not a contract.

## Evidence kept

The original outputs stay in `experiments/parallel-uia-calls/results/` (`transport.txt`, `iothread-count-mockapp.txt`, `iothread-count-serve.txt`, `ops-event-stacks.txt`) until that folder's last facts move.
