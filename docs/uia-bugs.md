# Faults in UI Automation

This file records the faults in Windows' UI Automation (UIA) that Verbatim has established by experiment: what triggers each, what a caller sees, what was tried against it, and whether NVDA meets it. A behaviour gets a heading here only once a test or a recorded experiment shows it; a suspicion stays in the design record or the roadmap until then. Where a statement is an inference rather than a measurement, it says so.

Every figure comes from the recorded output of the experiment named beside it, on Dickson's x64 machine (Windows 11, build 26200) unless another machine is named. The experiments live in [`experiments/`](../experiments/readme.md), one folder per entry that has them, named after its heading: each folder's readme defines every experiment the entry names, with its setup, result and numbers, beside the scripts, tool patches and raw outputs; the design records cited hold the reasoning built on them. Tests that reproduce a fault or show a limitation, or show that Verbatim's workaround still holds, live in `crates/mockapp/tests/uia_bugs/`, one file per entry, named after its heading here; each entry's "Reproduction test" names them, or says why there is none.

Terms used throughout:

- A remote operation is a small program a UIA client sends into an application's process, where UIA runs it against the application's provider and returns the results in one round trip (`CoreAutomationRemoteOperation`, built in `verbatim-uia-rops`).
- A provider is the code in an application that answers UIA for its windows. A window with no provider of its own is read through a proxy: UIA's client side builds an element for it in the calling process, from the window's MSAA object or from the window itself (the MSAA proxy, named "MSAA Proxy" in an element's provider description).
- The listener is Verbatim's process that receives every application's focus events and judges and reads each focus; an outpost is the process that serves one application's events and reads; a spare outpost is one started in advance and given to a new application when one appears (`docs/architecture.md`, D9 and D13, and `docs/glossary.md`).
- A lookup by handle is a new connection to a window by its window handle: `ElementFromHandle` or `ElementFromHandleBuildCache`. The provider probe is `UiaHasServerSideProvider`, which asks whether a window has a provider of its own.
- conhost is the console host, `conhost.exe`, which draws classic console windows (class `ConsoleWindowClass`) and provides their UIA. Windows Terminal is a different application and is named as such.
- A focus WinEvent is the system's `EVENT_OBJECT_FOCUS` notification, which UIA's client side also listens to.

## Faults in UI Automation

### A new connection fails while another process's remote operation is queued

#### Trigger

Process A sends a remote operation into an application; while that operation waits to be evaluated there, from its arrival in the application until UIA begins evaluating it, process B makes a new connection by window handle to any window of that application, or walks up from an element it already holds into a window of that application. The wait is short while the application's UI thread pumps messages, longer for the first operation in the process because UIA sets itself up there first (about 7.5 ms on this machine), and as long as the UI thread is busy, which is how an application that is still starting makes it long.

- Any remote operation opens the window, not only the first in the process (`e1`).
- Every connection by handle is exposed, not only a client's first: a second probe from a client that had probed the same window before also failed (`e9`).
- The whole process is exposed: a probe of a window on the application's other, idle UI thread failed too (`e16`, and with a 1-second busy period `e28d`, 3 of 3, after about 3,015 ms).
- The order does not matter: a connection sent first, and still waiting for the busy UI thread when the operation arrives, failed as well (`e31`, 6 of 6).
- A walk from a held element up into a window of another process fails the same way (`e24`, 6 of 6, the window proxy after about 2,045 ms), also when B already held an element of that process's main window (`e24b`, 6 of 6) and when A's operation went into another top-level window of the process (`e24c`, 3 of 3).

#### Symptoms

The call fails without an error. `UiaHasServerSideProvider` answers "no" after about 3 seconds; `ElementFromHandle` and `ElementFromHandleBuildCache` return UIA's window proxy, named by the window's title, after about 2 seconds, in place of the application's own element (`e1`; in `e33`'s positive control, with a 300 ms busy period, after 2,300 to 2,337 ms in 6 of 6). A caller that trusts the answer reads the window through MSAA.

In Verbatim the operation was the listener's focus read and the connection an outpost's probe or lookup, or the reverse. About 3 in 100 Windows Terminal launches read the terminal's first focus through MSAA, spoken as "pane", or announced it twice, and about 1 in 25 Notepad openings did the same (measured 2026-10-17, before Verbatim kept its own calls from colliding). In one slow Windows Terminal launch the outpost's probe of the terminal's `InputSite` window waited 3,008 ms and answered "no" while the listener's focus read waited for the busy UI thread (`wt-launch-slow/` in this fault's experiments folder).

A related cost that is not a failure: a remote operation waiting for one busy UI thread delays every other client's call into the process, on any thread, held elements included, by the same time (`e28`, 9 of 9 delayed to the end of a 1,000 ms busy period, against under 1.1 ms in the control `e28c`).

A second form needs new connections from two processes: a new connection waiting in a busy application fails in some trials (3 of 42; with the listener's implicit connection as one of them, the listener's read one connection timeout late in 5 of 42) when another process's new connection waits there too and a third process has a call pending. Two connections from two threads of one process beside a held call did not fail (12 of 12), nor did a connection beside another's that was waiting when its process ended (12 of 12).

A call that times out leaves its request queued in the application, which processes it when it wakes, so a connection made then by another process can still collide.

#### Not affected

- Two remote operations sent into the same application at the same time from different processes do not conflict with each other: in `e27`, B's operation into the application while A's waited, with B's first operation there and with one it had run before, read the provider's own element in 6 of 6 pairs, B's ending within a millisecond of A's.
- A plain property read or a tree walk sent to the busy application (`e5`).
- Calls on elements already held, which are only delayed (`e10`), including walks down into a child window's provider in the same process or another (`e25`, `e26`, `e26b`, 27 of 27) and walks up within one process (`e23`, 6 of 6).
- A walk down from the desktop root to the application's top-level window, by sibling walk in the raw or control view, `FindFirst` or `FindAll` (`e33`, 24 of 24).
- The operating process's own lookups (`e12`), event registrations, and connections made once evaluation has begun (`e14`).
- MSAA calls, through UIA's bridge to the provider or the system's standard proxy (`e29`, 12 of 12).
- A raw `WM_GETOBJECT` with `UiaRootObjectId` sent to the window: it answered `0x10000` in 6 of 6 trials in which `UiaHasServerSideProvider`, sent at the same moment, answered "no" (`e34`).
- `GetFocusedElement` is unsettled: it failed in an earlier experiment and succeeded in 3 of 3 of the latest.
- conhost did not show the fault in `e35`: a lookup by handle during a queued operation returned conhost's own element in 6 of 6, and in 5 of 5 on a fresh conhost window for each trial (`e35b`), while the same method of stalling reproduced it against the collide tool's server (`e35c`, 5 of 5). Why conhost differs is not known.

#### Workarounds that worked

- Only one process connects to an application during another process's operation: only an application's own outpost sends it a remote operation or makes a new UIA connection to it, and the focus listener reads every focus from elements it holds (`docs/design/listener-without-connections.md`, section 2). It covers only Verbatim's own processes: remote operations by other software, a request still queued after a timeout, the end-to-end agent's two UIA requests, and UIA's own focus tracking in the listener's process are not covered.

#### Workarounds that didn't work

- UIA's setup done in each application by the listener before the application was given its outpost. A later remote operation collided just as the setup's had, so in Windows Terminal it did not prevent the 3-second probe failure and made it more frequent (4 of 12 launches against 0 of 10), and it held each new application 72 to 278 ms before its outpost was given.
- Making the connection first, so it is under way before the operation arrives: it failed in 6 of 6 (`e31`).
- Asking again: a later probe fails the same way while an operation is queued (`e9`), and every later operation opens the window again (`e1`).

#### NVDA

No NVDA issue or capture records this fault. NVDA is one process, and a process's own lookups are not exposed to its own operations (`e12`), so NVDA alone is inferred not to meet it; beside another client that sends remote operations into the same application, it would be exposed like any other client (inferred, not measured).

#### Reproduction test

No test in `crates/mockapp/tests/uia_bugs/` reproduces it: a test would have to wait for evidence that the operation is queued and that the probe has reached the stalled thread, which needs a mockapp stall that names the messages it is posted and sent, and mockapp has none. A test written against such a stall passed 20 of 20 runs, and its control with no operation queued 20 of 20; it found the probe's "no" about 3 seconds after the probe was made whatever the connection timeout (500, 1,000, 2,000 and 5,000 ms), and the element lookup's window proxy after about 2 seconds at 500 or 1,000 ms and after 5 seconds at 5,000 ms.

#### Evidence

[`experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/`](../experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md): the collide tool's experiments `e1` to `e36` (including `e24b`, `e24c`, `e26b`, `e28c`, `e28d`, `e35b` and `e35c`), with the tool as patches, and the Notepad and Windows Terminal launches that met the fault.

[`experiments/two-new-connections-beside-a-pending-call/`](../experiments/two-new-connections-beside-a-pending-call/readme.md): the second form's frequency, two outposts' lookups and the listener's read beside a pending call.

### UiaHasServerSideProvider says no for a new conhost window while another process has an event handler

#### Trigger

A client that has not connected to a conhost window calls `UiaHasServerSideProvider` on it moments after the window is created, while another process has any UIA event handler registered. One idle `ToolTipOpened` handler over the desktop root is enough. No remote operation, listener read, WinEvent hook, outpost registration or processor load is needed (`e38`).

#### Symptoms

The probe answers "no" after 3,022 to 3,058 ms, so the window is read through MSAA. In `e38` the rate was highest when the call reached conhost before its window thread first pumped messages, and it also happened up to 77 ms after the window was found: in phase 1, 7 "no" in the 24 trials at 0 ms and 4 in the 96 at other delays (0.3, 7.6, 46.2 and 77.3 ms). With every trial at 0 ms: a full listener stand-in, 4 of 30; a passive one, which registers Verbatim's handlers and reads nothing, 10 of 30; an idle tooltip handler alone, 7 of 30. The figures vary between runs: in `e39`, under the passive listener, the probe followed by a lookup gave no "no" in 120 trials without debugging hooks and 6 of 60 with them.

In the "no" trials of `e39`, conhost received UIA's request 24 to 31 ms after the window was found and answered `0x10000` in about 3.5 ms, as in the "yes" trials: the window answered, and UIA's client waited afterwards. A lookup by handle meets the same fault: `ElementFromHandleBuildCache` alone, at 0 ms under the trigger, returned UIA's MSAA proxy (its process id the console's client, not conhost) after 5,030 to 5,043 ms in 7 of 120 trials (`e39`).

In the end-to-end suite this is inferred to be why `conhost_two_windows` fails about 1 run in 20: each failing run's probe came from a spare outpost newly given the second console's conhost, 1 to 3 ms after the window's foreground event, with no Verbatim call pending in that conhost (`kept-runs/` in this fault's experiments folder, cited in `e38`).

The same call, made by an outpost as its probe of other applications' new windows, also took about 3 seconds on File Explorer `DirectUIHWND` windows (the "Explorer Pane" and "Items View" windows), answering "no" in some of those runs.

#### Not affected

- With no other process holding a handler, no "no" in 110 trials (`e38`) and 180 (`e37`), at delays from 0 to 100 ms; another process that fetched the desktop root and registered nothing, 0 of 40.
- The raw probe: `SendMessageTimeoutW(WM_GETOBJECT, 0, UiaRootObjectId)` answered `0x10000` in all 360 raw probes of `e38`, and in the 40 trials with no `UiaHasServerSideProvider` from any process (`e38-rawpath`) the lookup that followed returned conhost's own element every time.
- A lookup by handle made right after a "no" from the same process: conhost's own element in 1.2 to 2.5 ms, 38 of 38 (`e38`).
- A conhost window 2 seconds old: every probe and lookup correct (`e37`'s controls, `e39`'s settled sets).

#### Workarounds that worked

- The raw probe, which the focus listener and the outposts make, so that Verbatim no longer calls `UiaHasServerSideProvider` (`verbatim_uia::raw_probe`; `design-raw-probe.md` in this fault's experiments folder; `docs/design/listener-without-connections.md`, decision 2, the outposts' share decided by Dickson on 2026-10-20). The raw probe is one `SendMessageTimeoutW` sending `WM_GETOBJECT` with wParam 0 and lParam `UiaRootObjectId` sign-extended (as UIA itself sends it), where an answer of exactly `0x10000` means the window has a provider. Under the trigger at 0 ms, the raw probe and then the lookup were right in 120 of 120 trials, median 45.6 ms in all (37 to 56), against 39.2 ms for `UiaHasServerSideProvider` and then the lookup; on a settled conhost window 9.6 ms against 10.2 (`e39`). Across 88 windows of real applications it agreed with `UiaHasServerSideProvider` on every window that answered 0 or `0x10000`; 3 File Explorer child windows answered `0xffffffff80040201` where UIA said no, which is why only `0x10000` counts as yes. Costs and risks: Microsoft documents that [applications never send this message directly](https://learn.microsoft.com/en-us/windows/win32/winauto/wm-getobject) and documents no meaning for its answer; that the lookup after it succeeds because it starts after conhost has pumped is inferred, not shown.

#### Workarounds that didn't work

- The lookup by handle as the check, judging the window native when its element's main provider is not one of UIA's proxies: it returned the MSAA proxy after about 5 seconds in 7 of 120 trials under the trigger, and it misclassifies 11 File Explorer child windows that answer the raw probe but whose element has an MSAA or non-control proxy as its main provider (`e39`).
- Probing later: it lowers the rate but does not remove the fault, which occurred up to 77.3 ms after the window was found (`e38`); in `e39`, installing the debugging hooks first moved the probe to about 18 ms and gave no "no" in 60, which shows only a lower rate.
- A lock per application around Verbatim's colliding calls: it could not prevent this, since no Verbatim call is pending when it happens (`e38`; `conhost_two_windows` failed four times in eleven runs under it).

#### NVDA

NVDA asks the same question with the same call (`nvda/source/UIAHandler/__init__.py`, lines 1274 to 1286) and always has its own handlers registered, so it is inferred to be exposed; no NVDA capture shows it. Verbatim asks it with the raw probe instead, so it is not exposed where NVDA is inferred to be, and a new console window's first focus that NVDA might read through MSAA, Verbatim reads through UIA.

NVDA records a different fault of the same call: `UiaHasServerSideProvider` on a hung application blocks its caller, and NVDA's watchdog, which recovers by `CoCancelCall` on its main thread and by cancelling its own window messages, cannot cancel it, nor the UIA element lookups (`ElementFromHandleBuildCache`, `GetFocusedElementBuildCache`, `ElementFromPointBuildCache`). [NVDA #20170](https://github.com/nvaccess/nvda/pull/20170) (fixing [NVDA #20169](https://github.com/nvaccess/nvda/issues/20169), 2026-05-26, in the pinned source) runs those calls through `watchdog.cancellableExecute`, which abandons the thread making the call; its comment calls the probe "a blocking cross-process call that the watchdog cannot otherwise cancel". UIA's client calls in general are not cancelled by `CoCancelCall`: UIA's client waits for its answer on a channel of its own, not in a COM call, so the cancel finds nothing to end ([`experiments/cocancelcall-does-not-end-a-uia-client-call/`](../experiments/cocancelcall-does-not-end-a-uia-client-call/readme.md)); Verbatim bounds such calls by abandoning the thread too (`crates/verbatim-uia/src/bounded.rs`).

#### Reproduction test

No test reproduces it reliably yet. The collide tool reproduced it in at most a third of trials, which is not enough for a test; a mockapp version would need a window stalled before its first pump (`design-raw-probe.md` in the experiments folder, section 6, judge 5).

#### Evidence

[`experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/`](../experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/readme.md): experiments `e37`, `e38` (including `e38-rawpath`) and `e39`, with their scripts, outputs, summaries and tool patches, the kept `conhost_two_windows` failures, and `design-raw-probe.md`; NVDA commit [961db0e66](https://github.com/nvaccess/nvda/commit/961db0e66cdb079144d49ca4b8b505a3a65741d3) ([NVDA #20170](https://github.com/nvaccess/nvda/pull/20170)).

### A hung window named in a system focus event holds a process's UIA events

#### Trigger

A process holds any UIA focus registration: a desktop-wide focus handler, or a focus event in an event handler group scoped to one window, even another application's. A focus WinEvent then names a window that UIA reads through its proxy and that is not answering messages. Tested with mockapp's comctl32 tree view, stalled on its window thread.

#### Symptoms

All of that process's UIA event delivery waits, every application's events and not only focus, until the hung window answers or UIA gives up, and then the held events arrive. UIA's client runs all event work in a process on one work-item thread; for the focus WinEvent its focus tracking makes two `SendMessageTimeoutW` calls in turn to the hung window, a `WM_GETOBJECT` for `UiaRootObjectId` (flags `SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT`) and the registered message `UiaServerConnection_HookMessage` (flags 0), each waiting the connection timeout, and every other event waits behind it in the queue (`h1_base`, `h2_base`).

- The hold is twice the connection timeout per focus WinEvent, or until the window answers if sooner: 812 ms with the connection timeout at 500 ms; 3,806 to 3,821 ms at the default 2,000 ms; with 10,000 ms, 7,773 ms in an 8-second stall and 19,767 ms in a 20-second stall, each the end of the stall (`h3_*`).
- Each focus WinEvent naming the hung window pays the full wait again: three held 5,814 ms at 1,000 ms, one held 1,824 ms.
- Each client object in the process that holds a focus registration runs its own focus tracking on the same thread: two client objects with a desktop handler each made four calls (1,820 ms at 500 ms).
- With the connection timeout at 10 seconds, a 15-second stall held 14,800 ms, and a name change at a desktop-wide registration was not delivered within 25 s (E-hung-proxy).
- Verbatim sets the connection timeout to UIA's default 2 seconds (`CONNECTION_TIMEOUT`), so the listener's hold is up to about 4 seconds per focus WinEvent, as measured at 2,000 ms above.

#### Not affected

- A name-change WinEvent for the hung window held nothing (2 to 3 ms); only the focus WinEvent triggers it (`e_hung_cause`).
- A process with no focus registration of any kind: another application's name change arrived in 1 ms during the same stall (`e_hung_cause`; P4's `N_B`, 1 ms; in `e_cross`, a stand-in outpost's name and text events 0 to 1 ms).
- A hung application that no focus WinEvent names delays nothing (E-hung-proxy, 0 ms).
- Explicit reads on other threads of the held process: 1 ms throughout (E-hung-proxy).
- The held process's MSAA WinEvent hooks keep delivering: 0 ms (`e_cross`).
- A server-side provider held while raising its own event holds only its own events (E-delivery, section 11).

#### Workarounds that worked

- Verbatim's choice: only the listener holds a UIA focus registration; outposts hold none, so a hung window stalls the listener's UIA focus delivery only, and every outpost's UIA events, explicit reads and MSAA hooks keep working (`e_cross`). While it lasts UIA focus is not announced, as in NVDA; announcing from MSAA meanwhile was considered and rejected by Dickson (2026-10-10), because live checks showed MSAA could stand in for only some UIA focus changes and would sound different (the focus pipeline design, section 12.3).
- A shorter connection timeout bounds the hold, at two costs: it is one value for the whole process, so it also shortens every read there (see "The connection timeout applies to the whole process"); and a focus change in a window that stays busy past both of UIA's waits for it, each one connection timeout, is dropped, never delivered. In `h6_*`, an MSAA mockapp raised its focus from inside a 3-second stall: at 10,000 ms it arrived 3,003 ms into the stall, and at 500 ms and the default 2,000 ms it had not arrived 8 seconds after the stall began, though at 2,000 ms the stall was shorter than UIA's two waits together.
- Emptying the client's proxy factory mapping (`ClearTable`) removes the second call and halves the hold (1,812 ms), but leaves that client with no proxies for any Win32 window, so it is not used.

#### Workarounds that didn't work

- Removing only the focus entries from the proxy factory mapping: 14,789 ms against 14,801 ms (`e_mapping`, `e_hung_cause`). The mapping maps focus only for `SysListView32`, so the tree view's entry was never involved; the cause is UIA's focus tracking, not the mapping.
- The transaction timeout: 3,810 ms at 500 ms (`h3_tto500`). The first measurement's "both timeouts at 1 second", identical at 14,803 ms, had been undone by a later client creation that set 10 seconds for the whole process.
- A separate client object for focus, on the same thread or a thread of its own (815 to 819 ms at 500 ms, the same as one object); a separate client object for other events (there is still one queue thread).
- A single-threaded apartment (3,809 ms); `ConnectionRecoveryBehavior` on or off (3,818 and 3,821 ms); `CoalesceEvents` on or off (3,814 and 3,819 ms); an empty cache request (3,806 ms); a group-scoped focus registration in place of the desktop handler (3,809 ms).
- The older `CUIAutomation` coclass: its second call waited until the window answered, and its objects have no `IUIAutomation2`, so its timeout cannot be set at all (`h2_cui`).
- `SMTO_ABORTIFHUNG` never took effect here: Windows did not flag the stalled window hung even after 9 s with a message posted to it, presumably because no input reaches a hidden desktop (`h4_*`). For a window Windows does flag hung, the first call is inferred to return at once and the second, with flags 0, still to wait one connection timeout.

#### NVDA

NVDA is inferred to meet it: it is one process with a desktop-wide focus registration and its focus mapping left on (`docs/design/focus-pipeline.md`, section 12.3). No NVDA capture or issue records it.

#### Reproduction test

No test reproduces the hold, its absence in a process with no focus registration, Verbatim's workaround, or the dropped focus of `h6`: each would have to wait for evidence that UIA's two messages have reached the hung window, which needs a mockapp stall that names the messages it is sent, and mockapp has none. `a_provider_held_raising_its_event_holds_only_its_own_events`, in `crates/mockapp/tests/uia_bugs/a_hung_window_named_in_a_system_focus_event_holds_a_processs_uia_events.rs`, shows E-delivery's case under "Not affected", holding mockapp's provider in its call rather than stalling its thread; it passed 20 of 20 runs.

#### Evidence

[`experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/`](../experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md): experiments `h1` to `h6` (`h1_base`, `h2_base`, `h2_cui`, the `h3_*` variants including `h3_tto500`, `h4_*`, `h5_*` and `h6_*`) with the `focusproto` tool, P4 (including `N_B`), and the October experiments E-delivery, E-hung-proxy, `e_mapping`, `e_hung_cause` and `e_cross`, whose code is in [`experiments/mockapp-experiments-2026-10-10/`](../experiments/mockapp-experiments-2026-10-10/readme.md); `docs/design/focus-pipeline.md`, section 12.3, for the decision taken from them.

### The connection timeout applies to the whole process

#### Trigger

Any client object in a process sets `IUIAutomation2::ConnectionTimeout`.

#### Symptoms

The value applies to every client object in the process, and the most recent setting by any of them wins. An explicit read on a client object that had set 500 ms waited 10,013 ms after another client object in the process set 10,000 ms; the reverse case waited 508 ms (`h5_*`). A client object created later that sets nothing does not reset it. The transaction timeout behaves the same: a client set both to 1,000 ms, and a client created earlier on another thread then read 1,000 and 1,000, where it had read 10,000 and 20,000 before (E1, section 11).

So one client that sets a long timeout lengthens every other client's waits: a client creation that set 10 seconds explained a 10-second `GetFocusedElement` in File Explorer (E1).

#### Not affected

Not measured across processes; each process has its own value (inferred from the per-process results above).

#### Workarounds that worked

- Verbatim's choice: both timeouts are set once per process, to `CONNECTION_TIMEOUT` (2 seconds) and `TRANSACTION_TIMEOUT` (5 seconds), during UIA's first-time setup in `crates/verbatim-uia/src/client.rs`, and nothing else sets them. Tests that need a different value change it for the whole test process and run alone (`TRANSACTION_TIMEOUT` in `crates/mockapp/tests/remote_ops.rs`, and the runner's process per test).

#### Workarounds that didn't work

- A shorter timeout per client or per call: any other client's setting replaces it (E1).

#### NVDA

NVDA leaves UIA's defaults (focus pipeline section 6.1), so it does not meet this.

#### Reproduction test

These are in `crates/mockapp/tests/uia_bugs/the_connection_timeout_applies_to_the_whole_process.rs`; each passed 20 of 20 runs:

- `timeouts_set_through_one_client_are_read_by_another` reproduces E1: both timeouts set through one client are what a client made before, on another thread, reads.
- No test reproduces `h5`'s waits, a call on one client waiting the connection timeout another client set last: Dickson decided (2026-10-19) not to keep tests for them in this milestone, so they are recorded here with their figures.
- `the_first_client_in_a_process_has_verbatims_timeouts` and `making_a_client_keeps_the_timeouts_another_set` show that Verbatim's workaround holds. With the setup's setting removed, the first failed 20 of 20 runs, reading UIA's defaults of 2,000 and 20,000 ms; with client creation setting the timeouts as well, the second failed 20 of 20.

#### Evidence

[`experiments/the-connection-timeout-applies-to-the-whole-process/`](../experiments/the-connection-timeout-applies-to-the-whole-process/readme.md): experiments E1 and `h5`, whose files are kept with the hung-window fault's experiments and the mockapp experiments; `docs/design/focus-pipeline.md`, section 11, E1.

### Desktop-wide focus handlers in a second process deliver late

#### Trigger

Two or more processes each hold a desktop-wide `AddFocusChangedEventHandler`, and focus changes come from a window UIA proxies (mockapp's MSAA backend, a focus every 150 ms).

#### Symptoms

Every process after the first receives the focus events late. With one process, at most 9 to 10 ms after the raise; with two, the second process at most 682 and 1,112 ms (two runs); with four, 670 to 2,677 ms for processes 2 to 4 (`p2c`). At a 500 ms pace the late processes showed waits of about 1,000, 1,500 and 2,000 ms, and some events had not arrived 0.5 s after the last raise (`p2b`). The cause is not known.

#### Not affected

The same number of handlers in one process were never late: 2 and 4 handlers, at most 9 to 11 ms.

#### Workarounds that worked

None was tried. Verbatim holds a desktop-wide focus handler in the listener only, so it is inferred not to meet this with itself; beside another screen reader, which holds one too, it is inferred to be exposed.

#### Workarounds that didn't work

None was tried.

#### NVDA

Not known. Whether it happens with a real Win32 application is not shown: focus WinEvents raised for a real window without the keyboard focus reached no desktop-wide handler at all (`p2d_real_win32.py`), so the real-application case needs real keyboard focus.

#### Reproduction test

No test reproduces it yet; the experiment is P2c.

#### Evidence

[`experiments/desktop-wide-focus-handlers-in-a-second-process-deliver-late/`](../experiments/desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md): experiment P2's delivery timings, `p2b`, `p2c` and `p2d`.

### A new process's focus events reach the desktop-wide handler late or not at all

#### Trigger

A newly started application, a File Explorer folder window in its own process or the Settings app, moves its focus while a desktop-wide `AddFocusChangedEventHandler` is registered in another process, Verbatim's listener.

#### Symptoms

The listener's `WinEvent` hook sees the focus move, and UI Automation delivers the matching focus events late, or none of them: none of a new folder window's items, or none of its first 50 to 130, in some `measure_repeat_explorer` runs (`docs/design/listener-without-connections.md`, decision 2, risk 2); in `settings_system_page`, the `CoreWindow`'s focus `WinEvent`s (object ids -4 and 205) with no UIA focus in 14.5 seconds, where it usually follows within 1 to 35 ms. Delivered late, the events can arrive in a burst after their elements are gone, and every call on them fails with `UIA_E_ELEMENTNOTAVAILABLE` (risk 13). The cause is not known.

#### Not affected

Not established. In steady state a UIA focus follows its `WinEvent` in the same millisecond at the median in File Explorer, but a running process was affected too: the first folder window of `measure_focus_explorer` had its UIA focus about a second after its `WinEvent` focus each time it took the focus back after a new window closed.

#### Workarounds that worked

Not a workaround of the fault but a recovery from it (Dickson, 2026-10-21): the application's outpost, which sees the `WinEvent` focus copy, waits a second from the copy's observation for its UIA focus and otherwise reads the application's focus itself, at once when the UIA focus came after its sender was gone, with no window above it (decision 11 of the same design). The full suite's `conhost_two_windows` has met the late delivery, its second console window's UIA focus coming 2.15 seconds after its `WinEvent` focus, and the read at the bound reported the focus with its window. The burst's recovery is tested in mockapp.

#### Workarounds that didn't work

An early connection to the new window from another process (decision 2's `watchnew` runs) did not prevent the burst; an outpost's connection at its first sight of the window lowered its rate without removing it.

#### NVDA

By its source, NVDA loses these focuses: it drops a `WinEvent` focus in a native UIA window and a UIA focus whose element has no window (`docs/parity/focus-events.md`, "A focus UIA does not deliver"). Not captured.

#### Reproduction test

None: the fault does not reproduce reliably, and no mockapp simulation is built for such a fault (Dickson, 2026-10-19). The recovery's tests are `focus_reports.rs`'s `a_winevent_focus_` tests.

#### Evidence

None is kept in the repository; the fault was met in the end-to-end scenarios `measure_repeat_explorer`, `measure_focus_explorer` and `settings_system_page` (`docs/design/listener-without-connections.md`, decision 2, risks 2 and 13).

### A registration that times out reports success

#### Trigger

`AddEventHandlerGroup` on a window whose provider does not answer, because a provider call is held (in `FragmentRoot`) or the window thread takes no message (a stall), with a timeout shorter than the hold: the transaction timeout at 1 second in E-TT. In the test below, the registration's wait followed the connection timeout instead: 540 to 549 ms with the connection timeout at 500 ms and the transaction timeout at 5,000 ms, and 5,453 to 5,504 ms the other way round. A group holding UIA's focus event, registered during a stall, also returns after the connection timeout, whatever the transaction timeout: 532 to 551 ms at 500 ms, 2,190 to 2,226 ms at 2,000 ms, and 5,514 to 5,541 ms at 5,000 ms (e57).

#### Symptoms

The registration returned success after 1,089 ms, though the provider never answered (E-TT, section 11).

Such a registration delivers nothing until about 2 seconds after its call returned (1,981 to 2,001 ms), or until the stall ends if that is later, and then delivers every event raised. An event raised after the stall has ended but before that point is lost: it is never delivered, late or otherwise. The point follows neither the connection timeout (500, 2,000 and 5,000 ms) nor the transaction timeout (500, 5,000 and 20,000 ms), and the next event raised does not bring it forward. A registration made in a second process behaves the same as one made in the test's own process. These figures come from e57, 180 trials.

The registration's own request waits for the stalled thread and runs as the stall ends: a provider that is told of registrations (`IRawElementProviderAdviseEvents`) was advised of it then, once, and no second advise came (e58, 10 trials). With no event raised, nothing reached the application in the 6 seconds after the stall's end: no advise, provider call, sent message or posted message (e58, 5 trials). With a focus event raised every 100 ms, the registration began to deliver about 2 seconds after its call returned, and the events raised in the gap reached the application's UIA side, which made provider calls and took messages after each one (e58, 5 trials). That those events are then lost in the registering client is inferred, not observed: e58 watched nothing on the client's receiving side, and state kept per client process in the application's UIA side would fit the same results.

The gap silences every registration the registering process has on that window, including one made before the stall on another client object, which loses the same events. A registration on another window in the same process, and one on the same window in another process, hear every event (e58, 5 trials).

During the gap, `RemoveEventHandlerGroup` on the registration fails at once with `UIA_E_TIMEOUT` and leaves it in place: the handler had not been freed 15 seconds later, and the registration delivers once the gap ends. Removed after the gap, it is removed at once and its handler released (e58, 5 trials each).

#### Not affected

Single reads under the same timeout fail as expected: `CurrentName` after 1,095 ms and `CurrentHasKeyboardFocus` after 1,092 ms, each with `UIA_E_TIMEOUT` (E-TT).

#### Workarounds that worked

None is built. With the outposts' connection timeout at 2 seconds, Verbatim's exposure was measured on new File Explorer windows, which are busy 2.7 to 3.9 seconds as they open: in 20 runs (`measure_repeat_explorer` and `explorer_folder_window`, 10 each, 2026-10-19) the outposts made 3,621 registrations, the longest 32 ms, and 3,602 removals, every one successful (`docs/design/listener-without-connections.md`, section 10, step 8), because an outpost registers on an element only once its focus has been read. Proposed: treat a registration that took longer than the timeout as failed and make it again (section 11, E-TT), or, for a hand-over of an application's focus, trust a registration only once its first event has arrived (section 13.2). The first leaves the failed registration to remove, and no safe time to remove it is known: a removal during the gap fails and leaves it delivering, nothing marks the gap's end, removal after the gap was tried at one time only, 3 seconds after the stall's end (e58, 5 trials), and whether a removal retried after `UIA_E_TIMEOUT` succeeds is not tested. Verbatim makes that retry once: a group a move failed to remove is kept and tried again when its registration closes (`Follower::close` in `crates/verbatim-uia/src/subscribe.rs`), since removing every live group is what a close means; its outcome is untested for the same reason, and a failed removal is logged at warn whatever its time, so a group left live and delivering shows at the default level. The second rests on what e57 shows: a registration that timed out does start to deliver, and it loses no event raised after its first delivered one. That first event comes no sooner than about 2 seconds after the registration returned. Until then the registration loses every event raised, so whatever served the window before must go on serving it, and an application that raises no event never confirms the registration. Whether a second registration made after the stall ends delivers at once is not tested.

#### Workarounds that didn't work

None was tried.

#### NVDA

Not known.

#### Reproduction test

`a_group_registered_while_its_provider_is_held_reports_success` in `crates/mockapp/tests/uia_bugs/a_registration_that_times_out_reports_success.rs` reproduces it, with both timeouts at 500 ms: the registration returns success while mockapp still holds its provider in `FragmentRoot`. `a_read_while_its_provider_is_held_fails`, beside it, shows the case under "Not affected": a name read, held in `GetPropertyValue`, fails with `UIA_E_TIMEOUT` while mockapp still holds its provider, with the transaction timeout at 500 ms and the connection timeout set longer than the test waits, so the transaction timeout is what bounds a read on a held element (e59). Each passed 20 of 20 runs.

No test reproduces the loss of events raised after the stall. Nothing marks the end of the gap: in e58, with no event raised, nothing reached the application in the 6 seconds after the stall's end. So a test cannot wait for evidence that the gap is over, and without that it cannot tell an event lost in the gap from one delivered late. The loss rests on e57 and e58.

#### Evidence

[`experiments/a-registration-that-times-out-reports-success/`](../experiments/a-registration-that-times-out-reports-success/readme.md): experiments E-TT, e57 and e58; `docs/design/focus-pipeline.md`, section 11, E-TT.

### A remote operation's keyboard focus read can differ from a plain read

#### Trigger

Windows Terminal raises a focus event for its terminal from its second identity (see "Windows Terminal raises focus from a second runtime identity"), here for a second window opened in its running process, and the listener reads the sender's `HasKeyboardFocus` inside its remote operation.

#### Symptoms

Inside the remote operation the sender answered that it had the keyboard focus; a plain property read of the same element answered that it had not, as NVDA's read found 9 ms after it had taken the terminal's focus from its usual identity. The listener took the second identity as a new focus, and "terminal" was said twice (`windows_terminal_two_windows` in each of three runs, `windows_terminal_leave_flood` in one). That the imported element inside the operation is the provider's own and answers for itself, where the plain read is answered otherwise, is inferred, not shown.

#### Not affected

The usual identity's focus read the same both ways in those runs.

#### Workarounds that worked

- Verbatim's choice: the listener reads the sender's keyboard focus with a plain property read after the acceptance test, as NVDA does, and drops the focus when it has none; one call more per focus (ab18245e; `docs/performance.md`). The remote operation's own check stays, for a focus that moves on while it is read.

#### Workarounds that didn't work

None was tried.

#### NVDA

NVDA reads the sender's keyboard focus plainly in `shouldAllowUIAFocusEvent` (live since [NVDA #20764](https://github.com/nvaccess/nvda/pull/20764), from the event's cache before that), and the second identity answered no either way (focus pipeline section 16.9).

#### Reproduction test

No test reproduces it yet; mockapp has no second identity. `crates/mockapp/tests/focus_reports.rs` and `call_counts.rs` pin the plain read and its cost.

#### Evidence

`docs/design/focus-pipeline.md`, sections 16.8 and 16.9; commit ab18245e.

### Inside a remote operation, conhost's window has no handle or title

#### Trigger

A remote operation reads the ancestors of a focus in conhost's text area, which has no window of its own, up to conhost's window element (runtime id `[42, hwnd]`).

#### Symptoms

Read inside the operation, the window element has no native window handle and no name; only the client side's window proxy supplies them. The outpost reported the focus with no window, so it was never treated as a terminal, which is decided by the console window's class, and Core ignored it (3f4dbb44); with the handle supplied, the window still had no name, so the outpost took it for layout and dropped it, and only "terminal" was spoken (`conhost_short_output`, 1b2e5420). Whether the fault is conhost's provider's or UIA's is not established.

#### Not affected

- mockapp's window elements, whose root provider names its host (`UiaHostProviderFromHwnd`): UIA merges the host's title and handle into a read inside the provider too (1b2e5420).
- The client side's read of the same window, through the proxy.

#### Workarounds that worked

- Verbatim's choice: the listener reads every focus by the classic walk, through the client side, so its ancestors come through the window proxy. An outpost's read of its application's focus, the focus-now query's and the read after a `WinEvent` focus no UIA focus followed, reads by the remote operation, and reads again through the window's proxy any ancestor that is a window's own element (runtime id `[42, handle]`) read without its handle, one `ElementFromHandleBuildCache`. Without it, such a read in `conhost_two_windows`'s second window reported the terminal with no ancestors, so the window went unannounced; a probe against a real console host showed the operation's window element nameless, with handle 0 and the disabled state, and the proxy's read of it named, with its handle, as the classic walk reads it.

#### Workarounds that didn't work

None was tried.

#### NVDA

NVDA reads the raw-view parent through the proxy (`nvda/source/NVDAObjects/UIA/__init__.py`, lines 2136 to 2141), so it is inferred not to meet it.

#### Reproduction test

No test reproduces it yet: mockapp's provider cannot produce an in-provider window element without its host's properties, and a root with no host provider fails UIA's own search of the window (1b2e5420). The outpost's read again is tested by its choice of ancestor (`read.rs`, `a_window_element_read_without_its_handle_is_named_by_its_runtime_id`), and the console window reported with a read after a `WinEvent` focus by `focus_reports.rs`'s `a_winevent_focus_in_a_console_window_no_uia_focus_follows_is_read_with_its_window`, which passes without the read again, since mockapp's window element has its title in the provider.

#### Evidence

Commits 3f4dbb44 and 1b2e5420.

### A cache request fails while UIA's first-time setup runs on another thread

#### Trigger

In a process where UIA has not been used yet, several threads create clients and build cache requests at once, or one thread calls the provider probe while another builds a cache request.

#### Symptoms

While UIA's first-time setup runs on one thread, `CreateCacheRequest` on another fails with `E_FAIL`. With six threads creating clients at once in a fresh process, five failed nearly every time; it made a mockapp arbitration test fail about one run in five (7c402534). With Verbatim's workaround removed, on 2026-10-18, one fresh process racing from 2 to 16 threads met no failure in about one race in 10 to 30, and the test below, which races six threads in each of four fresh processes, failed 50 of 50 runs with `E_FAIL` (0x80004005).

#### Not affected

A call 50 ms later succeeded, and no call failed once a first client had built a cache request (7c402534).

#### Workarounds that worked

- Verbatim's choice: every client creation and the provider probe first wait for UIA's first-time setup, which creates a client and builds a cache request once, under a lock, on a thread of its own, with COM's multithreaded apartment kept for the life of the process (`ensure_ready` in `crates/verbatim-uia/src/client.rs`).

#### Workarounds that didn't work

None was tried.

#### NVDA

NVDA creates its UIA client on one thread, so it is inferred not to meet it.

#### Reproduction test

`clients_created_at_once_in_a_fresh_process_all_build_cache_requests` in `crates/mockapp/tests/uia_bugs/a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread.rs` shows that the workaround holds: it passed 20 of 20 runs with it, and failed 50 of 50 without it.

#### Evidence

Commit 7c402534.

### A process that used UIA can hang or crash as it exits

#### Trigger

A process that has connected to UIA providers as a client exits normally, running its DLLs' process-detach code.

#### Symptoms

The process sometimes hangs at full processor use, or crashes with an access violation, after its work is done. Resolved with Microsoft's public symbols, the fault is in `UIAutomationCore.dll`'s own detach code: destroying its global telemetry object (`TelemetryUtility::m_instance`) walks a linked list of per-provider connection records that is corrupt by then, and the walk never ends or reads freed memory. It happened in about 1 to 3 percent of runs of the arbitration tests, and failed CI on GitHub's runner. What corrupts the list is not known.

#### Not affected

A process that never connected to a provider, whose list stays empty (inferred from the mechanism).

#### Workarounds that worked

- Verbatim's choice: its processes end themselves with `TerminateProcess` once they have shut down, so the detach code never runs: outposts and the listener (`docs/architecture.md`, section 1), and the mockapp test runner after printing its results (`crates/mockapp/tests/common/harness.rs`).

#### Workarounds that didn't work

- Releasing every UIA object first.
- Waiting after every provider had gone: 12 seconds still left 2 hangs in 100 runs.
- Fixing UIA's first-time setup race: the arbitration test binary hung in 1 run in 40 before and 1 in 100 after.
- `CoUninitialize`, which does not end UIA's own CoreMessaging thread.

Six threads each connecting to a mockapp of its own hung 1 in 40, against 0 in 40 for one thread, too few runs to tell whether concurrent connections corrupt the list.

#### NVDA

NVDA exits the same way after releasing its UIA client, so it is inferred to be exposed.

#### Reproduction test

No test reproduces it reliably; at 1 to 3 percent of runs it is not a test.

#### Evidence

Commits 1f69859e and fed532be (2026-10-03), which recorded the fault and made the mockapp test runner end its process without detach code.

## Limitations

These are not faults: UIA works as designed, or as far as is known, but the design rules out something Verbatim would have used.

### Focus events cannot be limited to one application

The documented call is refused: `IUIAutomation::AddAutomationEventHandler` for `UIA_AutomationFocusChangedEventId` on one window fails with `E_INVALIDARG` and UIA's message "Focus events must be registered using IUIAutomation::AddFocusChangedEventHandler" (E-scope, section 11; P1, 3 of 3). Two undocumented routes are accepted: the focus event in an event handler group registered on a window's element with `TreeScope_Subtree`, and the core API's `UiaAddEvent` on a window's node. Each delivered that window's provider-raised focus events and no other application's (E-scope; P1, 3 of 3; P5, 2 of 2), but neither received any focus event of a window UIA reads through its proxy, where the desktop-wide handler received both in every run (P1, 3 of 3; P5). So a scoped focus registration cannot serve classic Win32 applications (inferred; confirming it needs a real application with real keyboard focus). Each registered process also adds about 47 to 49 provider calls per focus event, on the application's UI thread (P2; E-cost). Evidence: [`experiments/focus-events-cannot-be-limited-to-one-application/`](../experiments/focus-events-cannot-be-limited-to-one-application/readme.md), experiments E-scope, E-cost, P1, P2 and P5; `docs/design/focus-pipeline.md`, section 11, E-scope and E-cost. Tests, in `crates/mockapp/tests/uia_bugs/focus_events_cannot_be_limited_to_one_application.rs`, each passing 20 of 20 runs: `a_focus_handler_on_one_window_is_refused`, `a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone`, `a_focus_event_in_a_group_on_a_proxied_window_hears_none_of_its_focus` and `a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone`; the last has a second core API registration, on the other application's window, feed the same callback, so that application's events are known to reach the process, and asserts the exact sequence of senders, in which one heard by the first registration too would be an extra; with a duplicate registration on the other window in its place, it failed 20 of 20 runs. E-cost's and P2's call counts vary from one event to the next, so no test pins them.

On a desktop where no window has the keyboard focus, as in mockapp's tests, desktop-wide focus registrations receive only the focus events UIA derives from a WinEvent, and scoped ones receive provider-raised focus (E-scope; P1).

### Elements cannot be passed to another process

`CoMarshalInterface` of an `IUIAutomationElement` with `MSHCTX_LOCAL` succeeded, but `CoUnmarshalInterface` of those bytes in another process failed with `E_FAIL`, in three runs; the free-threaded marshaler's in-process form failed there with `E_UNEXPECTED`. Within the owning process the same bytes gave a working proxy, and an event handler object of Verbatim's own marshaled to the other process worked until a call passed it an element. No route, documented or not, was found for giving another process an event's sender, so the process that receives a UIA event is the only one that can read from its element; this is why the listener judges and reads each focus itself. A sender stays usable in its own process for as long as the provider keeps the element (P3). Evidence: [`experiments/elements-cannot-be-passed-to-another-process/`](../experiments/elements-cannot-be-passed-to-another-process/readme.md), experiments E-marshal and P3; `docs/design/focus-pipeline.md`, section 11, E-marshal. Tests, in `crates/mockapp/tests/uia_bugs/elements_cannot_be_passed_to_another_process.rs`, each passing 20 of 20 runs: `an_element_marshaled_for_another_process_cannot_be_unmarshaled_there`, `an_element_marshaled_for_another_apartment_cannot_be_unmarshaled_in_another_process`, `an_object_passed_to_another_process_works_until_a_call_passes_it_an_element` and `a_kept_sender_is_read_live_until_its_provider_removes_it`, which pins P3's removal and exit but not its reads after 31 and 152 seconds.

### Ancestors cannot be cached or listened to

An event registration scoped to an element's ancestors (`TreeScope_Ancestors` or `TreeScope_Parent`, with or without the element itself) receives no ancestor's events: on Windows 11's taskbar, a registration on the clock button's child did not hear the clock button's name change each minute, while one on the clock button itself did; mockapp gave the same result, made as an event handler group and directly (`docs/parity/backends.md`, the local group). Microsoft's documentation of [`IUIAutomationCacheRequest::TreeScope`](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope) also excludes the parent and ancestors from a cache request, so an element's ancestors are read by a walk or a remote operation; Verbatim has not tested that restriction itself.

### A timeout bounds each provider call, not an operation

Under a 1-second transaction timeout, a `FindFirst` over a window's descendants held 13,208 ms before failing with `UIA_E_TIMEOUT`, waiting a full timeout for each provider call the held thread did not answer (E-TT, section 11; its experiments folder is shared with "A registration that times out reports success", [`experiments/a-registration-that-times-out-reports-success/`](../experiments/a-registration-that-times-out-reports-success/readme.md)). A remote operation is one transaction, so the timeout bounds it as a whole (`a_stalled_provider_holds_execute_until_the_transaction_timeout` in `crates/mockapp/tests/remote_ops.rs`). Operations made of many calls are run with a deadline of their own on a thread that can be abandoned (`crates/verbatim-uia/src/bounded.rs`). No test shows it: Dickson decided (2026-10-19) not to keep tests for this limitation in this milestone, so it is recorded here with its figures: `FindFirst` failed with `UIA_E_TIMEOUT` after three timeouts with both timeouts set to 100, 200 or 500 ms, and after four at 1,000 ms.

## Applications

Faults in applications' providers rather than in UIA, which Verbatim or NVDA has had to handle.

### File Explorer reuses a destroyed element's runtime ID

#### Trigger

Going back from a subfolder in File Explorer.

#### Symptoms

A new element took the runtime id of one File Explorer had destroyed: the parent folder's item Inner took the runtime id of the subfolder's item delta.txt. The outpost mapped it to delta.txt's node, Core took the focus for the one it already had and stayed silent, and the selection that followed spoke only "selected" (c67806c5).

#### Not affected

Not established.

#### Workarounds that worked

- Verbatim's choice, as NVDA judges a duplicate: before reporting a UIA focus whose runtime id already names a node, the outpost reads whether that node's element still has the keyboard focus, in the remote operation that reads the focus's ancestry (one call of its own when it reads classically), and when it has not, or cannot be read, gives the id a new node (c67806c5). UIA fails a whole remote operation whose imported element is gone before its first instruction, so the operation is run once more without the held element and answered as not focused.

#### Workarounds that didn't work

None was tried.

#### NVDA

NVDA treats a focus event as a duplicate only while the last focus still has the keyboard focus, read live, so it does not meet it.

#### Reproduction test

No test reproduces File Explorer's fault. mockapp's `take-runtime-id` command imitates it, and `a_reused_runtime_id_keeps_its_node_with_its_element_gone_remote` and `a_reused_runtime_id_keeps_its_node_with_its_element_gone_classic` in `crates/mockapp/tests/focus_reports.rs` test Verbatim's handling of it.

#### Evidence

Commit c67806c5.

### Windows Terminal raises focus from a second runtime identity

#### Trigger

Windows Terminal starting, or opening a second window in its running process.

#### Symptoms

Windows Terminal's XAML raises some focus events for its terminal from a second identity of the same control, a runtime id that starts 14 and carries a counter, instead of the usual one that starts 42 with the window's handle; `CompareElements` says the two differ, so a client announces it as a new focus, and "terminal" is said twice, or three times when the focus flips from the usual identity to the second and back. Measured with Verbatim's registration: raised in 2 of 150 launches and 19 of 150 second windows; NVDA receives it in second windows too, 6 times in 15 pairs. Locally the second identity's plain keyboard focus read almost always answers false; on GitHub's slower runner it answers true more often, and the flip is commoner there. An early second identity, raised before the terminal's island is joined to its window, has no window above it. Whether UIA delivers it at launch depends on the client's registration: a client whose focus handler is registered alone, or after its event handler group, received it at launch, and one registered in NVDA's order, the desktop root fetched first, then the focus handler, then the group on that root, did not (focus pipeline section 16.8).

#### Not affected

No registration prevents the second window's second identity: its rate was the same with NVDA's whole global group (`docs/design/windows-terminal-twin-focus.md`, "Rates").

#### Workarounds that worked

- The listener registers in NVDA's order (`crates/verbatim-uia/src/subscribe.rs`), drops a focus with no window above it, and drops one whose plain keyboard focus read answers no (see "A remote operation's keyboard focus read can differ from a plain read"). A rule that recognizes the second identity as the twin of the current focus is designed for a Windows Terminal app module in M5 (`docs/design/windows-terminal-twin-focus.md`); until then the scenarios accept one repeated "terminal" as an optional utterance (`docs/testing.md`).

#### Workarounds that didn't work

- Registering through one handler object for the focus handler and every part of the group, as NVDA does: 2 of 150 launches and 11 of 150 second windows, 2 launches announced twice.
- NVDA's whole global group on the root: none of 150 launches, but 19 of 150 second windows.
- Registering the group through a second client in the same process (received 3 times), and the client's age, NVDA's local groups, and making an element from each new window's handle as it appeared (section 16.8).

#### NVDA

NVDA receives it in second windows and takes none: its `shouldAllowUIAFocusEvent` finds that the second identity has no keyboard focus (section 16.8 and 16.9).

#### Reproduction test

No test reproduces it; mockapp has no second identity. The identity judge in its experiments folder measures it live.

#### Evidence

[`experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`](../experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/readme.md): the identity judge and its three 150-round runs; `docs/design/windows-terminal-twin-focus.md`; `docs/design/focus-pipeline.md`, sections 16.8 and 16.9.

### A Qt application's cached keyboard focus can be stale

#### Trigger

Page Up or Page Down in WeChat's conversation list, whose Qt UIA provider raises an intermediate focus event for the list container before the item's.

#### Symptoms

The keyboard focus cached with the container's event can still say it has the focus when the event is judged, so a client that judges by the cached value announces the container before the item ([NVDA #20764](https://github.com/nvaccess/nvda/pull/20764), fixing [NVDA #20763](https://github.com/nvaccess/nvda/issues/20763)).

#### Not affected

Up Arrow and Down Arrow ([NVDA #20764](https://github.com/nvaccess/nvda/pull/20764)).

#### Workarounds that worked

- NVDA reads the sender's keyboard focus live when it judges a focus event ([NVDA #20764](https://github.com/nvaccess/nvda/pull/20764), in the pinned source). Verbatim's listener reads it plainly too (ab18245e).

#### Workarounds that didn't work

None recorded.

#### NVDA

This is NVDA's record; Verbatim has not met it.

#### Reproduction test

No test reproduces it; Verbatim has no Qt application in its tests.

#### Evidence

NVDA commit [3ca80a5fa](https://github.com/nvaccess/nvda/commit/3ca80a5faaee941bb4deb76728088cef091012e0) ([NVDA #20764](https://github.com/nvaccess/nvda/pull/20764)).

### Excel 2016 freezes when asked for its provider in response to its own events

#### Trigger

In certain builds of Excel 2016, calling `UiaHasServerSideProvider` on the `EXCEL7` window in response to the MSAA WinEvents Windows proxies from Excel's UIA events.

#### Symptoms

A freeze of several seconds (NVDA's comment, `nvda/source/IAccessibleHandler/internalWinEventHandler.py`, lines 139 to 150).

#### Not affected

Not recorded.

#### Workarounds that worked

- NVDA drops proxied WinEvents for `EXCEL7` windows with a positive object id early, before asking.

#### Workarounds that didn't work

None recorded.

#### NVDA

This is NVDA's record; Verbatim has not met it.

#### Reproduction test

No test reproduces it.

#### Evidence

`nvda/source/IAccessibleHandler/internalWinEventHandler.py`, lines 139 to 150.
