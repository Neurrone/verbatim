# The focus pipeline rework: its remaining builds

The builds of [the focus pipeline rework](focus-pipeline-rework.md) after the ownership refactor ([the ownership model](ownership.md)) and M4.5. The second build is built, and its section keeps the problems, choices and limits. The third build is as written on 2026-10-30; it is redesigned against the code before it is built, and then also gains one node per window (defect A) and keeps its merge evidence fixes.

## The second build: Core's foreground record and the gap

Built on 2026-11-02 (f7eabd1d to b9e6edd5). The mechanism is in the crate guides: Core's foreground record, the left mark and the title rule in `docs/crates/verbatim-core.md`; the listener's foreground watch in `docs/crates/verbatim-listener.md` and `docs/crates/verbatim-listener-logic.md`; the outpost's tests on its reducer's thread, the gap record and the held window's text in `docs/crates/verbatim-outpost.md` and `docs/crates/verbatim-outpost-logic.md`. The departures from NVDA are in `docs/parity/focus-events.md` and `docs/parity/focus-context.md`, and the decisions of record in `docs/architecture.md`, D13 and D14.

Terms: a foreground change is the listener's `ListenerToCore::Foreground`; a whole-window focus is a `FocusChanged` whose node stands for a top-level window, no control in it read; the entered window is the top-level window the user's focus line was last in; Core's focus is left when a foreground change into an unrelated window was observed after every report Core has judged since it took that focus.

Cost, measured before and after the build on the debug build: `measure_keys_text_box`, 8 seconds of typing, 15,538 wake-ups and 1,328 ms on a processor before, 14,436 and 1,198 ms after, one run each; idle Verbatim, 8 seconds, 56 wake-ups after, the run before being disturbed. Per foreground change no message is added, since the listener sent one before, and the gap's timer already existed.

### The outpost's foreground tests, on its reducer's thread

Problem. The worker tested a foreground change and an MSAA focus when it handled them, and every call into an application waits behind a stuck one in its one request reader ([UI Automation](../explainers/uia.md), "Inside the provider process: the one request reader"), so a test could be made seconds after its event. Core also tested each focus again on window facts read at publication, so a focus its reader accepted, published after a switch, was dropped unheard.

Choice (Dickson, 2026-11-02). Every test against the foreground is made once, on the outpost's reducer thread from Windows' state, as the entry is taken in, nearer the event than NVDA's test in its pump. Core judges each focus with window facts against its record's front, the listener's newest change: dropped when the front was observed after it and is unrelated to its top-level window and root owner, so popups, owned dialogs and owned windows pass. Reason: one test per fact; a reader cannot see a switch after its own test, and the front says it from the listener's one report, so a held application's late focus is not spoken in the window returned to.

Limitation. A slow focus published after Alt+Tab into an unrelated window is dropped, where NVDA speaks it and then cuts it (Dickson, accepted).

### One foreground change, from the listener

Problem. Found on 2026-11-01, 4 of 4 by hand: switching from Notepad's edit to an application whose UIA calls are held, then back, said nothing. The busy outpost dropped the change into a window it judged UIA and its focus read never answered, so Core heard nothing of the switch, refused Notepad's report as its focus's window and dropped the edit's focus as a duplicate. NVDA is silent there too.

Choice (Dickson, 2026-11-02). Silence in the busy application is accepted; the return is always read, and said as the same switch without a stall. The listener's foreground message is the only report of a foreground change, sent for every foreground `WinEvent` whose window `GetForegroundWindow` names, with its observation, process and root owner; one not yet in front is kept pending and sent when a later focus event finds it in front, since File Explorer's folder window and a console raise their only event early. Core takes it for the keyboard hook, attention (D14) and its record's front, and marks its focus left when the window is unrelated to that focus and the change was observed after the ordering stamp (option A). A left focus reported again is taken as a new focus, resetting every state keyed on its node, so the return says the switch's words. Verbatim's own windows are taken for the hook alone.

Alternatives weighed. Three earlier mechanisms for the left mark, a sighting, in-flight conditions and a `FocusRaised` event, each failed its review; a placeholder focus for the window was rejected because NVDA keeps its focus object, so object navigation and reporting the focus would differ.

Limitations. A splash or tray window that takes the foreground and reports no focus leaves the focus like any window, so the return after it is spoken, where NVDA is silent after a UIA splash window (Dickson, accepted). A change reaching Core after the reports of the user's return marks nothing, so that return is silent. A listener replaced during the round trip loses the change, and the return is silent. An owned dialog of a held application shares its root owner, so a return within the second is silent. While the focus is left, a caret key in the busy application is still held for the left focus's caret read (follow-up below).

### The whole-window focus

Choice (Dickson, 2026-11-02). The outpost's report of a foreground change into a window it reads through MSAA is a `FocusChanged` marked `whole_window`, as NVDA queues a `gainFocus` on the window; it never moves attention, is judged by the front like every focus, and is refused while Core's focus is in its window and not left. Reason: Core cannot tell such a focus from its node, since an MSAA focus event on a window's client with no focused child reads as the same node, and NVDA speaks that.

### The title rule

Problem. Core said a top-level window only as an entered container, and when a foreground change and its focus shared an outpost batch, tried newest first as NVDA's pump tries them, the change was never tried, so a Windows Forms form's title was lost (`text_box_editing`, 2 of 3); NVDA loses it by the same timing.

Choice (Dickson, 2026-10-30). The window's title is said when the focus enters a top-level window it was not in, decided in Core from the focus's chain: a focus whose top-level window was the foreground window at publication, when that is not the entered window or Core's focus was left, says the chain's entry for the window unless its line already does. A whole-window focus taken first enters its window, so the title is said once. Program Manager and the taskbar are entered with no title, as NVDA takes no foreground event for them. A foreground change into one of Verbatim's own windows clears the entered window, so the return from Verbatim's menu, a popup that enters nothing, says the title whichever report comes first. Reason: the title no longer depends on pump timing.

Limitations. A chain with no entry for the window, or a nameless one, adds nothing. On the first Escape from Windows 11 Notepad's WinUI context menu NVDA says "Untitled - Notepad", and Verbatim says it only when Notepad's whole-window focus is tried in its batch.

### The held UIA application's title

Choice (Dickson, 2026-11-02). At the gap record's bound, a second after a foreground change or copy with no focus of that window reported, the outpost's shell tests the record's window against the foreground and, when that window is in front, sends its text from Windows' state as a whole-window focus on the plain window, "held-application window", whatever is in flight, and keeps the gap's read beside it. A window read through MSAA whose own report the worker still owes says its text the same way. Reason: a second is the silence budget, and tests of whether the application was answering missed cases. NVDA says nothing until the focus comes, a deliberate difference.

Limitations. An outpost replaced while its application is held loses the record, so no title is said. A marker that reaches the outpost before the hook's copy delays the record by up to `READ_DEADLINE`. A window whose probe waits on the held application opens no record until the probe ends.

### The gap, closed by the outpost's own reports

Problem. The record was closed by answers the listener computed from each UIA outcome, placed before the worker judged the read, so a read the worker then dropped still closed it, and nothing was told to Core (defect B below).

Choice (Claude, 2026-11-02). The record is closed only by a focus the outpost sent Core, placed by its window facts' top-level window, a whole-window focus never closing its own window's record. A drop carries one fact from the listener, whether its sender had no window above it, which has the record read at once; every other drop leaves the record to its bound. A foreground change opens or restamps the record and arms a new bound, so every visit to a held application says its title; a copy restamping a record whose bound has passed arms a new one too, since a gap read the worker could not finish leaves no other timer. Reason: only the outpost knows what it told Core.

Limitations. A next focus whose copy comes within 100 ms after the previous focus's report in the same window, and whose UIA focus never comes, is not recovered. An application whose lookups keep failing fast has every focus spoken through the gap, up to a second late. The moved-on rule is unchanged (Dickson).

### The keep-id evidence, read in NVDA's order

Problem. Run 41414227699 (2026-11-02), six Windows Terminal scenarios: Windows Terminal raises its terminal's focus twice at launch and then minimizes itself; the listener read the sender as focused and, a millisecond later, the earlier element as not focused, so Core took a new element and said the terminal again.

Choice (Dickson, 2026-11-02). The earlier element's keyboard focus is read before the sender's, as NVDA reads them, inside the remote program and the classic walk, with no call added. Limitation: a focus that leaves and returns between the two reads still gets through, as in NVDA.

### Defect B

Found by the ownership study in `explorer_folder_window` (`experiments/ownership-study/architecture-study.md`, item 5): the frame's listener read, dropped by the outpost's verdict, closed the list's record as it was pushed, so "Items View list" was never said. Fixed by the gap's rule above: the record stays open and the gap's read says the list up to a second late. Limitation: should the frame's own MSAA focus be reported no earlier than 100 ms before the list's copy, it closes the record and the list is still not said.

### Facts owned by two processes

- The foreground: Windows' state. The listener reports each change, the only report of one, and Core's record judges it; each outpost reads it for its own tests and as a report's window facts, which never move attention or set the record.
- Whether a focus's window was in front: its reader's one test, the listener's for a UIA focus and the outpost reducer's for an MSAA focus and a foreground change.
- Whether Core's focus is left, and the entered window: Core's own record.
- The gap record: the outpost's reducer, with the listener's one fact on a drop.

### Follow-ups found

- While Core's focus is left, a caret key pressed in the busy application is held for the left focus's caret read (`editing::text_focus`), a call into the application left and a delay of the key; what it then says was not established.

## The MSAA side, the desktop, the remaining defects and the records (the third build)

Designed 2026-10-30 (Fable), before the ownership model; partly reversed, as marked per section. The last build of the rework: the outpost's MSAA side (step 13: the fake focus's tests, the listener's hung test made once, the dialog text's bound), the desktop (step 14: the desktop switch and the virtual desktop's name; the delayed pump is not built, by its capture), the two possible defects (step 15), the design records (step 16), and one item found in a run: a window not reached by a walk's bound, which the outpost judges UIA, is said without "window". The build starts from the second build ("The second build: Core's foreground record and the gap"): a focus is tested against the foreground once, Core keeps one foreground record fed by the listener's foreground change, and a report closes the gap record. It does not change the commit point, MSAA equality or the reused runtime id's keep, which the parallel fix build owns.

The commits, each leaving `cargo xtask ci` passing: the UIA window past the bound; the MSAA side; the desktop; the defects; the records. The first depends on nothing in the second build and can be built at once.

### A window past the bound that the outpost judges UIA

What happened. In `windows_terminal_two_windows` run 3 of group 1 (rework-runs, `windows_terminal_two_windows-3-artifacts`), the first window's terminal raised its `WinEvent` focus at 04.242, and its UIA focus reached the listener at 04.366, 124 ms later. The nearest-window lookup took 44 ms. The walk then read the terminal's parent, an unnamed element, and its next step, to the `Windows.UI.Input.InputSite.WindowClass` window's element, stayed in its call until about 04.920, about 486 ms. Windows Terminal's UI thread was busy for that time: the outpost's probe of its XAML bridge window had waited 88 ms at 04.242, and the application raised its next burst of focus `WinEvent`s at 04.883 to 04.892, just before the call returned. This is the stall the design records for a Terminal launch (0.3 to 1.5 s). The focus's top-level window was not the previous focus's, so its bound was `MOVED_ON_GRACE`, and the listener published the read at 04.868, 502 ms after the observation, with one ancestor read. The bound worked as designed. The outpost completed the chain with `plain_windows_from`, which since 9fd28a66 describes every window that is not a ghost's or a hung application's as an MSAA client pane. The top-level `CASCADIA_HOSTING_WINDOW_CLASS` window became a pane with `top_level_client`, spoken by its title alone: "verbatim-e2e-two-a-mvugow8n", where the scenario requires "verbatim-e2e-two-a-mvugow8n window". The later full reads of that focus were duplicates, met by handle, so Core kept the pane.

What NVDA says. `CASCADIA_HOSTING_WINDOW_CLASS` is on NVDA's good UIA class list (`UIAHandler/__init__.py` 96 to 107), as it is on Verbatim's (`arbitration.rs`). NVDA's walk reaches that window through UIA (`correctAPIForRelation`, `NVDAObjects/window/__init__.py` 319 to 330), as a UIA window element named by its title: "<title> window", then "<title> terminal". That is NVDA's capture for opening and switching to a Windows Terminal window (2026-10-09 and 2026-10-22, the module comment of `terminal_windows.rs`), and the listener's own full read here read that element as role window. The pane is right only for a window NVDA reads through MSAA. So 9fd28a66 is undone for windows the outpost judges UIA. It stays for every other window: above the process boundary, and below it for windows judged MSAA or not yet judged. File Explorer's folder window, whose fix 9fd28a66 was, is judged MSAA and keeps its pane.

Where it runs: the outpost's reader, `plain_windows_from`, for each window it describes from local state below the process boundary, on the worker or on the watchdog's thread at a publication.

The rules:

- A window that is not a ghost's or a hung application's, and that the outpost judges UIA without a probe, is described as its UIA element: backend UIA, with the node id the UIA registry gives the runtime id UIA gives a window's host element, `[42, handle]`, the id the listener's read of the same window element carries. Its role is window when its parent window is the desktop, and pane otherwise. A top-level window is named by `InternalGetWindowText`; a child window has no name. It carries its handle and the plain window's states: unavailable from its style, never focused or invisible. Reason: NVDA reads a UIA window through UIA, and the UIA proxy gives a top-level window the window role and a child window the pane role, as the listener's read shows for Terminal's three windows. A child window is taken by Core as structure, unspoken, as it was as a pane.
- "Judged UIA without a probe" means `Arbitrator::verdict_at` at the report's event observation: a forced verdict, the class lists, or a kept verdict. A window with no verdict is never probed here. Reason: the walk was cut because the application is busy, and a probe is a call into it. A window with no verdict is described as MSAA's client, the pane of 9fd28a66, which the full walk could still read through UIA after its probe: a stated limit.
- Above the process boundary nothing changes. The outpost judges no verdict for another application's window ("Calls that reach another application").
- The same node id keeps one object one node within the outpost. A later full read of the window element meets that node by id, and Core meets it by handle, as before.

Speech. A Windows Terminal window whose walk is cut at its bound says "<title> window", "<title> terminal" and "blank", as when the walk completes. A console window the outpost judges UIA, cut likewise, says "<title> window" as `conhost_two_windows` expects. A WinUI 3 window is said by its title and "window" once its probe has been kept. No capture exists of NVDA at a cut walk, since NVDA never cuts one. The words are those of NVDA's full walk.

Cost: on a fallback only, per window described below the boundary, the window's two class names (`verbatim_win32::window::window_classes`, local calls) and one uncontended lock of the arbitrator, besides the style, the text and the ghost and hung checks already made. There are no wake-ups, messages or calls into the application, and nothing on the steady path. No UIA call is added, so there is nothing for remote operations to batch.

Tests. Updated: mockapp's `a_window_not_reached_by_the_bound_is_named_by_its_text`, whose mockapp UIA window is judged UIA by its kept probe. It now expects the window's entry to have role window, backend UIA, the node id of runtime id `[42, handle]`, its title and no `top_level_client`. Kept: `an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound`, whose window is still a pane, and `a_window_above_the_process_boundary_is_a_plain_window_with_no_call`. Added: mockapp's `a_uia_child_window_past_the_bound_is_an_unnamed_pane`, with mockapp's `child-window` under a UIA host. Scenarios: `windows_terminal_two_windows`, `measure_focus_windows_terminal`, `conhost_two_windows`, `explorer_folder_window`, `settings_system_page`.

Limitations. A native provider that gives its top-level window another role or name is described by Windows' state. A window never probed is described as MSAA's client.

### The fake focus's tests (difference 18)

Today. A menu or switcher end goes from the listener after `MENU_END_GRACE` (50 ms) to Core. Core asks the foreground application for its focus unless a focus observed since the end has been applied (`shell.rs`, `menu_or_switch_ended`). The app sends `Query::FocusNow` (`fake_focus`). The answer makes no focused-state test, and Core applies its duplicate rule to every answer, UIA or MSAA.

What NVDA does. `_fakeFocus` passes `objectWithFocus()` to `processFocusNVDAEvent` without force (`IAccessibleHandler/__init__.py` 991 to 1003, 762 to 783). For an IAccessible object only, it drops a duplicate of `lastQueuedFocusObject` by event address and refuses an object for which neither it nor any ancestor has the focused state (`shouldAllowIAccessibleFocusEvent`, `NVDAObjects/IAccessible/__init__.py` 874 to 892). A UIA object is queued with no test, and NVDA's event handler drops no repeated focus, so it is spoken again.

Where it runs: the app, which names the query's purpose; the outpost, which makes the focused-state test; Core, which applies the duplicate rule.

The rules:

- The query says why it is asked (`Query::FocusNow { purpose }`): `Fake` for a menu or switcher end, `Again` for the desktop's return (below), and `Plain` for every other use, as today. The app's request table keeps the purpose with the asker and marks the answer's input with it, as a field of `Input::Event` that only the app sets. Reason: each purpose has its own tests in NVDA, and only the app knows what asked.
- For `Fake`, the outpost makes NVDA's focused-state test, `has_focused_state` (difference 19's walk), on an MSAA answer: on its focused control, or on the window's object when it has no control. An answer that fails is sent with no window and no control, which the app turns into no input. Reason: NVDA refuses it before it is queued, so nothing is spoken and attention does not move. A UIA answer has no test.
- For `Fake`, Core applies its duplicate rule only to an MSAA answer (the node's backend), and takes a UIA answer for the node it holds as a new focus, spoken again. Reason: `processFocusNVDAEvent` tests duplicates for IAccessible objects only (question below). For `Again`, Core applies no duplicate rule, as `_handleUserDesktop` queues `gainFocus` directly. For `Plain`, Core keeps today's rule.
- The skip rule (a focus observed since the end and applied) and the 50 ms grace stay.

Speech. A classic Win32 context menu closed with Escape says "Notes edit selected hello" with no focus event (`captures/classic-context-menu.txt`): the menu's popup was Core's focus, so the answer is no duplicate. Verbatim says this today, and the build keeps it. A fake focus on the MSAA focus Core holds, as after a background application's menu end, says nothing. A fake focus on the UIA focus Core holds says it again. A fake focus on an MSAA object with no focused state says nothing. The last three come from the source alone. No capture exists, and none is known to produce a menu end while NVDA's focus has not moved.

Cost: per fake focus on an MSAA answer, the focused-state walk, usually one `accState` on the focus (the ledger's "A repeated or unfocused focus event, MSAA"); one byte more in the query and its input. There are no wake-ups, and nothing on the event path.

Tests. Added: mockapp's `a_fake_focus_on_an_msaa_object_without_the_focused_state_answers_nothing`, using `set-states` to clear the state, and the app's `a_refused_fake_focus_answer_is_no_input`; Core's `a_fake_focus_on_the_msaa_focus_core_holds_says_nothing`, `a_fake_focus_on_the_uia_focus_core_holds_is_spoken_again` and `a_desktop_return_s_focus_is_spoken_again_on_the_focus_core_holds`. Scenarios: `menu_and_settings_dialog`, `system_information_tree`, `quick_link_menu`.

Limitations. The focused-state test reads the object after the 50 ms grace and Core's round trip, where NVDA reads it at 50 ms.

### The listener's hung test made once (difference 13)

Today. The intake drops every UIA entry whose window the system reports hung when it plans a batch (`intake.rs`, `admit`), the listener's reads included, though the listener made NVDA's hung test at the event. A focus raised just before a stall of more than five seconds is lost. NVDA speaks it when the application answers (the audit's capture of 2026-10-17, not kept in `experiments/`). The worker also finds a text focus's own element and reads its caret before it reports the focus.

Where it runs: the outpost's intake and worker.

The rules:

- Every outcome of the listener's is exempt from the intake's hung test: a read, a published read and a drop. Reason: NVDA tests a UIA focus once, as it arrives, and the listener made that test. The outpost's own UIA events stay tested, as NVDA's handlers test them as they arrive.
- When the worker takes a listener read whose window is hung by `IsHungAppWindow` (a local call), it reports the focus first, with no find and no caret read. The commit records the find and the caret read as owed work (the commit point's "owed work"), attempted once under the owed work's deadline, and the line is sent as a caret report. Reason: a report must not wait on a hung application, and owed work is where calls the followed focus still needs go. A focus in a window that is not hung is handled as today.

Speech. The focus when it is read, and its line when the application answers. NVDA says both together when the application answers. That difference is deliberate (difference 13). If the owed work is abandoned, the field is silent until the next focus. The capture is missing: before the build, a mockapp UIA text node focused by `focus`, then `stall 8000` sent at once after it, captured under NVDA.

Cost: one fewer `IsHungAppWindow` per listener outcome in the intake, and one per listener read in the worker; no call, message or wake-up added.

Tests. Updated: the intake's `a_hung_windows_uia_events_are_dropped_its_msaa_events_kept_for_the_worker`, which now keeps a listener read. Added: mockapp's `a_text_focus_reported_while_hung_reads_its_line_when_it_answers`, which waits for `IsHungAppWindow` to report the window hung and then delivers the stand-in listener's read; it requires the focus report before mockapp's `ended` line and the caret report after it. Scenarios: `text_box_editing`, `notepad_editing`.

Limitations. An application hung past the owed work's deadline has no line read for that focus.

### A UIA dialog's own text (difference 9)

Where it runs: the outpost's reader, `dialog_text_of`, on the bounded client's thread.

The rule: gathering a UIA dialog's own text is bounded by the read's budget, `READ_BUDGET` (2 s), from the gathering's start, in place of `DIALOG_TEXT_WAIT` (1 s). Beyond it the dialog is reported without its text, a deliberate difference, since NVDA waits. Reason: the design's decision of 2026-10-29. It is counted from the gathering's start, not the observation, so a slow walk does not shorten the question's read (question below). MSAA dialogs are unchanged, bounded by the handling deadline.

Speech. A message box whose children answer within 2 s is said with its question, as `menu_and_settings_dialog` expects. No capture exists of NVDA on a slow dialog; the source says NVDA waits.

Cost: a slow dialog holds its focus report up to 2 s, where it held it 1 s. A dialog that answers costs the same calls (the ledger's "A dialog's text, UIA", whose bound line changes).

Tests. Added: mockapp's command `slow-children <id> <ms>` (UIA only: that node's provider answers its first-child navigation late), with its integration test beside `slow-parent`'s; and `a_uia_dialogs_text_answered_after_one_and_a_half_seconds_is_its_description`, with `dialog.json`. Scenarios: `menu_and_settings_dialog`.

### The desktop switch and the virtual desktop's name (differences 22, 23, 17)

The delayed pump (17) is not built. NVDA's capture of a classic context menu shows no batch that the pump would hold (`captures/classic-context-menu.txt`). Step 16 records it in the ledger as a deliberate difference.

Where it runs: the listener's `WinEvent` thread and one of its reading threads; Core.

The rules:

- The listener adds two hooks. One is global and takes only `EVENT_SYSTEM_DESKTOPSWITCH`. The other takes only `EVENT_OBJECT_NAMECHANGE` and is scoped to csrss.exe's process, the one that raises the desktop window's name change (`captures/desktop-name-hook.txt`; the process id comes from `GetWindowThreadProcessId` of `GetDesktopWindow()`, a local call). Each takes one event kind, so that no other event wakes the thread. Reason: a range widened to include them would deliver every event between.
- On a desktop switch, the callback makes NVDA's test, `OpenInputDesktop` then `CloseDesktop`, local calls, and on a return to the user's desktop sends `DesktopReturned` 200 ms later, as `_handleUserDesktop` is called later (`IAccessibleHandler/__init__.py` 786 to 813). It waits on the timer the menu end's grace uses. Core then asks for the foreground application's focus with `Again`. Reason: one delay mechanism of its kind is kept, with no Core timer added. Entering the secure desktop is left to M8.1.
- On a name change whose window is `GetDesktopWindow()`, the callback hands the event to the reading thread of csrss's process. That thread reads the desktop window's MSAA name, `AccessibleObjectFromEvent` on the event's address and `accName`, and sends `DesktopName { name }`, followed 250 ms after the event by `DesktopNameDue`. Reason: D13's callbacks make no cross-process call, and the read took about a millisecond, through an unidentified process. A reading thread is abandoned at `READ_DEADLINE`, as for any read.
- Core keeps the newest name pending. The next focus it takes speaks the name after the foreground's cancel and before the entered containers, and clears it, as `doPreGainFocus` calls `handlePossibleDesktopNameChange` (`eventHandler.py` 338 to 375, 414). At `DesktopNameDue`, a name still pending is spoken as a message, at NVDA's `ui.message` priority, and cleared.

Speech. "Desktop 3", "Desktop list", "Recycle Bin 1 of 13" after Windows+Control+D; "Desktop 2" alone after Windows+Control+Right with a window shown on all desktops in front; "Rework Desk" before the list item at a rename's Enter (`captures/virtual-desktop-name.txt`). After a UAC prompt, the focus again, from the source alone.

Cost: no wake-up beyond each switch and each desktop switch (the csrss-scoped hook received nothing else in 30 seconds). Per desktop name: one `AccessibleObjectFromEvent` and one `accName` on a reading thread, a median of 1.2 ms and at most 45.6 ms measured with the foreground application stalled, and two messages. Per return: two local calls, one message and one focus-now query. The event path is unchanged.

Tests. Added: Core's `the_focus_is_asked_for_after_a_return_to_the_users_desktop`, `the_virtual_desktops_name_is_spoken_before_the_next_focuss_containers` and `the_virtual_desktops_name_is_spoken_when_no_focus_follows`; the listener's `a_name_change_of_the_desktop_window_is_read_on_csrss_s_reading_thread`, with the event injected. Scenarios: `notepad_editing` and `menu_and_settings_dialog`, for no change.

Limitations. The listener's half of the return is untested, since only the secure desktop raises the switch, which Verbatim does not support yet (M8.1). No scenario switches virtual desktops, since doing so moves the harness's windows. A stall of the process that serves the desktop's name delays only that name. Keys held across the secure desktop leave the keyboard hook's modifiers set, which NVDA clears (`winAPI/secureDesktop.py` 43 to 45); that is a follow-up for M8.1.

### A spliced ancestor released by `NodesHeld` (the first possible defect)

Built by the ownership refactor's reducer build, through its release rule; not built here.

The sequence that makes it real. Core's focus is in application B and Core holds no node of application A. A's focus F1, in container C of window W, is observed before B's newest focus but reaches Core after it, so Core drops it as stale and sends no notice, holding nothing of A. A follows F1. Before Core's `NodesHeld` that acknowledges F1 reaches A's worker, A's next focus F2 in C is handled. Its walk meets the followed chain at C, which is still registered, and splices W from it (`read.rs`, `splice`). A splice copies snapshots without looking them up, so W is not recorded as reported by F2 (`Outbound::publish`'s `touched`). The `NodesHeld` then releases W, since W was last recorded at F1's position. Core takes F2 and holds W, which A no longer serves: object navigation up from the focus answers gone at W, and a change on W is matched against an id the registry no longer gives. NVDA has no such hole.

Where it runs: the outpost's reader and its release.

The rules:

- Every spliced entry is recorded as reported by the report that carries it, as a looked-up node is. Reason: a node is released only when Core has seen every message that may carry it.
- A walk meets the followed chain only at an entry that is still in its registry, and only when every entry it would splice is too. Otherwise it walks on and reads those ancestors live. Reason: an entry released before the walk cannot be served again.

Speech: unchanged; the defect is in object navigation and change targeting. Cost: per spliced report, one registry lookup per spliced entry, local, with no call. Tests. Added: mockapp's `an_ancestor_spliced_into_a_later_report_is_not_released_by_an_older_acknowledgement` and `a_walk_does_not_meet_the_followed_chain_above_a_released_ancestor`, with `NodesHeld` sent by the test. Scenarios: `explorer_folder_window`, `settings_system_page`.

### The outbound merge (the second possible defect)

Still this build's: the refactor's merges reproduce today's, and these evidence fixes are built here (Dickson, 2026-10-30), without the clause on `answers`, since read requests are not built.

The states themselves cannot differ: a state change carries the complete set, and the newer replaces the older. What a merge loses is what the older message carried about its own change:

- A tree view item expanded by Right Arrow raises two state changes. The first is newly expanded and carries the child count; the second, read after the record says expanded, carries none. Merged while a write is in flight, the report says "expanded" without "52 items" (`docs/nvda/speech.md`, "How many items an expanded tree view item holds").
- A focus report whose `earlier` says `NotFocused` or `Gone` merged under a newer report of the same node that says `Focused`: Core takes the merged report as a duplicate, so the new element is never announced and a gone element's caret and terminal state are never reset.
- A requested read's report, carrying `answers`, merged under a newer report of the same node: the request is never answered and the idle barrier waits until the outpost ends.

Where it runs: the outpost's outbound queue, `merged`.

The rules: a report carrying `answers` is never merged, as no answer to Core's request is. A merged state change keeps the older's child count when the newer has none and its states still include expanded. A merged focus report keeps the stronger `earlier`, in the order `Gone`, `NotFocused`, `Focused`, none. Reason: the merged message must say what Core would have concluded from both in order.

Speech: "expanded", "52 items" in every run. Cost: a compare per merge, no call. Tests. Added: the outbound queue's `a_merged_expansion_keeps_its_child_count`, `a_merged_focus_report_keeps_the_older_evidence_of_another_element` and `a_report_answering_a_read_request_is_never_merged`. Scenarios: `system_information_tree`.

### The design records (step 16)

One documents commit, after the scenarios pass:

- `docs/architecture.md`. D13's hook list gains the desktop switch and the csrss-scoped name change. The listener reads every UIA focus and skips only NVDA's MSAA class list, and the desktop's name is read on a reading thread. D14's paragraph on judgments that use the focus Core holds is replaced: readers judge on observation, Core judges every repeat, the foreground is tested once, and Core keeps one foreground record.
- `docs/design/focus-pipeline.md` and `docs/design/listener-without-connections.md`: each description of `CoreFocus`, `HeldElements`, the listener's probe and gap, `ancestors_unknown` and the kept chain is replaced by the present mechanism or a pointer to the crate guide, by the conventions for a built design.
- This design is reduced to its problem, alternatives, reasons and limitations, pointing to `docs/crates/verbatim-outpost.md` and `docs/crates/verbatim-core.md`. The roadmap's item moves to `docs/roadmap-done.md`, with its two follow-ups (a focus group's older entries lost with an abandoned worker; the text anchors' locks held across calls) kept under M4's follow-ups.
- The parity ledger: the audit's five "Records to correct", and the deliberate differences. These are the dialog text's 2 s bound; the probe's 3 s; the delayed pump not built; publication at the bound; the focus-now answer onto a hung window; the Settings frame's defer; a window above the boundary as MSAA's client; a window past the bound by the outpost's verdict; the hung focus's line after its focus; and the desktop return's untested half.
- `docs/parity/focus-events.md` and every parity file: each backticked scenario name checked against `crates/verbatim-e2e/src/registry.rs`, such as `notepad_and_verbatim_menu`, now `second_application_and_verbatim_menu`.
- `docs/performance.md`: the entries this build changes, and `docs/glossary.md`: the rework's terms (the entered window, the left focus, a whole-window focus, plain window, the followed focus).

### Questions for Dickson

- A fake focus on the UIA focus Core holds, as after a background application's menu end: NVDA speaks it again, since it tests duplicates for IAccessible objects only. Verbatim says nothing today. Default: speak it again, as NVDA.
- The UIA dialog's text bound: 2 seconds from the gathering's start, or the read's budget counted from the event's observation. Default: from the gathering's start, closer to NVDA's wait.
- The virtual desktop's name, read by the listener through an MSAA call into a process not identified. Default: built as NVDA reads it, on csrss's reading thread, abandoned like any stuck read.
- A window past the bound that the outpost judges UIA, said "<title> window". This undoes 9fd28a66's pane for those windows only. Default: as NVDA's full walk, by the capture.

### Facts owned by two processes

- What a window past the bound is: the outpost's verdict, by the class table the listener shares, with no probe.
- A focus-now answer's purpose: the app's request table, set when it asks and read by the outpost's tests and Core's duplicate rule.
- Whether the user's desktop has returned: the listener's test, timed by the listener and acted on by Core.
- The virtual desktop's name: read by the listener, held pending by Core until the next focus or its due message.
- A listener read in a hung window: the listener's one hung test at the event, and the outpost's local test deciding only whether the find and the caret read are owed.
- Which nodes the outpost serves: Core's `NodesHeld`, against every message that carries a node, spliced entries included.
- What a merged message says: the outpost's outbound queue, which must carry what Core would have concluded from both.

