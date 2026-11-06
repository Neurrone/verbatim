# A process that used UIA can hang or crash as it exits

## Fact

A process that has connected to UIA providers as a client sometimes hangs at full processor use, or crashes with an access violation, as it exits normally and runs its DLLs' process-detach code: `UIAutomationCore.dll`'s detach code destroys its telemetry object and walks a corrupt list of per-provider connection records. It happened in about 1 to 3 percent of runs of the arbitration tests.

The list is corrupted while the process runs, by two client threads connecting to different provider processes at once. The telemetry object keeps a `std::unordered_map` from the provider's process id to a record, and nothing locks it: each new connection inserts a record on the thread that connects (`TelemetryUtility::StartConnection`, from `ChannelBasedClientConnectionManager::CreatePipeToProvider`), and UIA's I/O thread erases it when the provider's end of the pipe closes (`TelemetryUtility::CloseConnection`, from `ChannelBasedClientConnection::OnRemoteCloseOrError`). Two threads inserting at once both read the list's last node before either links its own, so one record is linked from the list head's back link but not from the forward chain, and the size count can lose an update. At exit the destructor sets the next link of the node the head's back link names to null and frees nodes forward until it meets a null. As its code reads, when that node is not the forward chain's last node, the walk passes the head, frees it, and goes on through freed memory: it loops without end or reads a freed page. Erasing records as providers go can repair the links or not, so whether a corrupted list faults depends on what is left at exit.

With connections from one thread, from two threads to two provider processes, or from six threads to one provider process, none of 230 children faulted; with three or more threads, each connecting to provider processes of its own, 1 to 5 percent of children faulted, even when each only read a window's name. Making the connections one at a time, behind a lock of the process's own, prevented it: none of 1,050 children faulted. A proper teardown before the exit (every handler removed, every UIA object released and COM left on every thread) does not prevent it, and neither does leaving the providers running through the exit (`books/dev/src/reference/uia-bugs.md`, the fault of that name).

## What Verbatim relies on

Its processes end themselves with `TerminateProcess` once shut down, so the detach code never runs: outposts and the listener (`books/dev/src/explanation/architecture.md`, section 1), and the mockapp test runner (`crates/mockapp/tests/common/harness.rs`). A proper teardown does not prevent the fault. Making connections one at a time does, but would put one lock across every thread that connects, so ending without the detach code is what avoids it.

## Method

Each child process (the `exiter` role) makes UIA's first-time setup on its main thread, starts provider programs of its own and, from threads of its own, connects as a UIA client to their windows and reads each window's name, as its setup says. It then ends its providers, as the mockapp tests' providers ended before their test process, and ends normally with `std::process::exit`, running every DLL's detach code. Children run one after another. Only children that report their work done count. A child that has not ended 3 s after its work is a hang, since the detach code spins without end once it hangs, and one that ends with an access violation a crash; a child that fails in any other way makes the claim inconclusive, with its outcome among the evidence.

- `exit`: 150 children (Dickson, 2026-11-06) of the setup with the most faults a minute among the candidates: four provider programs with a UIA window each, a thread each, connecting and reading the name, the clients, elements and cache requests left in place. Judged as a rate against that setup's 8 of 230; 90 children would already make none differ.
- `candidates`, run only when named, 230 children of each setup: the first build's, six provider programs of three windows each (a UIA window, one with its own `IAccessible`, a plain one), a thread each, every connection reading, searching its subtree and registering a name-change handler; then that setup with one step removed or changed at a time, and smaller and larger ones. 230 children show a setup with none below the first build's rate of about 1 in 75. The rate and the faults a minute are printed as new.
- `clean`, run only when named: `exit`'s setup torn down before the exit: each thread removes its handlers, releases every element, cache request, handler and client and leaves COM, and the main thread does the same, the providers ended after the teardown in one run and before it in another, 120 children each, at which none separates the clean rate from 8 of 230 with a margin (0.965 to the 120th power is about 0.014). Judged against the dirty rate: differ when apart from it, pass when no difference shows.
- `serial`, run only when named: `exit`'s setup, each thread holding a lock of the child's own while it connects and reads, so no two connections are made at once; 150 children. Its record is a rate apart from 8 of 230, which none in 150 shows (0.965 to the 150th power is below 0.005): pass when apart, differ when no difference shows.
- `providers-alive`, run only when named: `exit`'s setup with the providers left running through the exit, so UIA erases no record before it; their jobs end them once the child has ended. 150 children, judged as `clean` is.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both; `serial` and `providers-alive` were checked the same way the same day).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| About 1 to 3 percent of the arbitration tests' runs; 2 hangs in 100 runs after waiting 12 s for the providers to go; 1 in 40 before the first-time setup fix and 1 in 100 after; six threads each with a mockapp of its own 1 in 40, one thread 0 in 40 | 2026-10-03 | Build 26200 | Commits 1f69859e and fed532be | A rate, 1 of 100 |
| First uia-facts run, `exit`: 0 of 150 by default and with both settings; 1 of 150, a crash, with `--recovery` and with `--coalescing` | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Inconclusive with none, pass with one, against 1 in 100 |
| `exit`, the children one after another, only access violations and hangs counted: 2 of 150, both hangs | 2026-11-06 | As above | 93fed1a0 | Pass |
| `candidates`, 230 children each, a 10 s hang limit: the first build's setup 6 (3.4 faults a minute); no search 6; no registration 8; connect and read only 6; one thread 0; the UIA window alone 11; one provider and one thread 0; connect and read the UIA window alone 8 (5.9 a minute); the same with two providers and threads 0; one provider with six UIA windows and six threads 0; one provider's window with six threads each to it 0; four providers and threads 8 (6.6 a minute); six providers and three threads 3; twelve providers and threads 5 | 2026-11-06 | As above | cdb6b4d6 | New; four providers and threads chosen |
| `exit` with four providers and threads: 2 of 150, both hangs, in 22 s | 2026-11-06 | As above | cdb6b4d6 | Pass against 8 of 230 |
| `clean`: the providers ended after the teardown 2 of 90, before it 3 of 90, all hangs | 2026-11-06 | As above | cdb6b4d6 | Pass: no difference from the dirty rate |
| `clean` at 120 children: the providers ended after the teardown 5 of 120 (4 hung, 1 access violation), before it 5 of 120, all hangs | 2026-11-06 | As above | f0b18b2a | Pass: no difference from the dirty rate |
| `serial`: 0 of 150 in each of seven runs, by default three times more and once with each client setting; `exit` in the same first run 2 of 150 | 2026-11-06 | As above | This commit | Prevents it: apart from 8 of 230 |
| `providers-alive`: 2, 13, 4, 9, 4 and 4 of 150 in the same six runs, hangs and access violations both | 2026-11-06 | As above | This commit | No difference shown; one run of 13 inconclusive |
| Under cdb with full page heap, 150 children of `exit`'s setup: two or three client threads inside `StartConnection` at once in most children; after it, the forward chain shorter than the size count, or the head's back link not naming the chain's last node, in 6 of them (children 9, 16, 69, 78, 81 and 100); none faulted at exit, since the providers' erases left each list consistent first | 2026-11-06 | As above | This commit | The mechanism |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-process-that-used-uia-can-hang-or-crash-as-it-exits.exe`, which runs `exit`; `--case candidates` takes about 20 minutes, `--case clean` about 35 seconds, and `--case serial` and `--case providers-alive` under a minute each. No desktop or application.

## Debugger guide

What it shows: the client threads that insert into the telemetry map at once, the list each insert and erase leaves, and the list the destructor walks. A child runs alone with `--role exiter` and the setup's words: providers, windows, every window a UIA window, threads, every thread to every window, search, register, serial, and the end (`dirty`, `clean-after`, `clean-before` or `providers-alive`). `exit`'s setup is `4 1 0 4 0 0 0 0 dirty`.

1. Fill the symbol cache first (`books/dev/src/how-to/debugging.md`, "Filling the cache without freezing anything"). The offsets below are for `UIAutomationCore.dll` 7.2.26100.9549; after an update, find them again with `uf UIAutomationCore!TelemetryUtility::StartConnection` and `uf UIAutomationCore!TelemetryUtility::CloseConnection` in a `-z` session, which give each function's address and its `ret`; the destructor is `TelemetryUtility::'scalar deleting destructor'`, found with `x UIAutomationCore!*Telemetry*`.
2. Start the child under cdb, breaking at process creation: `cdb -xe cpr -y <symbol path> -c "$<commands.txt" target\debug\a-process-that-used-uia-can-hang-or-crash-as-it-exits.exe --role exiter 4 1 0 4 0 0 0 0 dirty`.
3. Full page heap without `gflags`, which needs an elevated shell: at the creation break, `ed @$peb+bc 02000000` (the heap's page-allocation flag in the PEB's global flags), `sxe ld:verifier.dll`, `g`; at that load, `.reload /f verifier.dll`, `ed verifier!AVrfDphGlobalFlags 3`, `sxd ld`, `g`. cdb prints `page heap enabled with flags 0x3` before the initial breakpoint. With an elevated shell, `gflags /p /enable a-process-that-used-uia-can-hang-or-crash-as-it-exits.exe /full` does the same for every run, and `gflags /p /disable` with the same name undoes it.
4. Set the breakpoints when `UIAutomationCore.dll` loads (`sxe -c "$$<bps.txt; gc" ld:UIAutomationCore.dll`), each printing and going on with `gc`:
   - `UIAutomationCore+dfdd8`, `StartConnection`'s entry: `@$tid` and the key, the provider's process id, at `poi(@rsp+28)`.
   - `UIAutomationCore+dff82`, its `ret`, and `UIAutomationCore+2379a1`, `CloseConnection`'s: walk the list. The object is `poi(UIAutomationCore!TelemetryUtility::m_instance)`; at offset 0x18 is the list head's address and at 0x20 the size count. A node is its next link, then its back link, then the key. Walk from the head's next link to the head, checking each node's back link names the node before it and the head's back link names the last, and count the nodes against the size.
   - `UIAutomationCore+237858`, `CloseConnection`'s entry: the key in `@r8`.
   - `UIAutomationCore+237828`, the telemetry object's destructor: print the head's links and the nodes, at most a dozen.
   - `sxe -c "..." av`: on an access violation, `k 30`, the list, and `!ext.heap -p -a` on each register that holds a node's address, which names the stacks that allocated and freed it.
5. Run each child with a limit (25 s did) and kill cdb past it: a hung child spins in the destructor.

The stacks to expect, innermost first:

- An insert, on a client thread: `TelemetryUtility::StartConnection`, `ChannelBasedClientConnectionManager::CreatePipeToProvider`, `ChannelBasedClientConnectionManager::CreateLowBoxChannelBasedClientConnection`, `ChannelBasedClientConnectionManager::CreateChannelBasedConnection`, `ProviderEntryPoint::GetUiaNode`, `ProviderEntryPoint::GetNativeUiaNode`, `UiaNodeFactory::CompleteNodeInfo`, `UiaNodeFactory::FromPartialNodeInfo`, `UiaNodeFactory::FromProviderEntryPoint`, `CUIAutomation::GetNodeFromFactory`, `CUIAutomation::ElementFromHandle`, then the program's `use_uia`.
- An erase, on UIA's I/O thread: `TelemetryUtility::CloseConnection`, `ChannelBasedClientConnection::OnRemoteCloseOrError`, `ReadWriteChannelInfo::Service`, `OverlappedIOManager::IoThreadProc`, `OverlappedIOManager::StaticIoThreadProc`.
- The walk, on the exiting thread: `TelemetryUtility::scalar deleting destructor`, `std::_Ref_count<TelemetryUtility>::_Destroy`, `std::_Ref_count_base::_Decref`, the dynamic atexit destructor for `TelemetryUtility::m_instance`, `ucrtbase!execute_onexit_table`, `dllmain_crt_process_detach`, `dllmain_dispatch`, `ntdll!LdrShutdownProcess`, `ntdll!RtlExitUserProcess`, `KERNEL32!ExitProcessImplementation`, then the program's `std::process::exit`.

The result matches when a second `StartConnection` entry comes on another thread before the first one's `ret`, and a walk after the inserts finds fewer nodes than the size count, or the head's back link naming a node that is not the chain's last. In the record above that happened in 6 of 150 children. With `serial` set (`4 1 0 4 0 0 0 1 dirty`), in 20 children under cdb on 2026-11-06, each of the 80 inserts began after the previous one's `ret`, and no walk was short.

## Evidence kept

None: commits 1f69859e and fed532be.
