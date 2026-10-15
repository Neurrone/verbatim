# Alerts

Speech that arrives without a focus change: UIA notifications, toasts, other alerts, live regions, and the items of the MSAA audit not yet matched.

### UIA notification events

UIA notification events (snap layout hints etc.), incl.
interrupt-vs-queue by processing hint. NVDA:
`event_UIA_notification` ([The UIA client](../nvda/uia.md)). Verbatim:
**matched (unverified)**, foreground-gated in shell. NVDA speaks
notifications only from the focus's application, plus
per-application opt-ins; see `docs/parity/focus-events.md`, "Event
acceptance". A notification from an application with no outpost starts
none, the shell's window-snap results excepted: such an application
cannot hold the focus, so Core would drop it, as NVDA drops it.

### Toasts

NVDA: a toast's `alert` event is accepted from any application, and
the `Notification` behavior speaks the object; NVDA drops every WinEvent
from a native UIA window, and speaks a Windows 10 and later toast on
UIA's window-opened event instead ([Event handling](../nvda/events.md),
"Acceptance filtering"). Verbatim: **partial**. The listener hooks
`EVENT_SYSTEM_ALERT` desktop-wide; the outpost reports an alert only from
a window whose parent has the class `ToastChildWindowClass`, and the
reducer speaks the object queued from anywhere, never moving focus;
this is unverified. Different, by route rather than by decision:
Verbatim subscribes to neither of UIA's window-opened and
tooltip-opened events, and receives toasts only through the MSAA alert,
which it takes from any window whose parent is `ToastChildWindowClass`,
native UIA or not, and speaks whatever the object's role. A UIA
notification event is not a toast path in either: both speak those only
from the focus's application. Whether Windows 11 raises that MSAA alert
for its toasts at all is unverified; if it does not, Verbatim says
nothing for a toast that NVDA speaks (the roadmap's M4 "Stage B" item
captures one live and follows NVDA if so). Help balloons are not
affected and match. Not yet built (unscheduled): the setting to turn
toast reporting off.

### Other alerts

NVDA speaks an alert at once when the object's role is
alert, it has a name, description, or children, and it is not already
among the focus's ancestors (`event_alert` on IAccessible objects).
Verbatim: **not yet (unscheduled)**: it has the alert role (UIA and MSAA
map to it), but no alert events, so it reports no other alerts.

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
- NVDA handles the system caret's show event as a caret event on the
  focus; Verbatim follows the caret's location changes only, and reads
  the caret after every focus anyway.
- NVDA handles each focus event of a batch in order once another kind
  of event follows it, so a focus separated from a later one by other
  events is still handled; Verbatim handles a batch's newest focus
  first and falls back to older ones only when it cannot be reported.
- An alert-role object's alert event, as in "Other alerts".

### Live regions

Live regions (browsers). NVDA: in-process IA2 machinery
([IA2 usage](../nvda/ia2.md)). Verbatim: **not yet (M7)**.
