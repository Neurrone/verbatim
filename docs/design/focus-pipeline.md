# How events reach Verbatim and are processed: a design aligned with NVDA

Status: built. This document keeps the problem, the options weighed, the reasons for the choices and their limits. The mechanism is described in `docs/crates/verbatim-outpost.md` ("The hand-over of an application's MSAA events", "A UIA focus, judged and read by the listener", the spare outposts, the intake and the worker), `docs/crates/verbatim-uia.md` (registrations, time limits, the probe) and `docs/crates/verbatim-core.md` (the duplicate rule and acceptance). The experiments behind it are under `experiments/`, chiefly `mockapp-experiments-2026-10-10/`, `elements-cannot-be-passed-to-another-process/`, `focus-events-cannot-be-limited-to-one-application/`, `a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/` and `windows-terminal-raises-focus-from-a-second-runtime-identity/`; the UIA defects they found are in `docs/uia-bugs.md`. Section numbers are cited from code and other documents, so they are kept stable.

## 1. The problem

A UIA focus event's sender, the element that raised it, exists only in the process that received the event: COM cannot marshal an `IUIAutomationElement` to another process (section 11, E-marshal). NVDA judges and reads a focus from its sender. An outpost that never has the sender must find the element again, with `GetFocusedElement` or a search by runtime id, name and position, and judge the focus after it has often moved on; and an application's focus arriving by one route while its other events arrive by another lets an event raised after a focus be handled before it. Each of these had been patched where it showed up. This design removes the cause.

Dickson's constraints for any design:

- something must still notice a new application, so its outpost is started;
- the same event must not be processed twice, by the listener and by an outpost;
- a slow application must never stall the listener or another application.

## 2. Where focus is judged and read: the options

### 2.1 Option (a): the listener reads only the sender's keyboard focus, the outpost finds the element

The judgement would be made on the sender, but the outpost would still need the searches, and Windows Terminal raises its tabs' focus from elements that are not in its tree, so the search finds the wrong ancestors. Two processes would call into the application for one event. Rejected.

### 2.2 Option (b): the listener judges and reads the whole focus, on a thread per application

The judgement and the read are made on the sender, so the ancestors are the sender's own. The cost is that the outpost never holds the focus element, which it needs for its subscriptions, text, caret, review and navigation, and must find its own copy (section 12.5). This is the option built (sections 12 and 15).

### 2.3 Option (c): each outpost subscribes to focus events for its own application

UIA's focus registration, `AddFocusChangedEventHandler`, is desktop-wide; the direct call that would scope it to a window is refused, and only an undocumented route through an event handler group delivers one window's focus events (section 11, E-scope). Two variants were weighed:

- (c2), a desktop-wide registration in every outpost: each registration adds 47 to 90 provider calls on the focused application's UI thread per focus event (E-cost), multiplied by the number of outposts. Ruled out.
- (c1), each outpost's registration scoped to its own windows, with one "standby" spare outpost holding the desktop-wide registration and buffering the focus events of applications without an outpost. Rejected by Dickson on 2026-10-10: the standby discards the buffered events of every application it is not given to, and discarding events is a correctness bug.

## 3. One route per application

Every MSAA event of an application, focus-shaped ones included, reaches its outpost through the outpost's own process-scoped hook, in the order Windows raised them; the listener forwards them only until that hook is in (section 12.4). A UIA focus is judged and read by the listener, and its result follows a marker through verbatim.exe into the outpost's intake at the place the event arrived (section 12.4). So an application's events are handled in one order, as NVDA's one queue handles them.

## 4. One ordered input to Core, and NVDA's duplicate rule

### 4.1 One channel

Core takes outpost events and key commands from one input, in the order they reach it, as NVDA queues gestures with events. The order is arrival, not observation time: ordering by observation would need a reorder buffer that waits for slower outposts, a delay on every input, and NVDA does not do it. Arrival order already puts a key before the application's reaction to it. What the observation time decides costs no wait: a focus from another outpost observed before the focus Core has applied is dropped as it arrives (decision D14; `docs/parity/focus-events.md`, "Stale focus events").

### 4.2 NVDA's duplicate rule, defined once and judged against the focus Core holds

NVDA ignores a focus event whose object equals the last queued focus while it still has the keyboard focus, for any application. The rule is defined once, in `verbatim_core::focus_rules::repeats_focus` (decision D14, "Where the rules run"): a focus repeats when its event names the element of the focus Core holds, which still has the focus, and Core has judged every focus the judge sent it. Only the process that holds an element can read it live, and only Core knows the focus of every application, so each judge gathers the evidence its own way and judges against the focus Core holds, which Core sends after every focus event it judges and every change of its focus (`SupervisorToOutpost::CoreFocus`):

- Core: the node id, which names one element in one outpost incarnation, and the focus alive.
- The listener, before it reads a UIA focus: `CompareElements` with the element of the focus Core holds, when Core took that focus from one of the listener's reads, and that element's keyboard focus read live (section 16.3).
- An outpost, before it reads an MSAA focus: the event's address against the focus Core holds, a whole object rather than a child by id; and NVDA's two foreground tests.
- While a focus a judge sent waits for Core's judgement, Core's focus can yet become it, so the judges drop only what repeats whatever Core judges; a repeat that comes in that time is read, and Core's own rule drops it.

## 5. The MSAA side

NVDA's limiter, the newest 10 events per thread and 4 focus events per flush, applies only to WinEvents; UIA events are only coalesced one per element and kind, so Verbatim's outposts count only MSAA entries against those limits. The per-thread count is keyed by the thread that raised the event, which differs from the window's thread for console windows (decided by Dickson). Each consecutive run of MSAA focus and foreground entries is tried newest first, at its own place among the events around it, as NVDA's pump does, rather than the newest of the whole batch; a focus is never dropped merely because a newer one is waiting, since NVDA's pump cannot see a newer one either. The mechanism, including the order of a run whose events carry their raise times and what ends a run, is in `docs/crates/verbatim-outpost.md`, the intake.

### 5.4 Console windows

Every event for a console window goes to the console host's outpost, with the client process as the application's identity, as NVDA's app module does (decided by Dickson). The listener finds the console host from the event's own thread, a local call, for a window of class `ConsoleWindowClass`. So a console window is announced once, with one name, and its application is the shell.

## 6. Time limits

### 6.1 UIA: one transaction timeout and one connection timeout per process

Both timeouts are process-wide (E1), so each process sets them once, at its start, and no new client resets them; since an outpost serves one application, a process-wide setting is a per-application one. The values are `verbatim_uia::CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT` (`docs/crates/verbatim-uia.md`), below the worker's deadline so that a slow call fails and the worker carries on instead of being abandoned (Dickson, 2026-10-10). The transaction timeout bounds each call, not an operation (E-TT: a `FindFirst` held 13.2 seconds under a 1-second timeout), so an operation of many calls gets a deadline of its own or becomes one remote operation; and a registration that times out reports success (`docs/uia-bugs.md`).

### 6.2 MSAA calls

An `IAccessible` call has no bound but the worker's watchdog, which abandons the worker without cancelling the call. NVDA's watchdog cancels its main thread's call with `CoCancelCall`; `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/` records what a cancel reaches.

### 6.3 Window messages

Every window message an outpost sends goes through `SendMessageTimeoutW` with `SMTO_ABORTIFHUNG` (`docs/crates/verbatim-ia2.md`).

### 6.4 Watchdogs for every thread others wait on

The worker and each of the listener's reading threads have a watchdog that abandons and replaces the thread past its deadline. The registering thread has none: a registration into an application that does not answer returns at UIA's connection timeout. The listener's callbacks, Core's reducer and the supervisor make no calls into applications and wait on nothing that does.

## 7. Background speech when the foreground changes

NVDA executes events in queue order, and a foreground change cancels speech, so a background event queued before it, such as a toast or a snap result from the application being left, is cut. Core keeps the observation time of the last applied foreground change and drops a background event observed before it (`docs/crates/verbatim-core.md`), which gives the same outcome when the background event reaches Core from another outpost after the change.

## 8. The UIA probe asks once

Whether a window has a UIA provider is asked once, with the raw probe (`verbatim_uia::raw_probe`: one `WM_GETOBJECT` for `UiaRootObjectId`, no UIA connection; `docs/design/listener-without-connections.md`, decision 2), as NVDA asks once. A "yes" is kept for the window's lifetime (D15). A "no" is kept for half a second as NVDA keeps it, except that a window too busy to answer within the probe's 3 seconds is read through MSAA for the rest of the entry or read that probed it, and asked again at the next: NVDA times its half second from before the wait, so its slow "no" is already out of date when stored. Windows 11 Notepad while starting, and Windows Terminal's input site window (section 19.7), are the known cases.

## 9. What the design keeps

One outpost per application (D9), its worker, the watchdog and abandonment, and the failure path; Core's cross-outpost stale-focus drop, which gives NVDA's final outcome across parallel applications, and the culling of a focus's speech once a newer focus is applied; the outpost's acceptance test for every event but a UIA focus; and the focus-now query after a listener or outpost restart. An MSAA focus waits behind the events queued before it in its outpost, as NVDA's does (`docs/performance.md`, "A focus behind other objects' events").

## 10. What the user hears

### 10.1 Differences from Verbatim's earlier behaviour, each matching NVDA

1. A UIA focus whose element lost the keyboard focus before the listener read it is dropped, as NVDA drops it (`docs/parity/focus-events.md`, "A focus whose element is gone before its read").
2. Control+Tab in Windows Terminal: the tab left is announced, then cut when the focus moves on; NVDA queues it and culls it (decided by Dickson).
3. Windows Terminal's tabs: the tab's ancestors and position are read from the sender, so "tab control" is not said before "list".
4. A repeated focus on an element that still has the keyboard focus is not announced again, unless another application's focus came in between.
5. Of several MSAA focus events in a row, the newest valid one of each consecutive run is processed, up to four per run.
6. Bursts of UIA events are not cut at MSAA's limits.
7. A change raised before a focus is handled before it, and one raised just after a focus is not handled before it.
8. Background speech observed before a switch to another application is not spoken after the switch.
9. Key commands and events reach Core in arrival order: a typed character's echo always comes before the output it causes.
10. Console windows are announced once, with one name; the application is the shell.
11. A window too busy to answer the probe in time is read through MSAA for the entry that probed it (section 8).

## 11. Experiments

Run in mockapp on 2026-10-10 (`experiments/mockapp-experiments-2026-10-10/`), each in a process and desktop of its own, against mockapp's server-side provider. On a test's own desktop nothing has the system focus, so `GetFocusedElement` could not be aimed at mockapp, and the client-side proxy case for plain Win32 windows needed the live checks of section 12.3.

- E-concurrency: two threads in one process with their own clients call two applications independently; one held application does not stall a read of another.
- E-delivery: UIA delivers events per providing process; a held server-side provider holds only its own events.
- E-scope: a focus registration scoped to one window is refused by the direct call and works only through an event handler group, undocumented (`experiments/focus-events-cannot-be-limited-to-one-application/`).
- E-cost: each focus registration adds 47 to 90 provider calls per focus event on the application's UI thread.
- E1: both UIA timeouts are process-wide (`experiments/the-connection-timeout-applies-to-the-whole-process/`).
- E-TT: the transaction timeout bounds each call, not an operation, and a timed-out registration reports success (`experiments/a-registration-that-times-out-reports-success/`).
- E4: a provider that exits during a held call does not keep the client waiting.
- E5: an outpost whose `GetFocusedElement` lands in another, held application waits on it, the isolation that call breaks.
- E-marshal: an `IUIAutomationElement` cannot be unmarshaled in a process other than the one that created it, by any route tried (`experiments/elements-cannot-be-passed-to-another-process/`).

## 12. Option (b): the listener judges and reads each UIA focus, on a thread per application

### 12.1 The shape

The listener holds the one desktop-wide UIA focus handler, with NVDA's base cache request and two properties that refine a role (`docs/design/uia-shapes.md`, decision 6). Its callback makes no call: it sends the element's application a marker and queues the sender for that application's reading thread. The thread judges the focus as NVDA's focus handler does (section 18), reads it from the sender (states, value, details and ancestors), and sends the result as a fact through verbatim.exe. Nothing is discarded: every UIA focus event goes to its application's thread. The read makes calls only on elements the listener holds (`docs/design/listener-without-connections.md`).

### 12.2 Threads and their lifecycle

A reading thread is created on its application's first UIA focus event, in COM's multithreaded apartment, and ends when its application's outpost ends (section 16.6). It never adds or removes a handler, so Microsoft's rule that one thread makes the registrations is kept. Each has a watchdog: a read past its deadline, or a focus read the user has moved on from after half a second, is reported dropped and a new thread takes the application's later events; an application whose abandoned threads reach a small cap gets no new thread until one returns (`docs/crates/verbatim-outpost.md`).

### 12.3 How a slow application is contained, and D13

Each application's reads are on its own thread, so a held application stalls only its own (E-concurrency). The listener's callbacks, writer and registration thread make no calls. One exposure remains, measured and shared with NVDA: a hung window named in a focus `WinEvent` holds UIA's delivery of every UIA event to any process with a focus registration, with no timeout, until it answers (`docs/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events"). The stall stays inside that process, and its `WinEvent` hooks keep delivering; under option (b) only the listener holds a focus registration, so outposts' UIA events, explicit reads and every MSAA hook keep working. Clearing UIA's WinEvent-to-focus mapping does not contain it. Decision (Dickson, 2026-10-10): do as NVDA does, with no MSAA fallback; while a hung window holds the listener's UIA focus, UIA focus is not announced until the window answers, because announcing from MSAA would sound different and a visible failure is better. That is also why the listener is a process of its own (`docs/architecture.md`, "The focus listener (D13)").

### 12.4 One route per application

- MSAA focus, foreground, menu popup, alert and show events are in the outpost's own hook. The listener forwards an application's MSAA events only until its outpost's hook is in, then sends "forwarding ended" through the supervisor, so its first events start the outpost and none is lost.
- For a UIA focus, the marker takes its place in the outpost's intake in arrival order; entries behind it wait until the read or a drop arrives, and the focus is handled in the marker's place. This is NVDA's behaviour in effect: its UIA events pass through one thread in order, so later events wait for a focus's judgement. The wait is the reading thread's time, normally one read.
- A `WinEvent` focus in a UIA window, which the worker drops for its UIA focus to report, opens a wait for that UIA focus; when none comes within a second, the outpost reads its application's focus itself (`docs/design/listener-without-connections.md`, decision 11).
- The old focus's own state changes queued just behind a focus inside it are handled ahead of the focus, because a provider that moves focus into a child raises the old focus's state change first, but the two travel different routes; NVDA speaks a state change of the focus or of any ancestor, so either order is spoken. Name and value changes keep their places, since NVDA speaks those only for the focus itself.
- Every message from the listener to an outpost travels through the supervisor in verbatim.exe; no second route is added (Dickson, 2026-10-10).

#### The MSAA hand-over, exactly

While an outpost installs its hook, both hooks can receive the same `WinEvent`. Its time is a tick count that advances about every 15.6 ms, so identity alone cannot tell one event seen twice from two identical events raised in one tick, and matching by time windows would lose one of the two. The outpost therefore counts: a copy that the other source has an unmatched copy of is matched and dropped, any other is handled and counted. Every event raised after the hook went in arrives once from each source and is matched once; one raised before it arrives only from the listener; two identical events in one tick are matched pair by pair, so both are handled. No unduplicated event is discarded. Counting ends a bounded time after "forwarding ended" (`handover::TWIN_BOUND`); a copy later still would be handled twice, a limit. The supervisor relays one listener's messages to one outpost in order, which the hand-over depends on.

### 12.5 The outpost's own element

The outpost needs an element of its own for the focus-following subscriptions, text and caret, review, and navigation, and cannot receive the listener's. It finds it by the path the listener sent: from the nearest ancestor with a window handle, one remote operation walks down the chain's runtime ids. A focus without text is reported to Core at once and the search follows, so speech does not wait for it; a focus whose role may have text is found before the report, since its caret and line come in the focus's message (Dickson, 2026-10-21). An element not reachable from its window, as Windows Terminal's tab focus elements, has no outpost element; its report is still right, because the listener read it from the sender, and review and navigation start from its nearest found ancestor (decision 3 of section 15).

### 12.6 The first focus in a new application

The listener's marker starts the outpost, usually by giving the application a spare (section 19), while the reading thread reads the focus; the supervisor holds the result for the outpost until it is ready. Nothing is buffered and dropped.

### 12.7 Cost per focus change

One cache fill for the listener's registration, one read in the listener, and in the outpost one `ElementFromHandle` and one remote operation to find its element: after the report for a focus without text, before it for one with text. No process reads the system focus or searches a subtree, and no other process handles the event. Measured figures are in `docs/performance.md`.

## 13. The stale-focus race between the listener and the outpost

### 13.1 The race

An application's UIA focus reaches its outpost through the listener and verbatim.exe, its other UIA events through the outpost's own handlers. The marker narrows the gap, but an event the outpost's handlers receive before the marker reaches it is handled before the focus. Core culls the old focus's speech once the new one is applied and speaks property changes only for the focus and its ancestors, so nothing wrong is spoken about the old focus; what can happen is loss, such as a change on the new focus raised in the gap being judged against the old one and dropped. NVDA, with one process and one ordered thread, has no such gap, though its focus-following group moves only when its main thread runs the focus, so it misses a change raised before that.

### 13.2 Designs considered for closing it

- (b1), the listener as the only UIA route: closes the race, but moves the text and terminal code into the listener and gives up D9's reclamation and blast radius for most of what Verbatim reads. Not recommended.
- (b2), a per-window hand-over to a group-scoped focus registration in each outpost: rests on the undocumented route of E-scope, which Microsoft's guidance, NVDA's registration and Mozilla bug 1654970 ("focus event listening is global") all argue against. An experiment only.
- (b3), ordering both routes by observation time with a watermark: adds a fixed delay to every event. Kept as an option if the race proves costly.

### 13.3 The choice

Option (b) with Core's guards (Dickson, 2026-10-10), the race measured by the scenario of section 14.

## 14. Measuring the stale-focus window

`text_box_first_character_after_tab`, not yet built (`docs/roadmap.md`): in a window with two text boxes, press Tab and type one character at once, 100 times, and count the iterations whose echo is missing, with each one's trace. Its output is a count, not a pass or fail. Its target must be a field whose echo depends on UIA events, since the Windows Forms text box may echo from the keyboard hook, which the race cannot affect; NVDA's count on the same window, taken by hand, is the baseline.

## 15. Decisions

Each recommended and approved by Dickson on 2026-10-10 or 2026-10-11 unless dated otherwise.

1. The marker wait: later events of an application wait for its focus's judgement, as in NVDA.
2. MSAA focus, foreground and menu events in each outpost's own hook, with the listener's forwarding ended by the counting hand-over.
3. An element the outpost cannot find again is announced from the listener's read; review and navigation start from its nearest found ancestor.
4. Time limits: UIA's timeouts set once per process; a watchdog on every reading thread; operations of many calls bounded by a deadline or made one remote operation; a hung window's stall of the listener's UIA focus left as NVDA leaves it.
5. Background speech observed before a foreground change is dropped after it.
6. The UIA probe asks once.
7. One input channel into Core, in arrival order.
8. The first-character-after-Tab scenario, as a measurement (section 14).
9. D13 as amended: the listener judges and reads each UIA focus from its sender on a thread per application; its callbacks, writer and registration thread make no cross-process call.

## 16. Rules settled by the first live run of option (b)

### 16.1 Why

The first build of option (b) failed many scenarios. Most failures had one cause, the acceptance test run against the wrong window; the others showed where the design's statements did not hold, and each is settled below by NVDA's own behaviour.

### 16.2 The acceptance test against the nearest window

The listener's reading thread makes NVDA's acceptance test, after the repeat check and before the read, against the element's own window or, one call, its nearest ancestor's (`verbatim_uia::nearest_window_handle`, NVDA's `getNearestWindowHandle`), using the outpost's port of the test (`outpost::acceptance`, defined once for both) (Dickson, 2026-10-11). An element with no window in its chain, or whose walk to one fails, is dropped, as NVDA makes no object for it. A focus that fails is dropped and never becomes the last accepted focus. The outpost does not test a UIA focus; it keeps the test for every other event.

### 16.3 Repeated focus, judged as NVDA judges it

One last focus for the whole listener, not one per thread, so a focus in another application in between makes the next one new, as in NVDA. `CompareElements`, then the element's live keyboard focus, in place of comparing runtime ids, since Windows Terminal raises one control's focus under two runtime ids. The check comes first, before any read, as NVDA's does, so a repeat is dropped after two calls. Only a focus that passed the acceptance test is kept, and the comparison is with the focus Core holds (section 4.2): once Core holds a focus no read of the listener's gave it, an MSAA focus or a foreground change, nothing is compared, as NVDA compares only while its last focus is a UIA object; and a focus Core refused is never compared with. The outpost's MSAA duplicate test follows the same rule (Dickson, 2026-10-16 and 2026-10-17; D14 as amended on 2026-10-24).

### 16.4 Windows announced from the focus's ancestry

NVDA has no rule of its own for announcing a window. A window is spoken either as the object a foreground event focuses, or as a newly entered ancestor of a focus: when the focus's top-level ancestor changes, speech is cancelled and the entered presentable ancestors are spoken outermost first, then the focus. So Verbatim drops a foreground fact whose window is not in front when handled, or whose window is read through UIA, as NVDA drops them; treats the top-level window as an ordinary presentable ancestor by NVDA's rules; and speaks a window the foreground did not announce from the general entered-ancestors step (`docs/parity/focus-context.md`). Where the ancestry is the only source of a title, as for a console window, the chain must include the window's element.

### 16.5 The outpost finds its own element for every focus

Finding it only when something needs it was weighed and withdrawn, because it relied on a subscription over the focus's whole window, which section 17 removes.

### 16.6 Reading threads end with their application's outpost

A reading thread is not ended on idle: that would lose its last focus, and with it the evidence that a runtime id now names a different element, which NVDA never loses. It ends when the listener learns that its application's outpost has ended.

### 16.7 Limits

Why NVDA's capture omits Windows Terminal's named "Tab switcher grouping" ancestor is not established; nor is whether NVDA queues and culls File Explorer's item view container read as focused just before its first item.

### 16.8 The focus handler on the global group's client

Windows Terminal's XAML raises some focus events for its terminal from a second identity of the same control, which `CompareElements` says differs, so it was announced twice. UIA delivers that identity to a client with only a focus handler, and not to one whose focus handler is registered first and whose global group is then registered on the same root: NVDA's order, which with NVDA's own order reversed by a scratch plugin let the identity through to NVDA too. The listener follows NVDA's order exactly: the desktop's root fetched with the base cache request, the focus handler registered on the client, then the global group on that root, parts asking for the same properties sharing one cache request (`docs/crates/verbatim-uia.md`). A second window opened in a running Windows Terminal still raises the identity to both; NVDA's last check, the sender's keyboard focus read plainly, refuses it, so the listener makes that plain read too, after the acceptance test, where a read inside a remote operation answers yes. An early second identity with no window above it is dropped, as NVDA cannot make an object for it. Evidence: `experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`.

### 16.9 Rechecked with the pinned NVDA

With the NVDA build the source is pinned to, which reads the keyboard focus live, NVDA and Verbatim each said "terminal" once in every opening, and the Windows Terminal scenarios pass with no allowance for a repeat (2026-10-12).

## 17. Focus-following UIA subscriptions

### 17.1 The evidence

An outpost crashed inside UIA, which delivered a saved event whose record an earlier move of the outpost's registration had destroyed, and failed fast instead of dropping it. A related broken state was found in isolation: after a registration on a console window's whole subtree was added and removed, a registration on its text area received no text events. In the crashed run the outpost had re-registered on the same text area four times in two seconds, each time removing everything and adding it again. Both are UIA defects that the outpost's moves reached.

### 17.2 How NVDA follows the focus

With selective registration, NVDA builds its local groups once; on a focus it adds the matching group to the focused element unless that element already has one, and on losing focus it removes the group from that element alone. Its global group stays on the desktop's root for the program's life. Events raised before a focus's group is added are not heard, except through the global group, and NVDA accepts that gap.

### 17.3 The rule

One focus-following group per outpost, built once, with a text variant, all adds and removes made by the process's registering thread. On a new focus whose element the outpost has, the group is added to that element, then removed from the previous one with `RemoveEventHandlerGroup`; a move to the element already followed does nothing; nothing is ever registered on a window's subtree; and until the outpost has the focus's element, nothing is followed, as in NVDA's gap (Dickson, 2026-10-11). The mechanism is in `docs/crates/verbatim-uia.md` and `docs/crates/verbatim-outpost.md`.

## 18. The listener's focus judgement as a port of NVDA's focus handler

Three parity bugs had one cause: NVDA's focus handler had been rebuilt piece by piece, each check placed where it suited Verbatim. So the listener's judgement is NVDA's focus-changed handler ported as one function, in its order (Dickson, 2026-10-12):

0. An event received before the listener's desktop-wide UIA subscriptions are all installed is dropped, as NVDA drops events before its global handlers are registered; Verbatim's start asks the foreground application's outpost for its focus instead, as NVDA sets its initial focus.
1. The hung window: `IsHungAppWindow` on the sender's own cached window, a local call.
2. Whether the sender is native UIA: its nearest window, and whether that window uses UIA, decided as an outpost decides it; a window that does not is left to its MSAA events.
3. The duplicate rule (section 16.3).
4. The acceptance test against the same nearest window (section 16.2).
5. The object's own refusals: the console window's element, and the shell's windows that only stage a switch.
6. The sender's keyboard focus, read plainly (section 16.8).

Each process keeps its own arbitrator, so the outpost takes the listener's verdict for the window of each UIA focus it is handed, and the two never decide one window differently. A UIA menu opening is judged by the same function, as NVDA's automation event handler judges one it maps to a focus: dropped while a focus is pending, and compared first with the focus Core holds. Not ported, each recorded for the app-module track or as a known difference: NVDA's exceptions to the native check for Office and Internet Explorer; a focus Core asked for, which the listener holds no element for and so cannot compare with; and the listener counting a focus as current from its acceptance, before Core has queued it.

## 19. Spare outposts

A spare is an outpost started before any application needs it. Two are kept started and waiting; one is given to an application when the supervisor would otherwise start an outpost for it, a new one is started the moment one is given or fails, and an outpost once given never returns to the spares (Dickson, 2026-10-10 and 2026-10-16). Under option (b) the listener reads every UIA focus whatever outposts exist, so a spare registers nothing that concerns an application and buffers nothing.

### 19.1 What a spare saves

Starting an outpost takes about 40 ms in a debug build before it can take its first event. A spare saves all of it when a foreground change or an MSAA focus starts the outpost, as on a return to an application whose outpost was retired; up to about 30 ms when a UIA focus does, since the marker starts the outpost while the listener reads; and probably nothing for a new File Explorer folder window, whose own early foreground event starts the outpost long before the first focus.

### 19.2 What a spare is

A spare is the outpost started with `--spare`. It does every part of an outpost's start that does not need the application, its UIA client and focus-following registration scoped to nothing, and its `WinEvent` thread with no hook; on assignment it sets its application, installs the thread's first and only hook set, since a second hook set on one thread has stopped delivery on it, and says `Ready`. A spare makes no call into any application and receives no events (`docs/crates/verbatim-outpost.md`).

### 19.3 How the listener's facts reach a spare

As they reach a newly started outpost: the supervisor holds an application's facts until its outpost is ready, then releases them in order. Every path that starts an outpost takes a spare, except Core's own outpost, which is started directly because it must be ready before the first key.

### 19.4 Readiness

Verbatim's readiness, which the end-to-end harness waits for, waits for the two spares, so every scenario's applications take a spare rather than a spare in some runs and a direct start in others (`docs/testing.md`, one code path); a direct start is covered by unit and mockapp tests.

### 19.5 Failures: one path, with exponential backoff

Every outpost failure takes one path, whatever the cause: a crash or exit, a hook that fails to install, a failed launch, pings unanswered for three intervals, or eight abandoned workers while the application's windows are not reported hung. The supervisor ends the outpost and logs why; an application it served keeps its held facts and is given the next spare at once, whether or not it holds attention. Nothing is ever given up: an application's first failure is replaced at once, each further one after 1 second, then 2, 4, 8, 16, and 30 for every one after, the count starting again once an outpost has stayed up a minute or the application comes to the foreground; a waiting spare's failures are spaced the same way. "Stuck" means the outpost process does not answer the supervisor, never that its application is slow, since the I/O thread answers pings while the worker waits. The abandoned-worker rule bounds resources, each abandoned worker being a blocked thread with about a megabyte of stack that only the process's exit frees.

### 19.6 Cost

Measured on 2026-10-16 in a debug build: each waiting spare's working set is about 14 MB, 2 MB of it private; the two add about 50 wake-ups and 5 to 6 ms of processor time a minute while idle, their pings and UIA's client threads with nothing registered. Per event nothing changes. With spares, the time from the supervisor's first held fact to `Ready` fell from 30 to 150 ms to under 12 ms, and the first event's speech came sooner at the median by 48 ms for a Notepad brought forward, 31 ms for an application returned to after retirement, and 4 ms for a launched Windows Terminal. The cost of starting an outpost per new window is moved to the replacement spare's start, not removed.

### 19.7 Limitations

With a spare, the outpost handles Windows Terminal's first focus about 35 ms sooner, while the terminal is still starting; in about 3 percent of launches the single provider probe of its input site window answers "no" after 3 seconds, so the focus is read through MSAA and spoken as "pane", or the terminal is announced twice. It is the mechanism behind the Notepad case of section 8; `docs/design/listener-without-connections.md` addresses it.

### 19.8 Decisions (Dickson, 2026-10-16)

1. The replacement spare starts the moment a spare is given or fails, independent of the application; if its start is found to compete with the first focus, the remedy is a lower priority, not a delay.
2. Verbatim's readiness waits for the two spares.
3. Nothing is ever given up; repeated failures are spaced by an exponential backoff up to 30 seconds, started again once an outpost has stayed up, and for an application when it comes to the foreground; during a backoff an application's facts stay held.
4. Spares are built: 40 ms is at the threshold of being noticed.
5. An outpost whose `WinEvent` hook fails to install reports the failure and exits instead of sending `Ready`, and is handled as an outpost that ended before `Ready`.
6. There is one failure path for outposts, whatever the cause, and two spares are kept waiting.

## 20. The UIA calls that collide

Verbatim takes no lock around the UIA calls that collide. Only an application's own outpost sends it a remote operation or connects to one of its windows, and the listener reads every focus from elements it holds, so Verbatim's own calls cannot make the collision; `docs/design/listener-without-connections.md` describes the rule and its limits, and `docs/uia-bugs.md` the UIA defect ("A new connection fails while another process's remote operation is queued").
