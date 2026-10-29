# Experiments

This folder keeps the experiments behind `docs/uia-bugs.md`, the record of faults in Windows' UI Automation that Verbatim has established: for each one, the scripts or code that were run, the tool patches and prototype source they ran with, the raw text outputs, and a readme that defines each experiment by its identifier, with its setup, its result and its numbers. They were scratch work, run on Dickson's x64 machine (Windows 11, build 26200) between 2026-10-07 and 2026-10-26. Nothing here is built or tested by the workspace: prototype manifests are kept as `Cargo.toml.txt`, and each readme says how to rebuild and rerun its experiments.

Left out everywhere: screen recordings, ETW traces, symbol caches, binaries and build output. Paths to a session's scratch folder are redacted to `<scratch folder>`.

## Faults in UI Automation

- [A new connection fails while another process's remote operation is queued](a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md): the collide tool's `e1` to `e36`, and the Notepad and Windows Terminal launches that met the fault.
- [UiaHasServerSideProvider says no for a new conhost window while another process has an event handler](uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/readme.md): `e37` to `e39`, the kept `conhost_two_windows` failures, and the raw-probe proposal.
- [A hung window named in a system focus event holds a process's UIA events](a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md): `h1` to `h6`, P4, and the October experiments E-delivery, E-hung-proxy, `e_mapping`, `e_hung_cause` and `e_cross`.
- [The connection timeout applies to the whole process](the-connection-timeout-applies-to-the-whole-process/readme.md): E1 and `h5`.
- [Desktop-wide focus handlers in a second process deliver late](desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md): P2's delivery timings, `p2b`, `p2c` and `p2d`.
- [A registration that times out reports success](a-registration-that-times-out-reports-success/readme.md): E-TT and E3, which also serve the limitation that a timeout bounds each provider call, not an operation, `e57`, when such a registration starts to deliver, and `e58`, what reaches the application and other registrations meanwhile, and whether it can be removed.

The other faults in `docs/uia-bugs.md` rest on commits and tests named in their entries, with no separate experiment.

## Limitations

- [Focus events cannot be limited to one application](focus-events-cannot-be-limited-to-one-application/readme.md): E-scope, E-cost, P1, P2's cost figures and P5, and the option (c) prototype that P1 to P5 used.
- [Elements cannot be passed to another process](elements-cannot-be-passed-to-another-process/readme.md): E-marshal and P3.
- A timeout bounds each provider call, not an operation: shares the folder of [a registration that times out reports success](a-registration-that-times-out-reports-success/readme.md).

## Applications

- [Windows Terminal raises focus from a second runtime identity](windows-terminal-raises-focus-from-a-second-runtime-identity/readme.md): the identity judge and its three 150-round runs.

## Design experiments

- [The listener's read below another process's window](the-listeners-read-below-another-processs-window/readme.md): how a busy parent window of another process, and a remote operation queued in its process, delay or drop the listener's read of a focus in the child window (`docs/design/listener-without-connections.md`, decision 13 and risk 14).
- [The console host raises its caret events on its blink timer](the-console-host-raises-its-caret-events-on-its-blink-timer/readme.md): whether the console host gives a held caret key's watch any evidence sooner than its console caret event; every caret signal it raises comes on the next tick of its 530 ms cursor blink timer, and nothing between ticks (2026-10-26).
- [A terminal's read ends at the document end it started with](a-terminal-read-ends-at-the-document-end-it-started-with/readme.md): why a read during a flood under a scroll region leaves out the footer below the caret (`conhost_footer_flood`, GitHub run 40317394858); output that moves the document's end between the document range and the snapshot's text cuts the text at the old end, in both terminals, in 93 and 65 percent of reads during a fast flood and none after it (2026-10-26).
- [CoCancelCall ends an MSAA COM call, but not a window message](cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/readme.md): whether `CoCancelCall`, as NVDA's watchdog uses it, can end the outpost's MSAA calls into a hung application; it ends an `IAccessible` call through a proxy, but not the `WM_GETOBJECT` of an acquisition (`docs/design/phase6-design.md`, "Time limits that hold (2026-10-11)").

- [The outpost's focus read below its checked window](the-outposts-focus-read-below-its-checked-window/readme.md): whether the outpost can find its application's keyboard focus by a connection by handle to its checked focus window and a remote search below it, in place of `GetFocusedElementBuildCache`, with the search's instructions and provider calls in mockapp (`docs/design/outpost-focused-element-study.md`, 2026-10-29).
- [GetFocusedElement after the focus moves](getfocusedelement-after-the-focus-moves/readme.md): whether the outpost's `GetFocusedElementBuildCache` connects into another application when the system focus moves to it between the outpost's check and the call, with the race made to happen every time between two shown mockapp windows (the same study, 2026-10-29).
- [Two new connections beside a pending call](two-new-connections-beside-a-pending-call/readme.md): the second form of the collision made again, 3 of 42, and with the listener's read below a busy parent as one connection, the read one connection timeout late in 5 of 42 (`docs/design/cross-process-calls-study.md`, 2026-10-29).
- [The runtime-id search enters another process's child window](the-runtime-id-search-enters-another-processs-child-window/readme.md): the outpost's search of a top-level window's subtree connects to a shown child window of another process and makes 481 provider calls there, and with that process busy waits 2 seconds and answers not found (the same study).
- [A process-scoped hook takes events by the window's owner](a-process-scoped-hook-takes-events-by-the-windows-owner/readme.md): a `WinEvent` one process raises naming another's window reaches the hook scoped to the window's owner, not the raiser's, and an outpost handed such an event calls into the other process (the same study).
- [A walk up whose window changes after its check](a-walk-up-whose-window-changes-after-its-check/readme.md): with the order forced, the classic walk's step after a reparenting or an owner shown connects into the other process and waits on its thread, the chain still right (the same study).
- [accFocus names no other process's child window](accfocus-names-no-other-processs-child-window/readme.md): with the keyboard focus moved into another process's child window after the MSAA focus read's check, Windows' standard proxy names nothing (the same study).
- [A replacement outpost's lookup beside its predecessor's operation](a-replacement-outposts-lookup-beside-its-predecessors-operation/readme.md): the first fault with the outposts' own calls, the replacement's lookup refused after the busy time and one connection timeout, 6 of 6 (the same study).
- [The listener's registration waits on the foreground application](the-listeners-registration-waits-on-the-foreground-application/readme.md): the desktop-wide registration reads the foreground application's focus and waits up to about 6 seconds for one that does not answer (the same study).
- [The MSAA continuation waits on the parent window's application](the-msaa-continuation-waits-on-the-parent-windows-application/readme.md): a focus below a busy parent window is reported only when the parent answers, and never when it is busy for 12 seconds (the same study).
- [CompareElements makes no call](compareelements-makes-no-call/readme.md): the listener's repeat comparison with another application's element calls neither provider (the same study).
- [The focus pipeline rework's measurements](focus-pipeline-rework-measurements/readme.md): step 1 of `docs/design/focus-pipeline-rework.md`, the repeats Core did not hold, menu openings, refusals, the hand-over's hold and the gap's firings tallied from 111 end-to-end runs of a scratch build, three programs on `UiaHasServerSideProvider`, probes per new window and the provider description, and ten captures of NVDA and Windows, with each of the design's rules applied and the questions they send to Dickson (2026-10-29).
- [The terminals report no language, links or annotations](the-terminals-report-no-language-links-or-annotations/readme.md): a survey of every UIA text attribute and annotation in Windows Terminal, the console host and Windows 11 Notepad, with what each costs to read remotely and classically, the material for which attributes the caret's read fetches (2026-10-07).
- [The Settings frame comes in front after its foreground event](the-settings-frame-comes-in-front-after-its-foreground-event/readme.md): how long after its `EVENT_SYSTEM_FOREGROUND` the Settings frame becomes `GetForegroundWindow`, for "The foreground wait" in `docs/design/focus-pipeline-rework.md`; launched from the desktop it comes in front 24 to 78 ms after its event, median 51, within 35 ms in 8 of 20 launches and within 20 ms in none, while switched to with Alt+Tab it is in front before its event; and NVDA's capture of five launches, which says "Settings" from the frame's client pane, as the focus when its defer finds the frame in front and as an entered ancestor of the content's focus when it does not (2026-10-30).

## Shared code

- [The cross-process calls study's program](cross-process-calls-study-program/readme.md): the one scratch program behind that study's nine experiments, with its patch.
- [The mockapp experiments of 2026-10-10](mockapp-experiments-2026-10-10/readme.md): `crates/mockapp/tests/experiments.rs` from the unmerged commits f86da7f, 8b491dc and b0a1989, and the rest of that branch as a patch.
