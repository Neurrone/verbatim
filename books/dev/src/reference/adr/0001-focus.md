# ADR 0001: Focus

Status: decided. First decided 2026-10-10; amended 2026-10-29, 2026-11-02, 2026-11-07 and 2026-11-08.

How focus works today is explained in [How focus works](../../explanation/design/focus.md). This record keeps why it works that way: the problem, the options weighed, what was chosen and by whom, and the limits accepted.

## The problem

A screen reader must announce each focus with the containers newly entered, in the order the application raised its events, without being stalled by an application that is slow or hung. NVDA does this in one process with one ordered queue and one focus ancestry. Verbatim is split into processes (D9: one outpost per application, so a stuck call is reclaimed by ending its process; D13: a separate focus listener), and three measured UI Automation facts constrain where focus can be read:

- An element cannot leave the process that received its event ([UIA defects](../uia-bugs.md), "Elements cannot be passed to another process").
- A focus registration is desktop-wide; the scoped form is undocumented and misses proxied windows ("Focus events cannot be limited to one application").
- A hung window named in a focus `WinEvent` holds every UI Automation event of a process registered for focus ("A hung window named in a system focus event holds a process's UIA events").

A fourth fact limits who may call an application: two Verbatim processes connecting to one application collide inside UI Automation ("A new connection fails while another process's remote operation is queued"), so only an application's own outpost connects to it.

Dickson's constraints for any design: something must still notice a new application, so that its outpost is started; one event must not be processed twice, by the listener and by an outpost; and a slow application must never stall the listener or another application.

## Decisions

### Who reads a UI Automation focus (Dickson, 2026-10-10; weighed again 2026-10-29)

Chosen: the focus listener judges and reads every UI Automation focus from its sender, on a thread per application, and the application's outpost finds its own copy of the element afterwards by the path the listener sent. While a hung window holds the listener's UI Automation events, no UI Automation focus is announced until the window answers, as in NVDA, with no MSAA stand-in, because announcing from MSAA would sound different and a visible failure is better.

Weighed:

- The listener reads only the sender's keyboard focus and the outpost finds the element again: rejected, because the outpost's search finds the wrong ancestors where an application raises focus from elements outside its tree (Windows Terminal's tabs), and two processes would call the application for one event.
- Every outpost holds a desktop-wide focus registration: rejected, because each registration adds 47 to 90 provider calls on the focused application's UI thread per focus event, multiplied by the outposts.
- Each outpost holds a registration scoped to its windows, with a standby holding the desktop-wide one: rejected, because the standby discards the events of applications it is not given to.
- Each outpost reads its own application's focus with `GetFocusedElement`: rejected, because `GetFocusedElement` does not always name the event's sender (Start's search results), failed reads of a starting application drop the focus, and every new window's first focus would wait on a connection.
- One process hosting every UI Automation client: declined (Dickson, 2026-10-20), because a hung window holding that process's events would be heard everywhere.

### The listener's checks, ported from NVDA's focus handler (Dickson, 2026-10-12; amended 2026-11-07)

The listener's checks before a read are NVDA's focus handler, ported as one pure step function in NVDA's order: the native check by NVDA's MSAA class list, the duplicate check, the acceptance test on the sender's nearest window, the console window's and the hidden frame's refusals, and the sender's keyboard focus. Reason: three parity defects had one cause, NVDA's handler rebuilt piece by piece with each check placed where it suited Verbatim, and a pure function makes each verdict testable from its inputs. NVDA's hung test is made once, as the event arrives; the listener makes no other. The earlier element's keyboard focus is read before the sender's, as NVDA reads them (Dickson, 2026-11-02), because Windows Terminal raises its terminal's focus twice at launch, and reading the sender first took the earlier element for another one and said the terminal again.

### One route per application, and the marker (Dickson, 2026-10-10; kept 2026-10-25 and 2026-10-29)

An application's MSAA events all reach its outpost through one process-scoped hook in raise order, and a UI Automation focus takes its place in that order through a marker that holds the application's later events until the listener's read arrives. Reason: an event raised after a focus must not be handled before it, as NVDA's one queue guarantees. Until a new outpost's hook is in, the listener forwards the application's `WinEvent`s; the outpost holds its own hook's stream until forwarding ends, then drops its copies of what was forwarded (Dickson, 2026-10-25).

Weighed and rejected: ordering both routes by observation time with a watermark, which delays every event; a per-window hand-over to a group-scoped registration, which rests on an undocumented route; the listener as the only UI Automation route, which would move the text and terminal code into the listener and give up D9's reclamation; and, for the hand-over, matching each forwarded copy with the hook's copy by counting, which handled a copy twice when it arrived after the counting ended.

### MSAA focuses stay with the outpost (Dickson, 2026-11-07)

Weighed: the listener reading MSAA focuses too, so that one process would read every focus event. Rejected, because the outpost needs an application's whole MSAA stream in order (NVDA's event limiter and its runs of focus events), the window's backend verdict would again need two owners, and every MSAA focus would need the marker hold; it would undo the one-route rule to solve a smaller problem.

### One input to Core, in arrival order (Dickson, 2026-10-10)

Core takes events and key commands from one channel in the order they reach it, as NVDA queues gestures with events. A focus from another outpost observed before the focus Core has applied is dropped (the stale rule), and a background event observed before the last applied foreground change is dropped, as NVDA's foreground change cancels speech queued before it. Rejected: ordering Core's input by observation time, which needs a reorder buffer that waits for the slowest outpost, a delay on every input that NVDA does not have.

### One judge per fact (Dickson, 2026-10-29; amended 2026-10-30)

Core holds the one focus ancestry and judges repeats, staleness and the foreground; the listener and the outposts make every accessibility call and drop an event only on what they observe of it then. Each window has one backend verdict, its outpost's (class lists, then one raw probe, kept as NVDA keeps it), and the listener skips a read only when the sender's nearest window's class is on NVDA's MSAA class list, a function of the class names that both processes compute from one table. Evaluated and discarded: sending every reader a copy of Core's focus to judge against, because a copy judged in another process disagrees with the original while messages are in flight, and a focus returning to an element while Core's notice was on its way was never spoken; and a verdict made in each process, because two verdicts for one window spoke a focus late or twice where NVDA speaks it once.

### The foreground and the window title (Dickson, 2026-10-30 and 2026-11-02; amended by Claude, 2026-11-08, pending Dickson's acceptance)

- Every test against the foreground is made once, on the outpost's reducer thread from Windows' state, as the entry is taken in, and Core judges each focus against its record's front (Dickson, 2026-11-02). Reason: one test per fact; a worker can wait behind a call into a held application, so a test made there came seconds after its event.
- The listener's foreground message is the only report of a foreground change, and a change into a window unrelated to Core's focus marks that focus left (Dickson, 2026-11-02). Reason: the return from a busy application is always read and said as the same switch. Rejected: a placeholder focus for the window, because NVDA keeps its focus object, so object navigation and reporting the focus would differ.
- The outpost reports a foreground change into a window it reads through MSAA as a whole-window focus, as NVDA queues a focus on the window (Dickson, 2026-11-02).
- The title rule: the window's title is said when the focus enters a top-level window it was not in, decided in Core from the focus's ancestry, so that the title does not depend on the timing of the application's events (Dickson, 2026-10-30).
- Core records the window a foreground change left its focus for, and a report from that window ends the left mark, so the two ancestries are compared as they are; the title rule adds no title when the ancestries meet at an object they share (Claude, 2026-11-08, pending Dickson's acceptance). Reason: NVDA compares its focus ancestry with the one before, and speaks the window in front only when its focus difference level is at most 1, that is when the two differ just below the desktop (`eventHandler.doPreGainFocus`). Evaluated and discarded: meeting a left ancestry only above the new foreground window's own entry, which was right on the way into a window owned by another process's window but said the owner's title on the way back.

### A focus read is complete or nothing (Dickson, 2026-11-07)

Chosen: Verbatim waits for a slow or hung application as NVDA does and says nothing for it until it answers. The only limits are UI Automation's own per-request timeout, one deadline per reading thread or outpost program (13 and 11 seconds), and the moved-on rule, which NVDA's watchdog also has. A control focused just before a hang is spoken with its line when the application answers (capture of 2026-11-07). While a listener read's marker waits, the outpost checks the window in front for a hang, so the hang is told even though nothing is in flight in the outpost (Dickson, 2026-11-07).

Evaluated and discarded, because the application is not responding anyway and anything said before it answers is partial or guessed:

- Speaking a partial read when a walk takes too long, completing the rest of the ancestry from Windows' state.
- Bounding the checks before a read by a time budget, after which cached values stand in for live ones: every check reads live, as NVDA's.
- Abandoning a read or program as soon as its window is reported hung: once one call waits on a busy UI thread, every UI Automation call into that process waits too (`books/dev/src/explanation/windows/uia.md`), so a fresh thread gains almost nothing. The "not responding" sound is kept as a notice that abandons nothing.
- Saying a held application's window title, or answering a request for the focus from a hung window's text, before the application answers: NVDA says nothing until it answers.

Kept at 13 seconds: the deadline (Dickson, 2026-11-07). A hang longer than about 13 seconds right after a focus loses that focus, where NVDA speaks it when the stall ends.

The listener's focus events waiting for one application's reading thread are not capped: a slow but responsive application that raises many focus events has each read in order, as NVDA reads its queue, even though the last focus is then spoken late (Dickson, 2026-11-08).

A tree dump and a shell items query are judged by their deadline and the hang check only, not by the moved-on rule, because they are not about the focus (Claude, 2026-11-08, accepted by Dickson); NVDA's watchdog judges its recovery against its focus object. A shell items list whose walk could not read every button fails whole rather than answering the buttons read so far (Claude, 2026-11-08, pending Dickson's acceptance), by the same rule.

### A focus UI Automation does not deliver (Dickson, 2026-11-07)

A `WinEvent` focus in a UI Automation window is dropped, as NVDA drops it. Evaluated and discarded: reading the application's focus when no UI Automation focus follows a `WinEvent` focus, since an application that has not raised its focus is usually not responding anyway, and NVDA has no such read.

### When an application's call fails (Dickson, 2026-11-07)

A failed or timed-out walk step continues from the parent window of the outermost entry's nearest window, by that window's own object, read live, as NVDA's fallback does, bounded only by the deadline. A failed or timed-out check before the read lets the focus through, as NVDA's handling of a COM error does. A window element that cannot be read drops the focus, as NVDA's unguarded call does. Weighed and rejected: completing the ancestry from Windows' state after a timed-out step, which would speak the focus at about 10 seconds where NVDA keeps calling.

### Object identity (Dickson, 2026-10-30 and 2026-11-07)

Within one outpost, a UI Automation element keeps the node its runtime id has, and an MSAA object is given a kept node's id only when NVDA's `IAccessible._isEqual` holds, its location compared (Dickson, 2026-10-30). Evaluated and discarded: taking two MSAA containers of one role and name as one, because a second unnamed toolbar was then not announced. Two objects of different backends are never equal, as NVDA's (Dickson, 2026-11-07). A window above the process boundary is described from Windows' state with no call into the process that serves it (Dickson, 2026-10-29), because a call into a busy host held the focus below it for the host's whole stall. Such a window is a stand-in that equals any entry with the same handle (the reason is Claude's, from the parity audit): NVDA reads that window through its own API, so both of its ancestries hold the same object there and meet, and the stand-in is the only way Verbatim's can. Weighed: having the owner's outpost read that window; deferred, because it needs a request between outposts, and the stand-in gives NVDA's result whenever Core already holds the owner. It is the named alternative if the stand-in causes problems.

### Reusing an earlier ancestry (Dickson, 2026-11-07)

The problem: each focus reader keeps an ancestry cache per application and stops its walk at the cache join, copying the cached ancestors above it. A cached ancestor can be stale after a visit elsewhere, so Alt+Tab back to Windows Terminal spoke its old title. NVDA never speaks a stale name: it walks live to the lowest common ancestor with its focus ancestry, which after a visit elsewhere belongs to the other application.

Chosen: readers keep their caches, and two checks make reuse safe.

- The title check: before reusing, a focus reader compares each cached window's current text, read locally from Windows' state, with the text it cached, and walks live on any change.
- Core's check: when Core takes a report, `needs_live_ancestry` decides from Core's own focus ancestry whether any cached ancestor would be spoken, entered or used for the window's title. If so, Core sets the report aside and asks the same focus reader to read the same element again, live (the live re-read).

Reason: whether a cached ancestor would be heard depends only on Core's focus ancestry, so Core is the one place that can tell exactly, and nothing has to tell the readers where the focus has been.

Weighed and rejected:

- No reuse at all: exact with no mechanism, rejected for its cost of one call per ancestor on every focus (Dickson).
- A counter of application switches kept by the listener's `WinEvent` thread: it missed focuses raised only through UI Automation.
- Having the listener forget its cache at a foreground change: the forgetting could run after the first focus of the return.
- Core completing a cut report with an ancestors query: a focus waiting between processes collided with Core's checks, held nodes, dialog text and the outpost's followed focus.
- A record owned by the listener, fed by both its event sources, with a notice to outposts: the focus moves between the listener and the outpost within one application (Alt+Space in Windows Terminal), which the listener cannot see.
- Notices from Core whenever its focus ancestry left a reader: they fail unsafe, since a reader can save a cache from a report Core refused or never saw.
- Grants from Core for each report it took: Core's own title rule spoke cached ancestors, and the steady state read live anyway, because a read often starts before the previous grant returns.
- For the live re-read, asking the outpost for the application's current focus: rejected, because the current focus can differ from the event's element, and NVDA reads the event's own object.

A repeat of the same element while its live re-read waits is judged as a new report: it asks its own live re-read, or is spoken if it was read live (Claude, 2026-11-08, pending Dickson's acceptance). Evaluated and discarded: dropping it as a duplicate of the report set aside, because a focus reader answers a live re-read only for the read it holds last, so the older request was answered gone and nothing was spoken.

### Report focus (Dickson, 2026-11-07)

Verbatim+Tab asks the application for its focus live, so it reports the real focus even when it moved without an event, and speaks the answer without taking it as Core's focus. The query goes to the process of the foreground thread's focus window, so Settings answers from its content's process rather than its frame's. NVDA+Tab reports NVDA's own focus object with live properties; its history records no reason. Weighed: taking a different answer as Core's focus, silently or as a new focus; rejected, because a real focus event of the same element on its way would then be dropped as a duplicate.

When the query is answered with nothing to report, Verbatim says "No focus", as NVDA+Tab does when it has no focus object; when it is abandoned at its deadline or replaced by a later press, nothing (Claude, 2026-11-08, accepted by Dickson). A waiting press is ended only by a focus observed after it, so a focus already on its way when the key was pressed is spoken and then the answer, as NVDA runs its script after the focus queued before it (Claude, 2026-11-08, pending Dickson's acceptance).

### Window messages, shell items and test limits (Claude, 2026-11-07, pending Dickson's acceptance; the shell items' wait Dickson's, 2026-11-07)

A window message is sent once, bounded by its program's deadline. NVDA's 400 ms slices were measured: a timed-out send is never delivered later, and `SMTO_ABORTIFHUNG` did not refuse slices after the window was reported hung, so slicing ends NVDA's wait only at its watchdog's cancel, and one send matches its outcome.

Shell items keep one clock, the outpost's answer-by time of 63 seconds (Dickson, 2026-11-07): the marker can hold the query behind it for three focus read deadlines and one report, and the query's own program adds one handling deadline, so an answer that comes is never thrown away. The end-to-end suite's step limits are 75 seconds, above the same worst step.

### The desktop's return and name (Dickson, 2026-10-29)

The listener hooks the desktop switch and, scoped to csrss's process, the desktop window's name change. A return to the user's desktop asks the application in front for its focus again 200 ms later, taken without the duplicate rule, as NVDA's handler of the user's desktop does; the virtual desktop's name is read on csrss's reading thread and said before the next focus's entered ancestors, or alone 250 ms after the change, as NVDA says it.

### The outpost's own element (Dickson, 2026-10-10 and 2026-10-21)

The outpost finds its own copy of a UI Automation focus by the path the listener sent, one remote operation down the chain's runtime ids from the nearest ancestor with a window. A focus with no text is reported at once and found after; one whose role may have text is found before the report, since its caret and line come in the focus's message (Dickson, 2026-10-21). An element the outpost cannot find is announced from the listener's read, and review and navigation start from its nearest found ancestor (Dickson, 2026-10-10).

### Focus-following subscriptions (Dickson, 2026-10-11)

One focus-following group per outpost, built once, added to the new focus's element and then removed from the previous one, never registered on a window's subtree, as NVDA follows the focus. Reason: moving a registration by removing everything and adding it again reached two UI Automation defects, a crash on a saved event and a text area that stopped receiving events.

### Time limits (Dickson, 2026-10-10; amended 2026-11-06)

UI Automation's connection and transaction timeouts are process-wide, so each process sets them once at its start: 2 seconds and 10 seconds, the transaction timeout below the outpost's 11-second handling deadline, so that a slow call fails and the worker carries on instead of being abandoned. Every thread others wait on has a watchdog.

### Spare outposts (Dickson, 2026-10-16)

Two outposts are kept started and waiting, and one is given to an application when the supervisor would otherwise start an outpost for it; a replacement starts the moment one is given or fails, and an outpost once given never returns. Reason: starting an outpost takes about 40 ms before it can take its first event, at the threshold of being noticed. Verbatim's readiness waits for the two spares, so every scenario's applications take a spare. Every outpost failure takes one path, whatever the cause; nothing is ever given up, and repeated failures are spaced by an exponential backoff up to 30 seconds, started again once an outpost has stayed up or its application comes to the foreground.

## Limits accepted

- A focus whose application hangs past the deadline is lost, and nothing marks it.
- A walk whose application is slow past one UI Automation timeout, but not reported hung, loses its focus at the outpost's deadline, where NVDA waits for every call.
- A focus UI Automation never delivers is lost, as in NVDA.
- A focus whose element is gone and whose window cannot be read is lost, as in NVDA.
- UI Automation's title-bar proxy answers a navigation that passes its 2-second connection timeout as no element and no error (observed under parallel test load, when reading the title-bar buttons' state waited on the Desktop Window Manager), so a walk over a window with a title bar can miss its title-bar buttons without any sign of it.
- After a return between top-level windows, a focus whose report would speak a cached ancestor costs a live re-read: the reader's partial walk, one round trip and a full walk, where NVDA pays only the full walk. While one live re-read waits, every further focus event of the returned element asks its own, so a window that raises two focus events for its control on a return costs two.
- A report set aside for a live re-read whose element is gone by the re-read is not spoken, and an answer from a window the user has left by then is refused.
- A container renamed while the focus was elsewhere is caught only by Core's check, at the cost of a live re-read.
- A window above the process boundary is a stand-in described from Windows' state, spoken by its title.
- A window left for a window owned by another process's window, whose own focus never arrives, re-enters the owner on the return, where NVDA would not.
- The "not responding" sound, which NVDA does not have, plays only when the application holds attention, and while a listener read waits a hang is told only when the hung window is in front.
- One application's later events wait behind its slow read, up to the deadline each, and one burst can wait about 39 seconds behind abandoned reads; other applications do not wait.
