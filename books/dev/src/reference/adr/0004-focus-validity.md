# ADR 0004: One definition of focus validity, decided in Core

Status: decided, 2026-11-18, by Dickson; the decisions marked as delegated were Claude's, reported to him. [Focus validity](../../explanation/design/focus-validity.md) explains how it works today, and the crate guides of [verbatim-core](../crates/verbatim-core.md), [verbatim-outpost](../crates/verbatim-outpost.md), [verbatim-ia2](../crates/verbatim-ia2.md) and [verbatim-listener-logic](../crates/verbatim-listener-logic.md) describe the code. Every rule, with NVDA's source, is in [the parity entry for focus events](https://github.com/Neurrone/verbatim/blob/main/docs/parity/focus-events.md).

This record rests on [ADR 0003](0003-readers-tell-core-asks.md): readers tell Core of each event, read only when Core asks, and Core holds every rule that decides what is spoken.

## Problem

A focus candidate is an event or read that may become the focus Verbatim speaks: an MSAA focus, foreground or menu start, a UI Automation focus or menu opening, the read after a menu closes with no focus, and the focus read again after a hung window answers. Whether a candidate is valid is asked twice: at its own turn, to decide whether to speak it, and while it waits, to decide whether a focus is pending behind a caret key, a caret event, an alert or a menu opening. A pending focus silences a caret key.

NVDA answers both on one thread, reading the application while it judges: it queues only a focus that passes its reads, and a focus counts as pending only once queued. Verbatim reads applications from other processes and Core never blocks on one, so the two questions had been answered by different code. The turn's judgement was split between Core and the outpost, which tried an application's candidates in a loop and judged them itself. The pending judgement used only what the tell carried. So a focus Verbatim would refuse at its turn could silence a caret key, and a focus it would speak could fail to. Examples: a foreground on the text box's own window, which is refused, silenced Down Arrow's line; a stale tree focus in the Open dialog, never focused, silenced Left Arrow's "blank". A list item whose focus arrived twice was said twice, or was dropped by a guard that some paths missed.

## Decision

One definition of validity, in Core, answers every question about every candidate, at its turn and while it waits, with the same rows in the same order. It judges against Core's facts as they are now and against whatever a reader reported of the candidate. It answers valid, duplicate, refused with a reason, or needs a read (the focused state, or a menu start's role).

The readers report facts and judge nothing. An outpost answers whether it skipped the object (its window is read through UI Automation), whether the object is there, and its focused state or role, and reads it in full only to the depth Core's request names. The listener's check thread makes the calls NVDA's UI Automation handler makes at delivery and carries the results. The listener keeps only the drops that use no fact of Core's, as NVDA's callback-level checks do; any rule that compares with the last focus, the foreground or a window's verdict is a row of the definition.

## MSAA candidates

- A run of waiting MSAA candidates is walked strictly newest first across applications, as NVDA's pump has no grouping by application (delegated). Each candidate is its own request to its own outpost, naming its own window, so a hung application holds only its own read. A refused candidate sends the walk to the next older one, a duplicate ends the run, and the first valid one is spoken.
- A focus event takes NVDA's redirect: a list's own focus is read as the child its `accFocus` names, and another object's own child id 0 as its focused child. A menu start takes no redirect and is read at its event's address, as NVDA builds it (Dickson, 2026-11-18).
- The duplicate test. For a plain object, held by child id 0, a later focus with the same window, object and child ids is a duplicate with no read, as NVDA's test. For an item, held by a child id above 0, NVDA's test never finds a duplicate, as child ids are positions. Verbatim adds the identity check there (Dickson, 2026-11-18). A later MSAA focus is a duplicate when all of these hold:
  - no key and no failed try has come since;
  - it reaches the last focus's ids;
  - it reads as Core's record of what was spoken, compared in this order and stopping at the first difference: states, role, position in the set and the set's size where the control gives them with no walk, name, value.

  The outpost makes the comparison against the values Core names and answers "the same" as a fact; Core makes it the duplicate. A pending judgement makes no identity read.
- A caret key confirms an unconfirmed MSAA focus raised before its bound with a read of the focus's facts alone, stored on the waiting candidate, so each is read at most once; it counts only if the read finds it focused (Dickson, 2026-11-18). A menu start behind the key is confirmed for its role, as NVDA's wait pumps and its pump queues a menu popup start (delegated).
- The check after a menu, popup or switcher closes with no focus reads the application's current focus and is judged by the same definition: NVDA's MSAA duplicate test and the focused state.
- A candidate on the desktop window's own object is refused unread, since no reader reads the desktop's process; NVDA reads it and refuses it as never focused, so the speech is the same (Dickson, 2026-11-18).

## UI Automation candidates

- The check thread's delivery-time calls are judged in Core: the nearest window, the probe, each kept element's keyboard focus, the element's own keyboard focus, and the overrides' extra reads. Core's backend cache is the one owner of a window's verdict; the check thread's probe record only spares a repeated probe and never ends a check (delegated).
- The duplicate test compares runtime ids and asks whether the element Verbatim kept for the last focus still has the keyboard focus, by the answer the tell carries, as NVDA's live read of its earlier object.
- UI Automation tells are never held behind a foreground event; the foreground hold keeps only MSAA tells, as NVDA's defers only its `WinEvent`s (Dickson, 2026-11-17).
- A menu opening on the committed element is that focus executed again, after acceptance and its keyboard-focus answer, as NVDA redirects it to its focus object (delegated).
- A UI Automation candidate never needs a read, so it counts as pending as soon as its rows pass. Caret events, alerts and UI Automation menu openings do not count an unconfirmed MSAA focus and make no read for it, as NVDA's events not yet pumped are not pending (Dickson, 2026-11-18).

## What both share

- One pending judgement walks what waits behind an entry item by item, each run as its turn would walk it: the first valid candidate counts, a newest duplicate hides older ones, and a candidate needing a read is unconfirmed. Every user handles all three answers.
- The caret key's stage is derived from the request it has out, never stored, as ADR 0003 rejects Core's sequences written as named stages; the edit field's path and the terminal's each have their own table of what every arrival does in every stage (Dickson, 2026-11-18).
- The readers' last focus, NVDA's last queued focus, is the one fact every duplicate test, foreground refusal, menu start rule and identity check compares with. It keeps the fence of the entry that executed it, the newest key that entry arrived behind, so a candidate behind a later key has another fence; a failed try clears it.
- The read after a hung window answers again has one row: a UI Automation answer on the last focus's element is a duplicate. Any other answer goes to Core's comparison of its new read with the kept focus, which is not a candidate's validity.

## Options weighed

- **The pump ported into Core: events judged in batches as NVDA's main thread gathers them.** Rejected and stopped. NVDA can judge a batch at once because its thread blocks while it reads; Verbatim, reading at the queue's front, needed new waiting states to stand in for that, and the batch boundaries are timing, not a rule. Each review found more sequences where the copy and NVDA disagreed.
- **Reading the application's current focus on every focus event, speaking it if it changed.** Rejected. It meets most of the recorded cases, but classic menus raise no focus, so a second route is needed for them. It also cannot tell an item from another at the same position without comparing contents, gates every focus on the foreground window, and changes several decisions of record for no gain in speech.
- **A bounded blocking read while judging.** Rejected. Core would stop taking keys and speech reports while a read is out. A bound short enough to keep the keyboard alive times out ordinary reads, and a timeout changes speech or falls back to the asynchronous judgement anyway.
- **Judging in the outpost.** Rejected. Validity depends on the last focus and the foreground, which are facts across applications that only Core has in order; an outpost would also have to hear events again.
- **Carrying validity in the tell: the listener reads each MSAA candidate's facts before telling Core.** Rejected. The listener would read every candidate where NVDA stops at the first valid one, and earlier than NVDA reads it, and choosing the newest valid candidate there still depends on Core's last focus and foreground.
- **The outpost trying an application's candidates in one request.** Rejected for one request per candidate, so that Core decides every candidate and no reader judges. A candidate refused after its read costs one more round trip than before; a run whose newest candidate is valid costs nothing more.
- **For a stale MSAA focus behind a caret key: count it on the tell's facts, never count it, or confirm it with a read.** Confirming was chosen, as NVDA counts only a focus its reads pass. Counting on the tell's facts silenced keys over focuses NVDA refuses; never counting spoke keys NVDA silences.
- **Confirming for every user of the pending judgement.** Rejected: caret events, alerts and menu openings make no read, as NVDA does not pump for them.
- **A guard against a list item said twice, by ids alone.** It refused a later focus at the redirected child's ids until another focus was spoken. Rejected for the identity check: it held only where a run spoke the focus, so other paths said the item twice, and it would silence a different item that took the same position.
- **The identity check as a separate guard, or inside the duplicate test.** Inside the duplicate test, so there is one test of whether a focus is the one spoken, extending only the branch NVDA leaves open.
- **The identity check applied to the pending judgement.** Rejected, to match NVDA, which queues both focuses of a split pair; behind a caret key the check never holds anyway, since the key moves the fence.
- **The position compared only when position reporting is on.** Rejected: a setting decides what is spoken, never which focuses are.
- **Comparing against a second read of the old object.** Rejected. NVDA once compared its new focus object with the old one and removed the check, because reading the old object again found its position's new occupant, so a deleted item's replacement went unspoken. Verbatim compares with its own record of what it said, which no later read changes.
- **File Explorer's rule refusing a focus at the last focus's ids whatever came between.** NVDA has it for some Explorer list views. Removed: after Delete on the desktop it silenced the next file, now at the deleted item's position. The identity check says Explorer's double focus once and reads a focus after a key or with another reading.

## Dickson's decisions

All dated 2026-11-18 unless marked.

- The foreground hold holds only MSAA tells (2026-11-17).
- One definition of validity; a caret key confirms an unconfirmed MSAA focus with a read, counting it only if found focused.
- Core decides every candidate; the outpost only reads and reports facts.
- The identity check is not applied to the pending judgement.
- Caret events, alerts and UI Automation menu openings do not count an unconfirmed MSAA focus and make no read.
- A list item whose focus events split is said once, where NVDA says it twice.
- A failed try ends the identity check's fence, so the mouse return to a list after a click whose read failed is spoken.
- The identity check, compared in the order states, role, position and set size, name, value; placed inside the duplicate test.
- Position and set size are read whatever the position-reporting setting.
- After Delete on the desktop the next item is read; File Explorer's ids-only rule is removed.
- A menu start takes no list redirect.
- The desktop window's object is refused unread, the speech being NVDA's.
- Focus-to-speech latency is measured after the build: at most 50 ms required, 10 ms preferred.

## Limits

- A different item at a reused position that reads exactly as the one spoken, with the same position and set size, is not spoken where NVDA speaks it. Only an identical item deleted and another added at once can do this. For a tree view item, or a list item outside a list view, the position is not compared, so an identical item at the deleted one's position is not spoken either.
- A stale candidate refused between a split pair clears the fence, so the item is said twice.
- A click whose candidate is refused with no read keeps the fence, so the return to the list is not spoken where NVDA speaks it.
- Startup, or the return after a desktop switch, following a run says the item twice, as NVDA does.
- With no backend verdict yet for a UI Automation focus's window, the verdict refuses nothing and the focus counts as pending.
- NVDA queues a foreground only if its object can be made, so a foreground whose object is already gone counts as pending here and not there.
- A confirming read behind a hung application waits for the hang check, and the key then ends silent.
- A refused candidate costs one more round trip to its outpost than a single request per application did.

## What would reopen this

A way for Core to judge a candidate with a read and no waiting state, or a recorded application in which the identity check silences a focus a user needs to hear.
