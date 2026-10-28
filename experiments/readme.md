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

- [The terminals report no language, links or annotations](the-terminals-report-no-language-links-or-annotations/readme.md): a survey of every UIA text attribute and annotation in Windows Terminal, the console host and Windows 11 Notepad, with what each costs to read remotely and classically, the material for which attributes the caret's read fetches (2026-10-07).

## Shared code

- [The mockapp experiments of 2026-10-10](mockapp-experiments-2026-10-10/readme.md): `crates/mockapp/tests/experiments.rs` from the unmerged commits f86da7f, 8b491dc and b0a1989, and the rest of that branch as a patch.
