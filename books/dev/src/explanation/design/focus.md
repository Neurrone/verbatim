# How focus works

This page explains how a focus change travels from an application to speech: which process does each step, what NVDA does at the same step, and what happens when an application is slow or hung. Why it works this way, with the options weighed and the limits accepted, is [ADR 0003: Readers tell, Core asks](../../reference/adr/0003-readers-tell-core-asks.md), which amends [ADR 0001: Focus](../../reference/adr/0001-focus.md). Every rule, with NVDA's source for each, is in [build 1's design](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks-build1.md); the mechanisms are in the crate guides named at each step.

## The shape, and why it is forced

Three processes take part in every focus:

- The focus listener, `verbatim-listener.exe`, holds every desktop-wide registration: the UI Automation focus handler and global event group, and every `WinEvent` hook but a console host's. It tells Core of each event it hears, and reads a UI Automation focus only when Core asks, from the element it kept with the event. A check thread per application makes, as each UI Automation event is delivered, the calls NVDA's handler makes for it, and the tell carries their results.
- One outpost per application makes every call into that application on one worker, one request at a time, only when Core asks: it reads an MSAA focus, completes the listener's read of a UI Automation focus, reads text and the caret, and follows the property changes of the objects Core names.
- Core, in `verbatim.exe`, makes no call into an application. Every tell, key and command joins its one event queue; it holds every rule that decides what is spoken, the acceptance test included, and the one focus.

The shape follows from two decisions of record and four measured UI Automation facts:

- D9: a call into an application cannot be cancelled, so each application gets an outpost whose stuck thread can be abandoned, and the process ended if it stops answering.
- D13: the process holding the focus registration is a separate process, the listener, because of the third fact below.
- An element cannot leave the process that received its event (`books/dev/src/reference/uia-bugs.md`, "Elements cannot be passed to another process"), so the listener keeps the element and makes the first read of a UI Automation focus.
- A focus registration is desktop-wide; the scoped form is undocumented and misses proxied windows ("Focus events cannot be limited to one application"). So one process receives every application's focus events.
- A hung window named in a focus `WinEvent` holds every UI Automation event delivered to a process registered for focus ("A hung window named in a system focus event holds a process's UIA events"). So the process holding the registration holds nothing else whose stall would be heard.
- Two Verbatim processes connecting to one application collide inside UI Automation ("A new connection fails while another process's remote operation is queued"). So only an application's own outpost connects to it, and the listener calls only elements it already holds.

NVDA does all of this in one process, with one ordered event queue, one focus object and one last queued focus. Verbatim keeps all three in Core: the readers tell it of each event in the order they hear them, and nothing is read before Core has seen the event in its order and asked.

## Terms

These are defined in `books/dev/src/reference/glossary.md`:

- **Tell:** a message a reader sends Core when an event arrives, carrying what came with the event and reads of Windows' own state that cannot hang; a UI Automation event's tell also carries its check thread's results.
- **Check thread:** the listener's thread, one per application whose UI Automation events it hears, that makes NVDA's delivery-time calls before the tell is sent.
- **Core's inbox:** where everything arrives on Core's thread, in arrival order; what NVDA handles outside its queue is handled there at once.
- **Event queue:** Core's one ordered queue of events, keys, commands and the entries Core makes itself; only its front entry may have a request out.
- **Committed focus:** the focus Core has executed, NVDA's focus object.
- **Readers' last focus:** the newest focus a reader read that Core executed, NVDA's last queued focus; empty at startup and after the foreground leaves for an unrelated top-level window.
- **Ancestors Core already knows:** the committed focus and its ancestors, sent with a focus request while the readers' last focus is set, except what an ended outpost generation read; a reader's walk up the ancestor chain stops at the first of them it reaches. A walk that met one the committed focus's line no longer holds when its answer arrives is not completed: the application's focus is read afresh.
- **Due time:** when Core stops waiting for a request: 13 seconds for a focus read's whole chain, 11 for any other, from when Core decides to ask.

## One UI Automation focus, end to end

1. **The event arrives in the listener.** The callback drops it only by NVDA's local checks that cannot hang, such as a sender whose own window is hung, keeps the element, and hands the event to its application's check thread. NVDA's counterpart: `UIAHandler`'s `IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent`, on its UI Automation thread.
2. **The check thread makes NVDA's delivery-time calls.** It walks to the element's nearest window, probes that window for a native UI Automation provider when no verdict is in force, reads the element's keyboard focus and that of each element the listener keeps under the same runtime id, and reads the facts the acceptance test needs about the nearest window and the foreground. It judges nothing. The tell goes to Core with those results and the element's cached properties. A hung application stalls only its own check thread. NVDA's counterpart: the rest of the same handler, with `isNativeUIAElement` and `shouldAllowUIAFocusEvent`.
3. **Core queues and judges it.** The tell joins the event queue behind every tell, key and command that arrived before it. At the queue's front Core judges it on the tell's facts, with no call: NVDA's native-element test with the window's backend verdict, NVDA's four focus overrides (the console window's own element, File Explorer's task view frames, Excel's panes, nameless Office menu items), the acceptance test against the foreground record, the keyboard-focus answer, and the duplicate test against the readers' last focus, the rows of [the one definition of focus validity](focus-validity.md). A focus that fails any of them ends unspoken. Mechanisms: `books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects".
4. **Core asks the listener to read it.** The request names the element's token, the properties to read, and the ancestors Core already knows. The listener's reading thread for that application reads the element and walks its ancestors until it reaches one Core knows, the process boundary or the top, and answers a snapshot and the element's route. NVDA's counterpart: `eventHandler.doPreGainFocus` and `api.setFocusObject`, which walk the parents until they meet NVDA's focus ancestry.
5. **Core asks the outpost to complete it.** Under the same due time, Core sends the route to the application's outpost. The worker finds its own copy of each element on the route, registers the property changes of the focus and its ancestors before it reads the focus's states, goes on from where the listener's walk stopped (at a window it reads through MSAA it switches to that window's MSAA client, as NVDA's `correctAPIForRelation` does; above the process boundary it describes each window from Windows' state, with no call into the other process), reads the caret's line and a dialog's text, and answers the focus with the node ids it minted. Mechanism: `books/dev/src/reference/crates/verbatim-outpost.md`.
6. **Core executes and speaks it.** Before it takes a focus inside the committed focus, Core judges the committed focus's waiting state changes (the narrow rule), so a combo box's "expanded" is heard before the item focused inside it. It then commits the focus, cuts speech when the new ancestry differs from the old just below the desktop, speaks the entered ancestors outermost first, the window's title when the readers' last focus was empty, then the focus and its line, and tells the outpost which objects to follow. NVDA's counterpart: the speech of `doPreGainFocus` and the focus object's `event_gainFocus`.

## One MSAA focus

The listener's desktop-wide hook hears the MSAA focus and foreground events of every application and tells each to Core. A foreground tell is sent only once `GetForegroundWindow` names its window, holding the tells behind it for up to 20 ms, as NVDA defers its pump. In the event queue NVDA's limiter runs on the waiting MSAA entries (a foreground tell removes a waiting focus of its window, the newest four focus entries and the newest ten others per thread are kept), and each run of consecutive focus and foreground tells is tried newest first across applications: Core refuses what it can judge with no read, then asks each remaining candidate's own outpost about that candidate alone. The worker acquires the object from its event's ids with NVDA's child redirect, skips a window read through UI Automation, reports the focused state over the object and its ancestors, and reads it in full, walking `accParent` to an ancestor Core knows or the top, only when it is focused (a foreground always). Core judges the answer and goes on to the next older candidate when it is refused; it executes and speaks the first valid focus as in step 6. How each candidate is judged, here and while a caret key waits, is [Focus validity](focus-validity.md). NVDA's counterpart: `IAccessibleHandler.winEventToNVDAEvent` and `processFocusWinEvent`.

## Worked examples

### A move within a window

Tab moves from one button of a dialog to the next. The listener's check thread reads the new button's keyboard focus and nearest window; Core accepts the tell and asks for the read with the committed button and its ancestors as the ancestors it already knows. The walk meets the dialog's pane at its first step above the new button and stops. The lowest common ancestor is the pane, nothing is entered, and Core says only the new button.

### A return to an application

The user is in Windows Terminal, switches to Notepad, and switches back with Alt+Tab.

- The listener's foreground tell for Notepad reaches Core's inbox. Core updates its foreground record at once and stops waiting for any request whose window is outside Notepad's window. At the foreground tell's turn in the queue, Notepad's window is unrelated to the committed focus in the terminal, so the readers' last focus is emptied.
- Notepad's focus is read with no ancestors Core already knows, so the walk goes to the top, and Core says the window's title and the focus.
- The same happens on the switch back: the terminal's focus is read to the top, and Core says "<title> window" and the control, with the title as it is now, as NVDA does, since its focus ancestry is in the other application.

### A window owned by another process

A UI Automation window in one process opens a dialog that a second process creates, owned by the first window (the scenario `owned_dialog_across_processes`).

- The dialog's foreground tell is related to the committed focus's window by its owner, so the readers' last focus stays set.
- The dialog's outpost completes the dialog's focus read. Above its process boundary it describes the owner window from Windows' state, as a stand-in, which equals any entry with that window's handle. The two ancestries meet at the owner window, which is not entered again, and the title rule adds no title when the ancestries meet at an object they share. Core says "<dialog title> window" and "OK button", as NVDA does.
- When the dialog closes, the owner raises the focus of its button again; the ancestries meet at the owner window, and Core says only "Open button", as NVDA does.

### A hung application

A text field has just taken the focus when its application stops answering for 8 seconds (mockapp's `stall 8000`).

- The listener's callback finds the window not yet hung and tells Core. Core's shell checks the window before sending the read, finds it answering, and sends it; the listener's reading thread waits for the application.
- Every half second while the read is out, the shell asks Windows again whether the window is hung. Once Windows reports it hung, after about 5 seconds, Core stops waiting: it tells the listener to cancel, leaving its thread inside the call, plays the "not responding" sound because the application holds attention, and takes the next entry, so keys behind the read are freed. The window joins Core's recovery set.
- The half-second check goes on with the window. When Windows no longer reports it hung, Core asks the application's outpost for its current focus and judges the answer as a re-take: the text field is not the committed focus, so Core says "Notes edit Hello world", the focus and its line together. The listener's own read, finishing late, is dropped by its request's number.

Had the window never been reported hung, Core would have waited for the read up to its due time, 13 seconds.

## Slow and hung applications

A focus read is complete or nothing: Verbatim waits for a slow application as NVDA does, and says nothing for it until it answers. Only Core stops waiting, and only for these:

- The live hang check. Core's shell asks Windows whether a request's window is hung before it sends the request, mapping a ghost window to its hung window, and answers a request to a hung window without sending it: a read of an application's current focus with the window in front, read live, any other "not responding"; it asks again every half second while the request is out. There is no stored mark, so a hung window does not stop requests to the application's other windows.
- The due time: 13 seconds for a focus read's chain of requests, 11 for any other, counted from when Core decides to ask, so time spent waiting for an outpost to start counts. A window that is slow but never reported hung holds the queue for the whole due time.
- A foreground change: a request whose window is outside the new foreground window's tree and owner chain is stopped at once, and a waiting entry whose request would name such a window ends. Toasts, help balloons and tooltips, MSAA alerts accepted as topmost and desktop-window events are exempt, so a toast waiting behind a slow read is spoken after an Alt+Tab, as NVDA speaks it.

On a stop Core tells the reader to cancel, with the reason. A request not yet started is dropped. Only when the foreground moved away does a reader abandon a thread still inside the call and start a fresh one, since a fresh thread would stick in a hung or slow window. Every late answer is dropped by its request's number. A window Core stopped waiting on, or refused, as hung joins the recovery set; while its application holds attention, Core asks for that application's focus again when the window answers, as NVDA reads its focus live once its call returns. The "not responding" sound plays once for each request Core refuses or stops waiting for as hung while its application holds attention.

A read of the focus NVDA makes itself, the check after a menu or popup end, the read at startup and the read after the return from the secure desktop, is the exception. When the shell refuses it as hung and finds the ghost of the hung window in front, Core commits NVDA's plain window object of the ghost as the focus, through the ordinary commit, and speaks it, "hungwin (Not Responding) window", with no sound, watching the hung window; with a responsive window in front it says nothing. A menu, popup or switcher end whose check finds no window in front, as when the menu closes over a hung window with no ghost and Windows activates nothing, focuses the desktop, "Desktop window", as NVDA's fake focus does. The plain window's reader is Core's own plain reader, which answers every request on it at once with what NVDA's plain window gives, so every command treats it through its own path and nothing calls into the hung application. When the application answers again, its window is spoken.

Inside a reader, a UI Automation call is bounded by its 10-second transaction timeout; a walk step that fails goes on from the parent window of the outermost entry's nearest window, by that window's own object, as NVDA's `Window._get_parent` does. An application whose abandoned listener reading threads reach the cap of two gets no new thread until one returns.

Mechanisms: `books/dev/src/reference/crates/verbatim-core.md`, "Core's requests"; `books/dev/src/reference/crates/verbatim-listener.md`; `books/dev/src/reference/crates/verbatim-model.md` for the limits.

## A dialog read once it is shown

A dialog's name and text are read only once it is shown (Dickson, 2026-11-19). A common file dialog, a Run dialog or a property sheet is often brought to the foreground and given its focus while still hidden, and shown 14 to 221 ms later; read while hidden, it gives no text, since every static child it gathers its text from is hidden too.

- **The outpost reports a fact.** A focus read that Core asks to check (an MSAA focus's or foreground's read, a read of the current focus, a UI Automation focus's completion) first takes its window's top-level window and whether it is visible, two local calls. Only when it is hidden does the outpost read the role of its client object, two calls into the application; a dialog's role (a dialog, an alert or a property page) answers that window alone, and nothing more is read. A role read that fails answers "not there", never "not a dialog".
- **Core asks the listener to tell it when the window is shown.** The listener's WinEvent thread reads the window's visibility as the request arrives, answering at once when it is shown or gone, and otherwise keeps a waiter, answered by its hook's own window-object show or destruction, whatever the window's class. No show reaches Core but the answer, and no thread is woken that the hook does not already run on.
- **Core reads the same focus again from its start**, under the same due time, and judges, commits and speaks it as any focus: the dialog with its text, then its field.

The wait ends early when the user is elsewhere, a condition on the queue that holds whenever the input making it true arrived, so it stops a wait from beginning as well as ending one: the foreground record naming any window other than the dialog, owner and owned windows included; a menu or popup start outside the dialog, accepted, from the application holding attention; or a focus outside the dialog that the rows of [the one definition](focus-validity.md) needing no read accept. Nothing is then said for the hidden dialog, and the entries behind are judged as they came: an owned message box is spoken at once, a dialog closed by hiding leaves the owner's focus to be spoken, a tray menu over a hidden window is spoken. A focus that ended a wait carries the dialog with it, through a merge too; should its run then execute nothing while the dialog is still in front, Core reads the current focus as a re-take, which waits for the show in turn.

A key pressed while the dialog is hidden ends the wait too, but does not skip the dialog: Core reads the focus at once, hidden, and commits and speaks it, the dialog without its text and then its field, before the key is handled, so the key is judged against that focus and its cut silences what remains. When a key and the user being elsewhere both hold, the user being elsewhere wins.

No focus or foreground event is ignored for being hidden, as NVDA ignores none: only the read is postponed. The added silence is the time left until the show, up to 221 ms for Verbatim's own Import a Theme dialog, plus one round trip to the listener; a dialog shown at its foreground waits for nothing. A dialog never shown, with no key and no other ending, holds the queue until the due time, 13 seconds, then ends unspoken. Mechanisms: `books/dev/src/reference/crates/verbatim-core.md`, "A dialog read once it is shown"; `books/dev/src/reference/crates/verbatim-listener.md`; `books/dev/src/reference/crates/verbatim-outpost.md`.

## Verbatim+Tab

Verbatim+Tab is an entry of the event queue like any key. At its turn it reads the committed focus's node live in one request and speaks it as NVDA+Tab speaks its focus object: the object, then its line. The answer never becomes Core's focus. A second press spells its name, and a third spells it with character descriptions. Each press waits its turn, so a press made before a focus arrived reads the old focus, as NVDA's script does. With no committed focus it says "No focus" and asks nothing; any outcome but an answer says nothing. Mechanism: `books/dev/src/reference/crates/verbatim-core.md`, "Verbatim+Tab".

## Differences from NVDA

The parity ledger records each difference with its reason: `docs/parity/focus-events.md` (event acceptance, duplicates, slow and hung applications), `docs/parity/focus-context.md` (the focus ancestry, the window title, identity above the process boundary) and `docs/parity/review.md` ("Report focus"). The ones a user can notice:

- Keys, commands and typed characters behind a frozen window wait until Windows reports it hung, the due time passes, or the foreground leaves the window. Held caret keys still reach the application 150 ms after their press.
- A window above the process boundary is described from Windows' state, so a host NVDA reads through UI Automation is said by its title alone where NVDA says "<title> window".
- The "not responding" sound, which NVDA does not have.
- A dialog focused while it is hidden is read once it is shown, so its text is said with it; NVDA reads it as the events come, and says its text only when it happens to read it after the show.
- The readers' last focus is emptied when the foreground leaves for an unrelated top-level window, which NVDA keeps: a return to an application says the window's title and the focus.
- A state change of the old focus that reaches Core after a focus elsewhere is not said: the listener's and an outpost's events come by two routes with no order between them, and the narrow rule repairs only the case of a new focus inside the old one.
