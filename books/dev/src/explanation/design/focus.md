# How focus works

This page explains how a focus change travels from an application to speech: which process does each step, what NVDA does at the same step, and what happens when an application is slow or hung. Why it works this way, with the options weighed and the limits accepted, is [ADR 0001: Focus](../../reference/adr/0001-focus.md). The mechanisms are in the crate guides named at each step.

## The shape, and why it is forced

Three processes take part in every focus:

- The focus listener, `verbatim-listener.exe`, holds Verbatim's one desktop-wide UI Automation focus registration and its desktop-wide `WinEvent` hooks. It reads every UI Automation focus, on a reading thread per application.
- One outpost per application holds that application's own process-scoped `WinEvent` hook and its UI Automation registrations. It reads every MSAA focus, finds its own copy of each UI Automation focus the listener read, and reads text, caret and everything else the user asks for.
- Core, the reducer in `verbatim.exe`, makes no call into Windows or any application. It holds the one focus ancestry and decides what is spoken.

The shape follows from two decisions of record and four measured UI Automation facts:

- D9: a call into an application cannot be cancelled, so each application gets an outpost whose stuck thread is reclaimed by ending its process.
- D13: the process holding the focus registration is a separate process, the listener, because of the third fact below.
- An element cannot leave the process that received its event (`books/dev/src/reference/uia-bugs.md`, "Elements cannot be passed to another process"), so the process that receives a UI Automation focus event must judge and read it.
- A focus registration is desktop-wide; the scoped form is undocumented and misses proxied windows ("Focus events cannot be limited to one application"). So one process receives every application's focus events.
- A hung window named in a focus `WinEvent` holds every UI Automation event delivered to a process registered for focus ("A hung window named in a system focus event holds a process's UIA events"). So the process holding the registration must hold nothing else whose stall would be heard.
- Two Verbatim processes connecting to one application collide inside UI Automation ("A new connection fails while another process's remote operation is queued"). So only an application's own outpost connects to it, and the listener calls only elements it already holds.

NVDA does all of this in one process, with one ordered event queue and one focus ancestry. Verbatim keeps NVDA's order within an application with a marker (below), and NVDA's queue and single ancestry in Core, which takes the applications' reports in the order they reach it.

## Terms

These are defined in `books/dev/src/reference/glossary.md`:

- **Focus ancestry:** Core's ordered list of the focus's ancestors and the focus, from the desktop down.
- **Reported ancestry:** the ancestors a focus report carries.
- **Focus reader:** one source of focus reports for an application: the listener's reading thread, or the outpost's own reads.
- **Ancestry cache:** a focus reader's saved ancestors from its last read in an application, used to stop a later walk early.
- **Cached ancestor:** an entry of a reported ancestry taken from the ancestry cache rather than read live.
- **Cache join:** the entry where a focus reader's walk met its ancestry cache and stopped.
- **Lowest common ancestor:** the innermost entry a new reported ancestry shares with Core's focus ancestry.
- **Entered ancestors:** the entries of a new reported ancestry below the lowest common ancestor, which Core speaks as entered.
- **Title check:** a focus reader's comparison of the window titles saved in its ancestry cache with their live text, before it uses the cache.
- **Live re-read:** Core setting aside a report that would speak a cached ancestor, and asking its focus reader to read the same element again with no ancestry cache.
- **Focus marker:** the message the listener sends an application's outpost the moment a UI Automation focus event arrives; the outpost holds every later entry of that application behind it until the read arrives.

## One UI Automation focus, end to end

1. **The event arrives in the listener.** The listener's callback makes NVDA's arrival test, whether the sender's own window is hung, by a local call, and makes no other call. It sends the application's outpost a marker through `verbatim.exe` and queues the sender for the application's reading thread. If the application has no outpost yet, the marker starts one, usually by giving it a spare outpost. NVDA's counterpart: `UIAHandler`'s `IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent`, on its UI Automation thread.
2. **The listener judges it.** The reading thread makes NVDA's checks before a read, in NVDA's order, as one pure step function (`verbatim_listener_logic::checks::next`): whether the sender's nearest window is native UI Automation, by NVDA's MSAA class list; whether it repeats the element read before, by `CompareElements` and that element's keyboard focus; the acceptance test against the foreground, on the nearest window; the console window's and Verbatim's hidden frame's refusals; and the sender's own keyboard focus. A check that fails or times out lets the focus through, as NVDA's handling of a COM error does. NVDA's counterpart: the rest of the same handler, and `shouldAllowUIAFocusEvent`.
3. **The listener reads it.** It reads the sender's properties and walks its ancestors, one call per ancestor or one remote operation. The walk stops early where it meets the ancestry cache, once the title check finds every saved window title unchanged; a changed or gone title makes the walk go live to the top or the process boundary. Every call waits for the application, up to UI Automation's own 10-second timeout per request. The read goes to the outpost as `UiaFocusRead`, through `verbatim.exe`. NVDA's counterpart: `eventHandler.doPreGainFocus` and `api.setFocusObject`, which walk the parents live until they meet NVDA's focus ancestry.
4. **The outpost completes it.** The read takes the marker's place in the outpost's intake, so the application's later events are handled after it, as NVDA's one queue orders them. The worker goes on from where the listener's walk stopped: at a window it reads through MSAA it switches to that window's MSAA client, as NVDA's `correctAPIForRelation` does, and above the process boundary it describes each window from Windows' state as a stand-in, with no call into the other process. It finds its own element by the path the listener sent, reads the caret's line and a dialog's text, and reports the focus with `first_cached_ancestor`, the index of its innermost cached ancestor, computed from the entries it emits. Mechanisms: `books/dev/src/reference/crates/verbatim-outpost.md`, "How focus reaches an outpost" and "Reading the focus".
5. **Core judges and queues it.** Core keeps NVDA's two facts: the focus object, the focus last executed, and the last queued focus, the newest focus it accepted. It drops a report from another outpost generation observed after which Core accepted another report into its order (the stale rule; a whole-window report of the report's own window does not count), and tests the report's window against its foreground record. A report it keeps is queued behind those not yet decided. At the queue's front, every report ahead of it decided, Core judges it against the last queued focus: whether a foreground change left that focus for another window (the left mark), whether a whole-window focus is of that focus's window, and whether the report repeats it (NVDA's duplicate rule). It then computes one plan (`plan_focus`) against the focus object: the lowest common ancestor, the entered ancestors, and the window title the title rule would add. If the plan would speak a cached ancestor, as entered or as the title (`focus_rules::needs_live_ancestry`), the report waits at the front for a live re-read, and every report after it waits behind it.
6. **Core executes and speaks it.** Each report at the front of the queue that is ready is executed against the focus object. It cancels speech when the new ancestry differs from the old just below the desktop, speaks the entered ancestors outermost first, the title when the title rule adds one, then the focus and its line. The outpost follows the newest focus it reported: its focus-following subscription moves to the element, and its text and caret are read from it. NVDA's counterpart: the speech of `doPreGainFocus` and the focus object's `event_gainFocus`. Mechanisms: `books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects" and "The ancestry cache".

## One MSAA focus

An MSAA focus never passes through the listener once the application's outpost has its hook in; until then the listener forwards the application's `WinEvent`s and the outpost's hand-over drops the copies its own hook also delivered. The outpost's intake applies NVDA's event limiter and tries each run of consecutive focus and foreground events newest first, at its place among the events around it, as NVDA's pump does. The worker acquires the object from the event's address, makes NVDA's focused-state test and child redirect, walks `accParent` to the top, meeting its own ancestry cache as the listener does, and reports. Core then plans and speaks as in steps 5 and 6. NVDA's counterpart: `IAccessibleHandler.winEventToNVDAEvent` and `processFocusWinEvent`. Mechanism: `books/dev/src/reference/crates/verbatim-outpost.md`, "The hand-over of an application's MSAA events" and "Event flow".

## Worked examples

### A move within a window

Tab moves from one button of a dialog to the next. The listener's walk from the new button meets its ancestry cache at the dialog's pane, and the title check finds the dialog's title unchanged, so the walk stops there and the report carries the dialog and everything above it as cached ancestors. Core's focus ancestry holds the same dialog, so its lowest common ancestor is the pane, below every cached ancestor: nothing cached is entered, the title rule adds nothing, and Core says only the new button. No live re-read happens, and no call is made for the ancestors above the pane, which is where the ancestry cache saves calls.

### A return to a window, with the title check and the live re-read

The user is in Windows Terminal, switches to Notepad, and switches back with Alt+Tab.

- The listener's foreground change for the terminal reaches Core first. The terminal's window is unrelated to Core's focus in Notepad, so Core marks its focus left, for the terminal's window.
- The terminal raises a focus on its control. The listener's walk meets its ancestry cache at the terminal's window.
- If a program changed the terminal's title meanwhile, the title check sees it, and the walk goes live to the top. The report has no cached ancestor, and Core says the new title, "<title> window", then the control, as NVDA does.
- If the title is unchanged, the report carries the window as a cached ancestor. Core's focus ancestry is in Notepad, so the lowest common ancestor is the desktop and the window would be entered: Core sets the report aside and sends `CoreToListener::LiveReread` for it. The reading thread reads the element it holds for that read again, by the classic walk with no ancestry cache, and the outpost completes the answer without its own cache. Core executes the answer in its place and says "<title> window" and the control. A focus the terminal raised meanwhile waits behind it and is spoken after it, against it.

NVDA walks the whole ancestry live on such a return, because its focus ancestry is in the other application. Verbatim says the same words. A live re-read happens on returns between top-level windows whose titles did not change; moves within a window never need one, since their cached ancestors lie above the lowest common ancestor. A window that raises two focus events for its control on a return can cost two live re-reads, one for each. Core counts every live re-read, and `verbatim-inspect` prints the count.

### A window owned by another process

A UI Automation window in one process opens a dialog that a second process creates, owned by the first window (the scenario `owned_dialog_across_processes`).

- The dialog's foreground change marks Core's focus left, for the dialog's window.
- The dialog's outpost reads the dialog's focus. Above its process boundary it describes the owner window from Windows' state, as a stand-in, which equals any entry with that window's handle. The report comes from the window the focus was left for, so the left mark ends and Core compares the two ancestries as they are: they meet at the owner window, which is not entered again, and the title rule adds no title when the ancestries meet at an object they share. Core says "<dialog title> window" and "OK button", as NVDA does.
- When the dialog closes, the owner raises the focus of its button again. Its report comes from the window the focus was left for, the ancestries meet at the owner window, and Core says only "Open button", as NVDA does.

NVDA reads the owner through its own API, since it is one process; Verbatim's stand-in gives the same meeting point without a call into the owner's process.

### A hung application

A text field has just taken the focus when its application stops answering for 8 seconds (mockapp's `stall 8000`).

- The listener's callback finds the window not yet hung, and sends the marker. The reading thread's calls wait for the application.
- The outpost holds the application's later entries behind the marker. From half a second after the marker arrived, and every half second while it waits, the outpost checks the window in front by local calls (`Effect::CheckFront`): a ghost window is mapped to its hung window, and a window of this application that Windows reports hung is told to Core by `NotResponding`, once for the hang, and Core plays the "not responding" sound only when the application holds attention.
- Nothing else is said while the application does not answer.
- When the application answers, the read arrives, the outpost reads the caret, and Core says "Notes edit Hello world" once, the focus and its line together, as NVDA does when its main thread returns.

Had the stall lasted past the focus read deadline, 13 seconds, the reading thread would have been abandoned and the focus lost.

## Slow and hung applications

A focus read is complete or nothing: Verbatim waits for a slow application as NVDA does, and says nothing for it until it answers. Three things end a read early, and only these:

- UI Automation's transaction timeout, 10 seconds per request (`UIA_TRANSACTION_TIMEOUT`). A walk step that fails by it, or by any error, goes on from the parent window of the outermost entry's nearest window, by that window's own object read live, as NVDA's `Window._get_parent` does. A window read through UI Automation whose element cannot be read loses the focus, as NVDA's unguarded `ElementFromHandleBuildCache` ends its event; an MSAA client that cannot be acquired is described from Windows' state.
- One deadline per thread: `FOCUS_READ_DEADLINE`, 13 seconds, for a listener read, an outpost's focus query (`Query::ReadFocus`) and the outpost's completion of a listener read; `HANDLING_DEADLINE`, 11 seconds, for every other outpost program. Past it the thread is abandoned and its work is lost. An MSAA call and a window message have no bound but the deadline. An application whose abandoned listener threads reach the cap of two gets no new thread: its focus events wait for one to return, up to one more deadline, and are then dropped as they arrive until one does.
- The moved-on rule, which NVDA's watchdog also has (`watchdog._shouldRecoverAfterMinTimeout`): from half a second after a focus read starts, and every half second, the read is abandoned once the window in front is the same application's on another UI thread. A tree dump and a shell items query are not about the focus, so they end only at their deadline, and their window is still checked for a hang.

While the listener's read waits, the outpost's marker holds that application's later events, and only that application's: other applications are not delayed. One burst of an application's focus events can wait about 39 seconds behind two abandoned reads and the cap's wait. A query held behind the marker, such as the taskbar's list of running applications when the taskbar and a folder window share one `explorer.exe`, has one clock, the outpost's answer-by time of 63 seconds, set when the query is asked.

The not-responding notice is the outpost's, kept as one value: no hang told, or told of one window. Two checks find a hang, both local calls and both on one timer of the outpost's reducer: the half-second check of a program in flight, against the program's window, and the front check while a marker waits. A commit naming a window hung counts only once a local check confirms it. One hang is told per application: a second window hung while the told window is still hung is the same hang. While a hang is told, the told window is judged by a local call on each input that takes an entry, applies a commit or starts a program; found not hung, or readable in front, or destroyed, its hang ends, so a window still hung then, or its own later freeze, is a new hang and is told again. The notice abandons nothing.

Mechanisms: `books/dev/src/reference/crates/verbatim-listener.md`, "How a UIA focus is judged and read"; `books/dev/src/reference/crates/verbatim-outpost-logic.md`; `books/dev/src/reference/crates/verbatim-model.md` for the limits.

## Verbatim+Tab

Verbatim+Tab asks the application for its focus live, so it reports the real focus even when the focus moved with no event. The app sends `Query::ReadFocus` with the purpose `Report` to the outpost of the process that serves the foreground thread's focus window (`GetGUIThreadInfo`, else the active window, else the window in front, a ghost mapped to its hung window), all local calls. The outpost reads the application's focus and its ancestry, and neither follows nor adopts the answer.

Core speaks the answer as NVDA+Tab speaks its focus object: the object alone, then its line. It never takes the answer as its focus, so its focus ancestry and later events are unchanged. A second press within the repeat interval while the first waits spells its name after it is spoken, and a third spells it with character descriptions; a press after the interval replaces the waiting one, whose answer is not spoken. A focus observed after the press ends the waiting press, so a focus already on its way when the key was pressed is spoken and then the answer. "No focus" is said when nothing can answer: no window in front, an ignored process, no outpost, the outpost's end, or an empty answer. In a hung application the query waits; abandoned at its deadline, it says nothing. Mechanism: `books/dev/src/reference/crates/verbatim-core.md`, "Verbatim+Tab".

## Differences from NVDA

The parity ledger records each difference with its reason: `docs/parity/focus-events.md` (event acceptance, duplicates, slow and hung applications), `docs/parity/focus-context.md` (the focus ancestry, the left mark, the window title, identity above the process boundary) and `docs/parity/review.md` ("Report focus"). The ones a user can notice:

- A hang longer than about 13 seconds right after a focus loses that focus, where NVDA speaks it when the hang ends. A walk step that times out twice in an application Windows does not report hung also loses the focus at the deadline.
- A window above the process boundary is described from Windows' state, so a host NVDA reads through UI Automation is said by its title alone where NVDA says "<title> window".
- The "not responding" sound, which NVDA does not have.
- A return between top-level windows can cost a live re-read: a partial walk, a round trip to Core and a full walk, where NVDA makes the full walk only. A focus raised behind it is spoken only after it.
- Focuses A, B, A from two applications in two top-level windows that reach Core as A, the repeat of A, then B say nothing after the first A, where NVDA says B then A.
- A focus in a window shared by two processes, a frame's control or the page's content, observed before the other process's ordinary focus report in that window but reaching Core after it, is not spoken.
- A focus from an application the user has just left that reaches Core after the switch is not spoken, where NVDA speaks it before the switch.
