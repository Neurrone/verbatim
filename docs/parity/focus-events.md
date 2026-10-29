# Focus events

How focus events are accepted, deduplicated, and ordered: duplicate and stale focus events, event acceptance, recovery after an outpost is replaced, held objects, and when speech is cut off.

### Duplicate focus suppression

NVDA: each backend has its own duplicate test, the MSAA one by event
address and the UIA one by `CompareElements` while the last focus NVDA
queued still has the keyboard focus, read live. Both compare with the
one last focus queued from any backend and application, and each only
while that focus is its own backend's object, so a focus from the other
backend or another application in between makes the next one new; it
has no test for a foreground or menu focus
([Event handling](../nvda/events.md),
[The UIA client](../nvda/uia.md)).

Verbatim: **matched (verified)**, with one rule of its own; the two
differences are the next two entries. The rule is defined once
(`verbatim_core::focus_rules::repeats_focus`; decision D14, "Where the
rules run"): a focus repeats when its event names the element of the
focus Core holds, which still has the focus, and Core has judged every
focus the judge sent it. Core applies it, and the focus listener and the
outposts apply it before they read, against the focus Core holds, which
Core tells them after every focus event it judges and every change of
its focus (`SupervisorToOutpost::CoreFocus`;
`docs/design/focus-pipeline.md`, sections 4.2 and 16.3;
`docs/crates/verbatim-outpost.md`, "A UIA focus, judged and read by the
listener").

- Core drops a focus on the node of the live focus it holds, before the
  stale and foreground checks, so it neither moves attention nor counts
  as the newest focus a later focus from another application is judged
  stale against; its newer reading is kept, and what Core last spoke of
  the focus is not moved, so a change the repeat read before its own
  event came is spoken when the event comes, as NVDA compares a change
  with what it last spoke (Core's
  `a_name_change_after_a_repeat_carried_it_is_spoken`). Core compares the node id
  only, not the states, name, or ancestors, which a second report can
  read mid-change (Notepad's edit control settling, File Explorer's title
  being filled in), and not the selected item inside a list; a new
  selected item is announced by its selection event. This rule across
  backends is Verbatim's own, kept in the reducer because only Core sees
  focus across applications
  ([verbatim-core](../crates/verbatim-core.md), M3 noise suppression); it
  covers MSAA's focuses and the answers to a focus-now query. Core's
  `a_duplicate_focus_does_not_make_a_later_focus_stale` and
  `the_same_focus_after_another_applications_focus_is_announced`.
- The focus listener makes NVDA's UIA test itself, in NVDA's order,
  before it reads anything of the event's sender: an event whose sender
  `CompareElements` equates with the element of the focus Core holds,
  while that element's `HasKeyboardFocus` reads true, is dropped; a
  comparison or read that fails lets the event through. A repeat costs
  those two calls. A focus Core refused, as a shell window that only
  stages a switch, is never compared with. Mockapp's
  `a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read`,
  `a_focus_is_no_repeat_once_core_holds_another`, and
  `a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus`.
- Each outpost makes NVDA's MSAA test against the focus Core holds, by
  its address, a whole object rather than a child by id: a focus event at
  that address is a repeat when Core holds a focus of this outpost's and
  it, and every focus this outpost reported that Core has not judged yet,
  is that object. A foreground change Core takes, and a focus Core takes
  from another application or from a UIA focus of the same one, replace
  it. Mockapp's
  `a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object` and
  `a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus`.
- A runtime id is unique only among live elements, and File Explorer
  reuses them: going back from a subfolder, an item of the parent folder
  can take the runtime id of the subfolder's item, destroyed with its
  list. Before a UIA focus whose runtime id already names a node is
  reported, the listener reads live whether the element the id stood for
  still has the keyboard focus, as NVDA checks it; when it does not, or
  cannot be read, the outpost gives the runtime id a new node, so Core
  announces the focus. Mockapp's `focus_reports` tests and Core's
  `a_new_node_for_a_reused_runtime_id_is_announced_and_its_selection_follows`.

### A repeat while a focus waits for Core's judgement

NVDA compares with the last focus it queued, which becomes the focus it
holds when it handles the event. Verbatim: **different (D14)**, "Where
the rules run": Verbatim's judges compare with the focus Core holds,
which a focus they sent becomes only once Core has judged it. While one
waits, nothing is a repeat: the listener reads a repeat of a focus it
read moments before (mockapp's
`a_focus_waiting_for_core_s_judgement_is_no_repeat`), and an outpost
reports an MSAA focus repeated at the address it reported moments
before, unless Core holds a focus of its and every focus it judges
against is that object. Core's own rule then drops the repeat, its newer
reading kept unspoken, so nothing heard changes; the read is the cost.

### A repeat when a read's time is up

NVDA reads the sender's keyboard focus live, and a live read that fails
lets the focus through (`except COMError: return True`,
`nvda/source/NVDAObjects/UIA/__init__.py`, lines 1632 to 1637).

Verbatim: **different** (Dickson, 2026-10-20): once the listener's read
of a focus has used its time budget, 2 seconds, it makes no more calls
into the application, and the repeat test is skipped, since it needs the
last focus's keyboard focus read live; a repeat in an application that
slow is spoken where NVDA drops it. Its other checks are made where no
call is needed (`docs/design/listener-without-connections.md`, section
5), and the sender's keyboard focus is taken from the event's cache:
anything but a cached `true` drops the focus. A provider that answers
the property empty, or whose answer fails, leaves UIA's default,
`false`, in the cache, and NVDA's live read of such a provider gets the
same `false` and drops the focus too. Mockapp's
`a_repeated_focus_out_of_time_is_reported` pins the skipped repeat test,
`a_focus_out_of_time_whose_event_says_it_lacked_the_keyboard_focus_is_dropped`
the drop from the cache, the events test
`uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one`
what UIA stores for an empty or failed answer, and `verbatim-uia`'s
`a_focus_event_lacks_the_keyboard_focus_unless_its_cache_holds_true` the
reading of it.

Matched (Dickson, 2026-10-21): within the budget, a live read of the
sender's keyboard focus that fails, by UIA's transaction timeout or
otherwise, lets the focus through, as NVDA's does; the event's cache
judges the focus only when the budget is out before the live read is
made. Mockapp's `a_focus_whose_keyboard_focus_read_times_out_is_reported`.

### Stale focus events

NVDA has no timestamp arbitration; it relies on queue-time freshness
plus cancellable speech, its one queue handles every application's
events, and its keys, in the order observed, and it judges a foreground
event against the foreground window when it processes it
([Event handling](../nvda/events.md), [Speech](../nvda/speech.md)).

Verbatim: **matched (unverified)**; the `rapid_tabbing_in_settings` and
`notepad_and_verbatim_menu` scenarios cover it.

- Order within an application comes from its outpost's single queue and
  worker. Core takes the outposts' messages and its keys and commands
  from one channel, in the order they reached it
  (`docs/design/focus-pipeline.md`, section 4.1), and holds none back.
- Across outposts, events can arrive out of the order they happened in,
  so the reducer drops a focus event from one outpost observed before the
  newest focus it applied from another (decision D14). Every process
  stamps every observation on the run's shared clock
  (`verbatim_process::clock`), in microseconds from one clock read, and
  a tie is no earlier, so neither is stale: UI Automation delivers focus
  events in bursts, several in one millisecond, and in an Alt+Tab check the
  switcher's pane, read first, reached Core after the console switched
  to and was announced over it.
- What each focus's time is: a UIA focus, when the focus listener's
  callback received it, since UI Automation gives an event no time of its
  own; a `WinEvent` focus, when the hook's callback that took it in began;
  a focus read on request (at startup, after an outpost is replaced, or
  after a menu closes), when the read began, so it is ordered with the
  events as NVDA's queue orders the focus it reads; a focus the outpost
  reads when no UIA focus followed a `WinEvent` focus, the copy's
  observation; and the end of a menu or of the switcher, when the
  listener's callback observed it.
- A foreground change is ordered by the time its window was confirmed as
  the system's foreground window, not when Windows raised the event:
  Windows raises it before the change completes, and the old foreground
  window's focus events in between would otherwise be newer and make the
  change stale. This orders it where NVDA's processing does. The latency
  timeline still starts when the foreground event was first observed.
- A UIA focus fact whose element is not the focused element is dropped
  when the focus is in another application by then, and otherwise
  reported from the fact with its window: its own, or for a windowless
  element the keyboard focus window the listener found in the element's
  process when it captured the event, standing in for the nearest window
  NVDA walks up to (exact when the event is current; for a late event in
  an application with several windows it can be another of them); only
  with neither is it judged by its application.
- A focus in the same top-level window as the newest focus is never
  stale, since one window can hold several applications (a Settings
  page's content inside ApplicationFrameHost's frame) whose own focus may
  be observed before the frame's foreground is confirmed.
- A focus with no window facts is taken to be in the attended window,
  since only that let it be accepted, so a later foreground report for
  that window does not replace the control as the focus; NVDA always
  knows the focus's window.

### Event acceptance

NVDA: every event except UIA notifications must come from a window
related to the system's foreground window at the moment NVDA processes
it: a descendant of it, sharing its root owner, a topmost window or one
whose root is topmost, or, for `Windows.UI.Core` windows, a descendant
of the input thread's active window. Allowlisted tooltip and
notification-bar windows, toast alerts, and, as an option, background
progress bars are accepted from anywhere. UIA notifications are spoken
only from the focus's application, with per-application opt-ins such as
the shell's window-snap results. The test (`shouldAcceptEvent`) is made
per event as NVDA processes it, before it creates the event's object,
so a background window's event costs no call into its application, and
not in the event callback, which lost the focus events of applications
starting up (issue 4001). A foreground event is held for at most two
core cycles until its window is the foreground (issue 3831), then
dropped if it is not ([Event handling](../nvda/events.md), "Acceptance
filtering" and "Foreground windows"; [MSAA and winevent
handling](../nvda/msaa.md), "Pump-time processing"; [The UIA
client](../nvda/uia.md)).

Verbatim: **partial**. The tests below match NVDA; the background cases
in "Background events" are partly not implemented, and the foreground
window NVDA compares against is "Attention" below. The test is defined
once (`outpost::acceptance`; decision D14, "Where the rules run"), for
the outposts and the focus listener, with the window's relation to the
foreground window (`foreground_relation`) reported with an event's
window facts, which the reducer judges a focus by after the read
(`verbatim_core::focus_rules::ForegroundRelation::related`).

- Each outpost's worker makes NVDA's test, rule by rule in NVDA's order,
  with local calls only, before any call into the application, for MSAA
  name, description, state, value and selection changes, the system
  caret's location changes and shows (both tested as a location change;
  NVDA tests a caret show by its focus's own rule instead,
  `docs/parity/text.md`, "The system caret beside other caret events"),
  focus, foreground and popup menu facts, and alerts. Show facts pass by
  their class, which the focus listener's hook already tested, as NVDA's
  show rule depends on the class alone. Outside the test, as in NVDA's
  callback: an object destroyed, and Verbatim's own events, the text
  selection of edit controls, and the console host's updates and caret.
- Different: with no window in the foreground, an event from an invalid
  window is refused, where NVDA's comparison of the two root owners, both
  0, accepts it; nothing is spoken differently, since such a window
  cannot be read.
- MSAA, after acceptance and in NVDA's order: an event is dropped when
  its own window is no window, has a ghost window standing in for it, or
  belongs to an application the system reports hung, then when its
  window is UIA's, and only then is its object acquired. The intake's
  hung test on the top-level window, for the batch limits, stays.
- UIA, in NVDA's order: whether the event is the focus's, then its
  window's UIA test, then acceptance with the event's window, the
  focus's recorded window for the focus's own events, or the element's
  nearest window; a windowless element whose window is not found has no
  window to test, and is accepted, where NVDA walks to the nearest
  window. A windowless UIA focus is never tested as hung by the intake,
  as NVDA's hung-window test passes a UIA event whose sender's cached
  window is 0; a focus with a window of its own is. Property changes,
  selections, and a text control's caret and text changes are tested, as
  NVDA's automation event handler tests them; a windowless selection is
  tested with its nearest window (`FocusReader::check_fact`).
  Notifications and the active text position are not tested, as in NVDA,
  but a notification whose sender has no window above it is dropped, as
  NVDA's app module default drops it, the shell's window-snap results
  excepted, as File Explorer's app module lets them through.
- The UIA focus is tested by the focus listener, in NVDA's order
  (`docs/design/focus-pipeline.md`, sections 16.2 and 16.3): after the
  repeat check and before the focus is read, against the element's own
  window or its nearest ancestor's (`getNearestWindowHandle`, one call);
  a focus that fails is dropped and never becomes the last focus
  accepted, as NVDA's `lastQueuedFocusObject` is set only for a focus it
  queues (mockapp's
  `a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is`).
  A UIA menu opening is judged by the focus listener as a focus on the
  menu, with the checks of NVDA's focus handler but the duplicate test,
  NVDA's keyboard focus read included, and a drop while a focus is
  pending.
- UIA's MSAA proxy is told never to raise, from `WinEvent`s, the UIA
  events Verbatim subscribes to, as NVDA's `ignoreWinEventsMap` tells it
  (issue 7345); focus changes keep their mapping.
- Each registration's cache request differs from NVDA's twelve base
  properties: the listener's and the focus property changes' carry what
  a node is spoken or its states are read from, and a text focus's caret
  and text events only their process, window, and class name, from which
  nothing is spoken; a difference in the provider's work only.

### Attention

NVDA compares each event with the system's foreground window, and holds
a foreground event for at most two core cycles until its window is the
foreground ("Event acceptance", above). Verbatim: **different (D14)**:
the reducer keeps an attention
record, the application and top-level window of the most recent
foreground change, standing in for the system's foreground window.

- A foreground fact is always accepted and moves attention. The outpost
  handles a batch that holds a foreground fact at once and drops the fact
  when its window is not the foreground window then, as NVDA's
  `processForegroundWinEvent` drops it once its deferral has passed; a
  foreground fact beside MSAA focus facts is tried with them, newest
  first, as NVDA's `pumpAll` tries a run of focus and foreground events
  ([backends](backends.md), "How an outpost turns events into focus
  reports"). It does not wait for the window to come in front: the
  system's foreground event reaches an out-of-context hook only once
  `GetForegroundWindow` names its window, after a restore,
  `SetForegroundWindow`, Alt+Tab, or a launch (measured for console
  windows, File Explorer, Notepad, msinfo32, Windows Terminal, a Windows
  Forms window and Verbatim's menu and dialogs). The foreground events
  that come before it are stale (the Alt+Tab switcher's staging windows,
  already passed), raised by the application itself (File Explorer
  raises one as it creates its folder window, 140 to 400 ms before the
  window is in front, and the system raises its own once it is), or
  those of Core's hidden frame, which the outpost drops whatever the
  foreground.
- A foreground fact whose window is in front is reported at once,
  whether or not the window has been shown, as NVDA tests only that the
  window is the foreground window. File Explorer's folder window, opened
  in front, raises its one foreground event before it is in front, which
  is dropped, and is announced from the focus's ancestry, its title by
  then the folder's.
- A focus event is attended when its window was in the system's
  foreground window when its outpost read it, topmost, or a
  `Windows.UI.Core` window under the input thread's active window, as
  NVDA tests it against the real foreground window; every other event is
  classified against the attention record from the window facts its
  outpost attached. Windows can raise a window's foreground event while
  its foreground lock keeps another window in front, and raises none
  when the window is given the foreground later, so a focus in the
  system's foreground window that is unrelated to the attention window
  moves attention, as NVDA takes the foreground from the focus's
  ancestry. Nothing else moves attention, so a topmost popup menu that
  takes focus without becoming the foreground leaves attention where it
  was, and focus returning from it is still attended.

### Background events

NVDA accepts allowlisted tooltip and notification-bar windows, toast
alerts, and, as an option, background progress bars from anywhere, and
UIA notifications only from the focus's application ("Event
acceptance", above). Verbatim: **partial**.

- Matched: toast alerts and the shell's window-snap results are always
  queued, as NVDA's Explorer module queues them, and other UIA
  notifications only from the focus's application, as NVDA drops
  notifications from any other. Accepted background events never move
  focus or the navigator and are spoken queued. Core drops a background
  event observed before the newest foreground change it applied but
  reaching it afterwards, from another outpost, as NVDA's one queue has
  its speech cancelled by that change (`docs/design/focus-pipeline.md`,
  section 7; Core's
  `a_toast_observed_before_a_foreground_change_is_dropped`).
- Matched: a help balloon, shown in a standard tooltip window
  (`tooltips_class32`), is spoken from any application as NVDA speaks it
  by default (the listener hooks the show event desktop-wide and
  forwards only a tooltip window's), and an ordinary tooltip is not, as
  NVDA's tooltip reporting is off by default (the `msaa_events` mockapp
  test, scripted; no end-to-end scenario).
- Not implemented: the setting that turns tooltips on (not yet, M4.3, the
  settings package in the roadmap) and a per-source cap on background
  events (not yet, unscheduled). The opt-ins application support asks
  for (`requestEvents`) have a table that nothing fills yet. NVDA's rule
  for windowless Chromium documents is not ported, as Verbatim has no
  Chromium support. The other window classes NVDA accepts show events
  from belong to one application or input method (Internet Explorer's
  notification bar, a chat application's tray alert, IME candidate
  windows) and are deferred with application support.
- Background progress bars are not yet reported (M4.3); NVDA's are
  silent by default (Dickson, 2026-10-10).

### A slow read the user has moved on from

NVDA: its watchdog waits up to 10 seconds for a main thread stuck in a
call before cancelling it, but only half a second when the evidence says
the user has moved on: no window has the keyboard focus, a system menu
NVDA has not caught up with is open, the active window is no longer the
focus object's, or the window with the keyboard focus belongs to another
thread than the focus object's window
(`nvda/source/watchdog.py`, `MIN_CORE_ALIVE_TIMEOUT` at line 47,
`_waitUntilNormalCoreAliveTimeout` at lines 133 to 148,
`_shouldRecoverAfterMinTimeout` at lines 206 to 232). The call is
cancelled, and calls that cannot be cancelled run through
`cancellableExecute` (lines 406 to 426), whose thread is abandoned; the
focus being read is then dropped
([Main loop and watchdog](../nvda/main-loop-and-watchdog.md), "The
watchdog: detection" and "What a live capture shows"). NVDA's watchdog
cancels only its main thread's call (`nvda/source/watchdog.py`,
`_recoverAttempt`, line 237).

Verbatim: **different** (Dickson, 2026-10-20 and 2026-10-21); unit tests
of the outposts' worker and of the listener's reading threads pin the
rule, and no end-to-end scenario makes an application slow enough to
show it. An outpost's worker and the focus listener's reading thread for
an application each abandon a call that has taken half a second
(`MOVED_ON_GRACE`) once the window in front belongs to the same
application on another UI thread than the window being handled or read,
the application being each window's provider process, until their own
deadlines (10 seconds for the worker, `READ_DEADLINE`, 11 seconds, for a
reading thread); the event is dropped and a new worker or reading thread
takes the application's later events
(`docs/crates/verbatim-outpost.md`, "A UIA focus, judged and read by the
listener", owns when each judges). Console host's own Find, Properties
and Defaults windows are on the console window's real thread, while
`GetWindowThreadProcessId` names the client's thread for the console
window, so with one of them in front a slow read of the console window
is abandoned after the half second, as NVDA's watchdog, comparing the
same two thread ids, abandons it. A console window whose host cannot be
read, such as an elevated client's, is never judged moved on from, nor
is moving to it. By case:

- Matched: the read of a focus, or of a menu opening, which NVDA maps to
  a focus (`nvda/source/UIAHandler/__init__.py`, line 261), after its
  judgement: the calls NVDA's main thread makes once the handler has
  queued the focus.
- Different, deliberately: the judgement's own calls once the read's
  window is known, the probe, the console check, the comparisons and the
  live keyboard focus read, which NVDA makes in its focus handler on
  UIA's callback thread and never cancels, are judged too, so a slow one
  can drop a focus NVDA would speak late.
- Not judged, as in NVDA: a windowless focus's nearest-window lookup
  (`nvda/source/UIAHandler/__init__.py`, line 924), during which its
  window is not yet known, and the whole check of a selection or a
  notification (line 1114 for a notification's lookup), which NVDA makes
  on its callback thread, so a slow selection or notification is passed
  on late rather than dropped.
- Narrower by design: NVDA's one main thread serves every application,
  so a move to any other window frees it, while each of Verbatim's
  applications has its own worker and reading thread; only the same
  application's windows on other threads, such as File Explorer's folder
  windows, taskbar and Alt+Tab switcher, wait behind a slow one. NVDA's
  other triggers, no focused window and an unacknowledged system menu,
  are not ported, nor its allowlist of window classes that always get
  the long wait (`safeWindowClassSet`).
- NVDA judges against its own focus object's window, the last focus it
  accepted (`nvda/source/watchdog.py`, lines 224 to 232), so when the
  user moves to another thread's window and that window's own focus read
  is slow, NVDA cancels it; Verbatim deliberately does not, judging
  against the window being read (Dickson, 2026-10-20).
- NVDA judges again every half second while it waits
  (`_waitUntilNormalCoreAliveTimeout`, lines 133 to 148); Verbatim judges
  when the half second ends and on foreground events, so a move of the
  keyboard focus to another thread with no foreground event, such as to
  a child window owned by another thread, is caught by NVDA and missed
  by Verbatim.
- NVDA compares only threads, by `GetWindowThreadProcessId`, which names
  a console window's client and one of the client's threads
  (`nvda/source/watchdog.py`, lines 230 to 232). Verbatim also requires
  both windows to be served by one process
  (`verbatim_uia::provider_process`): a move between a console window and
  a window of its client's own on another thread is not a move within
  one application, and no read of either is abandoned for it.

### Reads held behind an application's abandoned reads

NVDA has no reading threads to abandon. Its rule dropping a UIA menu
opening while a focus is pending is global
(`isPendingEvents("gainFocus")`, `nvda/source/UIAHandler/__init__.py`,
lines 772 to 777): any queued focus drops any application's menu
opening.

Verbatim: **different** (Dickson, 2026-10-21). An application with two
abandoned reading threads still in their calls gets no new thread until
one returns: its events wait, until `READ_DEADLINE` after its second
thread was abandoned. Then every event waiting is dropped, and each later
one as it arrives, until an abandoned thread's call returns and takes
the queue. The bound is one per application, not one per event, so the
application's outpost runs at most `READ_DEADLINE` behind the hung reads,
once, and a stream of events from the application cannot keep it
behind. A focus waiting so is a pending focus only for its own
application's menu openings, so that a hung window cannot silence every
other application's menus while its reads stay stuck; a focus being
read, or waiting for a live thread, still counts for every application,
as in NVDA.

### Recovery after an outpost is replaced

NVDA has no equivalent: it is one process, and after an application
crash it re-queries the real focus
([Focus and the navigator](../nvda/focus-and-navigator.md)).

Verbatim: **different (D9, D13)**. When an outpost
ends, the reducer keeps the focus's copied data but treats its node ids
as dead, and clears the navigator if it belonged to that outpost. If
the application holds attention, a replacement starts at once and is
asked for its current focus; when the reply arrives, it is compared
with the kept copy (role, name, value, states, and the ancestors' names
and roles, which a reply with its ancestors unknown leaves unchanged, as
the focus keeps its previous chain then), and if all match, the new
node ids are taken silently, otherwise the focus is announced as usual.
Tests: `crates/verbatim-core/tests/reduce.rs`,
`a_replacement_outpost_reporting_the_same_focus_is_taken_silently`,
`a_replacement_outpost_reporting_the_same_focus_with_its_ancestors_unknown_is_taken_silently`,
and `a_replacement_outpost_reporting_a_different_focus_announces_it`. The same question is asked
after the focus listener is replaced.

### A focus UIA does not deliver

UI Automation sometimes delivers a newly started process's focus events
late or not at all, while the system's `WinEvent` focus for the same
moves still comes, and sometimes delivers them after their elements are
gone (`docs/design/listener-without-connections.md`, risks 2 and 13).

NVDA, by its source at the pin (732d1ce2): it drops every `WinEvent`
whose window is native UIA before making any object
(`source/IAccessibleHandler/__init__.py`, `winEventToNVDAEvent`, lines
577 to 582), so the `WinEvent` focus is not used; and it drops a UIA
focus whose sender has no window above it, since `getNearestWindowHandle`
answers none for a gone element (`source/UIAHandler/__init__.py`, lines
1437 to 1444) and `isNativeUIAElement` then finds the element not native
(lines 1516 to 1575, its drop at 887 to 899 of the focus handler); and it
drops a UIA focus whose element no longer has the keyboard focus
(`shouldAllowUIAFocusEvent`, `source/NVDAObjects/UIA/__init__.py` lines
1632 to 1637, tested at line 948 of the focus handler). It has no other
path that reads the focus after a `WinEvent`, so by its source it loses
these focuses; that is not captured.

Verbatim: **different**, by that design's decision 11 (Dickson,
2026-10-21). When no UIA focus follows a `WinEvent` focus in a UIA window
within a second of the `WinEvent`, the application's outpost reads its
focus itself, as the focus-now query does, and reports it, once. The
listener's drops of the UIA focus it waits for fall into three classes:

- A drop that ends the wait: a verdict of NVDA's checks on the event,
  for which NVDA drops it too, or the listener giving up on the
  application when its read's time ran out, a read into an application
  that has just failed to answer not being made, among them a drop whose
  event said it lacked the keyboard focus when the read's time was up
  (Dickson, 2026-10-21).
- A drop that could not establish the focus: its element no longer has
  the keyboard focus (NVDA's drop at lines 1632 to 1637), its window does
  not use UIA, so it is another window than the copy's, its read was
  abandoned or never started, its element could not be kept or resolved,
  or a newer event of its element replaced it (that last one Dickson's,
  2026-10-21). The wait goes on to its second with nothing read at once,
  since these drops come in ordinary fast navigation, where the next
  key's focus event follows (Dickson, 2026-10-21).
- A sender with no window above it, as in a new window's first burst of
  focus events after their senders are gone: the event came and cannot
  be read, and the event for where the focus went may never come, so the
  outpost reads its focus at once.

Where NVDA drops a gone sender's focus event and speaks nothing until the
next one comes, Verbatim speaks where the focus now is. A read answers
only a copy in its own top-level window, so a UIA focus in another
window of the application answers nothing of it; a drop carries no
window and is matched by when it was observed alone, a limit. So where
UIA delivers late, Verbatim speaks the same words a second after the
`WinEvent`, earlier than NVDA would; where it never delivers, and for a
burst of gone elements, Verbatim speaks the focus where NVDA, by its
source, is silent. A late UIA focus for the element already read is
Core's duplicate and says nothing. Tests: `focus_reports.rs` in
`mockapp`, its `a_winevent_focus_` tests.

### A focus whose element is gone before its read

NVDA, by its source at the pin (732d1ce2), handles a UIA focus whose
element is gone in one of two ways, by when it went. Gone by the time the
callback's window walk runs (line 1438): `isNativeUIAElement`
(`source/UIAHandler/__init__.py`, lines 1516 to 1575) asks
`getNearestWindowHandle` (lines 1383 to 1460), whose live
`NormalizeElementBuildCache` walk fails and answers none (lines 1439 to
1444), so the element is not native (lines 1573 to 1575) and the callback
drops the event (lines 887 to 899); nothing is spoken. Gone after the
callback but before the main thread handles the event: the callback found
the window and made the object (lines 924 to 958),
`shouldAllowUIAFocusEvent` (`source/NVDAObjects/UIA/__init__.py`, lines
1632 to 1637) allows it on the COMError of its live read, and the event is
queued (line 958) and handled (`source/eventHandler.py`, `executeEvent` at
lines 312 to 359, `doPreGainFocus` at lines 378 to 429);
`api.setFocusObject` (`source/api.py`, lines 80 to 194) walks `container`
(line 145), and the UIA object's parent (`source/NVDAObjects/UIA/__init__.py`,
lines 2136 to 2154) catches the failed live parent read (lines 2142 to
2143) and falls back to the window hierarchy
(`source/NVDAObjects/window/__init__.py`, lines 287 to 293), so the UIA
ancestors such as a list are left out. The role comes from the event's
cache (`source/NVDAObjects/UIA/__init__.py`, lines 1898 to 1902); the name
from the cache only while no main-loop pass has cleared the per-cycle
cache (lines 1148 to 1153 and 1172 to 1184; `source/core.py`, line 1035),
otherwise from a live read that fails, which `_get_name` (lines 1883 to
1887) answers with an empty name; the states through the variant that
handles COMErrors (lines 1155 to 1170). The focus is spoken without its
UIA ancestors, with or without its name, by its source; not captured.

Verbatim: **matched (unverified)**; the listener's own read decides both
cases (`crates/verbatim-outpost/src/focus_read.rs`). NVDA's first case is
the listener's nearest-window walk finding no window, which drops the
event as `NO_WINDOW`; there Verbatim differs, as "A focus UIA does not
deliver" records: the outpost reads the focus at once and speaks where
it now is. In NVDA's second case the listener's classic read of the
sender fails, and the listener reports the focus as the event's cache
gives it, its ancestors unknown. A walk whose step from an element below
the desktop root to its parent fails or finds no parent goes on, as NVDA's
failed parent read falls back to the window hierarchy
(`Window._get_parent`), from the parent window of its outermost element's
nearest window, by that window's own object: the listener, which connects
to no window, names the window, and the outpost goes on from it; the
outpost's own walks go on the same way, and a walk cut by the read's time
budget or its depth limit is completed by plain windows from that nearest
window. A step that reaches a ghost's or a hung application's window
other than the focus's nearest one ends every UIA walk there, as NVDA
makes that one step and then reads the window as a plain window, going on
by the window hierarchy with no further call (`correctAPIForRelation`,
`NVDAObjects/window/__init__.py` 85 to 97); the listener names the window
and the outpost goes on from it, and a focus-now query with such a window
above its focus reads the ancestors by the classic walk rather than a
remote program (Fable, 2026-10-30, pending Dickson's acceptance;
`focus_reports.rs`'s `a_hung_window_above_a_uia_focus_ends_the_listeners_walk`
and `a_hung_window_above_a_focus_now_is_a_plain_window_remote` and
`_classic`). The completion at the bound and the depth limit is a deliberate difference (`docs/design/focus-pipeline-rework.md`,
"A window's name, and the one fallback", decided 2026-10-29, which replaces
reporting such ancestors unknown, decided 2026-10-21). The listener reuses
only a chain that reached the top, and the outpost, as NVDA's focus line,
meets the previous chain where the new one meets it, so the next focus
inside such a focus keeps its chain (`docs/crates/verbatim-uia-rops.md`,
`failed_hop`; `docs/crates/verbatim-outpost.md`, "Reading the focus").

For the File Explorer placeholder the spoken result is to be checked by
`explorer_folder_window`: the placeholder's element can answer its
properties after it has left the tree, so only the parent step fails
(run 39613639083, with `read=0`; reported as known and empty, before the
chain went on from a window, the next focus counted every container as
entered and spoke the window's title). It is spoken without its list, and always with its name, which
NVDA loses when a main-loop pass came between. The time in which the
element must go differs: in NVDA from the callback to the main thread's
handling; in Verbatim the consecutive calls on one listener thread
between the walk and the classic read, plus the read budget. File
Explorer's "Working on it..." placeholder removed in that time is spoken
before "Items View list" rather than between it and the item; the
outpost's later search for its own copy of the element, which logs "the
focus's own element looked for by its path" with found=false beside it,
decides only what is followed. The observations are the logs of runs
39558690998 and 39613639083; `explorer_folder_window` accepts the
placeholder at either place, so a local pass of it does not prove it.
No capture of NVDA exists for this case.

Tests: `remote_ops.rs` in `mockapp`,
`a_focus_whose_read_fails_is_reported_whatever_its_event_says_of_its_focus`,
`a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown`,
and `a_focus_whose_parent_step_fails_goes_on_from_its_windows_parent`
(the failed step and the missing parent) for the listener's report;
`a_focus_now_whose_program_finds_no_parent_below_the_window_goes_on_from_its_windows_parent`,
`a_focus_now_whose_program_ends_below_the_window_at_an_element_reporting_a_handle_goes_on_from_that_window`,
`a_focus_now_whose_classic_walk_finds_no_parent_below_the_root_goes_on_from_its_windows_parent`,
and
`a_focus_read_after_a_winevent_focus_whose_walk_finds_no_parent_goes_on_from_its_windows_parent`
for the outpost's own walks; `focus_reports.rs`'s
`a_focus_after_a_failed_acc_parent_reaches_its_dialog` and
`a_chain_cut_at_the_depth_limit_is_completed_by_its_windows`; `call_counts.rs`'s
`uia_focus_chains_stop_below_the_process_boundary`, whose "child,
remote" case is a remote walk complete at a window below the process
boundary;
`a_focus_inside_one_whose_parent_step_failed_meets_its_chain_there`
for the next read; the unit test
`a_failed_step_is_told_by_its_code` in `verbatim-uia-rops`; and
`focus_reports.rs`'s `focus_without_a_path` and its caller
`a_focus_without_a_path_is_followed_from_its_next_focus_event`.

### Held objects

NVDA: an object lives as long as something refers to it, and an MSAA
event, which names its object only by window, object id, and child id,
is directed to the existing focus object when the two compare equal
([Event handling](../nvda/events.md)). The comparison requires equal
child ids; the same COM object is equal; two `IAccessible2` objects in
the same window compare by unique id; otherwise differing event
addresses are unequal, and the MSAA identity strings, location, role,
and name must all match.

Verbatim: **different (D9)**, by the outposts' node lifetime
([Architecture](../architecture.md), D9, and
[verbatim-outpost](../crates/verbatim-outpost.md), "Window destruction
and held objects"); no test pins it as a whole. NVDA releases an object
when nothing refers to it, Verbatim when Core reports it no longer holds
the node.

- Each outpost keeps its nodes, with the live UIA element or MSAA object
  behind each where it has one (a UIA node reported from a listener fact
  carries only cached properties), for every node Core still holds, plus
  every node reported after the last message Core has acknowledged, and
  releases the rest; a query for a released node answers "gone".
  Navigation, activation, and ancestor reads of an MSAA node use the
  object that was announced.
- A new MSAA sighting is the same node when it is the same COM object
  with the same child id in the same window, or when it was acquired at
  the same address as the node and a fresh read of the node's kept
  object gives the same role and the same identity string (both absent
  counts as the same); a failed role read is an unknown role, as for
  NVDA, so a kept object that no longer answers stops matching.
- An object reached through `accParent` that is one of a window's own
  standard objects has the address its identity string names
  (`DecomposeHwndIdentityString`), and is matched as one acquired there,
  so a window keeps one node whatever its title says, as NVDA, re-reading
  the old object live, finds the two equal. Any other object reached
  through `accParent`, or as a child object, has no address of its own,
  so it is matched only as the same COM object.
- `IAccessible2` unique ids are not read yet, and location and name are
  not compared when matching a sighting.
- A window's MSAA nodes are dropped when the window is destroyed; UIA
  nodes are not, since a runtime id has no documented structure naming
  its window, so a UIA node Core still holds after its window closed
  (the navigator left there) matches a new element given the same
  runtime id after the window handle value is reused, as an NVDA object
  compared by runtime id would.
- Positional child ids in simple list controls remain a known
  limitation that NVDA shares.

### When speech is cut off, and cancellation of expired focus speech

When speech is cut off, and cancellation of expired focus speech (focus
left before speaking). NVDA: focus speech is queued, never interrupting;
every key-down cancels speech before its gesture runs, bound or not,
modifiers and typed characters included, except the volume keys and the
unknown key `0xFF`; Shift alone pauses and resumes, its auto-repeat
ignored; speaking while paused cancels first, and a cancel ends the
pause ([Keyboard input](../nvda/input.md), "What a key press does to
speech"). A foreground change, which NVDA infers whenever the top of
the focus ancestry changes, cancels speech, named window or not, and so
does entering a menu bar, menu, or menu item. On each focus change,
speech for a focus no longer current is culled
(`FocusLossCancellableSpeechCommand`, which keeps speech for the focus,
its ancestors, the foreground object, an object that never had the
focus, and a menu item when the focus has moved to a popup menu;
[Speech](../nvda/speech.md), "Expired focus speech", and
[Event handling](../nvda/events.md)).

Verbatim: **matched (unverified)**. Reducer, input, and speech pipeline
tests cover the rules, and the end-to-end scenarios pass with them, but
no scenario asserts where live speech is cut off. Focus, value, state,
selection, navigation, and review speech is queued; the keyboard hook
cancels, or pauses and resumes, on the key-down before the gesture is
sent, with NVDA's exceptions, and a gesture injected through the control
plane cancels too, while the keys Verbatim injects for itself (the
Control tap that unlocks the foreground) leave speech alone, as NVDA
ignores its own; a foreground report or a focus in another top-level
window cancels, as does entering a menu; each focus announcement carries
what it is about, entered containers are their own utterances, and on
every focus change the speech manager stops what it has handed on when
any of that has expired, and judges waiting focus speech when its turn
comes, as NVDA does. A selection in a list the focus controls still
interrupts, as NVDA cancels for it, and notifications keep their
processing hint. NVDA's settings "Speech interrupt for typed
characters" and "Speech interrupt for Enter" (both on by default) are
implemented in the keyboard decision machine, stored in
`settings.toml`'s keyboard section and applied to the running hook,
with NVDA's rules for what counts as a typed character
([Keyboard input](../nvda/input.md)).

- The clause for a menu item whose focus has moved to a popup menu above
  it (NVDA issues 12624 and 14550, MSAA only) is matched against the
  focus the user just left only: Core names that item in
  `FocusNow::menu_item_of_focus`, where NVDA tests the object of every
  utterance still waiting. Speech for an older focus has already been
  dropped by the focus change that left it.
- Shift's pause: Verbatim toggles the pause on the speech queue thread,
  in order with cancels, and a cancel or speech arriving while paused
  ends the pause, as NVDA's does ([Keyboard input](../nvda/input.md)),
  reported on the control plane as a resume. Heard the same in every
  case checked: Shift and then a command key (the key cancels the paused
  speech, and what it causes is spoken); Shift and then a typed
  character with speech interrupt for typed characters on (the same)
  and off (Shift does not pause, and the character does not cancel, in
  both); Shift alone twice (paused, then resumed where it was).

Differences, in plain words:

- Audio cannot be taken out of the middle of the mixer's buffer, so
  when anything handed to the synthesizer or the mixer has expired,
  everything handed on is stopped; NVDA stops up to the newest expired
  utterance and keeps what was handed on after it.
- The foreground node the check uses is the window most recently
  reported as the foreground; NVDA asks for the real foreground object
  on every focus change that changes the top of the ancestry, so a
  focus that reaches another window without a foreground report leaves
  Verbatim's foreground node on the old window until one arrives.
- NVDA's advanced setting to turn the culling of expired focus speech
  off is not offered (not yet, M4.3, the settings package in the roadmap).
- Only in a race: a second Shift pressed before NVDA's main thread has
  applied the first pause pauses again, so speech stays paused, where
  Verbatim always resumes.
