# Requests per client operation

## Fact

Each client operation is a fixed number of requests on UIA's channel, every one read by the provider process's I/O thread (`books/dev/src/explanation/windows/uia.md`, "What one request is"): a `Current` property read 1; a cached read none; `GetRuntimeId` 1 the first time, then none; a walker step 2 (the navigation and the release of the element passed over), 4 with a cache request, and 4 to 6 to a window's root; `FindFirst` three levels down 7, one `Find` request holding all 338 of the walk's provider calls, and 9 with a cache request; `FindAll` of a 31-element subtree 34, and 84 with a cache request, 315 on a 102-node tree; `BuildUpdatedCache` 2; `ElementFromHandle` 5 with 2 `WM_GETOBJECT`s; `ElementFromPoint` and two reads 9; `GetCurrentPattern` 2; `DocumentRange` 2 and `GetText` 1; `Toggle` 12; `Invoke` 2; adding and removing a handler on one element 2 each; a remote operation 1; releases one request per provider element, 23 for 30 elements; an event's cache fetch 8. A search first reads its start's runtime id when it has not kept it, then tests the condition on the start, then its proxies' window messages, then one `Find` request inside which the provider's UI thread walks the tree, 81 of 83 navigations on a 24-node tree. Not known: where `AddFocusChangedEventHandler`'s 27 requests go, and what `GetFocusedElement` and a top-level element's next sibling reach in other processes.

## What Verbatim relies on

No code. The provider calls of Verbatim's own operations are pinned by `crates/mockapp/tests/call_counts.rs`.

## Method

A guide: requests are counted with cdb on both sides, which no program can judge. The client is `uia-answers-some-calls-in-the-client.exe --case operations --pause`, which against the provider program makes each operation once, marking it with `OutputDebugString` (`uia-facts op <name> begin` and `end`, and a mark after any setup inside the operation, such as `... pattern fetched`), and pausing for Enter before each. It also prints the provider calls each operation made, for trends. The provider program serves a 24-node tree; its node 1 is a button, node 2 a check box with the toggle pattern, node 3 a document with the text pattern, and node 21 is three levels down. Its counts differ from mockapp's where the tree differs, so a run here records them afresh, with mockapp's kept as history.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Every count in the fact, on mockapp | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "Requests per client operation" and "One call's requests", `recorded/ops-requests.txt` and `recorded/findall-requests.txt` | History: the provider program's tree differs |
| The provider calls of each operation against the provider program | 2026-11-05 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | The operations case's first run, below | Recorded for trends |

## How to rerun

Needs cdb and Microsoft's symbol server, as in [the channel guide](../the-uia-channel-and-the-provider-io-thread/readme.md).

## Debugger guide

1. Start `target\debug\uia-answers-some-calls-in-the-client.exe --case operations --pause`, and attach cdb to the provider and the client as its pause lines name them, each with Microsoft's symbols.
2. In the client: `bp UIAutomationCore!ChannelBasedClientConnection::DoCallWithReturn "r $t0 = @$t0 + 1; k 3; g"`, which counts each request and names the UIA function that sent it, and `bp KERNELBASE!OutputDebugStringW "du @rcx; .printf \"requests so far %d\\n\", @$t0; g"`. In the provider: `bp UIAutomationCore!ChannelBasedServerConnection::OnData "r $t1 = @$t1 + 1; g"` with the same marker breakpoint printing `@$t1`.
3. `g` in both, and press Enter in the client's console once for each operation. Each operation's count is the difference between the counts printed at its `begin` and `end` marks, less any setup before the operation's own mark.
4. Expect on mockapp's records: a `Current` read 1 (`GetPropertyValues`); a walker's first child 2 (`Navigate`, `Release`); a step to the window's root 4 to 6, with a `WM_GETOBJECT`; `FindFirst` 7, one of them `Find`, whose client frames are `FindingVisitor::CallRemoteAndMergeState` under `UiaNodeTraverser::Traverse`, and whose provider frames are `UiaNodeTraverser::Traverse` under `RemoteUiaNodeStub::Incoming_Find` on the provider's UI thread; `FindAll` with a cache request one `BulkFetch` per element the provider serves; `BuildUpdatedCache` 2; `ElementFromHandle` 5; `GetCurrentPattern` 2; `DocumentRange` 2 and `GetText` 1; `Toggle` 12, with a `SetFocus` first; `Invoke` 2; adding and removing a handler 2 each (`GetRuntimeId` with `EntireProcessAddEvent`, then `GetRuntimeId` with `RemoveEvent`); a remote operation 1; the release of a `FindAll`'s elements one request per element the provider serves, the provider delivering them in batches (`ProcessIncomingBatchedMessages`). The provider's count equals the client's for every operation into it.
5. The event cache fetch: register a handler with a cache request and raise its event (`a-registration-that-times-out-reports-success.exe --case between --pause`), and count the requests sent by the thread starting in `WorkItemQueue::StaticThreadProc` before the handler runs: 8 on mockapp for one selection event.
6. Not known: `AddFocusChangedEventHandler`'s requests to other processes and `GetFocusedElement`'s and a top-level element's next sibling's. Attach cdb to the client only, with step 2's breakpoint extended to print the pipe's peer (`!handle` of the handle `WriteFile` writes to, under `ReadWriteChannelInfo::BlockingWrite`), and compare the process ids in the pipe names.

The result matches when each count equals the record for the same tree; where the provider program's tree gives another count, the run records it.

## Evidence kept

The original outputs: `recorded/ops-requests.txt`, `recorded/findall-requests.txt` and `recorded/ops-event-stacks.txt`.
