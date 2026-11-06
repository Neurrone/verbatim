# UIA facts

Each folder here is one fact about UI Automation that Verbatim relies on, from `books/dev/src/reference/uia-bugs.md` (the faults and limitations) or `books/dev/src/explanation/windows/uia.md` (the model of how UIA works), with what reproduces it: a program that runs the fact and judges its outcome against the recorded figures, a guide for watching it under a debugger, or a mockapp test that `cargo xtask ci` already runs. A folder is named after its fact, the heading in `uia-bugs.md` where there is one, so a citation can be read across.

They are run by hand on Dickson's machine, before and after a Windows update, to learn whether a fact Verbatim relies on still holds. Each program takes over no desktop unless its folder says so, but it starts processes and holds them for seconds at a time. Some cases run their processes on a desktop of their own, as the original experiments ran, through a trampoline in support (`uia_facts_support::desktop`): nothing there reaches the user's screen or input.

## Building and running

`uia-facts/build.ps1` builds everything, locally and in `.github/workflows/uia-facts.yml`:

1. It copies the root `Cargo.lock` to uia-facts' own `Cargo.lock` (which git ignores) and runs `cargo metadata`, which keeps every locked version and drops the entries this workspace does not use. Every later step is `--locked`, so a re-resolution fails rather than building a second version of a crate. On the first build the derived lock held 196 of the main lock's 276 entries: the 93 this workspace does not use were dropped, the 13 uia-facts members added, and no version changed.
2. It builds `check-copies` alone and runs it (below), before the full build, so a drifted copy fails before a second `windows` is built. It depends on the `workspace-hack` crate, so the `toml` it builds is the one the full build reuses.
3. It runs `cargo build --locked` in `uia-facts/`, which builds the provider program, every fact program and `check-copies` into the main `target/debug`.

Then run a fact's program from `target/debug`, for example `target\debug\nothing-cancels-a-uia-call.exe`. Every program takes:

- `--case <name>`, repeatable: run only the named cases, which its folder lists.
- `--trials <n>`: run each claim n times, its verdict then needing all n to agree; each claim runs once by default, and a rate keeps its recorded n.
- `--pause`: before a case's calls, print the process ids it names and wait for Enter, so a debugger can attach (the debugger guides use it). A second client process is given the flag too; where it pauses, as the registration program's trials do once their provider is up, it reports its ids through the run, which waits for Enter and lets it go on. The other roles (`read`, `bench`, `send`, `listen`, `registrar`, `race` and `exiter`) do not pause; their ids are printed only where the run names them.
- `--verbose`: print each traced UIA call with its time and result, the provider program's call and message log, and each trial's figures, as they happen.
- `--recovery` and `--coalescing`: turn the client objects' connection recovery and event coalescing on (`IUIAutomation6`). Both are off by default; Verbatim's own client has both on. Support's `uia::client()` applies them to every client object, and a second client process started through a role is given the same flags. Each folder says whether they change its outcomes, checked once by running every client case by default, with each flag alone and with both; where they do, its README gives the outcome of each combination that changes it and the command for each.

The workflow builds and lints on x64 and ARM64 and runs no fact program, because each one judges the machine it runs on.

## Writing a fact program

These rules hold for every program here (Dickson, 2026-11-05 and 2026-11-06), each with its reason:

- The most minimal setup that reproduces the fact, in a tree the program controls (the provider program), because a real application brings behaviour of its own that the fact is not about. A real application is started only where the claim is about that application itself, such as the console host's windows; File Explorer is never started. An application's own behaviour, such as File Explorer's reused runtime id or Windows Terminal's second runtime identity, is not a UIA fact and has no folder here; its entry in `uia-bugs.md` stays.
- Run limits: a default run takes at most 10 s where its facts need no time to pass, and at most 30 s where they use timeouts or stalls, because a fact that takes minutes is not rerun. Setup and timeouts the claim does not need are cut; a claim that is a relation to a timeout uses a short timeout; a figure Verbatim relies on keeps its value. A stall lasts until the program releases it on evidence, not a fixed length with a margin, except where the fact is about a timed stall, as the folder says. A case Verbatim does not rely on may run only when named; its claims stay in the documents, and the folder's README says it is run only when named. A program over its limit says why in its README's method.
- Cases and trials run one after another, never at once, because two at once share the machine's UIA and its processor and judge each other's noise.
- One fixed target and one API path per case, and nothing branches on the environment, as `books/dev/src/reference/testing.md` asks. A missing precondition, such as a locked desktop, an application not installed or cdb absent, makes its claims inconclusive with the reason. Waits are for evidence, a provider's signal, an event or a reply, never a sleep, except a deliberate stall or pause the README names.
- Output: the header, one line per case saying what it sets up and does, a `waiting:` line before a wait of about a second or more, one line per claim with its verdict, and the evidence under any claim that does not pass; `--verbose` prints the raw figures. No README records a run's time, since it depends on the machine.
- Client settings: every client case runs once by default, with `--recovery` alone, with `--coalescing` alone and with both, when its program is built or changed; the README records a setting only where it changes an outcome, with the command for each combination that does.
- Programs that need the unlocked desktop run their desktop cases only when named, and a builder lists them for Dickson to run rather than running them, because they take over his desktop.
- Removal: a claim that fails when it is actually tried is deleted, with the evidence only it cites, once a defect in the case has been ruled out by rerunning the original experiment at its commit; the documents state the new result instead. A claim that no case tries is never removed for that. Code that relied on a removed claim stays unchanged, and the removal is reported with its new result and what relied on it. No document says that a claim "could not be reproduced", "differed" or that a cause was "not found": it states what was found.
- A new figure is an open question's answer: the program prints it as new, the first run's answer goes into the README as the record, and later runs judge against it.

## The UI Automation the records were taken with

The current records were taken on 2026-11-05 and 2026-11-06 with this `UIAutomationCore.dll`, on Windows 10.0.26200.9711:

- Path: `C:\Windows\System32\UIAutomationCore.dll`
- File version: 7.2.26100.9549
- SHA-256: `BE5037BF46E61566D3E5A83A6C628CEBE9035771E4AE741A7B9AA27B5696D989`

After a Windows update, this PowerShell command prints the installed file's version and hash; when either differs from the above, rerun the facts:

`$p="$env:SystemRoot\System32\UIAutomationCore.dll"; $v=(Get-Item $p).VersionInfo; "{0}.{1}.{2}.{3}" -f $v.FileMajorPart,$v.FileMinorPart,$v.FileBuildPart,$v.FilePrivatePart; (Get-FileHash $p -Algorithm SHA256).Hash`

The numeric file version is used because the file's version string stays at 7.2.26100.1 across updates.

## The workspace

`uia-facts/` is a Cargo workspace of its own, which the root `Cargo.toml` excludes, so it never joins the main build or `cargo xtask ci`. It depends on `crates/verbatim-uia` and `crates/verbatim-uia-rops` by path, so the facts call UIA through Verbatim's own code where Verbatim's own code is what they check, and it is GPL-3.0-or-later for that reason. Its code is original; an NVDA figure is cited as NVDA's record, never reproduced from NVDA's source.

It builds into the main `target/` (`uia-facts/.cargo/config.toml`), so third-party crates the main workspace built, `windows` above all, are reused rather than built again. That holds only while the two workspaces ask for those crates alike, and Cargo cannot share `[workspace.dependencies]`, `[workspace.lints]` or a profile between workspaces, so `uia-facts/Cargo.toml` copies them. `check-copies` fails when the copies drift: the windows family's entries, the lints, `[profile.dev]`, any package whose version or source differs from the main lock's, and the top-level `env` of the two workflows, which the build cache's key hashes. The eight path crates (`verbatim-model`, `verbatim-text`, `verbatim-core`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-uia`, `verbatim-uia-rops` and `workspace-hack`) build once more for uia-facts, since a path crate's unit hash includes its path from the workspace root. A uia-facts build and a main build in the same target wait for each other on Cargo's build lock.

## The provider program

`uia-facts/provider` is the facts' own UIA and MSAA provider; no fact launches mockapp, whose facts stay mockapp tests. A fact starts it from its own folder, so it always runs against the provider built with it, and drives it with one request a line on its standard input, each answered once the provider has acted on it. Its windows are hidden top-level windows on chosen UI threads: UIA windows serving a fragment tree of set size (24 nodes by default), with or without `UseComThreading`, a window with its own `IAccessible`, and plain Win32 windows. It can hold any provider method, make every call slow while its thread keeps pumping, keep a UI thread busy, wait in `CoWaitForMultipleHandles`, raise events on request or at a rate, raise a focus, name-change or foreground `WinEvent` for a window from a thread that is not the window's, move its tree's focus, show and hide its windows, and log every provider call and the window messages UIA sends. Its MSAA window has two children and reports itself and each child focused, as mockapp's MSAA objects did. Its module documentation, `uia-facts/provider/src/main.rs`, lists its command line and requests.

A figure measured on mockapp whose value depends on the provider's tree, such as a count of provider calls, is recorded again on the provider program's first run, with the mockapp figure kept as history.

## Output and verdicts

Each program prints a header (the date, the Windows build with its revision, `UIAutomationCore.dll`'s version, the processor, the commit and the client settings), then one line per claim with what it observed, the record, and a verdict. Before each case it prints one line saying in plain words what the case sets up and does, and before each deliberate wait of a second or more, a pause or a call left to wait on a timeout, a line naming it (`waiting: ...`). Under a claim whose verdict is differ or inconclusive it prints the evidence the case kept (`evidence: ...`): each timed call's outcome and time, the lines a second client process reported, such as a trial's events lost and heard with their times, and the figures the case records; a clean run prints none. It exits with 0 when every judged claim passes, 1 when any differs, and 2 when none differs but some are inconclusive.

- A deterministic outcome (an HRESULT, a count of provider calls, delivered or not, an order) passes when every run equals the record, differs when the runs agree on another value, and is inconclusive when they disagree.
- A relation (A answers while B waits, B waits behind A, a speed-up below a bound computed in the same run) passes when it holds in every run, differs when it holds in none, and is inconclusive otherwise. The durations around it are printed for trends, not judged, since they depend on the machine.
- A band, only where the figure is a timeout UIA sets: from the timeout T less one system tick (15.6 ms, since a wait can end up to a tick early: a 2,000 ms connection timeout once failed after 1,999.3 ms) to 1.15 times T plus 50 ms for one wait, and the sum of those for waits in turn. It passes when every trial lies inside, differs when every one lies outside, and is inconclusive otherwise.
- A rate (k of n against a recorded k of n) runs its recorded n, and passes when the recorded rate lies inside the observed 95 percent Wilson interval, differs when the two intervals do not overlap, and is inconclusive otherwise; with no occurrence it differs only when the recorded rate would almost surely have shown one.
- An open question prints its answer as new until its folder records an answer; from then on later runs judge against that record by the rules above.
- A missing precondition, such as a locked desktop, makes its claims inconclusive with the reason; nothing takes another path.

## The facts

| Folder | What it reproduces | Kind |
|---|---|---|
| [a-new-connection-to-a-stuck-application-times-out](a-new-connection-to-a-stuck-application-times-out/readme.md) | `uia.md`, "Making a connection": the connection timeout's failure | Program |
| [the-connection-timeout-applies-to-the-whole-process](the-connection-timeout-applies-to-the-whole-process/readme.md) | `uia-bugs.md`, the fault of that name | Kept CI test and program |
| [a-call-waits-its-timeout-from-its-own-start](a-call-waits-its-timeout-from-its-own-start/readme.md) | `uia.md`, "Timeouts and cancelling": how a call waits | Guide, program and kept CI test |
| [a-timeout-bounds-each-request-and-window-message-not-an-operation](a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md) | `uia-bugs.md`, the limitation of that name | Program |
| [a-registration-that-times-out-reports-success](a-registration-that-times-out-reports-success/readme.md) | `uia-bugs.md`, the fault of that name: the registration's return and the gap | Kept CI test and program |
| [the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window](the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window/readme.md) | `uia-bugs.md`, "A registration that times out reports success": the gap's reach, a removal during it, and the silence after the stall | Program |
| [nothing-cancels-a-uia-call](nothing-cancels-a-uia-call/readme.md) | `uia.md`, "The parts" and "Timeouts and cancelling" | Program and debugger guide |
| [a-provider-process-serves-one-uia-request-at-a-time](a-provider-process-serves-one-uia-request-at-a-time/readme.md) | `uia.md`, "Inside the provider process" and "Where the provider call runs" | Program |
| [the-uia-channel-and-the-provider-io-thread](the-uia-channel-and-the-provider-io-thread/readme.md) | `uia.md`, "The parts" and "Inside the provider process": the internals | Guide |
| [msaa-calls-do-not-pass-through-uias-request-reader](msaa-calls-do-not-pass-through-uias-request-reader/readme.md) | `uia.md`, "MSAA beside UIA" | Program |
| [parallel-client-threads-gain-at-most-twice-on-one-application](parallel-client-threads-gain-at-most-twice-on-one-application/readme.md) | `uia.md`, "Where the provider call runs" and "Calls from the client" | Program and debugger guide |
| [requests-per-client-operation](requests-per-client-operation/readme.md) | `uia.md`, "What one request is" | Guide |
| [uia-answers-some-calls-in-the-client](uia-answers-some-calls-in-the-client/readme.md) | `uia.md`, "Making a connection", what UIA answers in the client; `uia-bugs.md`, "UI Automation's copy of an element's runtime id lapses" | Kept CI test and program |
| [a-remote-operation-stops-after-exactly-10000-instructions](a-remote-operation-stops-after-exactly-10000-instructions/readme.md) | `uia.md`, "What one request is": the instruction limit | Kept CI test |
| [a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events](a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md) | `uia-bugs.md`, the fault of that name; `uia.md`, "Events into the client": the worker's focus tracking | Kept CI test and program |
| [a-process-with-no-uia-focus-registration-is-not-held-by-a-hung-window](a-process-with-no-uia-focus-registration-is-not-held-by-a-hung-window/readme.md) | `uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events": not affected, and Verbatim's workaround | Program |
| [a-focus-change-in-a-window-busy-past-both-of-uias-waits-is-dropped](a-focus-change-in-a-window-busy-past-both-of-uias-waits-is-dropped/readme.md) | `uia-bugs.md`, the same entry: a shorter connection timeout's cost | Program |
| [focus-for-a-newly-opened-or-shown-window-is-not-held](focus-for-a-newly-opened-or-shown-window-is-not-held/readme.md) | `uia-bugs.md`, the same entry: the open-and-show route | Program |
| [uias-client-threads-and-focus-tracking-calls](uias-client-threads-and-focus-tracking-calls/readme.md) | `uia.md`, "Events into the client": the client's threads and focus tracking's calls | Guide |
| [a-client-processs-events-run-on-one-worker](a-client-processs-events-run-on-one-worker/readme.md) | `uia.md`, "Events into the client": the worker | Program |
| [another-clients-stuck-call-holds-an-events-cache-fetch](another-clients-stuck-call-holds-an-events-cache-fetch/readme.md) | `uia.md`, "Events into the client": the worker's cache fetch | Program |
| [a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events](a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events/readme.md) | `uia-bugs.md`, the fault of that name | Program and debugger guide |
| [desktop-wide-focus-handlers-in-a-second-process-deliver-late](desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md) | `uia-bugs.md`, the fault of that name | Program |
| [a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all](a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md) | `uia-bugs.md`, the fault of that name | Program |
| [focus-events-cannot-be-limited-to-one-application](focus-events-cannot-be-limited-to-one-application/readme.md) | `uia-bugs.md`, the limitation of that name | Kept CI test and program |
| [elements-cannot-be-passed-to-another-process](elements-cannot-be-passed-to-another-process/readme.md) | `uia-bugs.md`, the limitation of that name | Kept CI test and program |
| [ancestors-cannot-be-cached-or-listened-to](ancestors-cannot-be-cached-or-listened-to/readme.md) | `uia-bugs.md`, the limitation of that name | Program |
| [a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread](a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread/readme.md) | `uia-bugs.md`, the fault of that name | Kept CI test and program |
| [a-process-that-used-uia-can-hang-or-crash-as-it-exits](a-process-that-used-uia-can-hang-or-crash-as-it-exits/readme.md) | `uia-bugs.md`, the fault of that name | Program and debugger guide |
| [a-new-connection-fails-while-another-processs-remote-operation-is-queued](a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md) | `uia-bugs.md`, the fault of that name; `uia.md`, "Predicting outcomes" | Program and debugger guide |
| [a-pending-connection-holds-its-own-processs-other-calls](a-pending-connection-holds-its-own-processs-other-calls/readme.md) | `uia.md`, "Making a connection": the pending connection | Program and debugger guide |
| [uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler](uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/readme.md) | `uia-bugs.md`, the fault of that name | Program |
| [the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider](the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider/readme.md) | `uia-bugs.md`, the raw probe in the entry above | Kept CI test and program |
| [a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read](a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read/readme.md) | `uia-bugs.md`, the fault of that name | Program |
| [inside-a-remote-operation-conhosts-window-has-no-handle-or-title](inside-a-remote-operation-conhosts-window-has-no-handle-or-title/readme.md) | `uia-bugs.md`, the fault of that name | Kept CI test and program |
| [real-applications-serve-uia-calls-one-at-a-time](real-applications-serve-uia-calls-one-at-a-time/readme.md) | `uia.md`, "Where the provider call runs" and "Predicting outcomes": the measured applications | Program |
| [a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call](a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call/readme.md) | `books/dev/src/explanation/architecture.md`, section 1: the killed client the clean shutdown rests on | Program |

## Cited, not reproduced

These claims link their source instead of a reproduction.

| Claim | Where it is stated | Its source |
|---|---|---|
| What `WM_GETOBJECT`, `UiaReturnRawElementProvider`, the two timeouts, `ProviderOptions`, caching, remote operations, `AddFocusChangedEventHandler`'s desktop-wide scope, `IUIAutomation6`, the threading advice, `CoCancelCall` and `IsHungAppWindow` are documented to do | `uia.md`, throughout | The Microsoft Learn pages in `uia.md`'s "References" |
| Which frameworks declare `UseComThreading`, and where their provider calls run: Windows Forms, Qt, Chromium, WinUI 3, Firefox, JavaFX, Flutter, WPF, Avalonia, the console host, Windows Terminal and AccessKit | `uia.md`, "Where the provider call runs" | The framework source named in `uia.md`'s "References"; WPF's measured hold is a measurement, not cited |
| An ancestor-scoped cache request is not allowed | `uia-bugs.md`, "Ancestors cannot be cached or listened to" | [IUIAutomationCacheRequest::TreeScope](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope) |
| NVDA's issues and their figures: #20654, #20169 and #20170, #6533, #7345 and #8539, #11002, #14888, #15850, #17602, #20608, #20168 | `uia.md`, "Case study: NVDA" | NVDA's issue tracker, linked from the case study |
| `conhost_two_windows`'s inference about `UiaHasServerSideProvider` | `uia-bugs.md`, "UiaHasServerSideProvider says no for a new conhost window while another process has an event handler" | Its kept runs in `experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/` (excluded by Dickson, 2026-11-04) |
| NVDA's exposure: #7345 and #8539 fitting the shared worker, #11002's one worker per process, and which way #20169's jam happened | `uia.md`, "Case study: NVDA" | Inferences left as stated (excluded by Dickson, 2026-11-04) |
| The client-wide event stall before Windows 10 version 1809 | `uia.md`, "Events into the client" | NVDA's record and [IUIAutomation6](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation6) (excluded by Dickson, 2026-11-04) |
| The applications never measured: the console host's provider, Qt, .NET 8 WPF, Windows Forms and Java | `uia.md`, "Not known" | Excluded by Dickson, 2026-11-04 |
| NVDA #20654's consistent 5-second freeze and its 19.8-second call | `uia.md`, "Not known" | [NVDA #20654](https://github.com/nvaccess/nvda/issues/20654) (excluded by Dickson, 2026-11-04) |
| Verbatim's own exposure in old builds: the Notepad and Windows Terminal launch rates, the listener held by `states`, `both` and the evening-long mockapp, `altf`'s spoken menu focus, and the 3,621 registrations | The entries that state them | History, linked to their commits and logs from each entry (Dickson, 2026-11-04) |
