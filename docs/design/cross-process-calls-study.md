# Calls that reach another application: a study

Status: study of 2026-10-29; Dickson took its five decisions, each as recommended, on 2026-10-29. Decisions 1, 4 and 5 are in the order of work and edge cases of `docs/design/focus-pipeline-rework.md` ("Decisions"); decisions 2 and 3 are two small changes made outside that rework's order of work. The corrections it lists for other documents are applied. The experiments behind its figures are listed under "Evidence" at the end.

## In short

The premise under study (`docs/architecture.md`, section 1; `docs/design/listener-without-connections.md`, section 2): each application's accessibility calls come only from that application's own outpost, and the listener reads only elements it already holds, so no Verbatim process connects to, sends a remote operation into, walks into or blocks on another application.

The verdict: the premise needs a design change. It holds for every remote operation and every call Verbatim makes by a window handle, with one decided exception (`GetFocusedElementBuildCache`, below), but not for blocking: six measured mechanisms make one application's reading wait on another application, and two of them lose a focus outright. The change asked for is small and shared: no thread that reports a focus waits on a process other than the one it serves, which three fixes below give. With them, and the limits they leave stated, the premise would hold as "holds with named, stated limits".

The decisions, taken by Dickson on 2026-10-29, each as recommended:

1. Above the process boundary, read nothing through the other process's accessibility: describe the parent or owner window as a plain window from Windows' own state, with no call. Recommended; it removes the measured silences of a child application under a busy host (instance 1) and the walk-up race's wait (instance 6), at the cost of the frame's MSAA name and role, which a capture must compare with NVDA's.
2. The listener installs its `WinEvent` hooks and says `Ready` before its UIA registration, which then runs without gating them. Recommended (instance 2).
3. Drop the runtime-id search of the application's top-level windows: a node whose kept element is gone and whose route finds nothing is answered gone, as NVDA's object for a gone element is gone. Recommended over a search bounded to the process (instance 3).
4. Give every call whose target the system picks at call time a provider-process check of what it returned, before anything else is done with it, as decided for `GetFocusedElementBuildCache`: the MSAA focus read's `accFocus` result, the MSAA focused-state walk, the walk-up step. Recommended (instances 6 and 8).
5. State as limits, with this study's figures, the second form of the collision inside the listener's read (instance 4) and the replacement outpost's refused lookups (instance 5). Recommended, since both need an application already busy and both recover at the next event.

## Terms

- An application is one process that serves accessibility for its windows; "another application" is any other process, Verbatim's own included.
- The process boundary is where a window's parent window, or a top-level window's visible owner, is served by another process (the listener design's terms).
- A connection is a UIA call that reaches a window by its handle, and also the message UIA's client side sends a window to begin one: `WM_GETOBJECT` with `UiaRootObjectId` and a nonzero wParam (decision 13 of the listener design).
- A plain window is a window described from Windows' own state with no call into its application: role window, named by `InternalGetWindowText` (`docs/design/focus-pipeline-rework.md`, "Terms").
- Heard means the user hears the effect: a focus spoken late, not at all, or wrongly.
- The second form is `docs/uia-bugs.md`'s second form of its first fault: a new connection waiting in a busy application fails when another process's new connection waits there too and a third process has a call pending.

## How the inventory was made

From the code at b15ad7bf, not from the design records: every UIA call wrapper in `crates/verbatim-uia/src/client.rs`, `element.rs`, `shape/mod.rs`, `nearest.rs`, `checks.rs`, `probe.rs` and `subscribe.rs`; every MSAA acquisition and walk in `crates/verbatim-ia2/src/acquire.rs`; every remote program's caller in `crates/verbatim-uia-rops`; and every call site of these in `crates/verbatim-outpost/src`, the listener (`listener.rs`, `focus_read.rs`) and the outposts (`outpost/`, `dialog_text.rs`, `terminal.rs`, `text/`, `arbitration.rs`). For each, where its target comes from, and whether a provider-process check (`served_by`) guards that target before the call. Implicit calls UIA's client side makes on Verbatim's behalf were measured, not inferred, wherever an experiment could reach them.

## The inventory

Each entry: the call; its trigger and path; how often; what can go wrong; measured or inferred.

### The listener

- The sender's nearest window (`nearest_window_lookup`, `NormalizeElementBuildCache` on the held sender, `focus_read.rs` 1465). Every UIA focus and menu opening without a window of its own, event path. Within the sender's application, except that the step from a fragment root to its host window cannot be checked first (risk 10 of the listener design). When the sender's window is the child of another process's window, UIA's client side sends that parent a connection message as the lookup returns the window's element: the read waits on the parent's UI thread, and drops the focus as having no window when that lasts past the connection timeout. Measured (`experiments/the-listeners-read-below-another-processs-window/`, 40 of 40 dropped at 5 seconds, 12 of 12 at 2).
- The raw probe of the nearest window (`uses_uia`, `focus_read.rs` 1508). The first focus in each window, event path. A window message to the sender's own window, up to 3 seconds. Not cross-process.
- The held console check (`held_console_reports_formatting`). Once per console window, event path. Held elements of the console host. Not cross-process.
- The repeat check's comparison with the element of the focus Core holds (`compare_elements`, `focus_read.rs` 1265), which can be another application's. Every focus, event path. No call: measured (`experiments/compareelements-makes-no-call/`, 10 of 10, no call in either application). Its keyboard focus read (1326) is made only when the two are the same element, so in the sender's application. Not an instance.
- The classic read (`focus_ancestry_classic`, `verbatim-uia-rops/src/focus.rs` 886). Every focus read, event path. Calls on held elements; the walk checks the boundary before each step and after it. Two cross-process effects: the second connection message to a parent across the boundary, measured with the lookup above; and the walk-up race, instance 6.
- The selection's and notification's nearest window (`check_fact`, `focus_read.rs` 1025 and 1049). Every UIA selection and notification, event path. Within the sender's application; the same boundary messages as the lookup above, inferred.
- The legacy MSAA state of a menu item. Held element. Not cross-process.
- The desktop-wide registration (`install_desktop_subscriptions`, `listener.rs` 1254; `subscribe.rs` 1062 and 1089). At the listener's start and at each replacement. Measured to connect into the foreground application and ask its provider for the focus, waiting up to about 6 seconds: instance 2.
- UIA's own focus tracking in the listener's process. Every focus `WinEvent`. Sends each window the raw probe's message and, for a window without a provider, connects through UIA's MSAA proxy; holds every UIA event of the process while a hung window is named (`docs/uia-bugs.md`, `h2`). Measured; stated in the listener design and shared with NVDA. Not reopened here.
- The `WinEvent` callbacks and the writer: local calls only (`listener.rs` 1349 to 1530). Not cross-process.

### The outposts

- `GetFocusedElementBuildCache` in the focus-now, gap and recovery reads (`read.rs` 939). Measured (20 of 20) and decided: keep it, check the element's process id first, state the remaining connection as a limit (`docs/design/focus-pipeline-rework.md`, "Decisions").
- The MSAA focus-now read (`focused_snapshot`, `acquire.rs` 1204 to 1220). The same triggers, for an MSAA window. A second check-then-act on the focus: the focus window is read again after the outpost's own check, its owner compared by `GetWindowThreadProcessId`, which for a console window names the client. Its `accFocus` follows an object into any window. Measured: Windows' standard proxy names no other process's child window (instance 8).
- The MSAA continuation above the boundary (`uia_chain_onward`, `read.rs` 380; `msaa_ancestors`, 675; `msaa_enrichment`). Every UIA focus whose chain stops below the boundary, and every MSAA focus whose parents climb into another process, event path. MSAA calls into the other process with no bound but the worker's deadline. Measured: instance 1.
- The MSAA focus event's focused-state walk (`has_focused_state`, `worker.rs` 3557). Every MSAA focus, event path. `accParent` up to 64 hops with no boundary check, so it can climb into another process's window. Inferred from the code; the same waits as instance 1.
- The runtime-id search (`resolve_uia_element`, `read.rs` 1114 to 1135). A query (navigation, ancestors, activation, a text request) or a dialog's text on a node whose kept element is gone and whose route finds nothing, user-initiated, a few times a session at most. Measured to enter another process's child window: instance 3.
- The other searches and walks down a subtree: the selection's search under the expected focus (`selected_element`, `worker.rs` 3197, a UIA selection with no element, event path), the controlled-element search (`Uia::controlled` and `controlled_selection`, `worker.rs` 3052, UIA selections in a list the focus controls, event path, under elements another process may serve), the dialog text gather (`dialog_text.rs` 152, 262 and 278, the first focus into a dialog, event path, bounded to 1 second by the bounded client), the shell items walk under `Shell_TrayWnd` (`shell_items.rs` 376 and 392, a query, the root checked by `serves` but not what is below it), and the tree dump (`read.rs` 1414, a diagnostic query). Inferred from the runtime-id search's measurement, except the tree dump: a remote dump stays in the provider's process, and the MSAA dump of a plain Win32 window enters the child (measured, instance 3's experiment).
- The walk-up race in the outpost's own UIA walk (`Uia::ancestors`, `client.rs` 724 to 789) and the remote program's classic fallback. Ancestors and focus-now queries and MSAA-to-UIA crossings. Instance 6.
- Hook events and routed facts naming a window (`worker.rs` 2453, 2459, 2621 to 2644, 2671, 2882, 3318, 3358, 3445 and 3455, 3493 to 3568, 3828 and 3831): the probe, the console check by handle (a connection), `AccessibleObjectFromEvent` and the reads after it, the tree view and edit control messages, with no provider-process check of the window. Every MSAA event, event path. Measured: a process-scoped hook takes events by the window's owner, so the reachable case is a console window (instance 7). The `served_by` check being added in the rework's step 3 covers these.
- The foreground report onto Core's hidden frame (`worker.rs` 3853, `read::foreground_window`): the probe and a lookup or MSAA read of verbatim.exe's own window. A menu opening while Core's frame is in front. A call into Core's process, which answers on its GUI thread; inferred, not measured. Found here; not in the review.
- Object navigation's raw-view step into another process's window (`Uia::neighbor`). One per key press, user-initiated. The accepted exception of the listener design, section 4.3; not reopened.
- The followed focus's reads and registration (`follow_focus`, text and caret reads, terminal reads): on elements the outpost found through a checked route or a checked focus window, so in its application. The registration on the focus's element advises that element's provider only (`Scope::Elements`). Not cross-process.
- Releasing held objects (`worker.rs` 2061): a COM release reaches whichever process served the object, so an object read across the boundary is released into that process, and a release waits on a hung server (`experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/`, `e1-release-in-a-stall.txt`, 4 seconds in a 4-second stall). Measured there; heard only through instance 1's objects.

### The supervisor

- The retirement overlap (`supervisor/owner.rs` 1290 and 1291; `supervisor/retire.rs`, `SHUTDOWN_LIMIT`). An outpost failing while its application runs. Two outposts call one application. Instance 5.

### The test of the rule

`the_listener_makes_no_connection` (`crates/mockapp/tests/call_counts.rs`) counts the connections Verbatim's wrappers make (`calls::count_connection`). By the code it cannot see a connection UIA's client side makes on Verbatim's behalf; the boundary test beside it counts the parent's messages instead and pins decision 13's. Not an instance; a limit of the test, which its doc should state.

## The instances, most severe first

### 1. A busy window above the boundary holds the focus of the application below it

What: for a focus in a child window whose parent is another process's (the Settings page's content inside `ApplicationFrameHost`'s frame, a WebView2 host's web content), two things wait on the parent's UI thread. The listener's read, through the connection messages UIA's client side sends the parent (decision 13), waits for it and drops the focus as having no window past the connection timeout. The outpost of the focus's application then continues the chain through MSAA in the parent's process before it reports the focus, with no bound of its own.

Proof: measured. The listener's side: `experiments/the-listeners-read-below-another-processs-window/` (the read waits the busy time; past the connection timeout the focus is dropped, 40 of 40 at 5 seconds, 12 of 12 at 2). The outpost's side: `experiments/the-msaa-continuation-waits-on-the-parent-windows-application/`: with the parent stalled for 4 seconds the focus was reported after 4,007 ms, 3 of 3; stalled for 12 seconds it was never reported within 30 seconds, 3 of 3. The focus-now read's MSAA continuation and the MSAA focused-state walk are inferred to wait alike.

Options:

- (a) Describe the parent or owner above the boundary as a plain window, with no call: one local read of its text and class per window, no call into the other process. The listener's chain stops below the boundary as now; the outpost appends the plain windows up to the desktop, the rework's fallback rule applied at the boundary always rather than only when the object has not answered. Cannot do: give the frame's MSAA name and role where they differ from its text and the window role; NVDA reads the Settings frame as a pane named by the title (its capture: "Settings", "Settings window"), so whether Verbatim's announcement changes needs a capture. Removes the outpost's wait entirely; leaves the listener's (decision 13), which only a stop below the child window's own element would remove, for which no stopping rule is known.
- (b) Keep the MSAA reads and bound them: run the continuation where it can be abandoned, publishing the chain at the rework's bound with plain windows for what did not answer (`docs/design/focus-pipeline-rework.md`, "the bound met when a call is stuck"). One abandoned thread per stuck continuation; the frame read through MSAA when it answers. Cannot do: avoid the wait up to the bound on every focus while the host is busy, nor the abandoned threads' accumulation into the outpost's failure limit.
- (c) Ask the parent window's own outpost for the ancestors, the alternative rejected in decision 7 of the listener design: a hop, a dependency on another outpost's readiness, and that outpost then waits instead.

NVDA: walks up into the other process through UIA (`NVDAObjects/UIA/__init__.py` 2136 to 2154, `GetParentElementBuildCache`) and switches API at the window (`NVDAObjects/window/__init__.py` 319 to 330); being one process its main thread waits on the host, and its watchdog cancels the COM calls (`watchdog.py` 235 to 237, `CoCancelCall`). So NVDA also waits, and speaks the full ancestry when the host answers.

Recommendation: (a), because it is a rule at the boundary with no special case and no bound to tune, and the boundary's other members (instances 6 and 8, the focused-state walk) then need no call into the other process at all; with (b) as the rework's general bound for calls within the application.

### 2. The listener's start waits on the foreground application

What: the listener's desktop-wide UIA registration reads the system's focus, connecting into the foreground application, and the listener installs its `WinEvent` hooks and says `Ready` only after it returns (`listener.rs` 915 to 1003).

Proof: measured (`experiments/the-listeners-registration-waits-on-the-foreground-application/`): with the foreground application stalled for 3 seconds the registration took 3,002 to 3,009 ms, 6 of 6, UIA or MSAA; stalled for 12 seconds, 6,044 to 6,058 ms, 4 of 4, then reported success; a stalled application not in front, 24 to 28 ms. At Verbatim's start, or when the listener is replaced, a hung foreground application keeps the listener deaf for up to about 6 seconds, MSAA facts included. That the supervisor's or Core's handling of a late `Ready` is otherwise harmless is inferred, not measured.

Options:

- (a) Install the hooks and say `Ready` first; make the registration afterwards on the registering thread, which already serves no one else at that point. No new calls; MSAA facts and the keyboard hook's foreground view flow at once. Cannot do: deliver UIA focus events before the registration returns.
- (b) Bound the registration's wait. UIA ends it itself after about 6 seconds; a shorter wait would need the registering thread abandoned, which the subscription design forbids (`subscribe.rs`, one registering thread per process, never abandoned).

NVDA: waits 2 seconds for its UIA thread's start (`UIAHandler/__init__.py` 467, `MTAThreadInitEvent.wait(2)`), its registration made on that thread (593), and ignores UIA events until it is done (763 and the like); its MSAA hooks are independent of it (`IAccessibleHandler/internalWinEventHandler.py` 217).

Recommendation: (a).

### 3. The runtime-id search goes into another process's child window

What: `resolve_uia_element` searches each top-level window's subtree (`FindFirstBuildCache`) for a node whose element is gone; the search enters any shown child window another process serves.

Proof: measured (`experiments/the-runtime-id-search-enters-another-processs-child-window/`): a connection and 481 provider calls in the child's process per search, 15 of 15 with a parent of each kind and 5 of 5 through the product's ancestors query; with the child's process stalled, the search waited 2,007 ms and answered "not found", 15 of 15. Heard as a navigation or activation key answered two seconds late, or answered gone for an element that is there.

Options:

- (a) No search: answered gone when the kept element and the route find nothing. No calls. Cannot do: find an element the registry lost while its node is still held, the case the search was added for (`read.rs` 1059 to 1064), which after the route through the listener's chain is rare and not measured.
- (b) Search inside the application's own process by a remote operation, which measured makes no call in the child (the UIA tree dump). Cannot do: a window read through UIA's proxies, which a remote operation cannot import; and a large tree passes the remote program's instruction budget (10,000 executed instructions), so the search would need continuing runs, the cost the outpost focused-element study weighed.
- (c) A classic walk that prunes at each element served by another process, reading each child's process id in its cache. Many round trips, more than the search it replaces.

NVDA: has no such search; its object is its element, and a gone element is gone (the only `FindFirstBuildCache` in its source is `NVDAObjects/UIA/wordDocument.py` 244, within Word's own document).

Recommendation: (a), and the same rule for the selection's search under the expected focus (`worker.rs` 3197). The searches that remain (the controlled-element search, the dialog text gather, the shell items walk) share the cause; they are bounded (the dialog text by 1 second) or user-initiated, and (b) is the fix if one is ever met.

### 4. The second form, inside the listener's read

What: under a parent window with a provider whose application is busy with a third process's call pending, the listener's implicit connection and the parent's outpost's lookup can meet the second form.

Proof: measured (`experiments/two-new-connections-beside-a-pending-call/`). The second form alone, two outposts' lookups: one returned UIA's proxies, which the proxy rule refuses, in 3 of 42 (not the 6 of 6 the listener design records); none without the pending call (0 of 12). With the listener's read as one connection: the read came one connection timeout late, about 2.3 seconds with the right chain, in 5 of 42, and the outpost's lookup never failed (0 of 42).

Options:

- (a) State it as a limit, with these figures: needs a busy host, a third process's pending call and the focus below it at once; heard as one focus 2 seconds late.
- (b) Stop the listener's walks below the child window's own element, the candidate fix that removed every message to the parent in mockapp (`experiments/the-listeners-read-below-another-processs-window/`, "The candidate fix"). Cannot do: tell where to stop without the element whose return sends the messages; the outpost reading the window's element then takes the wait.

NVDA: one process, so its own calls make only one of the connections; beside another client it is inferred to meet it (`docs/uia-bugs.md`).

Recommendation: (a). It shares instance 1's cause, a call that obtains a window element below the boundary, and decision 13's acceptance (Dickson, 2026-10-19) covers it once its figures are recorded.

### 5. A replacement outpost beside its predecessor

What: a failed outpost is retired on a thread while its replacement starts at once; an outpost that failed by its abandoned workers still has calls, remote operations among them, in its application.

Proof: the mechanism measured (`experiments/a-replacement-outposts-lookup-beside-its-predecessors-operation/`): the replacement's lookup was refused after the busy time and one connection timeout, 6 of 6; with nothing queued, its own element, 6 of 6. The overlap itself is read from the code (`supervisor/owner.rs` 1290 and 1291). Heard as the replacement's focus-now answer empty, so the application's focus is spoken at its next event.

Options:

- (a) State it as a limit. It needs an application busy enough to fail its outpost, and its refusals end when the queued requests are served.
- (b) Give the replacement only after the predecessor has exited: up to `SHUTDOWN_LIMIT` (21 seconds) with no outpost, worse than the refusal.
- (c) Kill the predecessor instead of retiring it: does not remove its requests already queued in the application (`docs/uia-bugs.md`, "a call that times out leaves its request queued"), and killing a client mid-call is the suspect in a Windows Terminal crash (`docs/architecture.md`, section 1).

NVDA: one process; no counterpart.

Recommendation: (a), merging it with risk 8 of the listener design, which already states the ended-predecessor case.

### 6. The walk up whose window changes between its check and its step

What: the classic walks check the boundary, then step; a window reparented, or an owner shown, in between makes the step a connection into the other process.

Proof: measured with the order forced (`experiments/a-walk-up-whose-window-changes-after-its-check/`): a connection and 43 provider calls in the other process, 10 of 10; the chain still right, the check after the step dropping the parent; with the other process stalled for a second, the walk waited 1,007 ms. How often it happens unforced is not measured; it needs a hierarchy change within the step's milliseconds.

Options: (a) state the one step as a limit, the check after the step already making the result right; (b) none removes the step: no client call tells a window element's parent without being the step.

NVDA: makes the step always, with no boundary check (`NVDAObjects/UIA/__init__.py` 2138).

Recommendation: (a). It shares instance 8's and `GetFocusedElementBuildCache`'s cause, a check of the hierarchy or the focus followed by a call that reads it again; with decision 1's plain windows the parent's element, once returned, is never read further.

### 7. A hook event naming a window another process serves

What: the outposts act on the window an event names with no provider-process check.

Proof: measured (`experiments/a-process-scoped-hook-takes-events-by-the-windows-owner/`): a process-scoped hook takes events by the window's owner, not by the process that raised them, so another application's `NotifyWinEvent` naming a window reaches that window owner's outpost; handed an event naming another process's window, an outpost probed it and made six MSAA calls into it, 5 of 5. The reachable case is a console window, owned by its client and served by its console host, in the hook of a client that has an outpost of its own: inferred.

Options and recommendation: the `served_by` check on every window the outpost judges or acts on, being added in the rework's step 3 (treated as decided). NVDA's hook is global (`IAccessibleHandler/internalWinEventHandler.py` 217) and has no counterpart.

### 8. The MSAA focus read's accFocus

What: `focused_snapshot` checks the focus window, then follows its client's `accFocus` into whatever object it names.

Proof: measured (`experiments/accfocus-names-no-other-processs-child-window/`): with the focus moved after the check into another process's child window, Windows' standard proxy named nothing, 5 of 5, and the read took the parent's client as the focus, a wrong report with no cross-process call; into the window's own child, an object, 5 of 5. An application whose own `accFocus` names another process's object was not found.

Options: (a) check the result's window against the outpost's application (`served_by` on the window `WindowFromAccessibleObject` gives) and answer no focused control on a mismatch, as for `GetFocusedElementBuildCache`; (b) none for the wrong report of the moved race except reading again, which races again.

NVDA: follows `accFocus` into another window and recomputes the window (`NVDAObjects/IAccessible/__init__.py` 549 to 563).

Recommendation: (a); the moved race's wrong report is a stated limit, followed by the focus event of where the focus went.

## Which instances share a cause

- Reads of another process's window above the boundary: instance 1 (both halves), instance 4, instance 6, the MSAA focused-state walk and object navigation's step. Decision 1's plain window above the boundary is their one fix for the outposts; the listener's half (decision 13's messages) stays a stated limit.
- Walks and searches down a subtree: instance 3, the selection's search, the controlled-element search, the dialog text gather, the shell items walk and the MSAA tree dump. Decision 3 removes the two that run on a node's loss; the rest are bounded or user-initiated.
- Check-then-act on the focus or the window hierarchy: `GetFocusedElementBuildCache` (decided), instances 6 and 8, `focused_snapshot`'s second read of the focus window, and the hook's window owner (instance 7). Decision 4's check of the result is their one rule, and each leaves one call it cannot prevent.
- Two Verbatim processes calling one application: instances 4 and 5, risk 8 of the listener design, and UIA's own focus tracking (risk 9). All are the first fault's two forms; none can be removed while UIA's client side connects on its own, so they are stated limits.
- A thread that waits on the foreground application at a start: instance 2 alone.

## The verdict

Needs a design change. The rule that only an application's own outpost makes Verbatim's connections and remote operations holds, except the decided `GetFocusedElementBuildCache` and the searches of instance 3. The claim that a slow or hung application holds only the threads that serve it does not: a host window's application holds the focus of the application below it (instance 1), the foreground application holds the listener's start (instance 2), and a child window's application holds a search (instance 3). With decisions 1 to 3 built, and instances 4 to 8 with decision 13 and risks 8 and 9 stated, the premise would hold with named, stated limits.

## Documents whose claims are false

The claims as they stood and their corrections, applied on 2026-10-29.

- `docs/architecture.md`, section 1, "The focus listener (D13)": "The listener's contract: **its callbacks, its writer, and its registration threads never make a cross-process call.**" Corrected: "The listener's contract: its callbacks and its writer never make a cross-process call; its registration thread's one desktop-wide registration, made at its start, waits on the foreground application while UIA reads the system's focus, up to about 6 seconds for one that does not answer (`docs/design/cross-process-calls-study.md`, instance 2)."
- The same section: "Of the listener's threads only the reading threads call into applications, and each call is one on an element the listener holds". Corrected: "Of the listener's threads the reading threads call into applications, each call one on an element the listener holds, and the registration thread's registration reads the foreground application's focus."
- The same section: "an outpost connects to, probes and walks up into only its own application's windows". Corrected: "an outpost connects to, probes and walks up into only its own application's windows, except one connection where the focus or the window hierarchy changes between its check and its call, and its runtime-id search, which enters any child window of another process below its application's top-level window (`docs/design/cross-process-calls-study.md`)."
- The same section: "A slow or hung application holds only the threads that serve it, with one exception, shared with NVDA". Corrected: "A slow or hung application holds only the threads that serve it, with these exceptions: a hung window named in a focus `WinEvent` holds the listener's UIA events (shared with NVDA); a window above the process boundary holds the reading of the focus below it, in the listener and in that focus's outpost; and the foreground application holds the listener's start (`docs/design/cross-process-calls-study.md`, instances 1 and 2)."
- `docs/design/focus-pipeline.md`, section 12.3: "The listener's callbacks, writer and registration thread make no calls." Corrected: "The listener's callbacks and writer make no calls; its registration thread's desktop-wide registration reads the foreground application's focus." The same in section 15, decision 9: "its callbacks, writer and registration thread make no cross-process call" becomes "its callbacks and writer make no cross-process call".
- `docs/design/listener-without-connections.md`, section 1: "A second form needs no remote operation: a new connection waiting in a busy application fails when another process's new connection waits there too and a third process has a call pending (6 of 6 against mockapp and against the collide server)." Corrected: "A second form needs no remote operation: a new connection waiting in a busy application sometimes fails when another process's new connection waits there too and a third process has a call pending: 3 of 42 against mockapp, and none without the pending call (`experiments/two-new-connections-beside-a-pending-call/`)."
- `docs/uia-bugs.md`, the first entry's "Symptoms": "A second form needs new connections from two processes: a new connection waiting in a busy application fails when another process's new connection waits there too and a third process has a call pending." Corrected: the same, with "fails" made "fails in some trials (3 of 42; with the listener's implicit connection as one of them, the listener's read one connection timeout late in 5 of 42)", and the experiment folder added to the entry's "Evidence".
- `docs/design/listener-without-connections.md`, risk 8: "This is the only case left in which two outposts' calls meet one application". Corrected: "This and a replacement given while its predecessor is still retiring (`docs/design/cross-process-calls-study.md`, instance 5) are the cases in which two outposts' calls meet one application."
- `docs/design/listener-without-connections.md`, section 4.3: "The tree dump for `verbatim-inspect` reads only the application's main window". Corrected: "The tree dump for `verbatim-inspect` starts only at the application's main window, which the filters find by provider process; its MSAA walk of a plain Win32 window enters a child window another process serves."
- `docs/crates/verbatim-uia.md`, `find_by_runtime_id`: "a search that finds nothing is `Ok(None)`; one that fails is an error." Corrected: "a search that finds nothing is `Ok(None)`, and so is one that passed over a child window whose process did not answer within the connection timeout; one that fails otherwise is an error."
- `docs/crates/verbatim-outpost.md`, "Who calls into an application": "Only an application's own outpost sends it a remote operation or makes a new UIA connection to its windows". Corrected: "Only an application's own outpost sends it a remote operation or makes a new UIA connection to its windows by a call of Verbatim's, with the exceptions `docs/design/cross-process-calls-study.md` lists: UIA's client side's connections to a parent across the boundary, the focus-read and walk-up races, and the runtime-id search's entry into another process's child window."

Not a document but false, a code comment: `SHUTDOWN_LIMIT`'s doc in `crates/verbatim-outpost/src/supervisor/retire.rs` says Verbatim leaves UIA's transaction timeout at its default of 20 seconds and the connection timeout at 10; Verbatim sets 5 and 2 (`TRANSACTION_TIMEOUT`, `CONNECTION_TIMEOUT`). Corrected with the documents.

## What was not proved

- Every experiment ran against mockapp. A real WebView2 host, the real Settings frame and a real console client with an outpost of its own were not measured; each is inferred from mockapp's windows.
- Instance 5's supervisor timing, the frequency of the races of instances 6 and 8 unforced, and how often a focus sits below a busy host in use are not measured; nothing logs them.
- Instance 2's effect on the supervisor and Core of a `Ready` 6 seconds late, and whether the registration that waited then hears the hung application's events, are not measured.
- The foreground report onto Core's hidden frame (`worker.rs` 3853) is read from the code, not run.
- Why the second form is intermittent is not known.

## Evidence

- `experiments/two-new-connections-beside-a-pending-call/`
- `experiments/the-runtime-id-search-enters-another-processs-child-window/`
- `experiments/a-process-scoped-hook-takes-events-by-the-windows-owner/`
- `experiments/a-walk-up-whose-window-changes-after-its-check/`
- `experiments/accfocus-names-no-other-processs-child-window/`
- `experiments/a-replacement-outposts-lookup-beside-its-predecessors-operation/`
- `experiments/the-listeners-registration-waits-on-the-foreground-application/`
- `experiments/the-msaa-continuation-waits-on-the-parent-windows-application/`
- `experiments/compareelements-makes-no-call/`
- The program they share: `experiments/cross-process-calls-study-program/`.
- Earlier: `experiments/the-listeners-read-below-another-processs-window/`, `experiments/getfocusedelement-after-the-focus-moves/`, `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/`.
