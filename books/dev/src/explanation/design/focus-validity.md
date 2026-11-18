# Focus validity: one definition, decided in Core

A focus candidate is anything that may become the focus Verbatim speaks: an MSAA focus, foreground or menu start event, a UI Automation focus or menu opening, the read Core makes after a menu closes with no focus, and the focus read again after a hung window answers. Focus validity is whether a candidate is a focus NVDA would have queued, and so whether Verbatim reads and speaks it. This page explains how that is decided today. Its decisions, the options weighed and the limits are in [ADR 0004](../../reference/adr/0004-focus-validity.md); the mechanism's types and functions are in the crate guides of [verbatim-core](../../reference/crates/verbatim-core.md), [verbatim-outpost](../../reference/crates/verbatim-outpost.md), [verbatim-ia2](../../reference/crates/verbatim-ia2.md) and [verbatim-listener-logic](../../reference/crates/verbatim-listener-logic.md); every rule, with NVDA's source and each deliberate difference, is in [the parity entry for focus events](https://github.com/Neurrone/verbatim/blob/main/docs/parity/focus-events.md).

## Overview

Validity is asked in two kinds of place. At a candidate's own turn in Core's event queue, Core decides whether to read it and speak it. While a candidate waits, other rules ask whether a focus is pending behind them: a caret key, a caret event, an MSAA alert and a UI Automation menu opening each behave differently when one is. NVDA answers both questions on one thread, reading the application as it judges. Verbatim reads applications from other processes, so the two questions were once answered by separate code, and the answers disagreed.

Now one definition answers both, in Core's `focus/validity.rs`. It judges a candidate against Core's facts as they are now: the two versions of the focus (the committed focus, NVDA's focus object, and the readers' last focus, NVDA's last queued focus), the foreground record, and the backend verdict of each window. It also takes what a reader read of the candidate, if anything. It gives one of four answers:

- Valid: read in full and executed, or, for a UI Automation menu opening on the focus already spoken, that focus executed again.
- Duplicate: the focus is the one already spoken. Nothing is read or said, and a run of candidates ends there.
- Refused, with a reason. A run goes on to its older candidates.
- Needs a read: the candidate cannot be judged until a reader reports its focused state, or a menu start's role.

The readers report facts and judge nothing. An outpost reads what Core's request names and answers what it found: that the window is read through UI Automation, so the object was skipped; that the object is not there; or that it was found, with its focused state or role. The listener's check thread makes, for each UI Automation event as it is delivered, the calls NVDA's handler makes, and carries their results in the tell. Core judges every result.

The listener keeps only the drops that use no fact of Core's: NVDA's checks on an event's own window and object that cannot hang, the drops by an event's own cached class and automation id, and the mouse pointer's shape changes. A future rule belongs there only if it reads nothing of Core's; a rule that compares with the last focus, the foreground or a window's verdict is a row of the definition.

The rows run in a fixed order, each answering or passing on. They differ by interface, so MSAA and UI Automation candidates are described apart below, then the parts they share.

## MSAA candidates

### Runs and requests

Consecutive MSAA focus, foreground and menu start tells waiting in the event queue form a run, after NVDA's limiter has merged and capped them. At the run's turn Core walks it newest first across every application, as NVDA's pump does. Each candidate is judged with any facts already stored on it:

- Refused: the walk goes on, with no request.
- Duplicate: the run ends silently.
- Valid, or needing a read: Core asks that candidate's own outpost, naming the candidate's window, so the hang check, the not-responding sound and the foreground stop all judge the window being read. The answer is judged by the same rows. A valid answer is executed and spoken; a refused one sends the walk on.

Each request names one candidate and how far its read goes:

- Facts only: whether the object is there, and its focused state or role. The caret key's confirming read uses this.
- In full if focused: the facts, and the full read only when the object or an ancestor has the focused state. Every MSAA focus is read this way unless the identity check applies.
- In full unless the same: as above, unless the object reached is the focus already spoken (the identity check, below).
- In full: a foreground or a menu start.

Core chooses the depth from the same definition, so the outpost holds no rule. An answer that breaks its depth's rule, such as a found and focused object with no read, is a reader's fault: it is logged as a malformed answer and the entry ends.

### What the outpost reads

The outpost acquires the object from its event's window, object and child ids. For a focus event it applies NVDA's redirect: a focus on a list's own object, or on a `SysListView32` window's client, is read as the child the list's `accFocus` names, and a focus on any other object's own child id 0 is read as the focused child `accFocus` names. A redirected focus carries the child's ids as its event's, as NVDA builds the redirected object. A menu start takes no redirect: it is read at its event's address, as NVDA builds it from the event alone. For a focus the outpost then tests the focused state of the object and each ancestor, read live, as NVDA's `shouldAllowIAccessibleFocusEvent` does; for a menu start it reads the role.

### The rows

In order:

1. The window's backend verdict: a window read through UI Automation refuses its MSAA candidates.
2. Verbatim's hidden frame's own object is refused.
3. Acceptance: the event's window must pass NVDA's acceptance test against the foreground record.
4. The desktop window: a candidate on the desktop window's own object is refused unread, since no reader reads the desktop process. NVDA reads that object and refuses it as not focused, so the speech is the same.
5. A foreground is refused when its window is not the one in front, when it is the last focus's window or an ancestor's, or when its ids are the last focus's.
6. The duplicate test, below.
7. A menu start is refused when the run before it executed a focus on an MSAA menu. Otherwise it needs its role: a menu popup is valid, anything else refused.
8. A focus needs its focused state: not focused, not there or skipped is refused.
9. Otherwise valid.

### The duplicate test

The readers' last focus records how it was read: for an MSAA focus, its event's ids and the child id its object is held by.

- A plain object, held by child id 0, is a duplicate of a later focus with the same window, object and child ids, with no read. This is NVDA's test.
- An item, held by a child id above 0, is never a duplicate by its ids, because MSAA child ids are positions and another item can take the same position. NVDA's test stops there. Verbatim adds the identity check, so that an item whose focus the application raises twice, or whose focus arrives once by the list and once by the item, is said once.

The identity check applies when all of these hold:

- the candidate is an MSAA focus;
- no key and no failed try has come since the last focus was executed;
- the last focus is held by a child id above 0;
- the last focus is still the committed focus.

Core records the first two as a fence: every entry is stamped, as it arrives, with the newest key before it, and the readers' last focus keeps the fence of the entry that executed it. A candidate behind a later key has another fence. A candidate tried at its turn and found refused, not there, or whose reader ended, clears the record. A candidate refused before any read, or skipped as read through UI Automation, leaves it.

When the check applies, Core's request names the last focus's ids and its identity, taken from Core's own snapshot of what was spoken: its states, role, position in its set and the set's size, name and value. The outpost tests the focused state as usual. If the object reached, after the redirect, is at the named ids, it reads the identity by the same functions a full read uses, in that order, and stops at the first value that differs. Position and size are compared only where the control gives them with no walk, a `SysListView32` item's. An object that reads as named is answered "the same", with nothing more read, and Core makes it a duplicate unless a row before the duplicate test now refuses it. Any difference is read in full in the same request and judged as a new focus. The focus read always asks the position and set size, whatever the position-reporting setting, which decides only whether they are spoken.

So the comparison is against what Verbatim said, never against a second read of the old object. A different item at a reused position reads differently and is spoken; a key between two focuses on the same item makes the second one spoken.

A pending judgement makes no identity read, so there an item's focus is never a duplicate, as in NVDA. Startup and the return after a desktop switch execute with no duplicate test, as NVDA queues those focuses.

### The check after a menu closes

When a menu, popup or switcher closes and the run that followed executed no focus, Core makes an entry 50 ms later, NVDA's fake focus. With the committed focus unchanged, it asks the foreground application for its current focus in full if focused, the outpost reading an MSAA focus's focused state over every ancestor up to the desktop. Core judges the answer by the definition's rows for this check: the MSAA duplicate test by the ids the answer carries, then the focused state. A UI Automation answer carries no ids and is spoken again when it is the focus Core holds, as NVDA's check tests only MSAA objects.

## UI Automation candidates

### The check thread's calls

The listener hears UI Automation focus and menu opening events and hands each to its application's check thread. As the event is delivered, that thread makes the calls NVDA's handler makes, in NVDA's order: the walk to the element's nearest window, the window's class verdict, a probe for a native provider, the keyboard focus of each element the listener keeps under the event's runtime id, a nameless Office menu item's previous sibling, the task switcher item's parent automation id, and the element's own keyboard focus. It judges nothing. Core's backend cache is the one owner of a window's verdict: the check thread keeps a half-second record of its probes only to spare a repeated probe call, and never ends a check on it.

A UI Automation tell is sent at once, never held behind a foreground event. The foreground hold, which keeps tells until `GetForegroundWindow` names a new foreground window, holds only MSAA tells, as NVDA's defers only its `WinEvent`s.

### The rows

In order:

1. A menu opening on the committed focus's own element is that focus executed again, as NVDA redirects it to its focus object: it passes acceptance and its keyboard-focus answer, or is refused.
2. A candidate whose walk found no window is refused.
3. The window's backend verdict: a window read through MSAA refuses its UI Automation candidates.
4. NVDA's focus overrides: the console window's own element, Excel's panes and File Explorer's task view frames are refused; a nameless first Office menu item is let through whatever its keyboard focus.
5. Verbatim's hidden frame's own element is refused.
6. Acceptance against the foreground record.
7. An element the keyboard-focus answer says has lost the focus is refused.
8. The duplicate test: the readers' last focus is a UI Automation element with the same runtime id, and the element the listener kept for it still has the keyboard focus by the answer the tell carries. A failed read, or a last focus the listener no longer keeps, is no duplicate. Runtime ids are unique only among live elements, so the test reads the kept element, not whatever now holds the id.
9. Otherwise valid, with no read needed.

A UI Automation candidate never needs a read: every fact its rows use came in its tell. So a waiting UI Automation focus that passes the rows counts as pending at once.

## What both share

### Whether a focus is pending

One function judges what waits behind an entry, item by item, each run walked as its turn would walk it:

- A run walks newest first. The first valid candidate counts, even behind a newer one needing a read, since that read could only make the newer one count too or send the walk down to the older one.
- A duplicate ends the walk: unconfirmed if a newer candidate needed a read, otherwise not pending. So a newest duplicate hides older candidates, as NVDA's pump stops at it.
- Over the whole queue: counts if any item counts, otherwise the newest unconfirmed candidate, otherwise not pending.

Its users, each matching how NVDA asks:

- A caret key, which confirms an unconfirmed focus with a read (below).
- A caret event, which asks nothing more when a focus counts. An unconfirmed MSAA focus does not count and is not read, as NVDA's events not yet pumped are not pending.
- An MSAA alert, which with a focus that counts behind it moves once behind everything waiting.
- A UI Automation menu opening, dropped as it arrives when a focus raised before it counts, the entry at the front included. An unconfirmed MSAA focus does not count.

### The caret key's confirming read

A caret key in an edit field or a terminal waits for its effect for up to its bound, 100 ms from the key's pass by default. A focus that counts as pending ends the wait and the key says nothing, as NVDA's caret scripts do. NVDA's wait pumps its MSAA events, so an MSAA focus counts only once its focused state is read. Verbatim matches that with a confirming read.

The key's stage is derived from the request it has out, never stored: starting (its starting read), watching, confirming, a sentence move, and for a terminal its final read after the bound. Each path has a table saying what each arrival does in each stage:

- A key, command or key Core sends withdraws the watch and a confirmation, and the key is silent.
- A focus that counts withdraws the starting read (Backspace's excepted, which still says what it deletes), the watch, a confirmation and a sentence move, and the key is silent.
- An unconfirmed MSAA focus raised before the bound withdraws the watch, and the key confirms it.
- A focus raised at or after the bound, or one that does not count, changes nothing.

To confirm, Core asks the candidate's outpost for its facts alone, naming the candidate's window, while the key's starting point stays held. The facts are stored on the waiting candidate, so each is read at most once, and the key judges again: counts, silent; another unconfirmed focus, confirm it; neither, watch for the rest of the bound. A stored refusal also serves the candidate's own turn, which then asks nothing. A menu start behind the key is confirmed for its role. If the candidate's application hangs, the hang check stops the confirmation and the key ends silent; if its outpost ends, the candidate is stored as not there.

### The readers' last focus

Both duplicate tests, the foreground refusals, the menu start rule and the identity check compare with the readers' last focus. It is set when Core executes a focus a reader read, with the fence of the entry that executed it, and emptied when the foreground moves to a top-level window unrelated to the committed focus. A desktop switch leaves it as it was.

### The recovered read

When a window Core stopped waiting on as hung answers again, Core reads the application's current focus. The definition has one row for that read: a UI Automation answer whose runtime id is the readers' last focus's is a duplicate, its keyboard focus taken as true since the read is of the keyboard focus. Any other answer, like the read Core makes after a reader is replaced, goes on to the re-take's comparison of Core's new read with the kept focus, which takes the same focus silently and speaks a different one. That comparison is not a candidate's validity but Core's own re-read of its committed focus, which NVDA, in one process, never makes.

### Outside the definition

- The listener's and the hook's drops on an event's own facts, NVDA's callback-level checks.
- The foreground hold, which compares `GetForegroundWindow` with the event's window.
- The event queue's limiter, whose merges and caps are queueing rules applied before any judgement, as NVDA's limiter runs before its pump.
