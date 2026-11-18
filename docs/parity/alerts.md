# Alerts

Speech that arrives without a focus change: UIA notifications, toasts, MSAA alerts, help balloons, live regions, and the items of the MSAA audit not yet matched.

### UIA notification events

NVDA: `event_UIA_notification` ([The UIA client](../nvda/uia.md)) speaks a notification only from the focus's application, as its default `shouldProcessUIANotificationEvent` drops a background application's (`NVDAObjects/UIA/__init__.py` 2541 to 2543) and one whose element has no window (`appModuleHandler.py` 842 to 845); File Explorer's module speaks the shell's window-snap results whatever the focus, queued and never cutting (`appModules/explorer.py` 527 to 528, 548 to 550). A notification processed "most recent" or "important most recent" is spoken at once, cutting speech, any other queued (`NVDAObjects/UIA/__init__.py` 2545 to 2556). Its handler never calls `shouldAcceptEvent` (`UIAHandler/__init__.py` 1064 to 1156), and NVDA handles each notification it is given: its event queue merges nothing (`eventHandler.py` 48 to 67), and its UI Automation limiter coalesces events only between UI Automation's delivery and its handler (`nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp` 30 to 97).

Verbatim: **matched (unverified)**. A notification is an entry of Core's event queue, judged at its turn with no request: dropped when its check found no window, the snap results excepted, which take the desktop window; dropped when it is not from the committed focus's application, or with nothing committed the application in front, the snap results excepted; dropped when it has no text; else spoken, "most recent" and "important most recent" cutting speech, every other processing queued, the snap results queued. Two notifications waiting together are both spoken, in arrival order: one never replaces another (ADR 0003, "Flood limiters and the merge"). A notification from an application with no outpost starts none. Windows Terminal's and the console host's notifications are dropped at the listener, which reads their text itself ([terminals](terminals.md)).

Different, decided without Dickson under his rule to match NVDA unless that is proven impossible or costly (2026-11-19):

- During say-all, NVDA speaks a "most recent" notification at its priority NOW (`NVDAObjects/UIA/__init__.py` 2551 to 2553), which interrupts lower-priority speech and resumes it after (`speech/priorities.py` 22 to 26). Verbatim speaks it at Next, after the line being read, and say-all goes on, because an interrupt-and-resume priority belongs to M6.3's rewrite of the speech queue, which plans it (roadmap, M6.3). Sequence: say-all is reading a line when the focus's application raises the "most recent" notification "3 results". NVDA says "3 results" at once and then the line from where it was cut; Verbatim says the line to its end, then "3 results".
- NVDA's limiter can coalesce two notifications raised close together while its handler is busy; Verbatim's listener keys each tell by an element token of its own, so none merges in transit, and Verbatim speaks both. Sequence: an application raises "Copying 1 of 3" and then "Copying 2 of 3" while NVDA's flusher is busy. NVDA says "Copying 2 of 3"; Verbatim says "Copying 1 of 3", "Copying 2 of 3". Floods are M6.1's to measure, with its bursts of notifications.

### Toasts

NVDA: a Windows 10 and later toast is spoken on UIA's window-opened event, by NVDA's toast class (`NVDAObjects/UIA/__init__.py` 1282 to 1287), whose `Notification` behavior speaks the object (`NVDAObjects/behaviors.py` 1008 to 1013); NVDA drops every WinEvent from a native UIA window ([Event handling](../nvda/events.md), "Acceptance filtering"). Verbatim: **built, awaiting confirmation**: that a Windows 11 toast reaches this path, and its exact words, wait on the `toast` scenario's run against NVDA. The focus listener's desktop-wide UIA registration includes the window-opened event, whose tell keeps the element for Core. At its turn, a window opened whose nearest window is a `Windows.UI.Core.CoreWindow` and whose element is native is read by the listener, its name, value, states, description and position, and announced queued from anywhere, never moving the focus, unless its runtime id equals the last spoken toast's within one second, when it is dropped, leaving the record, as NVDA's own record is of the last toast it spoke (`Toast_win10`). Any outcome of the read but a result drops it unspoken. A toast is exempt from the foreground stop. MSAA alert and show events from a native UIA window are dropped, as NVDA drops every WinEvent from one, from the backend verdict for the window. A UIA notification event is not a toast path in either. Limits: a toast whose host is restarted within one second has a new runtime id and is spoken. Not yet built (unscheduled): the setting to turn toast reporting off.

### Other alerts

NVDA: `event_alert` on an IAccessible object (`NVDAObjects/IAccessible/__init__.py` 2025 to 2066) runs every pending event first (`api.processPendingEvents`), then speaks the object when its role is alert, it has a name, description or children, and it is not among the focus's ancestors; then each descendant (`recursiveDescendants`) that is focusable, or, when the alert has no description, a heading, list or list item, or static text inside a paragraph; at its priority NOW. An alert-role object is NVDA's `Dialog` class (`_staticMap`, 2849 to 2850), whose description is the dialog's text (`NVDAObjects/behaviors.py` 189 to 194).

Verbatim: **matched (unverified)**. An MSAA alert is an entry of Core's event queue. With a focus pending behind it, it is moved once behind every pending entry; then Core asks its application's outpost one request, whether the object is one of the committed focus's ancestors and else its snapshot with its descendants, as its tell carries only ids. At the answer it is dropped when it is among the committed focus's ancestors, not of role alert, or has no name, description or child; else the object is spoken, then each descendant NVDA speaks: one that is focusable, or, when the alert has no description, a list or list item. Verbatim has no heading or paragraph role, so those rules never arise. Spoken queued.

No application is known that reaches this path, and none of its differences from NVDA is verified, so the following are recorded as gaps rather than fixed:

- The alert's description is what its snapshot holds, not the dialog's text NVDA's `Dialog` class gives it.
- It is spoken queued, where NVDA speaks it at NOW.
- A help balloon raising an alert is not told apart from any other alert.

A Win32 message box, the commonest dialog that looks like an alert, raises `EVENT_SYSTEM_ALERT` only on `OBJID_ALERT`, captured on a Windows Forms `MessageBox.Show` with a warning icon (2026-11-19, five runs alike), and no object of role alert raises one. NVDA's hook drops every object id at or below `OBJID_ALERT` (`IAccessibleHandler/internalWinEventHandler.py` 85 to 92), and so does Verbatim's (`verbatim-ia2`, `is_wanted`). NVDA says the dialog's text from the foreground dialog, as its focus's ancestor: "capalert, dialog, The file could not be saved because the disk is full.", then "OK, button"; closing it, "capalert", then "File name, edit, selected report". Verbatim's focus chain gives a dialog its text, so it should say the same; a Verbatim run against the same message box is still to confirm it.

### Help balloons

NVDA speaks a help balloon from the MSAA show of its `tooltips_class32` window: the object is NVDA's `Notification` class (`NVDAObjects/IAccessible/__init__.py` 2846), whose `event_show` is `event_alert` (`NVDAObjects/behaviors.py` 1008 to 1017), speaking it at normal priority with its role, "help balloon"; an ordinary tooltip is not spoken, as NVDA's tooltip reporting is off by default (992, 1008). The Caps Lock balloon of a Win32 password field raises no alert, only that show (captured 2026-11-19, five runs alike). NVDA says, with Caps Lock pressed in the field, "Caps Lock is On: Having Caps Lock on may cause you to enter your password incorrectly.  You should press Caps Lock to turn it off before entering your password., help balloon", then "caps lock on"; with Tab into the field while Caps Lock is on, "Password, edit, protected, selected, 6 star", then the balloon queued after it.

Verbatim: **matched (unverified)**. The hook keeps a show of a `tooltips_class32` window whose object is its client or a custom object, or its window, read as its client; acceptance takes it by its class, from any application, exempt from the foreground stop. At its turn Core asks the outpost for the object's snapshot and speaks it queued when its role is help balloon; any other role is dropped. Of the six classes NVDA accepts a show from, only `tooltips_class32` is handled; the other five wait for application support (`docs/parity/focus-events.md`, "Background events"). A Verbatim run against the same password field is still to confirm the role read, the words, and that the balloon comes before "caps lock on", whose speech has its own timing.

### Items of the MSAA audit of 2026-10-07 still not yet

Items of the MSAA audit of 2026-10-07 still **not yet (unscheduled)**,
each found against NVDA's source:
- A focused list view group header. When a focus event names a
  `SysListView32` itself and one of its group headers has the focus
  (`LVM_GETFOCUSEDGROUP`), NVDA makes the group the focus, named by its
  header, its footer as its value, collapsed or expanded, inside the
  list view, reading the group through its in-process helper. A
  control's group focus exists only while it has the keyboard focus,
  which mockapp's tests do not give it, so this waits for an end-to-end
  scenario to be tested against.
- NVDA asks `accFocus` again of what `accFocus` answered, until it
  answers the same object, when it reads the focus on request; Verbatim
  asks once.
- NVDA handles each focus event of a batch in order once another kind
  of event follows it, so a focus separated from a later one by other
  events is still handled; Verbatim handles a batch's newest focus
  first and falls back to older ones only when it cannot be reported.

### Live regions

Live regions (browsers). NVDA: in-process IA2 machinery
([IA2 usage](../nvda/ia2.md)). Verbatim: **not yet (M7)**.
