# UIA facts

Each folder here is one fact about UI Automation that Verbatim relies on, from `books/dev/src/reference/uia-bugs.md` (the faults and limitations) or `books/dev/src/explanation/windows/uia.md` (the model of how UIA works), with what reproduces it: a program that runs the fact and judges its outcome against the recorded figures, a guide for watching it under a debugger, or a mockapp test that `cargo xtask ci` already runs. A folder is named after its fact, the heading in `uia-bugs.md` where there is one, so a citation can be read across.

They are run by hand on Dickson's machine, before and after a Windows update, to learn whether a fact Verbatim relies on still holds. Each program takes over no desktop unless its folder says so, but it starts processes and holds them for seconds at a time.

## Building and running

`uia-facts/build.ps1` builds everything, locally and in `.github/workflows/uia-facts.yml`:

1. It copies the root `Cargo.lock` to `uia-facts/Cargo.lock` (which git ignores) and runs `cargo metadata`, which keeps every locked version and drops the entries this workspace does not use. Every later step is `--locked`, so a re-resolution fails rather than building a second version of a crate. On the first build the derived lock held 196 of the main lock's 276 entries: the 93 this workspace does not use were dropped, the 13 uia-facts members added, and no version changed.
2. It builds `check-copies` alone and runs it (below), before the full build, so a drifted copy fails before a second `windows` is built. It depends on the `workspace-hack` crate, so the `toml` it builds is the one the full build reuses.
3. It runs `cargo build --locked` in `uia-facts/`, which builds the provider program, every fact program and `check-copies` into the main `target/debug`.

Then run a fact's program from `target/debug`, for example `target\debug\nothing-cancels-a-uia-call.exe`. Every program takes:

- `--arm <name>`, repeatable: run only the named arms, which its folder lists.
- `--trials <n>`: the trials of a rate, in place of the recorded number.
- `--pause`: before each arm's calls, print each process id and wait for Enter, so a debugger can attach (the debugger guides use it).

The workflow builds and lints on x64 and ARM64 and runs no fact program, because each one judges the machine it runs on.

## The workspace

`uia-facts/` is a Cargo workspace of its own, which the root `Cargo.toml` excludes, so it never joins the main build or `cargo xtask ci`. It depends on `crates/verbatim-uia` and `crates/verbatim-uia-rops` by path, so the facts call UIA through Verbatim's own code where Verbatim's own code is what they check, and it is GPL-3.0-or-later for that reason. Its code is original; an NVDA figure is cited as NVDA's record, never reproduced from NVDA's source.

It builds into the main `target/` (`uia-facts/.cargo/config.toml`), so third-party crates the main workspace built, `windows` above all, are reused rather than built again. That holds only while the two workspaces ask for those crates alike, and Cargo cannot share `[workspace.dependencies]`, `[workspace.lints]` or a profile between workspaces, so `uia-facts/Cargo.toml` copies them. `check-copies` fails when the copies drift: the windows family's entries, the lints, `[profile.dev]`, any package whose version or source differs from the main lock's, and the top-level `env` of the two workflows, which the build cache's key hashes. The eight path crates (`verbatim-model`, `verbatim-text`, `verbatim-core`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-uia`, `verbatim-uia-rops` and `workspace-hack`) build once more for uia-facts, since a path crate's unit hash includes its path from the workspace root. A uia-facts build and a main build in the same target wait for each other on Cargo's build lock.

## The provider program

`uia-facts/provider` is the facts' own UIA and MSAA provider; no fact launches mockapp, whose facts stay mockapp tests. A fact starts it from its own folder, so it always runs against the provider built with it, and drives it with one request a line on its standard input, each answered once the provider has acted on it. Its windows are hidden top-level windows on chosen UI threads: UIA windows serving a fragment tree of set size (24 nodes by default), with or without `UseComThreading`, a window with its own `IAccessible`, and plain Win32 windows. It can hold any provider method, make every call slow while its thread keeps pumping, keep a UI thread busy, wait in `CoWaitForMultipleHandles`, raise events on request or at a rate, move its tree's focus, and log every provider call and the window messages UIA sends. Its module documentation, `uia-facts/provider/src/main.rs`, lists its command line and requests.

A figure measured on mockapp whose value depends on the provider's tree, such as a count of provider calls, is recorded again on the provider program's first run, with the mockapp figure kept as history.

## Output and verdicts

Each program prints a header (the date, the Windows build with its revision, `UIAutomationCore.dll`'s version, the processor and the commit), then one line per claim with what it observed, the record, and a verdict. It exits with 0 when every judged claim passes, 1 when any differs, and 2 when none differs but some are inconclusive.

- A deterministic outcome (an HRESULT, a count of provider calls, delivered or not, an order) passes when every run equals the record, differs when the runs agree on another value, and is inconclusive when they disagree. It runs 3 times unless its folder says otherwise.
- A relation (A answers while B waits, B waits behind A, a speed-up below a bound computed in the same run) passes when it holds in every run, differs when it holds in none, and is inconclusive otherwise. The durations around it are printed for trends, not judged, since they depend on the machine.
- A band, only where the figure is a timeout UIA sets: from the timeout T to 1.15 times T plus 50 ms for one wait, and the sum of those for waits in turn. It passes when all 5 trials lie inside, differs when the median lies outside, and is inconclusive otherwise.
- A rate (k of n against a recorded k of n) passes when the recorded rate lies inside the observed 95 percent Wilson interval, differs when the two intervals do not overlap, and is inconclusive otherwise; with no occurrence it differs only when the recorded rate would almost surely have shown one.
- An open question prints its answer as new; its folder records the first run's answer.
- A missing precondition, such as a locked desktop, makes its claims inconclusive with the reason; nothing takes another path.

## The facts

| Folder | What it reproduces | Kind |
|---|---|---|
| [a-new-connection-to-a-stuck-application-times-out](a-new-connection-to-a-stuck-application-times-out/readme.md) | `uia.md`, "Making a connection": the connection timeout's failure | Program |
| [the-connection-timeout-applies-to-the-whole-process](the-connection-timeout-applies-to-the-whole-process/readme.md) | `uia-bugs.md`, the fault of that name | Kept CI test and program |
| [a-call-waits-its-timeout-from-its-own-start](a-call-waits-its-timeout-from-its-own-start/readme.md) | `uia.md`, "Timeouts and cancelling": how a call waits | Guide, program and kept CI test |
| [a-timeout-bounds-each-request-and-window-message-not-an-operation](a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md) | `uia-bugs.md`, the limitation of that name | Program |
| [a-registration-that-times-out-reports-success](a-registration-that-times-out-reports-success/readme.md) | `uia-bugs.md`, the fault of that name | Kept CI test and program |
| [nothing-cancels-a-uia-call](nothing-cancels-a-uia-call/readme.md) | `uia.md`, "The parts" and "Timeouts and cancelling" | Program and debugger guide |
| [a-provider-process-serves-one-uia-request-at-a-time](a-provider-process-serves-one-uia-request-at-a-time/readme.md) | `uia.md`, "Inside the provider process" and "Where the provider call runs" | Program |
| [the-uia-channel-and-the-provider-io-thread](the-uia-channel-and-the-provider-io-thread/readme.md) | `uia.md`, "The parts" and "Inside the provider process": the internals | Guide |
| [msaa-calls-do-not-pass-through-uias-request-reader](msaa-calls-do-not-pass-through-uias-request-reader/readme.md) | `uia.md`, "MSAA beside UIA" | Program |
| [parallel-client-threads-gain-at-most-twice-on-one-application](parallel-client-threads-gain-at-most-twice-on-one-application/readme.md) | `uia.md`, "Where the provider call runs" and "Calls from the client" | Program and debugger guide |
| [requests-per-client-operation](requests-per-client-operation/readme.md) | `uia.md`, "What one request is" | Guide |
| [uia-answers-some-calls-in-the-client](uia-answers-some-calls-in-the-client/readme.md) | `uia.md`, "Making a connection", what UIA answers in the client; `uia-bugs.md`, "UI Automation's copy of an element's runtime id lapses" | Kept CI test and program |
| [a-remote-operation-stops-after-exactly-10000-instructions](a-remote-operation-stops-after-exactly-10000-instructions/readme.md) | `uia.md`, "What one request is": the instruction limit | Kept CI test |

## Cited, not reproduced

These claims link their source instead of a reproduction.

| Claim | Where it is stated | Its source |
|---|---|---|
| What `WM_GETOBJECT`, `UiaReturnRawElementProvider`, the two timeouts, `ProviderOptions`, caching, remote operations, `AddFocusChangedEventHandler`'s desktop-wide scope, `IUIAutomation6`, the threading advice, `CoCancelCall` and `IsHungAppWindow` are documented to do | `uia.md`, throughout | The Microsoft Learn pages in `uia.md`'s "References" |
| Which frameworks declare `UseComThreading`, and where their provider calls run: Windows Forms, Qt, Chromium, WinUI 3, Firefox, JavaFX, Flutter, WPF, Avalonia, the console host, Windows Terminal and AccessKit | `uia.md`, "Where the provider call runs" | The framework source named in `uia.md`'s "References"; WPF's measured hold is a measurement, not cited |
| An ancestor-scoped cache request is not allowed | `uia-bugs.md`, "Ancestors cannot be cached or listened to" | [IUIAutomationCacheRequest::TreeScope](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope) |
| NVDA's issues and their figures: #20654, #20169 and #20170, #6533, #7345 and #8539, #11002, #14888, #15850, #17602, #20608, #20168 | `uia.md`, "Case study: NVDA" | NVDA's issue tracker, linked from the case study |
| A Qt application's cached keyboard focus can be stale | `uia-bugs.md`, the entry of that name | [NVDA #20764](https://github.com/nvaccess/nvda/issues/20764) |
| Excel 2016 freezes when asked for its provider in response to its own events | `uia-bugs.md`, the entry of that name | NVDA's source comment the entry cites |
| `conhost_two_windows`'s inference about `UiaHasServerSideProvider` | `uia-bugs.md`, "UiaHasServerSideProvider says no for a new conhost window while another process has an event handler" | Its kept runs in `experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/` (excluded by Dickson, 2026-11-04) |
| NVDA's exposure: #7345 and #8539 fitting the shared worker, #11002's one worker per process, and which way #20169's jam happened | `uia.md`, "Case study: NVDA" | Inferences left as stated (excluded by Dickson, 2026-11-04) |
| The client-wide event stall before Windows 10 version 1809 | `uia.md`, "Events into the client" | NVDA's record and [IUIAutomation6](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation6) (excluded by Dickson, 2026-11-04) |
| The applications never measured: the console host's provider, Qt, .NET 8 WPF, Windows Forms and Java | `uia.md`, "Not known" | Excluded by Dickson, 2026-11-04 |
| NVDA #20654's consistent 5-second freeze and its 19.8-second call | `uia.md`, "Not known" | [NVDA #20654](https://github.com/nvaccess/nvda/issues/20654) (excluded by Dickson, 2026-11-04) |
| Verbatim's own exposure in old builds: the Notepad and Windows Terminal launch rates, the listener held by `states`, `both` and the evening-long mockapp, `altf`'s spoken menu focus, and the 3,621 registrations | The entries that state them | History, linked to their commits and logs from each entry (Dickson, 2026-11-04) |
