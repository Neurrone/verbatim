# Windows IPC primitives for screen reader work

Verbatim's processes talk over named pipes and (planned, for audio) shared
memory; NVDA's injected helper talks to its core over MS-RPC. This file
explains those primitives plus the kernel synchronization objects they rely
on, for a reader who knows POSIX equivalents.

## Kernel objects, handles, and waiting

Almost every primitive here is a *kernel object* referenced through a
`HANDLE` (per-process, like a file descriptor). Handles are closed with
`CloseHandle`, duplicated cross-process with `DuplicateHandle`, and many
object types can carry a *name* in a global namespace (`Local\Foo` per
session, `Global\Foo` machine-wide) so unrelated processes can open the same
object — names plus a security descriptor are the Windows answer to
filesystem paths for POSIX shared primitives.

The unifying feature POSIX lacks: nearly everything is *waitable* with one
family of calls. `WaitForSingleObject` / `WaitForMultipleObjects` block (with
timeout) until an object is signaled — an event is set, a mutex is free, a
process or thread has exited, a waitable timer fires. Waiting on up to 64
heterogeneous objects at once replaces `select`-style multiplexing for
non-I/O concerns, and "wait on {operation done, cancel event}" is the
canonical cancellable-blocking-call shape (NVDA's watchdog is built from
exactly this plus waitable timers).

The objects themselves:

- **Event**: a boolean flag; `SetEvent`/`ResetEvent`; manual-reset (stays
  signaled, releases all waiters) or auto-reset (releases one waiter and
  clears). The cross-process condition-variable substitute.
- **Mutex**: cross-process lock; a crashed owner leaves it *abandoned*,
  which waiters see as a distinct result — robust-lock semantics for free.
- **Semaphore**, **Waitable timer** (absolute or relative due time, used by
  NVDA's watchdog heartbeat).
- **Process and thread handles**: signaled on exit; `WaitForSingleObject` on
  a process handle is `waitpid`.
- **Job object**: groups processes; kill-on-close-of-last-handle
  (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) guarantees children die with the
  parent — the mechanism a supervisor uses so no orphan outposts or
  synth hosts survive a crash. Verbatim's architecture leans on this.

## Named pipes

`\\.\pipe\name` — the standard Windows local RPC substrate and Verbatim's
control-plane transport. Byte- or message-oriented duplex channels with
server/client roles:

- The server calls `CreateNamedPipe` (choosing message vs. byte mode,
  instance count) then `ConnectNamedPipe` per client; clients just
  `CreateFile` the path. Each connected client gets its *own instance* —
  the same path multiplexes.
- A *security descriptor at creation* controls who may connect — the
  access-control story is the pipe's, not invented by the protocol above
  it. Additionally `GetNamedPipeClientProcessId` identifies the peer.
- I/O is ordinary `ReadFile`/`WriteFile`, so all Windows async I/O styles
  apply: blocking calls on dedicated threads (simplest, what most Rust
  code does), OVERLAPPED with events, or completion ports. A blocking read
  with no writer parks the thread — pair every blocking pipe thread with a
  shutdown signal, or close the handle to unstick it (closing a handle
  another thread is blocked on aborts that operation with an error — the
  standard unblocking idiom, used deliberately by Verbatim's agent relay).
- Message mode preserves write boundaries (a read returns exactly one
  written message), removing the length-prefix framing chore byte streams
  need — but interoperates poorly with generic byte-stream abstractions,
  so protocol code written against generic readers (as Verbatim's JSON
  lines control plane is) typically uses byte mode and frames itself.

Pipes are also reachable over SMB (`\\server\pipe\...`); a local-only
service must not rely on obscurity — bind ACLs accordingly (Verbatim's
control plane deliberately never listens remotely and leaves remoting to an
explicit relay).

## File mappings (shared memory)

`CreateFileMapping(INVALID_HANDLE_VALUE, …)` creates a pagefile-backed
shared memory section, optionally named; `MapViewOfFile` maps it. Cross-
process sharing is by name or by duplicated handle. This is the bulk-data
channel: zero-copy, no syscall per access, and the natural transport for
audio sample streams (Verbatim's planned synth-host-to-core PCM path) or
large snapshots. What it does not give you: synchronization (pair it with
events or a lock-free ring protocol) and lifetime tracking (the section
lives while any handle or view exists). Cross-architecture note for an
x64/ARM64 project: layout in shared memory is ABI — fix sizes, alignment,
and endianness explicitly and never share raw Rust structs without
`#[repr(C)]` discipline.

## MS-RPC

MSRPC is DCE RPC, the machinery COM marshaling is built on, usable directly:
define interfaces in IDL, `midl` generates client stubs and server
dispatch, transports include `ncalrpc` (local, what NVDA uses between its
injected in-process code and the NVDA process) and named pipes. Calls are
synchronous procedure calls with real argument marshaling; the server
registers endpoints and serves calls on a thread pool. You will meet MSRPC
when reading NVDA ([Process injection](../nvda/process-injection.md)) — its injected DLL both
*serves* an RPC interface (so NVDA can call into app processes) and *calls
back* into NVDA (so in-process code can push events out). Verbatim does not
use raw MSRPC; its equivalent channel is the outpost protocol over pipes.

## COM as IPC

Remember ([COM](com.md)) that any cross-process COM interface is itself an IPC
channel with RPC underneath — when you hold an `IAccessible` into another
process, you are doing RPC with every property read. The practical
difference from the primitives above is that you control neither the
threading (STA delivery) nor the timeout (none by default), which is the
root of the hang analysis in [Main loop and watchdog](../nvda/main-loop-and-watchdog.md).

## References

- [Interprocess Communications overview (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/ipc/interprocess-communications)
- [Synchronization Objects (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/sync/synchronization-objects)
- [Named Pipes (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes)
- [File Mapping (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/memory/file-mapping)
- [Job Objects (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [RPC Start Page (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/rpc/rpc-start-page)
