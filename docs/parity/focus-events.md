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

Verbatim: **matched (verified)**, with one rule of its own; the
difference is the next entry. The rule is defined once
(`verbatim_core::focus_rules::repeats_focus`), and only Core applies it
(`books/dev/src/explanation/design/ownership.md`): a focus repeats when
its event names the node of the focus Core holds, whose element still
has the focus. Every reader reads a repeat in full and reports it, and
Core judges it in the order the focuses were observed.

- Core drops a focus on the node of the live focus it holds after the
  stale and foreground checks, which record its observation, so a focus
  from another application observed before the repeat and arriving after
  it is stale, as NVDA's in-order queue would have handled it first; its
  newer reading is kept, and what Core last spoke of the focus is not
  moved, so a change the repeat read before its own event came is spoken
  when the event comes, as NVDA compares a change with what it last spoke
  (Core's `a_name_change_after_a_repeat_carried_it_is_spoken`). Core
  compares the node id only, not the states, name, or ancestors, which a
  second report can read mid-change (Notepad's edit control settling,
  File Explorer's title being filled in), and not the selected item inside
  a list; a new selected item is announced by its selection event. This
  rule across backends is Verbatim's own, kept in the reducer because only
  Core sees focus across applications
  ([verbatim-core](../../books/dev/src/reference/crates/verbatim-core.md), M3 noise suppression); it
  covers MSAA's focuses, UIA's, and an MSAA answer to a fake focus's
  query, a UIA answer being spoken again.
  Core's `a_duplicate_focus_makes_a_focus_observed_before_it_stale`,
  `the_repeat_hole_in_the_order_observed_speaks_the_return` and
  `the_same_focus_after_another_applications_focus_is_announced`.
- The focus listener makes no repeat test: a UIA focus that passes NVDA's
  other checks is read in full (mockapp's
  `a_repeated_focus_on_the_still_focused_element_is_read_in_full`). An
  outpost makes none either: an MSAA focus at the address of its last
  report is read in full and reported, under the same node for a whole
  object (mockapp's `a_repeated_msaa_focus_is_read_in_full`). The read is
  the cost (`books/dev/src/reference/performance.md`, "One judge").
- A runtime id is unique only among live elements, and File Explorer
  reuses them: going back from a subfolder, an item of the parent folder
  can take the runtime id of the subfolder's item, destroyed with its
  list. Before a UIA focus whose runtime id already names a node is
  reported, the listener reads live whether the element the id stood for
  still has the keyboard focus, as NVDA checks it: the node keeps its id,
  and the report carries what was read, so Core takes it as a duplicate
  only when that element still has the focus, and announces it when it
  lost the focus or is gone. Either way the focus's text is taken afresh,
  its caret and terminal state forgotten, as for a new object. An element
  that is gone is not the focus before it, so the focus line meets above
  it and its ancestors are entered as for any new focus; one that only
  lost the focus is still the same object, as UI Automation compares two
  elements by runtime id, so the line meets at the focus and no ancestor
  is spoken again. Mockapp's `focus_reports` tests and Core's
  `a_reused_runtime_id_whose_element_is_gone_is_announced_and_its_selection_follows`
  and `a_reused_runtime_id_whose_element_lost_the_focus_speaks_no_ancestors`.
  The element's keyboard focus is read before the sender's, in NVDA's
  order (`UIAHandler/__init__.py` 902 to 917 and 948), so a focus only
  being lost as it is read, as Windows Terminal's terminal is when its
  window minimizes itself at launch, is dropped by the sender's read
  rather than taken as a new element (mockapp's
  `a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once`).
- Different, deliberately (Dickson, 2026-11-02): the return to a focus
  after the user went to another top-level window and back is always
  spoken, as the same switch is when the other application's focus was
  taken, though that application never answered, so its focus never
  came. NVDA is silent there: its last queued focus is still the one
  returned to, so the foreground event is a descendant of it and the UIA
  focus a duplicate. Core's left mark lives on its last queued focus
  (ADR 0001, "The foreground and the window title"): a foreground change
  into a window unrelated to that focus, observed after it or reaching
  Core after it, leaves it for the reports Core judges after the change
  that were observed after it, and such a report is taken as a focus from
  another window is (`focus_queue::Change`): Core's
  `a_return_after_a_foreground_change_says_what_the_switch_says_without_a_stall`
  and `the_state_after_a_return_is_the_state_after_the_switch_without_a_stall`.
  The same holds after a splash or tray window that takes the foreground
  and reports no focus, where NVDA, dropping a native UIA window's
  `WinEvent`s, is silent too; the two cannot be told apart from outside the
  application before `IsHungAppWindow` says hung (Core's
  `the_return_after_a_uia_splash_window_is_spoken`). Silence while in the
  application that does not answer matches NVDA's ("A focus UIA does not
  deliver").

### A focus whose keyboard focus read fails

NVDA reads the sender's keyboard focus live, and a live read that fails
lets the focus through (`except COMError: return True`,
`nvda/source/NVDAObjects/UIA/__init__.py`, lines 1632 to 1637).

Verbatim: **matched** (Dickson, 2026-11-07). The focus listener's read
has no time budget: every check before the read is made live, each call
waiting for the application up to UIA's transaction timeout, and a check
that fails or times out lets the focus through, as NVDA's COM error rule
does ([How focus works](../../books/dev/src/explanation/design/focus.md),
"One UI Automation focus, end to end"). A failed read of the sender's
keyboard focus lets it through to its walk; a failed read of the earlier
element's keyboard focus reads that element gone, so the focus is no
repeat. The nearest window's lookup is the one check whose failure drops
the focus, as NVDA's lookup answering none makes the element not native
("A focus whose element is gone before its read"). Mockapp's
`a_focus_whose_keyboard_focus_read_times_out_is_reported`,
`a_focus_whose_keyboard_focus_read_fails_is_walked` and
`a_focus_whose_earlier_elements_read_times_out_is_read_as_a_new_element`.

### Stale focus events

NVDA has no timestamp arbitration; it relies on queue-time freshness
plus cancellable speech, its one queue handles every application's
events, and its keys, in the order observed, and it judges a foreground
event against the foreground window when it processes it
([Event handling](../nvda/events.md), [Speech](../nvda/speech.md)).

Verbatim: **matched (unverified)**; the `rapid_tabbing_in_settings` and
`second_application_and_verbatim_menu` scenarios cover it.

- Order within an application comes from its outpost's single queue and
  worker. Core takes the outposts' messages and its keys and commands
  from one channel, in the order they reached it
  ([ADR 0001: Focus](../../books/dev/src/reference/adr/0001-focus.md),
  "One input to Core, in arrival order"), and holds none back.
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
  events as NVDA's queue orders the focus it reads; and the end of a menu or of the switcher, when the
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
once (`verbatim_reader_rules::acceptance`; decision D14, "Where the rules run"), for
the outposts and the focus listener, with the window's relation to the
foreground window (`foreground_relation`) reported with an event's
window facts. Each reader makes the test once, as NVDA makes it once,
before it queues a focus: the focus listener for a UIA focus, by its
nearest window, as the read starts; an outpost's reducer thread for an
MSAA focus and a foreground change, from Windows' state as it takes the
entry in, never its worker, which can wait behind a call into a held
application. Core does not test a focus again on facts read later: it
judges every focus with window facts against its record's front, the
window the focus listener last reported in front ("Attention"). NVDA's hold
for a foreground event's window is not built, a deliberate difference
(Fable, 2026-10-30; accepted by Dickson, 2026-11-01): the outpost judges a
foreground change when it comes ("Attention" below). The hold changes
no word on entering the Settings app, the one window measured to lag:
with the frame's own focus dropped, the content's chain says "Settings"
from the frame's MSAA client pane, as NVDA's run whose hold ran out did.
It would move that "Settings" earlier in only some launches, since the
frame came in front 24 to 78 ms after its event, and a hold of NVDA's 27
to 38 ms would put up to 38 ms of held events behind every lagging
foreground change in every outpost.

- Each outpost makes NVDA's test, rule by rule in NVDA's order, with
  local calls only, before any call into the application: on its
  reducer's thread as the entry is taken in for an MSAA focus and a
  foreground change, the result carried to the worker, and on the worker
  as it handles the event for MSAA name, description, state, value and
  selection changes, the system
  caret's location changes and shows (both tested as a location change;
  NVDA tests a caret show by its focus's own rule instead,
  `docs/parity/text.md`, "The system caret beside other caret events"),
  popup menu facts, and alerts. Show facts pass by
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
  nearest window; a windowless element whose nearest window is not found
  is dropped, as NVDA cannot make its object. A windowless UIA focus is
  not tested as hung as it arrives, as NVDA's hung-window test passes a
  UIA event whose sender's cached window is 0; a focus with a window of
  its own is. Property changes,
  selections, and a text control's caret and text changes are tested, as
  NVDA's automation event handler tests them; a windowless selection is
  tested with its nearest window (`FocusReader::check_fact`).
  Notifications and the active text position are not tested, as in NVDA,
  but a notification whose sender has no window above it is dropped, as
  NVDA's app module default drops it, the shell's window-snap results
  excepted, as File Explorer's app module lets them through.
- The UIA focus is tested by the focus listener, in NVDA's order
  ([ADR 0001: Focus](../../books/dev/src/reference/adr/0001-focus.md),
  "The listener's checks, ported from NVDA's focus handler"): after the
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
- Different, deliberately (Dickson, 2026-11-02): a focus whose read is
  slow, reported after Alt+Tab into an unrelated window, is dropped by
  Core's front, where NVDA speaks it and then cuts it when its slowness
  came after its own test. A focus of form M observed before its owned
  dialog D came in front is related through D's root owner and taken, as
  NVDA takes it (Core's
  `a_focus_observed_before_a_switch_to_an_unrelated_window_is_dropped` and
  `a_focus_of_a_form_observed_before_its_owned_dialog_came_in_front_is_taken`).
- The foreground event Windows raises for a switch reaches an
  out-of-context hook only once `GetForegroundWindow` names its window;
  an event raised earlier, by the application itself (File Explorer's
  folder window, 140 to 400 ms early, with no later event when it opens
  in front) or refused by Windows' foreground lock, comes before its
  window is in front. The focus listener sends Core only a foreground
  event whose window is in front as it handles it, and keeps an earlier
  one as its pending window, sent with the observation of a later focus
  or foreground event that finds it in front, unless a newer foreground
  event replaces it (the listener logic's
  `a_foreground_event_for_a_window_not_in_front_is_sent_when_a_later_event_finds_it_in_front`).
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
foreground ("Event acceptance", above), moving its foreground object
with the next focus (`doPreGainFocus`). Verbatim: **different (D14)**:
the reducer keeps a foreground record, fed only by the focus listener's
foreground change (`Input::Foreground`), the one report that a window
came to the foreground, sent for every foreground event whose window is
in front, Program Manager's and the taskbar's included: attention, the
application and window of the newest change, standing in for the
system's foreground window, and the front, that window with when the
listener observed it (Dickson, 2026-11-02).

- Attention moves only on the listener's foreground change, never on a
  focus or an outpost's report, so a topmost popup menu that takes focus
  without becoming the foreground leaves attention where it was, and a
  focus returning from it is still attended. A change to one of
  Verbatim's own windows is the keyboard hook's alone.
- An outpost's report of a foreground change into a window it reads
  through MSAA is a whole-window focus, judged by the front as every
  focus is: taken when the front is its window or was observed no later
  than it, dropped when the front is another, unrelated window observed
  after it; it never moves attention. The outpost reports a change only
  when its one test found the window in front, stamped with the test's
  time, whether or not the window has been shown, as NVDA tests only
  that the window is the foreground window, and reports it though the
  window leaves the front during its read, as NVDA makes no test after
  its object is made (mockapp's
  `a_foreground_change_whose_window_leaves_the_front_during_its_read_is_reported`).
- A focus with window facts is attended unless the front was observed
  after it and is a window it is unrelated to ("Event acceptance"); every
  other event is classified against the attention record from the
  window facts its outpost attached, or, with none, by its application:
  attention's, or that of Core's focus while that focus is in the
  attention window, as a Settings page's content is in
  `ApplicationFrameHost`'s frame.
- Different: Windows can raise a window's foreground event while its
  foreground lock keeps another window in front, and raise none when the
  window comes in front later; attention then stays on a window that is
  not in front until the next foreground change, where NVDA's next focus
  moves its foreground object. Events with window facts from the window
  really in front are still attended by their own `in_foreground`; only
  events with no window facts, judged by application, are dropped
  meanwhile. Program Manager's and the taskbar's changes move attention,
  where NVDA takes no foreground event from them but moves its foreground
  with the next focus, so attention matches NVDA's once that focus comes.

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
  its speech cancelled by that change ([ADR 0001:
  Focus](../../books/dev/src/reference/adr/0001-focus.md), "One input to
  Core, in arrival order"; Core's
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
deadlines (11 seconds for the worker, `HANDLING_DEADLINE`, and 13
seconds, `FOCUS_READ_DEADLINE`, for a reading thread); the event is dropped and a new worker or reading thread
takes the application's later events
(`books/dev/src/reference/crates/verbatim-listener.md`, "How a UIA focus is judged and
read", owns when each judges). Console host's own Find, Properties
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
- Not judged, deliberately (Claude, 2026-11-08, pending Dickson's
  acceptance): a tree dump and a shell items query, which are not about
  the focus, end only at their deadline, their window still checked for a
  hang; NVDA's watchdog judges its recovery against its focus object.
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
- Matched: NVDA judges again every half second while it waits
  (`_waitUntilNormalCoreAliveTimeout`, lines 133 to 148), and so do the
  outposts and the listener, from the half second's end until their
  deadlines, with no event needed as evidence (Dickson, 2026-11-01).
- NVDA compares only threads, by `GetWindowThreadProcessId`, which names
  a console window's client and one of the client's threads
  (`nvda/source/watchdog.py`, lines 230 to 232). Verbatim also requires
  both windows to be served by one process
  (`verbatim_win32::provider::provider_process`): a move between a console window and
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
one returns: its events wait, until `FOCUS_READ_DEADLINE` after its second
thread was abandoned. Then every event waiting is dropped, and each later
one as it arrives, until an abandoned thread's call returns and takes
the queue. The bound is one per application, not one per event, so the
application's outpost runs at most `FOCUS_READ_DEADLINE` behind the hung reads,
once, and a stream of events from the application cannot keep it
behind. A focus waiting so is a pending focus only for its own
application's menu openings, so that a hung window cannot silence every
other application's menus while its reads stay stuck; a focus being
read, or waiting for a live thread, still counts for every application,
as in NVDA.

### An application slow to answer

NVDA waits for a frozen core up to its watchdog's
`NORMAL_CORE_ALIVE_TIMEOUT` of 10 seconds (`nvda/source/watchdog.py`,
line 50), unless the user has moved on (above); it sends a window message
in 400 ms slices, up to 10 seconds, with `SMTO_ABORTIFHUNG` on each slice
(`cancellableSendMessageTimeoutTemplate`,
`nvda/nvdaHelper/local/nvdaHelperLocal.cpp`, lines 61 to 108), sets no
UIA timeout, so UIA's transaction timeout is its default of 20 seconds,
and gathers a dialog's text inside the focus event, under its watchdog
(`getDialogText`). Verbatim: **matched (unverified)** (Dickson,
2026-11-06): a slow application is waited for rather than cut off by a
short limit of Verbatim's own. One UIA request may take 10 seconds
(`UIA_TRANSACTION_TIMEOUT`), and an outpost's program 11
(`HANDLING_DEADLINE`), or 13 for a listener read, a focus query
(`Query::ReadFocus`) and the outpost's completion of a listener read
(`FOCUS_READ_DEADLINE`); a focus's walk, a navigation step, an
activation, a text request, a walk down a focus's path, a dialog's text
and the shell's items have no shorter limit of their own, and a focus is
reported once, in full ([How focus
works](../../books/dev/src/explanation/design/focus.md), "Slow and hung
applications"). Deliberately different:

- A call bounded by UIA's connection timeout, 2 seconds as in NVDA,
  still fails against an application busy for longer.
- A query's path walk or the shell items' walk that does not finish by
  its program's deadline answers with nothing, as NVDA's cancelled call
  does, and a shell items walk that could not read every button fails
  whole (Claude, 2026-11-08, pending Dickson's acceptance); a text focus whose element only
  a slow classic walk finds waits for it, and is lost if its program
  passes 13 seconds, as NVDA's caret read is cut only by its watchdog
  (Dickson, 2026-11-06).
- An activation whose provider runs its action synchronously holds that
  outpost's later entries until it returns, up to 11 seconds, as NVDA's
  `doAction` blocks its core.
- Matched in outcome (Claude, 2026-11-07, pending Dickson's acceptance):
  a window message is one send bounded by its program's deadline, where
  NVDA sends it in 400 ms slices up to 10 seconds. An experiment found
  that a timed-out `SendMessageTimeoutW` is never delivered later, and
  that `SMTO_ABORTIFHUNG` did not refuse the 400 ms slices after the
  window was judged hung, so NVDA's sliced send also waits to its limit
  or its watchdog's cancel. The window's readability is tested before the
  send, so a window already hung or ghosted fails at once, as NVDA's
  `SMTO_ABORTIFHUNG` aborts for a window already hung
  (`nvdaHelperLocal.cpp`, line 89), but for a message from `WM_APP` to
  `0xBFFF`, which NVDA exempts and none of Verbatim's is. A buffer it carries in the application's memory is freed only once the
  message is answered, where NVDA frees it after a cancelled message
  (`nvda/source/NVDAObjects/IAccessible/sysListView32.py`, lines 600 to
  655; `nvda/source/NVDAObjects/window/edit.py`, lines 400 to 420), which
  the application can then write into (Dickson, 2026-11-06).

### A window that stops responding

NVDA's watchdog makes no `IsHungAppWindow` test; its UIA handlers drop an
event whose sender's cached window is hung as it arrives
(`_shouldSkipEventForHungWindow`, `nvda/source/UIAHandler/utils.py`,
lines 438 to 455), its MSAA events are tested as each is turned into an
NVDA event (`winEventToNVDAEvent`,
`nvda/source/IAccessibleHandler/__init__.py`, line 570), and it plays no
sound for a hung application. Verbatim: **different**, by the sound and
the deadline (Dickson, 2026-11-04, 2026-11-06 and 2026-11-07; [How focus
works](../../books/dev/src/explanation/design/focus.md), "Slow and hung
applications"). The outpost abandons nothing for a hang: work in flight
waits for the application, ended only by its deadline or the moved-on
rule. By Windows' own state and local calls, two checks on one timer
find a hang: every half second from the grace's end, the window of its
program in flight, and, while a listener read's marker has waited half a
second, every half second, the window in front, a ghost mapped to its
hung window. Each hang of the application is told to Core once, however
many of its windows the hang is then met in. The told window is judged
on each input that takes an entry, applies a commit or starts a program,
and its hang ends when it is judged not hung, is readable in front, is
destroyed, or the outpost ends, so a window still hung then, or the told
window's own later freeze, is a new hang, told again. Core plays
the "not responding" sound for it only when its application holds
attention, so a hang first met in the background is never sounded. A
query whose window is already hung is answered at its check before its
first call, an activation silently. The focus listener's only hung test
is NVDA's, at arrival: it drops a UIA event whose sender's cached window
is hung as it arrives; a step of its walk that reaches a ghost's or a
hung application's window ends the walk there ("A focus whose element is
gone before its read"). It reports no hang, which only the outpost tells
Core. Mockapp's
`a_hang_while_the_listeners_read_waits_is_told_once_and_the_focus_spoken_when_it_answers`,
`a_query_whose_window_hangs_during_its_call_is_answered_when_it_answers`
and `a_report_focus_query_in_a_hung_window_waits_for_the_answer`.

- Different: a key whose answer needs a call, in a window that hangs
  while it is read, plays the sound once Windows marks the window hung,
  where NVDA plays nothing (`docs/nvda/main-loop-and-watchdog.md`).
- Different, deliberately (Dickson, 2026-11-07): a hang longer than about
  13 seconds, `FOCUS_READ_DEADLINE`, right after a focus loses that focus,
  where NVDA speaks it when the hang ends.
- Matched: a control in a hanging application is spoken with its line
  when the application answers, as NVDA speaks it (capture of
  2026-11-07).
- Verbatim+Tab in a hung application waits for the live answer, saying
  nothing while the application does not answer, as NVDA says nothing,
  and nothing if its query is abandoned at its deadline (Dickson,
  2026-11-07; [Review](review.md), "Report focus").
- An application slow past the deadline but never judged hung is silent,
  with no sound, as NVDA cancels at its watchdog's limit.

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
and roles), and if all match, the new node ids are taken silently,
otherwise the focus is announced as usual.
Tests: `crates/verbatim-core/tests/reduce.rs`,
`a_replacement_outpost_reporting_the_same_focus_is_taken_silently`
and `a_replacement_outpost_reporting_a_different_focus_announces_it`. The same question is asked
after the focus listener is replaced.

### A focus UIA does not deliver

UI Automation sometimes delivers a newly started process's focus events
late or not at all, while the system's `WinEvent` focus for the same
moves still comes, and sometimes delivers them after their elements are
gone (`books/dev/src/explanation/design/listener-without-connections.md`, risks 2 and 13).

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

Verbatim: **matched** (Dickson, 2026-11-07; [ADR 0001:
Focus](../../books/dev/src/reference/adr/0001-focus.md), "A focus UI
Automation does not deliver"). An outpost drops a `WinEvent` focus in a
window it judges UIA, as NVDA's `winEventToNVDAEvent` drops it, and
reads nothing in its place. The focus listener drops a UIA focus whose
sender's nearest window's lookup fails or finds none, as NVDA's handler
drops it (`verbatim_listener_logic::checks`, its
`a_sender_with_no_window_is_dropped` and
`a_timed_out_nearest_window_drops_the_focus`), and one whose element no
longer has the keyboard focus, read live. So a focus UIA never delivers
is lost, as in NVDA, and one delivered late is spoken when it comes. The
scenario `return_from_a_held_application` expects nothing on entering
the held window and the text box's announcement on the return.

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

Verbatim: **matched (unverified)**, but for the deadline below; the
listener's own read decides both cases
(`crates/verbatim-listener/src/focus_read.rs`). NVDA's first case is the
listener's nearest-window lookup failing or finding no window, which
drops the event (`NO_WINDOW`), as NVDA drops it ("A focus UIA does not
deliver"). In NVDA's second case the listener's live read of the sender's
keyboard focus fails and lets the focus through, as NVDA's COM error
rule does, its classic read of the sender fails, and the listener reports
the focus as the event's cache gives it, with the ancestors read before
the failure. A walk step, the listener's or the outpost's, that fails by
UIA's transaction timeout or any other error, or finds no parent below
the desktop's root, goes on, as NVDA's failed parent read falls back to
the window hierarchy (`Window._get_parent`), from the parent window of
the outermost entry's nearest window, never that window itself, by that
window's own object read live: its UIA element when the outpost judges
it UIA, its MSAA client otherwise. The listener, which connects to no
window, names the window, and the outpost goes on from it. A window read
through UIA whose element cannot be read on the chain's way loses the
focus, as NVDA's unguarded `ElementFromHandleBuildCache` ends its event
(`NVDAObjects/UIA/__init__.py` 1554 to 1557); an MSAA client that
cannot be acquired is described from Windows' state. A step that reaches
a ghost's or a hung application's window other than the focus's nearest
one ends every UIA walk there, as NVDA makes that one step and then
reads the window as a plain window, going on by the window hierarchy with
no further call (`correctAPIForRelation`, whose `getPossibleAPIClasses`
keeps such a window plain, `NVDAObjects/window/__init__.py` 85 to 97);
the listener names the window and the outpost goes on from it, and a
focus query with such a window above its focus reads the ancestors by the
classic walk rather than a remote program (Fable, 2026-10-30). Windows
described from Windows' state with no call, plain windows, stand only for
the desktop, a ghost's or a hung application's window, a window whose
MSAA client cannot be acquired, the windows above the process boundary,
and a chain cut at the depth limit, which is
completed by them from its outermost element's nearest window. A walk
has no bound but its read's deadline, and its focus is reported once, in
full ([How focus works](../../books/dev/src/explanation/design/focus.md),
"Slow and hung applications"). The outpost, as NVDA's focus line, meets
the previous chain where the new one meets it, so the next focus inside a
focus whose step failed keeps its chain
(`books/dev/src/reference/crates/verbatim-uia-rops.md`, `failed_hop`;
`books/dev/src/reference/crates/verbatim-outpost.md`, "Reading the
focus").

Different, deliberately (Dickson, 2026-11-07; [ADR 0001:
Focus](../../books/dev/src/reference/adr/0001-focus.md), "When an
application's call fails"): a walk step that times out, in an application
Windows does not report hung, and whose continuation from the parent
window times out again, passes the 13-second `FOCUS_READ_DEADLINE`, and
the focus is lost, where NVDA waits for every call.

File Explorer's placeholder element can answer its properties after it
has left the tree, so only the parent step fails. It is spoken without
its list, and always with its name, which NVDA loses when a main-loop
pass came between. The time in which the element must go differs: in
NVDA from the callback to the main thread's handling; in Verbatim the
consecutive calls on one listener thread between the walk and the
classic read. `explorer_folder_window` expects its "Working on it..."
placeholder not to be said, as its keyboard focus is read live. No
capture of NVDA exists for this case.

Tests: `remote_ops.rs` in `mockapp`,
`a_focus_whose_read_fails_is_reported_whatever_its_event_says_of_its_focus`,
`a_focus_whose_walk_times_out_is_reported_below_its_window`,
and `a_focus_whose_parent_step_fails_goes_on_from_its_windows_parent`
(the failed step and the missing parent) for the listener's report;
`a_focus_now_whose_program_finds_no_parent_below_the_window_goes_on_from_its_windows_parent`,
`a_focus_now_whose_program_ends_below_the_window_at_an_element_reporting_a_handle_goes_on_from_that_window`,
`a_focus_now_whose_classic_walk_finds_no_parent_below_the_root_goes_on_from_its_windows_parent`,
`a_failed_uia_ancestry_continues_from_the_parent_window_live` and
`a_failed_remote_program_continues_from_the_parent_window_live` for the
outpost's own walks; `focus_reports.rs`'s
`a_failed_uia_read_continues_from_its_nearest_windows_parent`,
`a_failed_read_s_chain_carries_one_entry_per_window`,
`a_uia_window_whose_element_cannot_be_read_loses_the_focus`,
`a_windowless_focus_with_a_failed_walk_is_found_from_its_nearest_window`,
`a_focus_after_a_failed_acc_parent_reaches_its_dialog`,
`a_depth_limited_chain_starts_at_its_unread_nearest_window`,
`a_hung_window_above_a_uia_focus_ends_the_listeners_walk`,
`a_hung_window_above_a_focus_now_is_a_plain_window_remote` and
`a_hung_window_above_a_focus_now_is_a_plain_window_classic`;
`call_counts.rs`'s `uia_focus_chains_stop_below_the_process_boundary`,
whose "child, remote" case is a remote walk complete at a window below
the process boundary;
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
([Architecture](../../books/dev/src/explanation/architecture.md), D9, and
[verbatim-outpost](../../books/dev/src/reference/crates/verbatim-outpost.md), "Window destruction
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
sent, with NVDA's exceptions, while the keys Verbatim injects for itself (the
Control tap that unlocks the foreground) leave speech alone, as NVDA
ignores its own; a foreground report or a focus in another top-level
window cancels, as does entering a menu; each focus announcement carries
what it is about, entered containers are their own utterances, and on
every focus change the speech manager stops what it has handed on when
any of that has expired, and judges waiting focus speech when its turn
comes, as NVDA does. NVDA's tests compare objects with `==`; Verbatim
compares them by its one identity rule ("Focus-ancestry context" in
[Focus context](focus-context.md)): the same node, or two windows of one
handle, so speech for a window one outpost reported holds while another
outpost's node for that window is the focus, an ancestor or the
foreground. A selection in a list the focus controls still
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
