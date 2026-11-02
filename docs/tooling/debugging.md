# Debugging other processes with cdb

How to take stacks from, and set breakpoints in, a running process on this
machine without freezing anything that matters. The worked examples are the
scripts of `experiments/parallel-uia-calls/`.

## Where cdb is

WinDbg is installed from winget (`Microsoft.WinDbg`). Its command-line
debugger is reached through app execution aliases in
`%LOCALAPPDATA%\Microsoft\WindowsApps`: `cdbX64.exe`, `cdbARM64.exe` and
`cdbX86.exe`, with `WinDbgX.exe` for the window. The binary itself is
`amd64\cdb.exe` under `(Get-AppxPackage Microsoft.WinDbg).InstallLocation`.
Use the debugger of the target's architecture, `cdbX64.exe` on this machine.

## Symbols

Use this symbol path, with `-y`:

```
srv*<repository>\target\symbols*https://msdl.microsoft.com/download/symbols
```

`target\symbols` is the cache `cargo xtask trace --symbol-server` also uses.
Add your own builds' PDB folders after a semicolon, such as mockapp's
`target\debug`. Windows DLLs then show real function names
(`UIAutomationCore`, `combase`, `oleacc`, `user32`); a program without public
symbols, such as Notepad or VS Code, shows module plus offset.

## Filling the cache without freezing anything

A first attach downloads a PDB for every module it walks, and the target is
frozen while it does: filling the cache for about 30 modules took 140
seconds. So fill it first, in one of two ways that freeze nothing that
matters:

- Open a DLL as a dump file, which runs no process:
  `cdbX64.exe -z C:\Windows\System32\UIAutomationCore.dll -y <path> -c ".reload /f UIAutomationCore.dll; q"`.
  The same form, with `x UIAutomationCore!*FindAll*`, looks up the names to
  set breakpoints on.
- Attach to a throwaway process you started, which loads the same DLLs, and
  run `.reload /f; qd`. Make one UIA call into it first, so the modules
  the call path loads are loaded too, and add `~*k` so every module on a
  stack is fetched.

`.reload /f <image>` takes the image's file name, dots included
(`.reload /f Microsoft.Terminal.Control.dll`); the module name
(`Microsoft_Terminal_Control`) loads nothing and leaves exports only.
Windows Terminal's binaries have public symbols on Microsoft's server.

Fill the cache before any attach whose breakpoints print stacks early: a
stack printed during an application's first `WM_GETOBJECT` fetched symbols
for every module on it, and the client's `ElementFromHandle` failed with
`UIA_E_TIMEOUT` after the 2-second connection timeout.

## Non-invasive and invasive attach

- Non-invasive, `-pv -p <pid>`: suspends every thread of the target from
  attach until `qd`. It allows no breakpoints. Use it for one snapshot of
  every stack, and keep it to seconds.
- Invasive, `-p <pid>`: attaches as the process's debugger. The attach
  stops every thread until the first `g`, and each breakpoint hit stops them
  again while its commands run. Breakpoints work. Detach with `qd`, which
  leaves the process running; `q`, or killing cdb, ends the target. A process
  has at most one invasive debugger.

An invasive attach can change what the target does: under it, every UIA call
into VS Code failed with `UIA_E_ELEMENTNOTAVAILABLE`, though the same calls
answered without the debugger. It also changes how UIA's client waits for a
reply: under it a call waited once for its whole transaction timeout, without
it in steps of about 110 ms that add a tenth to the timeout
(`experiments/parallel-uia-calls/`, "Timeouts"). Check a result with and
without the debugger, and time timeouts without one.

The attach also delays what comes after it, and the delay alone can change
what UIA's client sends. A search that began 0.3 s after its root element
was fetched sent no `GetRuntimeId`. One that began after an attach, 1.6 to
3 seconds after the fetch, sent it, and the search's result changed with
it. So attach before the client fetches the elements it will use: start
the target, attach, then start the client.

## Never attach to these

- The shell's explorer.exe: it owns the taskbar, and a first non-invasive
  attach that downloaded symbols froze it for eight minutes.
- Windows Terminal while it holds another session's work. Copy the
  end-to-end harness's portable release (`target\e2e-stage`) to a folder
  of your own and start it there: it runs as a process of its own, since
  an unpackaged Windows Terminal's single-instance identity comes from its
  executable's path (`crates/verbatim-e2e/src/windows_terminal.rs`).
- Anything you did not start: Dickson's applications, NVDA, a running
  Verbatim of another session. Start your own instance instead, with a
  profile or data folder of its own where the program allows one (VS Code's
  `--user-data-dir` and `--extensions-dir`).

## Breakpoints that count

A breakpoint that prints a marker and goes on (`gc`) counts calls without
stopping the target for long. Log with `-logo <file>`, put the commands in a
file passed with `-cf <file>` that ends with `g`, and count the markers in
the log afterwards:

```
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; gc"
bm /a mockapp!mockapp::uia::handler::*Navigate ".echo PROV_NAV; gc"
bp UIAutomationCore!ComInvoker::CallTarget "r $t0=@$t0+1; .if (@$t0==1) {~.; k 30}; gc"
bp USER32!DefWindowProcW ".if (@edx == 0xd) {.echo WM_GETTEXT; k 20}; gc"
g
```

- `bm` sets one breakpoint for every function a pattern matches; `/a` defers
  ones whose module has not loaded.
- `$t0` to `$t19` are counters. The `.if` above prints a stack on the first
  hit only.
- On x64 the first four arguments are in `rcx`, `rdx`, `r8` and `r9`, so a
  window procedure's message is `@edx`. At a function's first instruction
  the fifth and sixth are `dwo(@rsp+28)` and `dwo(@rsp+30)`, such as
  `SendMessageTimeoutW`'s flags and timeout.
- `bp <function> ".echo F; bp /1 @$ra \".echo F_RET; r eax; gc\"; gc"`
  prints a function's return value: the one-shot breakpoint at its return
  address hits on return, even when other breakpoints hit inside the
  function.
- `bp KERNELBASE!CreateRemoteThreadEx` with `k` prints the stack that
  creates each thread, such as the one that created UIA's I/O thread.
- `.printf "%y
", @rip` names the function a `bm` breakpoint hit, so one
  pattern counts several functions apart.
- `!handle @rcx f` inside a breakpoint prints the type and name of the
  handle a call such as `KERNELBASE!ReadFile` was given, and `du @rcx` the
  file name `CreateFileW` or `CreateNamedPipeW` was given.
- To split one log by operation, have the program you drive call
  `OutputDebugString` before each operation: cdb writes the string into the
  log of the process it is attached to. For the other process, break on
  something the driver does between operations, such as a window message it
  sends (`.if (@edx == 0x8003) {.echo FENCE}`), and count the markers.
- `.echotime` prints the debugger's clock, local time to the millisecond,
  to line hits up with times a client printed.
- Symbols of a Rust release build are mangled in the v0 scheme, such as
  `RNvNtCsfc8R8NVfkqM_4puia5serve7wndproc`, and `x` and `bm` match the
  mangled form: `bm puia!RNvNtCs*_4puia5serve7wndproc`.
- A Rust function in a release build needs `#[inline(never)]` to keep a
  symbol, which may carry a suffix (`name_read.llvm.<n>`), so match it with
  a wildcard in `bm`. The PDB is named after the crate, so a copy of the
  program under another name needs the build's folder in the symbol path.
- Each hit stops the target while its commands run, so timings under
  breakpoints are the debugger's: a mockapp FindAll that takes 7 ms took 1.9
  seconds with about 400 hits printing markers and short stacks. Print long
  stacks for a few hits, not every one.

## Taking every stack

- `~*k 40`: every thread's stack, 40 frames deep.
- `~.`: the current thread's number, id and name, such as `CrBrowserMain`.
- `k 30`: the current thread's stack.

`experiments/parallel-uia-calls/stacks.ps1` takes a non-invasive `~*k` while
calls wait on a suspended thread.

## Without a debugger

- `puia threads pid=<pid>` (`experiments/parallel-uia-calls/`) lists a
  process's threads with their start addresses as module and offset, and
  the processor cycles each has used, with no attach. UIA's I/O thread is
  the one that starts in `UIAutomationCore`'s
  `OverlappedIOManager::StaticIoThreadProc`; `ln UIAutomationCore+<offset>`
  in a `-z` session names an offset.
- A context-switch trace (`cargo xtask trace start` and `stop`, then
  `cargo xtask trace waits <trace> --symbol-server`; `measuring.md`, "The
  trace commands themselves") shows which thread runs a provider's code,
  from CPU samples with stacks (`--samples <image>:#<thread id>`), and each
  hand-off of one call between the client, UIA's I/O threads and the UI
  thread (`--timeline`), without stopping the target, and
  `--blocked <image>:#<thread id> <ms>` lists every wait of one thread
  with the stack it waited on and what ended it, such as a timer's expiry.
  With `<image>:*` and 0 ms it counts a provider's requests with no
  debugger: each wake of UIA's I/O thread from its idle wait is readied by
  the client's `WriteFile`, with the client's frame that sent the request
  (`LocalUiaNodeProxy::RealCrossProcess_Find`, for one). The client's own
  waits show which frames sent each request and how long each waited.
  `experiments/parallel-uia-calls/blocked-summary.py` prints one line per
  wait.
  Use them where an invasive attach changes the result: under one, calls into Windows
  Terminal failed with `UIA_E_ELEMENTNOTAVAILABLE`, as calls into VS Code
  did, though they reached its provider.

## Detaching cleanly from a script

A script cannot press Ctrl+Break. Break in with `DebugBreakProcess` on the
target, then write `qd` to cdb's standard input. A standard input redirected
from Windows PowerShell begins with a byte order mark that cdb rejects as a
command, so send an empty line first. `experiments/parallel-uia-calls/dbg.psm1`
does all of this. The same byte order mark begins the first line of any
child program's redirected standard input, so send such a program an empty
line first too. Afterwards check that the target still runs, and stop every
process you started.
