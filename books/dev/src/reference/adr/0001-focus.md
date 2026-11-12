# ADR 0001: Focus

Status: decided. First decided 2026-10-10; amended 2026-10-29, 2026-11-02, 2026-11-07, 2026-11-08 and 2026-11-13.

How focus works today is explained in [How focus works](../../explanation/design/focus.md). This record keeps why it works that way: the problem, the options weighed, what was chosen and by whom, and the limits accepted. The sections amended on 2026-11-13 state decisions of [ADR 0003](0003-readers-tell-core-asks.md) that are not built yet: until the builds of its phase replace it, the code works as How focus works and the crate guides describe, and the limits accepted below are those of that code.

## The problem

A screen reader must announce each focus with the containers newly entered, in the order the application raised its events, without being stalled by an application that is slow or hung. NVDA does this in one process with one ordered queue and one focus ancestry. Verbatim is split into processes (D9: one outpost per application, so a stuck call is reclaimed by ending its process; D13: a separate focus listener), and three measured UI Automation facts constrain where focus can be read:

- An element cannot leave the process that received its event ([UIA defects](../uia-bugs.md), "Elements cannot be passed to another process").
- A focus registration is desktop-wide; the scoped form is undocumented and misses proxied windows ("Focus events cannot be limited to one application").
- A hung window named in a focus `WinEvent` holds every UI Automation event of a process registered for focus ("A hung window named in a system focus event holds a process's UIA events").

A fourth fact limits who may call an application: two Verbatim processes connecting to one application collide inside UI Automation ("A new connection fails while another process's remote operation is queued"), so only an application's own outpost connects to it.

Dickson's constraints for any design: something must still notice a new application, so that its outpost is started; one event must not be processed twice, by the listener and by an outpost; and a slow application must never stall the listener or another application.

## Decisions

### Who reads a UI Automation focus (Dickson, 2026-10-10; weighed again 2026-10-29; amended 2026-11-13)

Chosen: the focus listener holds the desktop-wide UI Automation focus registration, tells Core of each focus event and keeps its element, and reads that element only when Core asks; with the listener's answer, a snapshot and the element's route, Core asks the application's outpost to complete the read, its node ids, an MSAA window above it and the chain above the process boundary, the two requests under one due time ([ADR 0003](0003-readers-tell-core-asks.md), "A UI Automation focus read"). Reason: only the listener holds the element, which cannot leave its process, and only the outpost connects to its application. While a hung window holds the listener's UI Automation events, no UI Automation focus is announced until the window answers, as in NVDA, with no MSAA stand-in, because announcing from MSAA would sound different and a visible failure is better.

Weighed:

- The listener reads only the sender's keyboard focus and the outpost finds the element again: rejected, because the outpost's search finds the wrong ancestors where an application raises focus from elements outside its tree (Windows Terminal's tabs), and two processes would call the application for one event.
- Every outpost holds a desktop-wide focus registration: rejected, because each registration adds 47 to 90 provider calls on the focused application's UI thread per focus event, multiplied by the outposts.
- Each outpost holds a registration scoped to its windows, with a standby holding the desktop-wide one: rejected, because the standby discards the events of applications it is not given to.
- Each outpost reads its own application's focus with `GetFocusedElement`: rejected, because `GetFocusedElement` does not always name the event's sender (Start's search results), failed reads of a starting application drop the focus, and every new window's first focus would wait on a connection.
- One process hosting every UI Automation client: declined (Dickson, 2026-10-20), because a hung window holding that process's events would be heard everywhere.

### The listener's checks, ported from NVDA's focus handler (Dickson, 2026-10-12; amended 2026-11-07 and 2026-11-13)

NVDA's focus handler's checks keep NVDA's order: the native check by NVDA's MSAA class list, the duplicate check, the acceptance test on the sender's nearest window, the console window's and the hidden frame's refusals, and the sender's keyboard focus. The acceptance test is judged in Core, a pure rule over the window facts the listener gathers as the event arrives and the tell carries, written from the parity document on focus events ([ADR 0003](0003-readers-tell-core-asks.md), "Acceptance judged in Core"); the checks that make calls into the application run inside the read Core asks for; the listener makes NVDA's hung test as the event arrives and judges nothing else. Reason: three parity defects had one cause, NVDA's handler rebuilt piece by piece with each check placed where it suited Verbatim, and a pure rule makes each verdict testable from its inputs. The earlier element's keyboard focus is read before the sender's, as NVDA reads them (Dickson, 2026-11-02), because Windows Terminal raises its terminal's focus twice at launch, and reading the sender first took the earlier element for another one and said the terminal again.

### One route per application, and the marker (Dickson, 2026-10-10; kept 2026-10-25 and 2026-10-29; amended 2026-11-13)

Each kind of event has exactly one teller, and every event reaches Core's one event queue in arrival order: the listener holds every desktop-wide WinEvent hook and the UI Automation focus registration, and an outpost holds no WinEvent hook, except a console host's outpost, which keeps the console's five events; an outpost tells Core of the UI Automation property changes of the objects Core has it follow, and of text and caret events ([ADR 0003](0003-readers-tell-core-asks.md), "Who holds what"). Reason: no event reaches Core twice by two routes with no order between them, and an event raised after a focus is not handled before it, as NVDA's one queue guarantees; with one route there is no marker and no hand-over of an application's first events. The listener's events and an outpost's UI Automation events still have no order between them, which only the narrow rule repairs ([ADR 0003](0003-readers-tell-core-asks.md), "The narrow ordering rule").

Weighed and rejected: ordering both routes by observation time with a watermark, which delays every event; a per-window hand-over to a group-scoped registration, which rests on an undocumented route; and the listener as the only UI Automation route, which would move the text and terminal code into the listener and give up D9's reclamation.

### MSAA focuses stay with the outpost (Dickson, 2026-11-07; amended 2026-11-13)

The listener tells Core of an MSAA focus with the rest of every application's MSAA events, through its desktop-wide hooks, and the application's outpost reads it at Core's request ([ADR 0003](0003-readers-tell-core-asks.md), "Who holds what"). NVDA's event limiter and its runs of focus events, which need an application's MSAA events in order, are applied in Core to the MSAA entries waiting in its event queue ([ADR 0003](0003-readers-tell-core-asks.md), "Flood limiters and the merge").

### One input to Core, in arrival order (Dickson, 2026-10-10; amended 2026-11-13)

Everything enters Core's inbox in the order it arrives, and every application event, key and command joins Core's one event queue and is judged in that order, as NVDA queues gestures with events ([ADR 0003](0003-readers-tell-core-asks.md), "Core's inbox and the event queue"). No read starts before Core's request, so no report is ordered by stamps: a foreground change ends the entries waiting or asked whose request names a window outside the new foreground window's tree or owner chain. Rejected: ordering Core's input by observation time, which needs a reorder buffer that waits for the slowest outpost, a delay on every input that NVDA does not have (weighed again and kept rejected by Dickson, 2026-11-08, when a reference model ordered by observation time could not be matched by any Core that takes its input in arrival order).

### One judge per fact (Dickson, 2026-10-29; amended 2026-10-30 and 2026-11-13)

Core holds the one focus ancestry and judges repeats, acceptance and the foreground; a reader drops an event only by NVDA's local checks that cannot hang, and every other rule is Core's ([ADR 0003](0003-readers-tell-core-asks.md), "Readers tell, Core asks"). Each window has one backend verdict, its outpost's (class lists, then one raw probe, kept as NVDA keeps it), and the listener's native check is a function of the class names that both processes compute from one table. Evaluated and discarded: sending every reader a copy of Core's focus to judge against, because a copy judged in another process disagrees with the original while messages are in flight, and a focus returning to an element while Core's notice was on its way was never spoken; and a verdict made in each process, because two verdicts for one window spoke a focus late or twice where NVDA speaks it once.

### The foreground and the window title (Dickson, 2026-10-30 and 2026-11-02; amended 2026-11-08 and 2026-11-13)

- Every test against the foreground is Core's, made in arrival order: a foreground change ends the entries whose request names a window outside the new foreground window's tree or owner chain, judged by window, so another window of the same application counts ([ADR 0003](0003-readers-tell-core-asks.md), "Core's inbox and the event queue").
- The listener's foreground message is the only report of a foreground change. A change into a top-level window unrelated to the committed focus empties the readers' last focus, in arrival order, and with it empty the duplicate tests, the whole-window refusal and the stop points behave as with no earlier focus, so a return says the window's title and the focus; the committed focus stays for commands and typed echo ([ADR 0003](0003-readers-tell-core-asks.md), "Stop points and leaving a window"). Reason: the return from a busy application is always read and said as the same switch. Rejected: a placeholder focus for the window, because NVDA keeps its focus object, so object navigation and reporting the focus would differ. Kept as a deliberate difference after being weighed against NVDA (Dickson, 2026-11-08): NVDA drops a foreground change into a UI Automation window and keeps its focus, so on a return from such a window that raised no focus its duplicate check says nothing, where Verbatim announces the switch.
- A foreground change into a window read through MSAA is a whole-window focus, as NVDA queues a focus on the window (Dickson, 2026-11-02).
- The title rule: the window's title is said when the focus enters a top-level window it was not in, decided in Core from the focus's ancestry, so that the title does not depend on the timing of the application's events (Dickson, 2026-10-30).
- The title rule adds no title when the new ancestry meets the earlier one at an object they share (Claude, 2026-11-08, accepted as it is by Dickson, 2026-11-08). Reason: NVDA compares its focus ancestry with the one before, and speaks the window in front only when its focus difference level is at most 1, that is when the two differ just below the desktop (`eventHandler.doPreGainFocus`). Evaluated and discarded: meeting the ancestries only above the new foreground window's own entry, which was right on the way into a window owned by another process's window but said the owner's title on the way back.
- A focus from an application the user has left that still waits in the event queue when the foreground changes ends unspoken, judged by its window, a kept difference: NVDA's queue would speak it before the change.

### A focus read is complete or nothing (Dickson, 2026-11-07; amended 2026-11-13)

Chosen: Verbatim waits for a slow or hung application as NVDA does and says nothing for it until it answers. Only Core stops waiting: it gives a read up on Windows' live hung report, at its due time (13 seconds for a focus read, 11 for others) or when the foreground leaves the read's window, and takes a late answer to a focus it gave up as a fresh focus, so the focus is spoken when the hang ends, as NVDA's is; the readers keep no deadline and no moved-on rule ([ADR 0003](0003-readers-tell-core-asks.md), "A frozen window"). A control focused just before a hang is spoken with its line when the application answers (capture of 2026-11-07).

Evaluated and discarded, because the application is not responding anyway and anything said before it answers is partial or guessed:

- Speaking a partial read when a walk takes too long, completing the rest of the ancestry from Windows' state.
- Bounding the checks before a read by a time budget, after which cached values stand in for live ones: every check reads live, as NVDA's.
- Abandoning the reader's thread as soon as its window is reported hung: once one call waits on a busy UI thread, every UI Automation call into that process waits too (`books/dev/src/explanation/windows/uia.md`), so a fresh thread gains almost nothing; Core gives the request up and the thread is left alone.
- Saying a held application's window title, or answering a request for the focus from a hung window's text, before the application answers: NVDA says nothing until it answers.

Focus events waiting behind a slow read wait in Core's event queue, merged and limited as NVDA's limiter does ([ADR 0003](0003-readers-tell-core-asks.md), "Flood limiters and the merge"), and each is read in its turn, as NVDA reads its queue, even though the last focus is then spoken late.

A shell items list whose walk could not read every button fails whole rather than answering the buttons read so far (Claude, 2026-11-08, accepted by Dickson), by the same rule.

### A focus UI Automation does not deliver (Dickson, 2026-11-07)

A `WinEvent` focus in a UI Automation window is dropped, as NVDA drops it. Evaluated and discarded: reading the application's focus when no UI Automation focus follows a `WinEvent` focus, since an application that has not raised its focus is usually not responding anyway, and NVDA has no such read.

### When an application's call fails (Dickson, 2026-11-07)

A failed or timed-out walk step continues from the parent window of the outermost entry's nearest window, by that window's own object, read live, as NVDA's fallback does, bounded only by the deadline. A failed or timed-out check before the read lets the focus through, as NVDA's handling of a COM error does. A window element that cannot be read drops the focus, as NVDA's unguarded call does. Weighed and rejected: completing the ancestry from Windows' state after a timed-out step, which would speak the focus at about 10 seconds where NVDA keeps calling.

### Object identity (Dickson, 2026-10-30 and 2026-11-07)

Within one outpost, a UI Automation element keeps the node its runtime id has, and an MSAA object is given a kept node's id only when NVDA's `IAccessible._isEqual` holds, its location compared (Dickson, 2026-10-30). Evaluated and discarded: taking two MSAA containers of one role and name as one, because a second unnamed toolbar was then not announced. Two objects of different backends are never equal, as NVDA's (Dickson, 2026-11-07). A window above the process boundary is described from Windows' state with no call into the process that serves it (Dickson, 2026-10-29), because a call into a busy host held the focus below it for the host's whole stall. Such a window is a stand-in that equals any entry with the same handle (the reason is Claude's, from the parity audit): NVDA reads that window through its own API, so both of its ancestries hold the same object there and meet, and the stand-in is the only way Verbatim's can. Weighed: having the owner's outpost read that window; deferred, because it needs a request between outposts, and the stand-in gives NVDA's result whenever Core already holds the owner. It is the named alternative if the stand-in causes problems.

### Reusing an earlier ancestry (Dickson, 2026-11-07; amended 2026-11-13)

The problem: a walk that stops at saved ancestors and copies them can speak a stale one, as Alt+Tab back to Windows Terminal spoke its old title. NVDA never speaks a stale name: it walks live to the lowest common ancestor with its focus ancestry, which after a visit elsewhere belongs to the other application.

Chosen: no reader keeps an earlier ancestry. A focus walk stops at the stop points, Core's committed focus's line of ancestors, sent with the request only while the readers' last focus is set, as NVDA walks until it meets its own focus line; a return from another application walks to the top ([ADR 0003](0003-readers-tell-core-asks.md), "Stop points and leaving a window"). Reason: nothing above the meeting point is reused, so no stale window title can be spoken.

Weighed and rejected: an ancestry cache per application in each reader, made safe by a check of each cached window's title and by Core asking the reader to read the element again live when a cached ancestor would be heard, because Core had to correct a reader by asking it again, and a return paid a partial walk, a round trip and a full walk where NVDA pays only the full walk. For such a second read, asking the outpost for the application's current focus was rejected too, because the current focus can differ from the event's element, and NVDA reads the event's own object.

Limit: a return to an application after another application's focus was committed walks its ancestry to the top live, 14.0 ms against 6.2 ms with a cache.

### One ordered focus queue (Dickson, 2026-11-08; amended 2026-11-13)

Chosen: Core's event queue is NVDA's one ordered queue, widened to every application event, key and command, and Core keeps NVDA's two facts apart: the committed focus, NVDA's focus object, and the readers' last focus, NVDA's last queued focus ([ADR 0003](0003-readers-tell-core-asks.md), "Core's inbox and the event queue" and "The two versions of the focus").

- Only the front entry may have a request out, and an entry is judged when it reaches the front, every entry ahead of it decided, as NVDA handles a focus at its place in its queue.
- A key or command joins the queue and runs in its turn, its action chosen at its press, as NVDA queues gestures with events; a held caret key is released to the application 150 ms after its press whatever waits ahead of it. The check after a menu's or the switcher's end is an entry Core makes itself.
- Waiting entries for the same object and kind are merged, the newest keeping its place; a wait is bounded by Core's give-up: a hung report, the due time, or the foreground leaving the request's window.
- The duplicate tests, the whole-window refusal, a run's refusals and the system caret's target judge the readers' last focus; commands, the narrow rule and typed echo judge the committed focus.
- After a reader is replaced, Core asks for the focus again and takes the same focus silently; with the readers' last focus empty, it is spoken.

Reason: one state value per lifecycle, with NVDA's two facts kept apart, removes the judgement of a report against a copy of a line it may never speak.

Weighed and rejected: one waiting report, replaced by any newer report accepted, because NVDA executes the older report first and its entered containers are heard; and judging the reports in observation order across the applications' channels, which is the reorder buffer the arrival-order decision rejects, and which still could not reorder speech already queued for a later report.

### The formatting last reported goes with the focus object (Dickson, 2026-11-08)

The problem: Core kept the formatting last reported, with its node, apart from the focus, and cleared it only when a spoken focus was taken, so whether formatting was said again depended on which kind of focus came between.

Chosen: the formatting last reported is part of the focus object, and each focus taken, a new object, starts with none, as NVDA keeps its cache on the object (`speech.py` 1494 to 1512) and creates a new object for each focus event. The silent re-take after an outpost is replaced is a new object too, so the next line says its formatting again, as it did before. A focus in a shell staging window and a nameless whole-window focus, which are not spoken, also start afresh, so a return to the text through either says its formatting again.

Rejected: keeping the formatting across the silent re-take, which would say less after a replacement than before it; and keeping a separate record with its own clearing rule, a second fact about the focus.

### Report focus (Dickson, 2026-11-07)

Verbatim+Tab asks the application for its focus live, so it reports the real focus even when it moved without an event, and speaks the answer without taking it as Core's focus. The query goes to the process of the foreground thread's focus window, so Settings answers from its content's process rather than its frame's. NVDA+Tab reports NVDA's own focus object with live properties; its history records no reason. Weighed: taking a different answer as Core's focus, silently or as a new focus; rejected, because a real focus event of the same element on its way would then be dropped as a duplicate.

When the query is answered with nothing to report, Verbatim says "No focus", as NVDA+Tab does when it has no focus object; when it is abandoned at its deadline or replaced by a later press, nothing (Claude, 2026-11-08, accepted by Dickson). A waiting press is ended only by a focus observed after it, so a focus already on its way when the key was pressed is spoken and then the answer, as NVDA runs its script after the focus queued before it (Claude, 2026-11-08, accepted by Dickson).

### Window messages, shell items and test limits (Claude, 2026-11-07, accepted by Dickson 2026-11-08; the shell items' wait Dickson's, 2026-11-07; amended 2026-11-13)

A window message is sent once, bounded by Core's due time for its request. NVDA's 400 ms slices were measured: a timed-out send is never delivered later, and `SMTO_ABORTIFHUNG` did not refuse slices after the window was reported hung, so slicing ends NVDA's wait only at its watchdog's cancel, and one send matches its outcome.

The taskbar's shell items reads take Core's 11-second due time, as every request does ([ADR 0003](0003-readers-tell-core-asks.md), "A frozen window"). The end-to-end suite's step limits are 75 seconds.

### The desktop's return and name (Dickson, 2026-10-29)

The listener hooks the desktop switch and, scoped to csrss's process, the desktop window's name change. A return to the user's desktop asks the application in front for its focus again 200 ms later, taken without the duplicate rule, as NVDA's handler of the user's desktop does; the virtual desktop's name is read on csrss's reading thread and said before the next focus's entered ancestors, or alone 250 ms after the change, as NVDA says it.

### The outpost's own element (Dickson, 2026-10-10 and 2026-10-21)

The outpost finds its own copy of a UI Automation focus by the path the listener sent, one remote operation down the chain's runtime ids from the nearest ancestor with a window. A focus with no text is reported at once and found after; one whose role may have text is found before the report, since its caret and line come in the focus's message (Dickson, 2026-10-21). An element the outpost cannot find is announced from the listener's read, and review and navigation start from its nearest found ancestor (Dickson, 2026-10-10).

### Focus-following subscriptions (Dickson, 2026-10-11)

One focus-following group per outpost, built once, added to the new focus's element and then removed from the previous one, never registered on a window's subtree, as NVDA follows the focus. Reason: moving a registration by removing everything and adding it again reached two UI Automation defects, a crash on a saved event and a text area that stopped receiving events.

### Time limits (Dickson, 2026-10-10; amended 2026-11-06 and 2026-11-13)

UI Automation's connection and transaction timeouts are process-wide, so each process sets them once at its start: 2 seconds and 10 seconds, the transaction timeout below Core's 11-second due time for a request ([ADR 0003](0003-readers-tell-core-asks.md), "A frozen window"), so that a slow call fails and the worker carries on. Every thread others wait on has a watchdog.

### Spare outposts (Dickson, 2026-10-16)

Two outposts are kept started and waiting, and one is given to an application when the supervisor would otherwise start an outpost for it; a replacement starts the moment one is given or fails, and an outpost once given never returns. Reason: starting an outpost takes about 40 ms before it can take its first event, at the threshold of being noticed. Verbatim's readiness waits for the two spares, so every scenario's applications take a spare. Every outpost failure takes one path, whatever the cause; nothing is ever given up, and repeated failures are spaced by an exponential backoff up to 30 seconds, started again once an outpost has stayed up or its application comes to the foreground.

## Limits accepted

- A focus whose application hangs past the deadline is lost, and nothing marks it.
- A walk whose application is slow past one UI Automation timeout, but not reported hung, loses its focus at the outpost's deadline, where NVDA waits for every call.
- A focus UI Automation never delivers is lost, as in NVDA.
- A focus whose element is gone and whose window cannot be read is lost, as in NVDA.
- UI Automation's title-bar proxy answers a navigation that passes its 2-second connection timeout as no element and no error (observed under parallel test load, when reading the title-bar buttons' state waited on the Desktop Window Manager), so a walk over a window with a title bar can miss its title-bar buttons without any sign of it.
- After a return between top-level windows, a focus whose report would speak a cached ancestor costs a live re-read: the reader's partial walk, one round trip and a full walk, where NVDA pays only the full walk. While one live re-read waits, every further focus event of the returned element asks its own, so a window that raises two focus events for its control on a return costs two, and every focus behind it waits for its answer before it is spoken.
- A held caret key pressed while a report waits for its live re-read reaches the application only after that wait, bounded by the re-read's moved-on rule and its 13-second read deadline, before its own 150-millisecond read bound starts (Dickson, 2026-11-08).
- Focuses A, B, A from two outpost generations in two top-level windows, reaching Core as A, the repeat of A, then B: nothing is said after the first A and Core ends on A, where NVDA says B then A (Dickson, 2026-11-08).
- A focus in a window shared by a frame and its content, observed before the other process's ordinary focus report in that window and reaching Core after it, is not spoken. Core ends on the same focus as NVDA, but the containers the earlier report entered are not spoken, where NVDA speaks them before the later focus; matching it would need the reorder buffer rejected above (kept, Dickson, 2026-11-08).
- An outpost that follows a UI Automation focus the listener read in its application no longer holds its last MSAA read, so a live re-read of that read is answered gone (Dickson, 2026-11-08).
- A report set aside for a live re-read whose element is gone by the re-read is not spoken, and an answer from a window the user has left by then is refused.
- A container renamed while the focus was elsewhere is caught only by Core's check, at the cost of a live re-read.
- A window above the process boundary is a stand-in described from Windows' state, spoken by its title.
- A window left for a window owned by another process's window, whose own focus never arrives, re-enters the owner on the return, where NVDA would not.
- The "not responding" sound, which NVDA does not have, plays only when the application holds attention, and while a listener read waits a hang is told only when the hung window is in front.
- One application's later events wait behind its slow read, up to the deadline each, and one burst can wait about 39 seconds behind abandoned reads; other applications do not wait.
