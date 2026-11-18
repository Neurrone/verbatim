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

Verbatim: **matched**, with two differences, the last two entries
below. Both tests are Core's alone, rows of the one definition of a
candidate's validity (`crates/verbatim-core/src/focus/validity.rs`, its
primitives in `duplicate.rs`), judged at the focus's turn in the event
queue and for whether it is pending alike, against the readers' last
focus, the newest focus a reader read that Core executed, which stands
for NVDA's last queued focus. A duplicate ends with no read, and nothing
is kept or said. With the readers' last focus empty nothing is a
duplicate.

- MSAA: both focuses MSAA, with equal window, object and child ids and
  the child id the earlier object is held by not above 0, as NVDA tests
  its object's own `IAccessibleChildID`, not its event's
  (`NVDAObjects/IAccessible/__init__.py` 863 to 872); compared by ids
  with no read. A list's focus on its own object that the outpost
  redirected to its focused item carries the item's ids as its event's,
  as NVDA builds the redirected object (`IAccessibleHandler/__init__.py`
  746 to 757), so its duplicate test, the limiter's exemption of the last
  focus and the foreground's same-ids test compare the item's ids, and by
  NVDA's test it is never a duplicate, its child id being above 0 (the
  identity check, the last entry, is Verbatim's own). A focus read
  through `accFocus` carries the ids it is held by, as NVDA's object gets
  its ids from `IAccIdentity` (`NVDAObjects/IAccessible/__init__.py` 803
  to 808). Core's
  `an_msaa_focus_with_the_last_focuss_ids_and_no_child_is_a_duplicate`,
  `a_repeat_of_a_focus_held_by_a_child_above_zero_is_not_a_duplicate`
  and `nothing_is_a_duplicate_of_a_focus_read_through_acc_focus`.
- Different, deliberately (Dickson, 2026-11-18: "I do want it to
  read"): File Explorer raises a second MSAA focus on some items, a menu
  item of a classic list view (`SysListView32`, the start menu's Places
  and frequently used programs) and an item of the desktop's list, when
  the focus moves to it (NVDA's #474) and when the desktop comes back
  from another application (#2988). NVDA's Explorer app module classes
  these items (`SysListView32EmittingDuplicateFocusEvents`,
  `appModules/explorer.py` 56 to 72, chosen at 265 to 272) and refuses a
  focus whose window, object and child ids equal the last queued focus's
  of the same class, whatever came between. Verbatim has no such rule:
  the identity check (the last entry) says the second focus on the same
  item once, with no key between, and reads a focus at the same ids
  after a key or with another reading. After Delete on the desktop the
  list raises destroy and create on its client, then a focus at the
  deleted item's child id, where the next item now is (recording of
  2026-11-18, 5 of 5 runs): NVDA says nothing, and Verbatim, the Delete
  key between, says the next item ("zz-verbatim-rec-c.txt 15 of 15").
  Core's `a_second_focus_on_the_desktops_item_with_no_key_between_is_said_once`
  and the replay `the_desktops_next_item_is_spoken_after_delete`.
- UI Automation: the readers' last focus is a UI Automation object, the
  runtime ids are equal, and the earlier element still has the keyboard
  focus. The listener keeps the element it read for each focus Core
  executes, and the check thread, at the new focus's delivery, reads the
  keyboard focus of each element it keeps with the event's runtime id,
  in NVDA's order after the walk and the probe
  (`UIAHandler/__init__.py` 902 to 923); Core uses the answer carried for
  the element that is the readers' last focus, with no read of its own. A
  failed read, or a last focus the listener no longer holds (after a
  re-take, whose focus an outpost read), means not a duplicate. Core's
  `a_uia_focus_on_the_last_focuss_element_still_focused_is_a_duplicate`,
  `a_uia_focus_whose_earlier_element_lost_the_focus_is_not_a_duplicate`
  and `a_failed_read_of_the_earlier_element_is_not_a_duplicate`.
- A runtime id is unique only among live elements, and File Explorer
  reuses them: going back from a subfolder, an item of the parent folder
  can take the runtime id of the subfolder's item, destroyed with its
  list. The test reads the keyboard focus of the element the listener
  kept, not of whatever now has that runtime id, so a reused id whose
  earlier element is gone or lost the focus is read as a new focus, as
  NVDA's live read of its earlier object finds it.
- The element's own keyboard focus is read after the kept elements', in
  NVDA's order (`UIAHandler/__init__.py` 902 to 917 and 948), so a focus
  only being lost as it is read, as Windows Terminal's terminal is when
  its window minimizes itself at launch, is dropped by its own answer
  rather than taken as a new element.
- Different, deliberately (Dickson, 2026-11-02): the return to a focus
  after the user went to another top-level window and back is always
  spoken, as the same switch is when the other application's focus was
  taken, though that application never answered, so its focus never
  came. NVDA is silent there: its last queued focus is still the one
  returned to, so the foreground event is a descendant of it and the UIA
  focus a duplicate. Core empties the readers' last focus when the
  listener's foreground report names a top-level window unrelated to the
  committed focus, at the report's marker's turn in the event queue, so a
  duplicate that arrived before the report is still judged against it
  ([ADR 0003](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
  "The ancestors Core already knows and leaving a window"); the return
  then says the window's title and the focus. Core's
  `a_return_after_a_foreground_change_says_what_the_switch_says_without_a_stall`
  and `a_marker_for_an_unrelated_window_empties_the_last_and_keeps_the_committed`.
  The same holds after a splash or tray window that takes the foreground
  and reports no focus, where NVDA, dropping a native UIA window's
  `WinEvent`s, is silent too (Core's
  `the_return_after_a_uia_splash_window_is_spoken`). Silence while in the
  application that does not answer matches NVDA's ("A focus UIA does not
  deliver").
- Different, deliberately (Dickson, 2026-11-18): the identity check. A
  later MSAA focus is not spoken when, with no key between, the object it
  reaches, after the outpost's redirect, has the ids of the readers' last
  focus and reads as it does. NVDA's duplicate test never holds for a
  child id above 0, as child ids are positions
  (`NVDAObjects/IAccessible/__init__.py` 863 to 872, the exit at 866 to
  867); the identity check extends only that branch. The readers' last
  focus keeps the speech fence of the entry that executed it (`since`):
  a run's or a menu start's executed candidate's own, any other entry's
  own. A key moves later candidates' fence; a candidate tried at its turn
  and refused, not there or whose reader ended clears it, so the mouse
  return to a list after a click whose read failed is spoken ("Alpha 1
  of 3"). An MSAA focus of an equal fence, whose last focus is held by a
  child id above 0 and is the committed focus, is asked unless it is that
  focus (`FullUnlessSame`): the outpost compares the reached ids and, if
  equal, the object's states, role, position in its set and the set's
  size where the control gives them with no walk (a `SysListView32`
  item's), name and value, in that order, stopping at the first that
  differs and then reading in full; an object that reads as named is
  answered `CandidateSame` and Core makes it a duplicate. The position
  and size are read whatever the position-reporting setting, which
  decides only whether they are spoken (Dickson, 2026-11-18). A pending
  judgement makes no identity read, so a child id above 0 is never a
  duplicate there, as in NVDA. Startup and the return after a desktop
  switch execute with no duplicate test, as NVDA queues those focuses.
  NVDA once compared objects this way and removed it (c47d518ee, the
  foobar2000 playlist) because it read the old object again, and a
  position-identified child then answered as its new occupant; Verbatim
  compares against Core's reading of the focus spoken, which no later
  read changes. Verbatim's settings dialog raises its list's own focus
  twice after the menu check read its item, and mockapp's list raises
  its own focus then its item's on a mouse return; NVDA says the item two
  or three times there, Verbatim once. Limits: a different object at
  reused ids that reads the same with the same position and set size,
  only an identical item deleted and another added at once, is not
  spoken where NVDA speaks it, and a tree view item's position is not
  compared, so its twin is not spoken either; a stale candidate refused
  between a pair clears the fence, so the item is said twice; a click
  whose candidate is refused with no read keeps the fence, so the return
  to the list is not spoken; startup or the return after a run says the
  item twice, as NVDA does. Core's identity tests
  (`the_items_own_focus_again_is_read_unless_it_is_the_same`,
  `the_lists_own_focus_reaching_the_same_item_says_nothing`,
  `a_try_refused_or_gone_clears_the_fence`,
  `every_order_of_the_check_and_the_pair_says_the_item_once`) and the
  replay `the_recorded_settings_dialog_says_its_item_once`.

### A focus whose keyboard focus read fails

NVDA reads the sender's keyboard focus live, and a live read that fails
lets the focus through (`except COMError: return True`,
`nvda/source/NVDAObjects/UIA/__init__.py`, lines 1632 to 1637).

Verbatim: **matched** (Dickson, 2026-11-07). The listener's check thread
makes these reads at the focus's delivery, each call waiting for the
application up to UI Automation's transaction timeout, and Core judges
their answers ([How focus
works](../../books/dev/src/explanation/design/focus.md), "One UI
Automation focus, end to end"). A read of the element's keyboard focus
that fails or times out lets the focus through, as NVDA's COM error rule
does; a failed read of a kept earlier element's keyboard focus makes the
focus no duplicate. The walk to the nearest window is the one check
whose failure drops the focus, as NVDA's lookup answering none makes the
element not native ("A focus whose element is gone before its read").
Core's `a_uia_focus_is_valid_with_no_read_unless_a_row_refuses_it` and
`a_failed_read_of_the_earlier_element_is_not_a_duplicate`.

### Stale focus events

NVDA has no timestamp arbitration; it relies on queue-time freshness
plus cancellable speech, its one queue handles every application's
events, and its keys, in the order observed, and it judges a foreground
event against the foreground window when it processes it
([Event handling](../nvda/events.md), [Speech](../nvda/speech.md)).

Verbatim: **matched (unverified)**, but for the limit below; the
`rapid_tabbing_in_settings` and `second_application_and_verbatim_menu`
scenarios cover it.

- Every focus and foreground event of every application reaches Core
  from one teller, the focus listener, in the order the listener placed
  its tells. Core takes the tells, keys and commands into its inbox in
  arrival order, and every tell but what NVDA handles outside its queue
  joins Core's one event queue, judged at its turn against the focus
  Core holds then
  ([ADR 0003](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
  "Core's inbox and the event queue"). No ordering stamp decides
  whether a focus is stale; the one comparison by time is a UI
  Automation menu opening's with a pending focus ("While a focus is
  pending").
- A UI Automation event's tell is placed in the listener's order once
  its application's check thread has made its delivery-time calls, so a
  hung application delays only its own events.
- Acceptance is judged on the foreground facts the listener read as it
  placed the tell, as NVDA judges an event against the foreground window
  as it processes it ("Event acceptance").
- Limit: an outpost's UI Automation property changes of the objects Core
  names come from another process, with no order between them and the
  listener's tells. When a focus answer's ancestors include the committed
  focus, Core first judges the committed focus's waiting state changes,
  the narrow rule, so a combo box's "expanded" is heard before the item
  focused inside it; a state change of the old focus arriving after a
  focus elsewhere is not said (ADR 0003, "The narrow ordering rule" and
  "Limits accepted").

### Event acceptance

NVDA: every event except UIA notifications must come from a window
related to the system's foreground window at the moment NVDA processes
it, or pass one of the exceptions before that relation is tested. The
test (`shouldAcceptEvent`, `nvda/source/eventHandler.py` 481 to 599)
takes the event's name and its window, and its rules run in this order,
the first that decides ending it:

1. An event with no window (a window handle of 0) is accepted, since
   nothing can be tested (488 to 490).
2. An event named in the table application support fills
   (`requestEvents`), keyed by the event's name, the process that owns
   its window, and its window's class, is accepted (492 to 498).
3. A value change is accepted when background progress bars are
   reported, an option off by default (499 to 503).
4. A hide is refused (504 to 505).
5. A show is accepted when its window's class is exactly one of
   `Frame Notification Bar` (notification bars), `tooltips_class32`
   (tooltips), `mscandui21.candidate`, `mscandui40.candidate`,
   `MSCandUIWindow_Candidate` (input method candidate windows) and
   `TTrayAlert` (a chat application's tray alert), and refused otherwise
   (506 to 515).
6. An alert whose window's parent (`GetAncestor` with `GA_PARENT`) has
   the class `ToastChildWindowClass`, a toast, is accepted (516 to 522).
7. A menu end, a switch end and a desktop switch are accepted from any
   window (523 to 528); "menu end" here is NVDA's name for both the menu
   end and the popup menu end `WinEvent`s
   (`IAccessibleHandler/internalWinEventHandler.py` 39 to 41). A menu
   start or popup start is not in this rule and is tested like any other
   event.
8. An event of the desktop window (`GetDesktopWindow`) is accepted, as
   the cursor's events come from it (529 to 531).
9. An event whose window's class begins with `Windows.UI.Core` (a
   prefix, so `Windows.UI.Core.CoreWindow` matches) is accepted when its
   window is the active window of the foreground thread
   (`GetGUIThreadInfo(0)`'s `hwndActive`) or a descendant of it
   (535 to 538); a `Windows.UI.Core` window failing this goes on to the
   rules below.
10. An event is accepted when its window's root owner (`GetAncestor`
    with `GA_ROOTOWNER`) equals the foreground window's root owner
    (544 to 548).
11. An event is accepted when its window is the foreground window or a
    descendant of it, or its window's root owner is the foreground
    window or a descendant of it (549 to 555).
12. An event is accepted when its window, or its window's root
    (`GA_ROOT`), has the topmost extended style (`WS_EX_TOPMOST`), as
    menus, combo box drop-downs and the task switching list have
    (556 to 566).
13. An event from a windowless Chromium document
    (`Chrome_RenderWidgetHostHWND`) is accepted when the window with the
    keyboard focus (the foreground thread's `hwndFocus`) has the class
    `Chrome_WidgetWin_0` and a UIA tree walk normalizing the document's
    root window's element to an element with that window handle finds
    one, calls into the application (567 to 598).
14. Anything else is refused (599).

"Descendant" in rules 9 and 11 includes the window itself:
`isDescendantWindow` is true when the two windows are equal or
`IsChild` holds (`winUser.py` 553 to 557), and `IsChild` follows parents
only, not owners. The window the test is given is the event's own window
for MSAA events; for UIA, the sender's nearest window. The MSAA pump
skips the test for a show or hide of the system caret (`OBJID_CARET`)
while there is a last queued focus, asking that focus's own rule
instead (`IAccessibleHandler/__init__.py` 1039 to 1055;
`docs/parity/text.md`, "The system caret beside other caret events").
UIA notifications are spoken
only from the focus's application, with per-application opt-ins such as
the shell's window-snap results. The test is made
per event as NVDA processes it, before it creates the event's object,
so a background window's event costs no call into its application, and
not in the event callback, which lost the focus events of applications
starting up (issue 4001). After a foreground `WinEvent`, the whole MSAA
pump, every waiting `WinEvent` and not only the foreground one, is held
for at most two core cycles until the event's window is the foreground
window (issue 3831; `MAX_FOREGROUND_DEFERS`,
`IAccessibleHandler/internalWinEventHandler.py` 30 and 233 to 263); then
the pump runs, and the foreground event is dropped if its window is
still not the foreground window (`processForegroundWinEvent`,
`IAccessibleHandler/__init__.py` 835 to 841) ([Event handling](../nvda/events.md), "Acceptance
filtering" and "Foreground windows"; [MSAA and winevent
handling](../nvda/msaa.md), "Pump-time processing"; [The UIA
client](../nvda/uia.md)).

Verbatim: **partial**. The tests below match NVDA; the background cases
in "Background events" are partly not implemented. The test is Core's
alone, a pure rule over facts the tell carries
(`crates/verbatim-core/src/focus/acceptance.rs`; ADR 0003, "Acceptance
judged in Core"), rule by rule in NVDA's order, rules 2 and 13 not
applied: the table application support fills is empty until extensions
(M5), and the windowless Chromium rule needs a read and Verbatim has no
Chromium support. The listener gathers the facts with calls to Windows
only, none into an application (`verbatim_win32::window_facts`): for
the tell's window, its class, its parent's class, its root and root
owner, topmost for the window and its root, the live foreground window
and its root owner, the window's and its root owner's relation to the
foreground window and to the foreground thread's active window, and
whether it is the desktop window. It reads them at a `WinEvent`'s
arrival, and for a UI Automation event as the tell is placed after its
check thread's calls, so an event is judged against the foreground of
that moment, as NVDA judges it as it processes the event. Core judges
the test at the entry's turn in the event queue, before any request, so
a background window's event costs no call into its application.

NVDA's hold for a foreground event's window is matched for MSAA events,
the ones NVDA's hold defers: while a foreground event's window is not
yet the one `GetForegroundWindow` names, the listener holds every MSAA
tell behind it, in order, until it is,
or for 20 ms, two of NVDA's cycles, a newer foreground event restarting
the hold for its own window; it then reads each held tell's foreground
facts and sends them in order (`verbatim_listener_logic::tell`,
`FOREGROUND_HOLD_US`; its
`a_held_foreground_is_sent_before_the_focus_that_finds_its_window_in_front`
and `the_hold_ends_at_twenty_milliseconds_whatever_the_foreground`). So
a new window's MSAA focus raised while `GetForegroundWindow` still names
the old window is accepted against the new one (Dickson, 2026-11-15). A
UI Automation tell is never held: it is sent at once, its foreground
facts read as it is sent, as NVDA judges a UI Automation event at its
delivery (`UIAHandler/__init__.py` 924 to 930; `eventHandler.py` 536 to
552), so a UI Automation focus raised while Windows still names the old
window is refused as unrelated (Dickson, 2026-11-17;
`a_uia_tell_passes_a_held_foreground_and_is_sent_at_once`). The
foreground tell goes before the focus that releases it, and the
listener's report that its window came to the foreground goes before
both, since Core judges the tell against its foreground record: the
listener's watch of the window in front and its hold judge each event on
one read of `GetForegroundWindow`, and the watch reports the pending
window on any event that finds it in front (`verbatim-listener`'s
`a_foreground_found_in_front_by_any_event_is_reported_before_the_tells_it_releases`).
A hold that ends by its timer with the window in front sends the
foreground tell before the watch has reported it, so Core refuses it: a
limit until the hold and the watch keep the pending window in one
place.

- MSAA, in NVDA's order: the listener drops an event whose window has a
  ghost window standing in for it or belongs to an application the
  system reports hung, read live as each arrives; at the entry's turn
  Core judges acceptance, then drops an event whose window is read
  through UI Automation by the window's backend verdict, the system
  caret's events excepted; only then does the application's outpost
  acquire its object, at Core's request.
- UI Automation, in NVDA's order: the listener drops an event whose
  sender's own cached window is hung as it arrives, so a windowless
  sender is never dropped as hung there, as NVDA's hung-window test
  passes a UIA event whose sender's cached window is 0. Core then judges
  a focus on its tell's facts: NVDA's native-element test, a walk that
  found no window or failed dropping it, with the nearest window's
  backend verdict; NVDA's four focus overrides; acceptance against the
  nearest window; the keyboard-focus answer; the duplicate test. A focus
  that fails any of them is dropped and never becomes the readers' last
  focus, as NVDA's `lastQueuedFocusObject` is set only for a focus it
  queues. A UIA menu opening is judged as a focus on the menu by the
  same checks ("While a focus is pending" for its drop). Property
  changes, selections, and a text control's caret and text changes are
  tested against their nearest window, as NVDA's automation event and
  property-change handlers test them, an outpost's property changes of
  the node Core named against that node's recorded window.
  Notifications and the active text position are not tested, as in
  NVDA, but a notification whose sender has no window above it is
  dropped, as NVDA's app module default drops it, the shell's
  window-snap results excepted, as File Explorer's app module lets them
  through.
- Different in form only: with no window in the foreground, the root
  owners are not compared, where NVDA's comparison of the two root
  owners, both 0, accepts an invalid window's event; the listener has
  already made such a window the desktop window or dropped its event ("A
  WinEvent with no window or an invalid one"), so nothing is spoken
  differently.
- The foreground event Windows raises for a switch reaches an
  out-of-context hook only once `GetForegroundWindow` names its window;
  an event raised earlier, by the application itself (File Explorer's
  folder window, 140 to 400 ms early, with no later event when it opens
  in front) or refused by Windows' foreground lock, comes before its
  window is in front. The listener's foreground report, which moves
  Core's foreground record ("Attention"), is sent only for a window in
  front as the listener handles it; an earlier one is kept as the
  listener's pending window and sent with the observation of a later
  focus or foreground event that finds it in front, unless a newer
  foreground event replaces it (the listener logic's
  `a_foreground_event_for_a_window_not_in_front_is_sent_when_a_later_event_finds_it_in_front`).
- UIA's MSAA proxy is told never to raise, from `WinEvent`s, the UIA
  events Verbatim subscribes to, as NVDA's `ignoreWinEventsMap` tells it
  (issue 7345); focus changes keep their mapping.
- Each registration's cache request differs from NVDA's twelve base
  properties: the listener's and the focus property changes' carry what
  a node is spoken or its states are read from, and a text focus's caret
  and text events only their process, window, and class name, from which
  nothing is spoken; a difference in the provider's work only.

### A WinEvent with no window or an invalid one

NVDA's `WinEvent` callback rewrites an event's window before it queues
the event for the pump (`IAccessibleHandler/internalWinEventHandler.py`
112 to 136), after its drops by object id, of location changes other
than the caret's, and of destroys, which it hands on at once (84 to
103), and after taking an event on the window object (object id 0,
child id 0) as one on the client object (104 to 111). A window of 0
becomes the desktop window (`GetDesktopWindow`) for every event kind. A
non-zero window that `IsWindow` says is no window becomes the desktop
window for a switch start, a switch end, a menu end and a popup menu
end; any other event with such a window is dropped there (130 to 136).
NVDA hooks no switch start (`winEventIDsToNVDAEventNames`, 34 to 63), so
that one never arrives. The desktop window then passes the acceptance
test by its own rule ("Event acceptance", rule 8), and a menu end,
popup end and switch end by theirs. Its later drops also judge the
rewritten window: the IME candidate window's menu events
and Program Manager's and the taskbar's foreground events by its class
(155 to 172), and `MSNHiddenWindowClass`, a chat application's hidden
window, whose events are all dropped (183 to 187).

Verbatim: **matched** (`tell_window` in `verbatim_listener_logic::tell`;
its `an_invalid_window_is_the_desktop_for_an_end_and_dropped_for_the_rest`
and `messengers_hidden_window_and_a_show_of_another_object_are_dropped`).
The focus listener makes the same rewrite before it tells Core: a window
of 0 becomes the desktop window for every event kind, and an invalid
non-zero window becomes the desktop window for a menu end, a popup menu
end and a switch end, any other event with one being dropped; and
`MSNHiddenWindowClass` is dropped. Core's acceptance then accepts the
desktop window by rule 8. A focus candidate on the desktop window itself
is refused with no read, with the reason that no reader reads it: Core
starts no outpost for the desktop window's process (csrss.exe, whose
launch fails), and the listener mints no node. NVDA reads the desktop
object and refuses its focus as not focused: the object's state is
focusable and never focused, and neither has its parent, the window
object, whose own parent is none, so the focused-state test fails
(`IAccessibleHandler/__init__.py` 692 to 760;
`NVDAObjects/IAccessible/__init__.py` 874 to 892; probed 2026-11-18,
the client's and the window object's states focusable only). The speech
is the same, nothing. Core's
`a_focus_on_the_desktop_window_starts_no_outpost_and_asks_nothing`.
Program Manager's and the taskbar's foreground events are not
queued as foreground events, as NVDA drops them, but they update Core's
foreground record ("Attention").

### The focus after a menu or the switcher closes

NVDA's MSAA pump keeps, of the menu ends, popup menu ends and switch
ends it accepts in one flush, the last one as its fake focus event
(`IAccessibleHandler/__init__.py` 1082 to 1084; the limiter's one menu
slot, `orderedWinEventLimiter.py` 76 to 78, already keeps only the newest
menu event of the four kinds). After the flush's loop, when no focus or
foreground event of that flush was valid (`validFocus`, set when
`processFocusWinEvent` or `processForegroundWinEvent` returns true), it
calls `processFakeFocusWinEvent` (1093 to 1103), which schedules
`_fakeFocus` 50 ms later with the focus NVDA has committed then
(`api.getFocusObject()`, 976 to 988). A menu start or popup start in
that slot is handled by `processMenuStartWinEvent` instead, valid focus
or not.

`_fakeFocus` (991 to 1002) does nothing when the committed focus is no
longer the same object; otherwise it asks the desktop object for the
object with the focus (`objectWithFocus`), does nothing when there is
none, and passes it to `processFocusNVDAEvent` without `force` (760 to
782). For an MSAA object that applies both of NVDA's MSAA focus tests:
the duplicate test against the last focus queued, when that is an MSAA
object (`isDuplicateIAccessibleEvent`, 774 to 777), and the focused-state
test (`shouldAllowIAccessibleFocusEvent`, 778 to 781); a focus that
passes is queued. An object of any other kind, such as a UIA object, is
queued with neither test. A focus dropped as a duplicate still counts as
valid for the flush's `validFocus`, since `processFocusNVDAEvent`
returns true for it (777).

Verbatim: **matched (unverified)**
(`crates/verbatim-core/src/focus/menus.rs`). The menu slot of Core's
event queue keeps the newest of the four menu kinds, as NVDA's limiter
does. A menu end, popup end or switch end reaching the front when the
run of focus and foreground tells before it executed no valid focus, an
MSAA duplicate counting as valid, notes the committed focus, and 50 ms
later Core appends a check entry, unless one noting the same committed
focus is already due within the limiter's pump window, as NVDA's pump
keeps one menu event and fakes the focus once; a focus committed between
the two ends puts the later end in a later pump of NVDA's, which fakes
the focus again. At its turn, if the committed focus is
unchanged, Core asks the foreground application's outpost for its focus,
the outpost reading an MSAA focus's focused state over every ancestor up
to the desktop as a fact and reading the focus in full only when it has
it; Core judges the answer by the one definition's menu-check rows, the
focused state and NVDA's duplicate test for an MSAA answer, neither for
a UI Automation answer, which carries no focused fact. The outpost finds an MSAA focus as NVDA's
`objectWithFocus` does: from the focus window's client, `accFocus`
followed until it names nothing or the object it was asked of, a child id
turned into the child's own object by `accChild` where it has one; NVDA
follows it with no limit, the outpost for at most 64 steps. The outpost takes the window with the
keyboard focus at once, before any call that can block, as NVDA's timer
takes the foreground window and its thread's focus window. Core's
`a_menu_end_checks_the_focus_50_ms_later_and_speaks_a_focused_answer`,
`a_checked_msaa_focus_answered_unfocused_is_refused_and_not_spoken`; mockapp's
`a_fake_focus_whose_focused_ancestor_core_knows_is_spoken`,
`a_focus_now_takes_the_focus_window_before_its_probe`,
`a_focus_committed_before_the_check_ends_it_with_no_read` and
`a_menu_end_after_a_run_that_executed_a_focus_makes_no_check`.

### Attention

NVDA compares each event with the system's foreground window, and after
a foreground event holds its MSAA events for at most two core cycles
until that event's window is the foreground ("Event acceptance", above),
moving its foreground object with the next focus (`doPreGainFocus`).
Verbatim: **matched** for acceptance, which reads the live foreground
with each tell, and for the hold ("Event acceptance"); **different
(D14)** in that Core also keeps a foreground record, fed only by the
focus listener's foreground report, the one report that a window came to
the foreground, sent for every foreground event whose window is in
front, Program Manager's and the taskbar's included. Attention is the
application of the newest report. Core's inbox applies the report at
once: it stops waiting for requests whose window is outside the new
foreground window's tree or owner chain ("A slow read the user has moved
on from"), and the keyboard hook takes its view of the foreground from
it.

- Attention moves only on the listener's foreground report, never on a
  focus, so a topmost popup menu that takes focus without becoming the
  foreground leaves attention where it was. A foreground change into
  one of Verbatim's own windows is like any other (ADR 0003, "The
  ancestors Core already knows and leaving a window").
- A foreground tell is a whole-window focus, read through MSAA by the
  window's outpost at Core's request. Core refuses it with no read when
  its window is not the foreground record's, or is the window of the
  readers' last focus or of one of its ancestors, or its ids equal that
  focus's (Core's `a_foreground_candidate_not_in_front_is_refused` and
  `a_foreground_candidate_of_the_last_focuss_window_or_an_ancestors_is_refused`).
- At execution, when the new focus's ancestry shares nothing with the
  old below the desktop, the foreground object is the object the
  answer read for it, the top ancestor only when that could not be read,
  as NVDA's `doPreGainFocus` moves its foreground object.
- Attention decides whether the "not responding" sound plays and whether
  a window Core stopped waiting on as hung is watched for its recovery
  ("A window that stops responding").

### Background events

NVDA accepts allowlisted tooltip and notification-bar windows, toast
alerts, and, as an option, background progress bars from anywhere, and
UIA notifications only from the focus's application ("Event
acceptance", above). Verbatim: **partial**.

- Matched: toast alerts and the shell's window-snap results are always
  queued, as NVDA's Explorer module queues them, and other UIA
  notifications only from the focus's application, as NVDA drops
  notifications from any other. Accepted background events never move
  the focus. A toast, a help balloon or tooltip, an MSAA alert accepted
  as topmost and a desktop-window event, all accepted by something other
  than the foreground relation, are exempt from Core's stop at a
  foreground change, so a toast waiting behind a slow focus read is
  spoken after an Alt+Tab, as NVDA speaks it (Dickson, 2026-11-16; Core's
  `a_toast_and_a_show_are_exempt_from_the_foreground_stop_and_a_selection_is_not`).
- Matched: a help balloon, shown in a standard tooltip window
  (`tooltips_class32`), is spoken from any application as NVDA speaks it
  by default, and an ordinary tooltip is not, as NVDA's tooltip
  reporting is off by default: the listener tells only the shows of
  NVDA's six classes, and for a tooltip window Core asks its outpost for
  the object and tells the two apart by its role, as NVDA does (Core's
  `a_help_balloon_shown_is_spoken_and_a_tooltip_is_not`; no end-to-end
  scenario).
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

Verbatim: **different** (ADR 0003, "A frozen window"; Dickson,
2026-11-13). No reader has a rule of its own for the user having moved
on, nor a deadline of its own: only Core stops waiting. When the
listener's foreground report names a window, Core's inbox at once stops
waiting for the request out whose window is outside the new foreground
window's tree or owner chain, and ends each waiting entry whose request
would name such a window, so keys behind it are freed. On that stop the
reader abandons a thread still inside the call and starts a fresh one,
since the window may still answer for a later request; the call's answer,
if it comes, is dropped by its request's number. Core's
`a_request_out_stops_when_the_foreground_leaves_its_window_even_in_its_own_application`,
`a_request_out_stays_when_the_foreground_holds_its_window` and
`a_waiting_request_ends_unsent_when_the_foreground_leaves_its_window`;
no end-to-end scenario makes an application slow enough to show it. By
case:

- Different, deliberately: Core stops at once, where NVDA waits half a
  second, so a switch frees the keys at once.
- Different: the test is by the request's window, not by threads or by
  the focus object's window, so a move to another top-level window of
  the same application, on any thread, stops a read in the window left,
  and a move to a window owned by it does not. A focus of a form whose
  read is slow when its owned dialog comes in front is related through
  the dialog's owner chain and still read, as NVDA takes it; a focus whose
  read is slow when the user moves to an unrelated window is dropped,
  where NVDA speaks it when its slowness came after its own test. NVDA's
  other triggers, no focused window and an unacknowledged system menu,
  are not ported, nor its allowlist of window classes that always get
  the long wait (`safeWindowClassSet`).
- Exempt, as the ADR rules (Dickson, 2026-11-16): entries accepted by
  something other than the foreground relation, a toast, a help balloon
  or tooltip, an MSAA alert accepted as topmost and a desktop-window
  event, whose hang check and due time still bound them (Core's
  `an_exempt_entrys_request_out_stays_when_the_foreground_changes`).
- Not stopped, deliberately (ruled 2026-11-15): requests outside the
  event queue, from Verbatim's own dialogs, the control plane, the tree
  dump, the taskbar list's reads and a terminal's output reads, which
  hold no key, since the notification-area list sends its click-position
  read as its dialog closes and Windows then activates another window
  (Core's
  `a_request_outside_the_queue_out_is_untouched_by_the_foreground`).
- Matched: the calls NVDA's UI Automation handlers make at delivery,
  which NVDA makes on its callback thread and never cancels, are made by
  the listener's check thread for the event's application, which Core
  never stops; a slow one delays only that application's later events.

### While a focus is pending

NVDA asks in several places whether a focus is pending:
`eventHandler.isPendingEvents("gainFocus")` (`eventHandler.py` 90 to
106) is true while at least one focus event has been queued and not yet
run, from any backend and application. A focus is counted from when it
is queued, so after `processFocusNVDAEvent` or the UIA focus handler has
passed it, not while it waits in the MSAA limiter; a caret key's wait
calls `api.processPendingEvents(False)` on each pass (`editableText.py`
96), which runs
the MSAA pump without running the queue (`api.py` 381 to 396), so a
focus `WinEvent` raised during the wait becomes pending. The places
that change what is spoken, from the pinned source:

- A caret key's wait for evidence: the loop in `_hasCaretMoved`
  (`editableText.py` 90 to 99) ends as soon as a focus is pending,
  answering that the caret moved with no new caret read (`True, None`).
  The speaking helper of the edit controls NVDA's objects use,
  `behaviors.EditableText._caretScriptPostMovedHelper`
  (`NVDAObjects/behaviors.py` 255 to 257), then returns before any read,
  so the key says nothing; the base helper alone would read and speak
  the caret (`editableText.py` 164 to 177).
- A sentence key (Alt with Up or Down Arrow) where the application moves
  nothing goes through the same wait (`_caretMoveBySentenceHelper`,
  `editableText.py` 240 to 262): a pending focus ends it as a move, so
  NVDA does not move by sentence itself, and the same helper says
  nothing.
- A selection key in a control without automatic selection detection
  (`EditableTextWithoutAutoSelectDetection.script_caret_changeSelection`,
  `editableText.py` 480 to 492) reports no selection change when a focus
  is pending after it sends the key (487).
- A caret event: the base object's caret event moves the review cursor
  and braille only when the object is the committed focus and no focus
  is pending (`NVDAObjects/__init__.py` 1424 to 1429); an edit control
  with automatic selection detection detects a selection change only
  under the same two conditions (`NVDAObjects/behaviors.py` 346 to 349);
  a classic edit control's caret event (`NVDAObjects/window/edit.py`
  1068 to 1081) first processes pending events with the queue, then
  returns when the object is not the committed focus or a focus is
  pending.
- A UIA menu opening is dropped in the UIA callback when a focus is
  pending, before any object is made (`UIAHandler/__init__.py` 772 to
  777).
- An MSAA menu item's focus (`MenuItem.event_gainFocus`,
  `NVDAObjects/IAccessible/__init__.py` 2723 to 2726) and the taskbar's
  (`Taskbar.event_gainFocus`, 2732 to 2737, which first runs the MSAA
  pump without the queue) say nothing when another focus is pending. The
  focus is still committed, since `doPreGainFocus` set it before the
  object's own event ran (`eventHandler.py` 378 to 387).
- An MSAA alert (`IAccessible.event_alert`,
  `NVDAObjects/IAccessible/__init__.py` 2025 to 2040) is spoken only when
  its role is alert and it has a name, a description or a child (2026 to
  2031). Then, when a focus is pending, it runs the MSAA pump and the
  whole event queue, every pending event and not only the focuses
  (`api.processPendingEvents()`, 2033 to 2035), and it says nothing when
  it is among the committed focus's ancestors (`api.getFocusAncestors()`,
  which does not hold the focus itself; 2036 to 2037).
- An input method candidate list: when the window of the focused
  candidate item is destroyed (object id 0, child id 0), focus moves to
  the item's container only when no focus is pending
  (`processDestroyWinEvent`, `IAccessibleHandler/__init__.py` 924 to 937);
  and a selected candidate is reported only when no focus is pending
  (`reportSelectedCandidate`, `NVDAObjects/IAccessible/mscandui.py` 16).

Verbatim: **partial**. Whether a focus is pending is judged by the one
definition of a candidate's validity, the same rows its own turn runs
(`crates/verbatim-core/src/focus/validity.rs`), over what waits behind
the entry judged, item by item, a run judged as a run: a candidate the
definition finds valid counts; one it refuses, or a duplicate, does not;
an MSAA focus or menu start that needs a read of its focused state or
role is unconfirmed. A run walks newest first, so a newest duplicate
hides the candidates older than it, as NVDA's pump stops at it
(`IAccessibleHandler/__init__.py` 1087 to 1092). So a focus counts from
the point NVDA would have queued it: its backend verdict, NVDA's
overrides, acceptance, a UI Automation focus's keyboard-focus answer, the
duplicate tests and a foreground's refusals all apply. Built:

- A caret key confirms an unconfirmed MSAA focus behind it with a read of
  its facts alone, as NVDA's wait pumps its `WinEvent`s and queues only
  a focus that has the focused state (`editableText.py` 93 to 99;
  `IAccessibleHandler/__init__.py` 762 to 782; Dickson, 2026-11-18): the
  read names the candidate's window and is stored on the waiting
  candidate, read at most once, and the key judges again. A focus that
  counts, by its tell or by its confirmation, withdraws the key's
  request and the key says nothing; Backspace and Control+Backspace first
  say the text they delete, from Core's copy of the starting read
  (Dickson, 2026-11-16). A focus raised at or after the key's 100 ms bound
  is not confirmed, as NVDA's wait ends at its timeout. Before the
  starting read's answer, an unconfirmed focus does not stop the key and
  is judged at the pass. Core's
  `a_focus_pending_withdraws_the_watch_and_the_key_says_nothing`,
  `backspace_with_a_focus_pending_says_its_deleted_text_then_the_focus_is_taken`,
  `a_focus_pending_drops_a_sentence_move`,
  `an_unconfirmed_focus_found_not_focused_lets_the_key_watch_on_and_speak`
  and `an_unconfirmed_focus_found_focused_silences_the_key_and_is_read_at_its_turn`.
- A caret event ends with no read when a focus counts (Core's
  `a_caret_tell_with_a_focus_pending_asks_nothing`); an unconfirmed MSAA
  focus does not count and is not read, as NVDA's events not yet pumped
  are not pending (Core's
  `a_caret_tell_with_an_unconfirmed_msaa_focus_behind_it_reads_its_selection`).
- A UIA menu opening is dropped as it reaches Core when a focus that
  counts as pending, and whose listener callback time precedes the
  opening's, waits or is asked ahead of it; an unconfirmed MSAA focus
  does not count; a focus raised after the opening, whose
  tell reached Core first because the opening waited for its check, does
  not count (Core's
  `a_menu_opening_behind_a_pending_focus_raised_before_it_is_dropped`).
  The rule is global, as NVDA's, over every application's focuses.
- An MSAA alert with a focus that counts behind it is moved once behind
  every entry waiting, as NVDA runs the whole queue, then judged at its
  turn (Core's
  `an_alert_with_a_focus_pending_behind_it_is_moved_once_behind_it`).

Limits: with no backend verdict in force for a UI Automation focus's
window, the verdict row refuses nothing and the focus counts; and NVDA
queues a foreground only if its object converts
(`IAccessibleHandler/__init__.py` 880 to 894), so a foreground whose
object is already gone counts here and not there.

Not yet: the MSAA menu item's and the taskbar's silent focus, the
selection key without automatic selection detection, and the input
method candidate list (M7).

### Reads held behind an application's abandoned reads

NVDA has no reading threads to abandon.

Verbatim: **different**, Verbatim-only (D9). A listener reading thread is
abandoned only when Core stops waiting because the foreground left the
request's window ("A slow read the user has moved on from"). An
application with two abandoned reading threads still in their calls
(`ABANDONED_LIMIT` in `verbatim_listener_logic::focus`) gets no new
thread until one returns: its requests wait for the first of those
threads whose call returns, each bounded by Core's due time, its hang
check and the foreground stop like any request, so a stuck application
holds only its own requests and never more threads.

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
(`UIA_TRANSACTION_TIMEOUT`). The readers keep no deadline of their own:
Core gives each request a due time from when it decides to ask, 11
seconds (`HANDLING_DEADLINE`), or 13 for a focus read's whole chain of
requests (`FOCUS_READ_DEADLINE`), and stops waiting at it; a focus's
walk, a navigation step, an activation, a text request, a dialog's text
and the shell's items have no shorter limit, and a focus is reported
once, in full ([How focus
works](../../books/dev/src/explanation/design/focus.md), "Slow and hung
applications"). A request stopped at its due time says nothing, as
NVDA's cancelled call, and its late answer is dropped by its number.
Deliberately different:

- A call bounded by UIA's connection timeout, 2 seconds as in NVDA,
  still fails against an application busy for longer.
- A shell items walk that could not read every button fails whole
  (Claude, 2026-11-08, pending Dickson's acceptance); a text focus whose
  element only a slow classic walk finds waits for it, and is lost if
  its read passes the 13-second due time, as NVDA's caret read is cut
  only by its watchdog (Dickson, 2026-11-06).
- An activation whose provider runs its action synchronously holds the
  event queue, and so the keys behind it, until it returns, up to its
  11-second due time unless Windows reports the window hung first, as
  NVDA's `doAction` blocks its core (ADR 0003, "Limits accepted").
- A window message is one send with no time limit of its own, where
  NVDA sends it in 400 ms slices up to 10 seconds; Core's hang check
  and due time end the wait, leaving the worker inside the send. An
  experiment found that a timed-out `SendMessageTimeoutW` is never
  delivered later, and that `SMTO_ABORTIFHUNG` did not refuse the 400 ms
  slices after the window was judged hung, so NVDA's sliced send also
  waits to its limit or its watchdog's cancel. Core's shell refuses a
  request whose window is already hung or ghosted before sending it, as
  NVDA's `SMTO_ABORTIFHUNG` aborts for a window already hung
  (`nvdaHelperLocal.cpp`, line 89), but for a message from `WM_APP` to
  `0xBFFF`, which NVDA exempts and none of Verbatim's is. A buffer the
  message carries in the application's memory is freed only once the
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
by stopping the wait (Dickson, 2026-11-04; ADR 0003, "A frozen window";
[How focus works](../../books/dev/src/explanation/design/focus.md),
"Slow and hung applications"). "Hung" is judged live and stored nowhere,
in two places. The focus listener drops events from a hung or ghosted
window as they arrive, as NVDA's handlers do: a `WinEvent` by its
window, a UIA event by its sender's own cached window. Core's shell asks
Windows whether a request's window is hung before sending it, a ghost
mapped to its hung window, and answers a request to a hung window "not
responding" without sending it; while any request is out it asks again
every half second for each, and stops waiting for one whose window is
hung, telling its reader to cancel and leaving the reader's thread
inside the call. The readers make no hang check of their own before a
call and report no hang. A step of a reader's walk that reaches a
ghost's or a hung application's window ends the walk there ("A focus
whose element is gone before its read").

The "not responding" sound plays once for each request Core refuses, or
stops waiting for, because its window is hung, while its application
holds attention, so a hang met in the background is never sounded. A
window Core refused or stopped waiting on as hung joins its recovery
set: while its application holds attention and the window lives, the
half-second check goes on with it, and when Windows no longer reports
it hung, Core asks that application's outpost for its current focus and
judges the answer as the re-take after a reader's replacement ("Recovery
after an outpost is replaced"), the same focus taken silently and a
different one spoken; a focused terminal in that window is followed and
read again (Dickson, 2026-11-16). Core's
`a_request_out_whose_window_is_found_hung_stops_with_the_sound_when_attended`,
`a_request_the_shell_finds_hung_is_answered_not_responding_with_the_sound_when_attended`,
`a_request_out_found_hung_while_unattended_stops_with_no_sound`,
`a_watched_window_that_answers_again_leaves_the_set_and_its_entry_joins_the_queue`
and
`a_recovered_window_reads_the_focus_and_a_uia_focus_on_the_last_runtime_id_is_a_duplicate`.

- Different: a key, command or focus whose request is stopped or
  refused as hung plays the sound, where NVDA plays nothing
  (`docs/nvda/main-loop-and-watchdog.md`).
- Matched: a control in a hanging application is spoken with its line
  when the application answers, as NVDA speaks it (capture of
  2026-11-07), by the read after the window recovers, up to about half a
  second plus one read later than the call would have answered, as NVDA
  reads its focus live once its call returns and replays nothing.
- Verbatim+Tab in a hung application says nothing, as NVDA says nothing
  (Dickson, 2026-11-07): its read is refused or stopped once Windows
  reports the window hung, with the sound while attended, or stopped at
  its due time ([How focus
  works](../../books/dev/src/explanation/design/focus.md),
  "Verbatim+Tab").
- An application slow past the due time but never judged hung is
  silent, with no sound, as NVDA cancels at its watchdog's limit.

### Recovery after an outpost is replaced

NVDA has no equivalent: it is one process, and after an application
crash it re-queries the real focus
([Focus and the navigator](../nvda/focus-and-navigator.md)).

Verbatim: **different (D9, D13)**. When the outpost holding the
committed focus's node ends, Core keeps the focus's copied data but
treats its node ids as dead, and appends one entry to its event queue
asking the current focus, with no ancestors Core already knows, of the
outpost of the application that holds attention or is the committed
focus's source; a replacement starts and answers it. The answer is
compared with the kept copy (role, name, value, states, and the
ancestors' names and roles): the same is taken silently with its new
node ids, the navigator moved only if none, and a different focus is
executed and spoken. With the readers' last focus empty it is spoken. The
same question is asked once when the focus listener ends, and none while
the input desktop is another, since the read after the return covers it.
Core's
`the_committed_focus_read_again_after_its_outpost_ends_is_taken_silently`,
`a_different_focus_read_after_its_outpost_ends_is_spoken` and
`the_listeners_end_clears_its_token_and_reads_the_attended_applications_focus`.

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
Automation does not deliver"). Core drops a `WinEvent` focus in a
window whose backend verdict is UIA, as NVDA's `winEventToNVDAEvent`
drops it, and reads nothing in its place. It drops a UIA focus whose
check thread's walk to the nearest window failed or found none, as
NVDA's handler drops it (Core's
`a_uia_focus_whose_walk_found_no_window_does_not_count`; the listener
logic's `a_focus_whose_walk_finds_no_window_makes_no_other_call`), and
one whose element no longer had the keyboard focus at its delivery. So
a focus UIA never delivers
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

Verbatim: **matched (unverified)**, but for the due time below. NVDA's
first case is the listener's check thread's walk to the nearest window
failing or finding no window, and Core drops the focus, as NVDA drops it
("A focus UIA does not deliver"). In NVDA's second case the check
thread's read of the element's keyboard focus fails and lets the focus
through, as NVDA's COM error rule does; at Core's request the listener's
read of the element (`crates/verbatim-listener/src/listener/asked.rs`)
then fails its classic read of the sender, and answers the focus as the
event's cache gives it, with the ancestors read before
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
read of an application's current focus with such a window above its
focus reads the ancestors by the
classic walk rather than a remote program (Fable, 2026-10-30). Windows
described from Windows' state with no call, plain windows, stand only for
the desktop, a ghost's or a hung application's window, a window whose
MSAA client cannot be acquired, the windows above the process boundary,
and a chain cut at the depth limit, which is
completed by them from its outermost element's nearest window. A walk
has no bound but Core's due time for the read, and its focus is reported
once, in full ([How focus works](../../books/dev/src/explanation/design/focus.md),
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
window times out again, passes Core's 13-second due time for a focus
read (`FOCUS_READ_DEADLINE`), and the focus is lost, where NVDA waits for
every call.

File Explorer's placeholder element can answer its properties after it
has left the tree, so only the parent step fails. It is spoken without
its list, and always with its name, which NVDA loses when a main-loop
pass came between. The time in which the element must go differs: in
NVDA from the callback to the main thread's handling; in Verbatim from
the check thread's walk to the reading thread's classic read. `explorer_folder_window` expects its "Working on it..."
placeholder not to be said, as its keyboard focus is read live. No
capture of NVDA exists for this case.

Tests: `remote_ops.rs` in `mockapp`,
`a_focus_whose_read_fails_is_answered_whatever_its_event_says_of_its_focus`,
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
  behind each where it has one, for every node Core still holds, plus
  every node reported after the last message Core has acknowledged, and
  releases the rest; a request for a released node answers "not there".
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
- NVDA's advanced setting to turn the culling of expired focus speech
  off is not offered (not yet, M4.3, the settings package in the roadmap).
- Only in a race: a second Shift pressed before NVDA's main thread has
  applied the first pause pauses again, so speech stays paused, where
  Verbatim always resumes.
