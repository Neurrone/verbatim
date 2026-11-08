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
"Acceptance filtering"). Verbatim: **built, awaiting confirmation**: that a
Windows 11 toast reaches this path, and its exact words, wait on the
`toast` scenario's run against NVDA. The focus listener's
desktop-wide UIA registration includes the window-opened event; its
callback keeps a window of control type window whose automation id
contains `ToastView`, from the event's cache, and runs once per window
opened anywhere, waking nothing else unless the window is such a
candidate. The candidate goes through its application's reading thread,
where the class of its nearest window, the element being XAML with no
window of its own, must be `Windows.UI.Core.CoreWindow`; one call reads
its value, states, description and position, and it is reported as
`NormalizedEvent::Alert` with `AlertOrigin::ToastWindowOpened`, carrying a
hash of its runtime id. The reducer speaks it queued from anywhere, never
moving focus, and drops a toast whose id equals the last spoken toast's
within one second, leaving the record, as NVDA's own record is of the
last toast it spoke (`Toast_win10`). MSAA alert and show events from a
native UIA window are dropped, as NVDA drops every WinEvent from one,
from the arbitrator's verdict for the window, which a window with none
gets by a probe. A UIA notification event is not a toast path in either:
both speak those only from the focus's application. The MSAA alert path
stays for a toast from a window whose parent has the class
`ToastChildWindowClass`, and help balloons are not affected. Limits: a
toast whose host is restarted within one second has a new runtime id and
is spoken. Not yet built (unscheduled): the setting to turn toast
reporting off.

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
- NVDA handles each focus event of a batch in order once another kind
  of event follows it, so a focus separated from a later one by other
  events is still handled; Verbatim handles a batch's newest focus
  first and falls back to older ones only when it cannot be reported.
- An alert-role object's alert event, as in "Other alerts".

### Live regions

Live regions (browsers). NVDA: in-process IA2 machinery
([IA2 usage](../nvda/ia2.md)). Verbatim: **not yet (M7)**.
