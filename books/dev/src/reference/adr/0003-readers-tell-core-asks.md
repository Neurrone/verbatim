# ADR 0003: Readers tell, Core asks

Status: decided, 2026-11-13, by Dickson; the model and every ruling in it are his. Nothing in it is built yet: the phase "Readers tell, Core asks" in [the roadmap](https://github.com/Neurrone/verbatim/blob/main/docs/roadmap.md) builds it, and until each build lands, [How focus works](../../explanation/design/focus.md) and the crate guides describe the code as it is.

This record keeps why Verbatim's readers stop reading on their own and read only when Core asks: the problem, the decisions with their reasons, the options weighed, and the limits accepted. Every rule in full, with NVDA's source for each, the order of the builds and the measurements are in [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md).

## The problem

NVDA handles an event as one function on one thread: it reads the application, consults its own focus and its queue between reads, and speaks. Verbatim reads each application from another process, so a reader made its reads before Core had seen the event. Each feature then decided in the reader what to read, on a guess about Core's state; avoided reading too much with a cache of a fact that is Core's; and tied the read back to its cause afterwards with stamps, notes and markers. One interaction kept state in two or three processes, and each hand-off between them became a waiting state.

The cost shows in the fixes. Of 519 defect fixes in the month to 2026-11-10, 71 were bookkeeping of a waiting state, 40 were ordering across a hand-off, and 16 were a fact judged away from its owner. One caret key's progress is kept today by the keyboard hook, Core's shell, Core's reducer and the outpost; a UI Automation focus crosses a marker the outpost waits behind, a reader's ancestry cache, Core's check of that cache and a live re-read through two processes. The protocol has 42 kinds of message, most of them serving one feature's hand-off.

## Terms

- Reader: a process that calls into applications for Core and hears their events. There are two kinds: the focus listener, which holds every desktop-wide registration, and one outpost per application.
- Tell: a message a reader sends Core when an event arrives, carrying only what came with the event and reads of Windows' own state that cannot hang. A tell makes no call into the application.
- Ask: a request from Core to a reader. Every call into an application is made for one, and it gets exactly one answer.
- Core's inbox: the one place on Core's thread where everything arrives, in arrival order. Nothing waits there.
- The event queue: Core's ordered queue of application events, keys, commands and entries Core makes itself. Only its front entry may have a request out.
- The committed focus: the focus Core has executed, NVDA's focus object. The readers' last focus: the newest focus a reader read that Core executed, NVDA's last queued focus; it is empty at startup and after the foreground leaves for an unrelated top-level window.
- Due time: when Core stops waiting for a request's answer. Give up: Core stops waiting, answers the request in its own table, takes the next entry and tells the reader to cancel.
- Stop points: the committed focus's line of ancestors, sent with a focus request, where the reader's walk up the tree stops.
- Held object: something a reader keeps behind an id Core names, such as a node or a position in a text, released when Core releases it.

## Decisions

### Readers tell, Core asks (Dickson, 2026-11-12)

Every event a reader hears is told to Core in one shape, whatever its interface: what happened, the application and window, when, a reference to the object (a token for a UI Automation element the reader keeps, or an MSAA event's window, object and child ids), what arrived with the event, and the window facts the acceptance test reads. A reader drops an event only by NVDA's local checks that cannot hang, such as an invalid window or a hung one. Every call into an application is made for a request from Core; the request carries what it needs and gets one answer, with one of five outcomes: a result; refused, with a reason; the object is not there; not responding; or the reader ended or stopped. A reader keeps nothing for Core between requests but held objects, the requests waiting in an outpost's table and a terminal's "changed" flag, and judges nothing about them.

Reason: Core sees every event in one order before anything is read, so no reader guesses at Core's state and no read has to be tied back to its cause; the five outcomes are the five ways Core behaves on an answer (Dickson, 2026-11-13).

### Core holds every rule (Dickson, 2026-11-12)

Core holds every rule that decides what is spoken, written to read like NVDA's, and its thread never blocks on an application. An outpost does mechanical work on objects it holds, only on Core's request and with the objects named by id: read, compare a caret with a starting point, report how a selection differs from one Core names, move the caret.

Reason: NVDA's rules are defined on one order, which only Core has ([ADR 0002](0002-no-screen-reader-per-outpost.md)).

### Who holds what (Dickson, 2026-11-12; amended 2026-11-13)

- The listener holds every desktop-wide registration: the UI Automation focus handler and global event group, and every WinEvent hook, desktop-wide, as NVDA hooks them in one process. Reason: one desktop-wide hook keeps Windows' order of events across applications and removes the hand-over between two routes.
- An outpost holds no WinEvent hook, except a console host's outpost, which keeps the console's five events. It keeps a UI Automation subscription to property changes on the objects Core tells it to follow, at NVDA's scope of the element and its ancestors, and tells Core of those and of text and caret events. Reason: each kind of event has exactly one teller, so no event reaches Core twice by two routes with no order between them.
- An outpost has one worker making every call into its application, one at a time, abandoned and replaced only when Core cancels a request it is still inside because the foreground moved away from that request's window. Reason: a stuck cross-process call can only be abandoned (D9), and a fresh thread helps only when another window can use it.
- An outpost has a table of requests waiting on an event or a time, such as a caret key's watch, which Core can cancel. Reason: a request that has the outpost do work and return a result keeps the checks of a wait beside the application, so each check is not a message to Core and a wake of Core (Dickson, 2026-11-13: "we want to minimize core traffic").
- Objects an outpost holds sit behind ids Core names, each carrying the generation of the store that holds it, and are released when Core releases them; a release spares every id a waiting request names. Reason: elements and text ranges cannot cross a process, and an id means something only to the generation that made it.
- The in-process helper (M7) is part of its application's outpost: it talks only to that outpost, as security mitigation X8 of [the security model](https://github.com/Neurrone/verbatim/blob/main/docs/design/security-model.md) and the M7 rules say, and what it sends reaches Core as the outpost's tells and answers. Its presence is a fact of each application. Reason: the outpost is the validator of what a possibly compromised application sends.

### Properties, event kinds and requests (Dickson, 2026-11-13)

- Properties and event kinds are each declared once. A property's entry gives its name, its type and how each interface reads it. A read names a set of properties fixed at compile time; the cached request and the remote program are built from that set, so every field a read needs is cached and costs nothing extra, and the compiler checks that a rule uses only what its read asked for. Extensions name entries of the same table at run time.
- One request list is shared by Core's own features and the extension surface: properties of an object, navigation, find a descendant by property, and text reads. A new command adds no kind of request.

Reason: today a new property touches about twenty files in five crates, and a new command adds a request variant with its dispatch; declared once, a property is one entry, and a command is its gesture and its handler.

### Core's inbox and the event queue (Dickson, 2026-11-12; amended 2026-11-13)

Everything enters Core's inbox in arrival order. Handled at once: answers, timers that come due, foreground changes, a desktop switch, a change of the lock state, progress facts, readers starting and ending, window destruction (passed on to the outpost), a terminal's "changed" tell, and the speech cut and failures from the speech side. Every other tell, every key and command, index marks, say-all's reads, and entries Core makes itself (the focus read at startup, the check after a menu ends) join the event queue and are handled in arrival order. Only the front entry may have a request out. Reason: this is NVDA's one queue, so keys and events are judged against the focus NVDA would have.

Requests from Verbatim's own dialogs, the control plane and an extension host's reads stay outside the event queue, answered in the inbox and holding no key. Reason: a stuck tray list or a tool must not hold the keyboard, and a host's read must not wait behind the entry whose hook asked it.

An entry is either waiting or asked. The rules that read the state:

| What happens | Waiting entry | Asked entry |
| --- | --- | --- |
| It reaches the front | judged at once, or its request is sent and it becomes asked | not applicable |
| Its answer arrives with a result | cannot happen | judged; a follow-up keeps it asked; otherwise executed, and the next taken |
| The foreground changes | ends if its request names a window outside the new foreground window's tree or owner chain | the same test; failing it, given up |
| Its reader ends | dropped if the ended reader kept its element | ends, with that reason |
| An asked entry ends, with a reason (below) | not applicable | ends unspoken; the next is taken |
| A key or command arrives | the key joins the back and marks every caret key ahead of it silent | the same |
| Another entry for the same object and kind arrives | merged, the newest keeping its place | not merged |

The reasons, with what Core does beyond taking the next: an answer other than a result, nothing more; Core gives up (hung report, due time or foreground change), cancel sent; the send finds the window hung, nothing sent; the reader ends, the new reader asked for the focus. A hung window plays the not-responding sound.

### Keys and commands (Dickson, 2026-11-12; amended 2026-11-13)

- A key or command takes its place in the event queue behind what arrived before it and is handled in arrival order, with no time limit of its own. Reason: Verbatim matches NVDA; the escape is on the stuck read, not on the key.
- A key's action is chosen at the moment of the press, against the focus Core has then, and run later in the queue's order; the key's report from the hook carries that choice. Only actions bound to the focused object use the node chosen at the press; a global command resolves its object when it runs. Reason: NVDA binds a script at the press, and a gesture swallowed at the press must run the command chosen then.
- The keyboard stays alive: a caret key the hook holds is released to the application 150 ms after its press, and keys the hook passes reach the application at once. A caret key released by that timer says nothing, with no exception for the last key of a burst; a caret key the hook did not hold says nothing, as NVDA. Reason: a stuck or slow read must not hold the user's typing.
- A caret key's life is in three places, each with its own job: the hook holds it, Core decides, the outpost watches. The shell keeps no copy of the line of held keys; it sends a key when Core says so. Reason: one owner per fact.
- A caret key at its turn: Core asks for the caret, which the outpost holds as the key's starting point; sends the key on; and asks the outpost to watch for the key's effect within 100 ms. The watch polls in the outpost, as NVDA's loop does, at once and then about every 10 ms (every 5 ms in the console host), and ends on today's "had an effect" test. A later key or a pending focus cancels it, and the key says nothing. Reason: no outpost hears the caret events of classic edit controls, MSAA text or IAccessible2 text under this model, so the watch cannot wait on events alone.
- Selection keys are not held: Core speaks the difference the outpost reports between each caret answer's selection and the selection Core last spoke from. Reason: that is NVDA's rule for controls that detect selection changes.
- A move by sentence is one request the outpost runs whole.

### A frozen window (Dickson, 2026-11-12; amended 2026-11-13)

- Only Core stops waiting. It keeps one due time per request, or one for a command's whole chain of requests: 11 seconds, 13 for a focus read, counted from the moment Core decides to ask, so time spent waiting for an outpost to start counts. Core gives a request up on its live hung check, at its due time, or when the foreground changes and the request's window is outside the new foreground window's tree or owner chain. The foreground test is by window, so it covers another window of the same application, and acts at once. Reason: a frozen application must not hold the user's keys, which wait behind the front entry, and attention and waiting are Core's facts. NVDA's watchdog recovers by a like test half a second after the foreground changes (`watchdog.py` 206 to 232).
- One detector, live, as NVDA checks each time: Core's shell asks Windows whether a request's window is hung before sending it, and answers it "not responding" without sending if so; and asks again every half second while a request is out. There is no stored mark, so a hung window of one application does not stop requests to its other windows.
- "Hung" is judged in two places, both live and neither stored, as NVDA: the listener drops events from a hung window as they arrive, and Core's shell checks before and while a request is out. The outpost makes no check of its own before a call.
- On a give-up Core tells the reader to cancel, with its reason. A request not yet started is dropped. When Core gave up because the foreground moved away from the request's window, a reader whose thread is still inside the call abandons that thread and starts a fresh one, so a window that still answers has a working thread. When Core gave up because the window is hung or the due time passed, the thread is left alone: a fresh thread would stick in the same window, and the call's answer, when the window answers again, is the late answer below (Dickson, 2026-11-13). The readers keep no deadline of their own and no rule of their own for the user having moved on.
- A late answer to a focus Core gave up on joins the event queue as a fresh focus, unless the user has switched away, so the focus is spoken when the hang ends, as NVDA.
- A request to a window that is slow but not reported hung waits out its due time, with no special case.
- The not-responding sound plays once for each request Core refuses or gives up because its window is hung, while its application holds attention, with no record of having played.

### The two versions of the focus (Dickson, 2026-11-12; amended 2026-11-13)

Core keeps the committed focus and the readers' last focus as separate facts, as NVDA keeps its focus object apart from its last queued focus, and every rule says which of the two it judges: the duplicate tests, the whole-window refusal, a run's refusals and the system caret's target judge the readers' last focus, as NVDA's judge its last queued focus; commands, the narrow rule and typed echo judge the committed focus. Core can also commit a focus itself, with no reader having read it: the sleeping focus while the secure desktop is up now, and later an expression being explored in maths, an object a module makes, or a recognition result. Reason: one mechanism, stated once, serves every feature that needs a focus no reader read.

### Stop points and leaving a window (Dickson, 2026-11-12; amended 2026-11-13)

- The stop points of a focus walk are Core's committed focus's line, as NVDA walks until it meets its own focus line, and are sent only while the readers' last focus is set. There is no store of an earlier ancestry per application, so a return from another application walks to the top. Reason: nothing above the meeting point is reused, so no stale window title can be spoken.
- When the foreground leaves for a top-level window unrelated to the committed focus, the readers' last focus is cleared, in arrival order; the committed focus stays for commands and typed echo. With no readers' last focus, the duplicate tests, the whole-window refusal and the stop points behave as with no earlier focus, so a return to an application that committed no focus while the user was away says the window's title and the focus, as the decision of 2026-11-08 chose. There is no separate mark. Reason: the three effects the mark had are what happens when there is no earlier focus, so one fact serves.
- Verbatim's own windows have no exception: a foreground change into one is like any other, and they are read by Verbatim's own ordinary outpost. Reason: an exception put two requests out at once and needed a fact "own" read by several rules.

### Acceptance judged in Core (Dickson, 2026-11-12)

The acceptance test, which decides whether an event's window counts (the foreground's tree, a topmost window, the shell's own windows and so on), is judged in Core as a pure rule over facts about the event's window and the foreground, which the listener gathers in its callback at the event's arrival, as NVDA reads them, and which the tell carries. It is rewritten in Core from [the parity document on focus events](https://github.com/Neurrone/verbatim/blob/main/docs/parity/focus-events.md), not moved from the ported code, which is deleted. The listener judges nothing with these facts. Reason: one judge per rule, on the facts NVDA judges it on.

### A UI Automation focus read (Dickson, 2026-11-13)

A UI Automation focus is read in two ordinary requests chained through Core under one due time: the listener reads the element it kept and answers a snapshot and the element's route; with that answer Core asks the application's outpost to complete the read (node ids, an MSAA window above it, the chain above the process boundary). Reason: only the listener holds the element and only the outpost connects to its application, and two ordinary requests need no message or lifetime rule of their own.

### The narrow ordering rule (Dickson, 2026-11-12)

The listener's events and an outpost's UI Automation events come from two processes with no order between them. When a focus answer arrives whose ancestors include the committed focus, Core first judges the committed focus's waiting state changes, in their order, then takes the new focus. Reason: a combo box's "expanded" must be heard before the item focused inside it, as today.

### Extensions (Dickson, 2026-11-12)

- Core calls every hook; an outpost never calls one and never waits on a host.
- An extension's reads travel through Core as ordinary requests from the shared list, outside the event queue; Core's own requests to an outpost are served first, and an extension has one read outstanding at a time.
- A hook that holds the event queue gets a short bound counted from when Core sends the call; past it, Core goes on with the default, cancels the call and ignores a late answer. The bound's figure is set before extensions build B is designed.
- A focus is committed before its hooks run, as NVDA; the application-switch hooks run before the commit.

Reason: an outpost waiting on a host would let one extension hold an application's reads, and Core already has the order the hooks are judged in.

### Progress bars (Dickson, 2026-11-09, kept 2026-11-12 and 2026-11-13)

The approved [progress-bar design](https://github.com/Neurrone/verbatim/blob/main/docs/design/progress-bars.md) stands as the one place a reader reads unasked: the listener reads a bar's role, percentage and place and reports facts, and Core judges them. Core's requests go ahead of bar reads on a reading thread. Reason: a bar's beep costs no request, and under the fallback that filters change events to the foreground's processes, this path is what keeps background progress bars heard. Revisited only if the measurement shows the fallback is not needed.

### Flood limiters and the merge (Dickson, 2026-11-12; amended 2026-11-13)

The listener's limiter makes no guess about the focus: it merges only waiting events with the same object, kind and raising thread. NVDA's whole limiter (the newest ten per thread, the focused object's always, the newest four focus events) runs on the MSAA entries waiting in Core's event queue, where the focus is known; caps are added at the listener only if the measurement misses its budget. The outpost keeps a limiter for what it still hears. Core also merges waiting entries of the event queue for the same object and kind, the newest keeping its place, as NVDA's limiter does. Reason: a limiter that guessed the focus could drop the focused object's change, and the backlog behind a slow read builds in Core's queue, not on the listener's pipe.

### The secure desktop and the lock state (Dickson, 2026-11-12)

- A switch to or from the secure desktop is handled at once, ahead of the event queue, as NVDA handles it before its queue. The tell carries whether the input desktop can be opened. On a switch away, Core cuts speech, commits the sleeping focus so every key passes to Windows, and asks the old application nothing. On a return, Core asks the application in front for its focus 200 ms later.
- Core holds the session's lock state, from the shell's session notifications and a query at startup, an unknown answer counting as locked (security mitigation B3). Everything Core speaks, including what it speaks with no read, is checked against it, and an object below the lock screen is never spoken. Reason: the lock state is a fact every rule in a pure reducer reads, so Core holds it rather than calling Windows at each decision.

### Messages that remain (Dickson, 2026-11-12; amended 2026-11-13)

- Reader to Core (12): the tell, the listener's foreground change, the listener's progress fact, the answer, and eight lifecycle messages (ready, pong, fault, target exited, spare ready, idle answer, exit handled, shutdown done).
- Core to the listener (6): a request (read this focus event with stop points, answering a snapshot and the element's route; read this menu opening; read this toast's or alert's text; check a notification's sender; read the desktop's name), cancel, ping, shut down, application exited, idle query.
- Core to an outpost (8): a request from the shared list (properties of an object, navigation, find a descendant by property, text and caret reads, actions, focus reads including the completion of a UI Automation focus read, the caret watch, terminal reads, follow these objects), cancel, the objects Core holds, window destroyed, ping, shut down, assign, idle query.

That is 26 kinds where there are 42. What goes, about 40 names: the routed facts, the marker and its companions, the hand-over, the live re-read, the key's note, the terminal's hold, cancel, shape and colours messages, the bar watch, the not-responding message, the settings pushed to outposts, and the forwarding of WinEvents to outposts.

## What this decision adds and removes

The design principles ask a design to list the waiting states, messages, timers and lifecycles held in two places that it adds and removes.

Added:

- An event-queue entry: waiting or asked (Core).
- A request in Core: waiting for its outpost, out, given up or answered, with its due time and, while out, the half-second live check. It replaces the shell's request table and its wait for an outpost.
- The caret watch in the outpost's table of waiting requests: queued, running a check, running its last read, watching, answered, with its poll and bound. Held in two places with Core's caret key (starting, watching): Core decides what the key says, the outpost runs the checks.
- A reader's kept elements: kept, in read, the last focus read, released. Held in two places, the element in the reader and its token in Core, because an element cannot cross a process.
- A terminal's "told, not yet read" in Core and its "changed" flag in the outpost. Held in two places: the outpost needs its flag to tell only once, Core its fact to decide when to read.
- The committed focus and the readers' last focus, both in Core.
- Timers: the due time and the half-second check per request, the 50, 200 and 250 ms delays moved into Core, the watch's poll and bound in the outpost.

Removed:

- The marker and its wait; the hand-over's hold; the live re-read's states; the stale rule's ordering stamps; the left mark's two orders of changes; the readers' ancestry caches and Core's title check of them.
- The key's note; the shell's copy of the line of held keys; the terminal's hold and its reading states.
- The outpost's reducer states that go (its own intake queue, its held reports, narrowing); the stored hang record; the outpost's bar watch.
- The listener's schedule states that go (the grace state and its timer, its queue place, when an application was left, its last top window, pending focus and last focus, the outpost-ended and life states, its ancestry cache); the readers' deadlines and moved-on rule.
- Sixteen kinds of message.

Three additions exist to save calls or wake-ups: the watch in the outpost (a round trip to Core per check), the progress-bar path (a request per bar change, and background bars under the fallback) and values carried in UI Automation tells (a request per change). Each is the owner's decision with that reason; no measurement of a missed budget stands behind them.

## What Verbatim keeps beyond NVDA

Every rule is marked either as restoring NVDA's rule or as Verbatim-only. There are 29 Verbatim-only items; each, with its reason, is marked in [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md), "Core's rules".

- Forced by reading from outside the application (8): holding caret keys with the 150 ms release; facts carried in requests; held objects and their generations; kept event elements, a UI Automation focus completed by the outpost in a second request; the narrow rule; the silent re-take after a reader is replaced; destroys told for windows only, routed by the raising thread; a request waiting for its outpost to start.
- Forced by Core never blocking (5): Core's give-up, with its cancel abandoning a running call only when the foreground moved away; the late answer as a fresh focus; requests outside the event queue; ending entries at a foreground change, judged by window; the below-the-lock-screen fact gathered at arrival, since the reducer makes no Windows call.
- Existing behaviour kept (13): the progress path's own backend verdict; the readers' last focus cleared when the foreground leaves for an unrelated window, which NVDA keeps; Verbatim's hidden frame; the speech cut at the press; multi-press counted at the press; the caret's effect test; the terminal model; the redirect of a list's focus to its focused child; values carried in UI Automation tells; the Alt state read from the hook for the switcher rule; the not-responding sound; report focus reading the application's focus; an unknown lock state counting as locked.
- New with this decision (3): the listener's per-application records let go when idle; Core's requests ahead of bar reads; stop points for a focus Core committed itself.

## Decisions of record this amends

Each changes on a fact that has changed: no reader acts on an event itself, no read starts before Core's request, and keys now wait behind the front entry, so a wait Core ends frees the user.

- [D2](../../explanation/architecture.md): the in-process helper is inevitable and is in every application with a text field, not only browsers; for browsers it builds and repairs the browse-mode buffer inside the application and reads live region text at the event, as NVDA's does; it talks only to its own outpost (Dickson, 2026-11-12).
- D9: unchanged; the outpost's own WinEvent thread goes, except in a console host's outpost.
- D13: the listener tells Core of each event and reads a UI Automation focus only when Core asks; with its answer Core asks the application's outpost to complete the read; the listener holds every desktop-wide WinEvent hook but the console host's five events.
- D14: a reader judges nothing but NVDA's local drops; every other rule is Core's, the acceptance test included; Core orders by arrival in the event queue. Facts Core carries in a request are used only inside that request's answer. Attention moving only on the listener's foreground change is unchanged.
- Architecture, section 1: the listener holds every desktop-wide registration; outposts hold no hook but the console host's; the hand-over goes; an extension host's reads go through Core, not over a pipe to the outpost.
- Architecture, "Bulk reads carry only cheap properties" (2026-10-24): screen review may collect object positions in bulk, an exception (Dickson, 2026-11-12).
- [ADR 0001](0001-focus.md), "One route per application, and the marker": removed.
- ADR 0001, "MSAA focuses stay with the outpost": the listener tells an MSAA focus and the outpost reads it at Core's request.
- ADR 0001, "Reusing an earlier ancestry": the caches, the title check and the live re-read go; stop points are Core's committed focus line.
- ADR 0001, "One ordered focus queue" and "One input to Core, in arrival order": kept and widened to every event, key and command; the ordering stamps go.
- ADR 0001, "The foreground and the window title": the foreground test is Core's, and the left mark is folded into the readers' last focus, cleared when the foreground leaves for an unrelated top-level window.
- ADR 0001, "A focus read is complete or nothing": still complete or nothing, but the readers' deadlines and the moved-on rule go; Core gives a read up on a hung report, at its due time or when the foreground leaves its window, and takes a late answer as a fresh focus.
- ADR 0001, "Time limits" and "Window messages, shell items and test limits": the 11-second handling deadline becomes Core's due time, which the taskbar's reads take too.
- ADR 0001, "The listener's checks": the checks that make calls stay inside the asked read; acceptance moves to Core.
- [ADR 0002](0002-no-screen-reader-per-outpost.md): "Core judges keys and focus" stays; a reader's own choice of what to read narrows to the local drops; the record is renamed "No screen reader per outpost" and gains the keyboard hook in each outpost as a rejected option, as Dickson asked on 2026-11-12.
- The progress-bar design (2026-11-09): kept whole; one sentence, that an outpost's hook still receives a bar's event, becomes false and goes.
- Also amended, each named with its build in [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md): the key-holding design's waits, selection-key holding and the shell's line of held keys, the caret and selection rework's rules, the outpost's hang record, the ownership design's scope rule, security mitigation B3's wording, and the parity ledger's text entries that describe them.

## Weighed and rejected

- Readers reading and judging on their own, as today. Each reader reads when the event arrives and decides what to read from its own observations, with Core checking afterwards. Rejected: each feature needs its own cache, stamps and repair when Core disagrees, and a reader's guess about Core's focus can change what is spoken.
- A queue per application in Core, one request out per application. Rejected: keys have one order across every application, so a queue per application either breaks the order of keys or stops being per application, and a long read would hold that application's keys.
- The screen reader's logic in each outpost, Core keeping only the hook, speech and attention. Rejected: an outpost would judge facts that are Core's, such as whether the focus has been elsewhere, from notices that reach it by another route, so a return to an application could be judged a repeat and stay silent.
- A keyboard hook in each outpost, so keys and reads meet beside the application. Rejected: a key must be ordered across applications, only one hook sees every key before any application does, and Windows calls every low-level hook in turn on each key.
- All readers in one process, the listener and the outposts merged. Rejected: when a focus event names a hung window, UI Automation holds every event to the process registered for focus for seconds, so every application's UI Automation events would stop while one application hangs, and Verbatim would be little better than NVDA there.
- No UI Automation focus registration, the focus taken from WinEvents alone. Rejected: NVDA takes a UI Automation window's focus from UI Automation's own event, and a focus change that raises no WinEvent would go unspoken; nothing shows that every application raises both.
- Core polling for a caret key's effect, asking again on a timer until the caret moves. Rejected: each check would be a round trip to Core and a wake of Core, up to ten for each key whose caret has not moved (twenty in the console host); the outpost makes the checks and returns one result.
- Progress bars as ordinary requests, Core asking the listener to read a bar when told of its change. Rejected: under the fallback that filters change events to the foreground's processes, a background bar's change never reaches Core, so it would go unheard.
- Only focus events moved to the listener, the other MSAA events left in each outpost. Rejected: an application's focus and its other events would arrive by two routes with no order, so a state change raised before a focus could be handled after it and go unsaid.
- A time limit on how long a key waits for a focus ahead of it. Rejected: NVDA has none, and a key pressed after a slow focus would act from the old position where NVDA acts from the new one.
- A stored not-responding mark per application. Rejected: one hung window stopped requests to the application's other windows that still answer, the mark had no clean way to end, and NVDA checks each window live each time.
- Core's sequences written as step functions in a reducer, each wait a named stage. Rejected: a chain of requests is split across several functions, where async functions read top to bottom like NVDA's, at half the size per feature.
- A logic thread waiting on each request beside an inbox thread. Rejected: it needs a thread inside the platform-neutral Core, which the determinism rule forbids, and each question about the queue becomes a round trip between threads.

## Limits accepted

- Commands are silent behind a frozen window until the give-up. A key, command or typed character behind a stuck entry waits until Windows reports the window hung (its threshold is about 5 seconds, an estimate, plus up to half a second to the next check), the due time passes (13 seconds for a focus read, 11 for others), or the foreground leaves the window. A window slow but never reported hung, such as one whose application opens a dialog inside an activation, holds them for the whole due time. Held caret keys still reach the application after 150 ms. NVDA queues scripts the same way; while its watchdog recovers, keys pass to the application unhandled.
- A caret key released by its 150 ms timer says nothing. In an application whose caret reads take longer than 150 ms, the last line of a burst may not be spoken: with reads of about 180 ms and Down Arrow pressed three times 60 ms apart, none is spoken, where NVDA speaks the last line (derived, no capture). NVDA has no such release.
- The order between the listener's events and an outpost's UI Automation events is not guaranteed: a UI Automation focus usually follows its WinEvent by 1 to 35 ms and sometimes by seconds. The narrow rule repairs the old focus's own state changes when the new focus is inside it; a state change of the old focus arriving after a focus elsewhere is unsaid. NVDA has two arrival paths with no order between them too.
- Two corners of clearing the readers' last focus while the user is away: a system caret event arriving then has no target and is ignored; an outpost replaced then has its focus spoken when Core re-takes it, rather than taken silently. NVDA keeps its last queued focus across a foreground change.
- Every background event reaches Core. With desktop-wide hooks and every judgement in Core, each event the listener's drops and merge let through is a tell and a step of Core's reducer: derived figures of about 1,100 a second for a File Explorer in the background, and about 30,000 more tells per run of the console flood. A backlog delays a key's report, and so the 150 ms release. NVDA hooks the same events desktop-wide in one process. Measured before the build; the ruled fallback, if the budget is missed, is that the listener filters change events by the foreground's processes.
- A listener replacement loses every WinEvent of every application until the new listener's hooks are in: a listener that stops answering is replaced 9 to 12 seconds after its last pong, a crashed one at once, and its UI Automation registration can take about 6 more seconds on an application that does not answer. Core then asks the foreground application for its focus once. Not shared with NVDA, which has one process.
- Two more crossings per event-driven read: a tell to Core and a request back before the first call, and one more wake of Core per UI Automation focus, read in two requests. A burst of focus events is read one round trip at a time; for the console flood of 1,911 reads, about 3,800 more crossings and 1,900 more wakes of Core. NVDA reads in its own process.
- One more caret check per key: every caret key costs up to about ten checks (twenty in the console host), each a call into the application, while its caret has not moved, and the read after every focus with text, caret move and say-all move that today's outpost skips is made. NVDA's loop polls the same way.
- A return to an application after another application's focus was committed walks its ancestry to the top live: 14.0 ms against 6.2 ms with today's cache. NVDA walks to its own line the same way.
- Further limits, each with its size, are marked "a limit" in [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md).

## Decided after the reviews (Dickson, 2026-11-12)

The reviews left eight questions, each put to Dickson with a sequence and the speech under each option. His answers:

- The classic edit control's value-change test is judged by Core when the answer arrives: the outpost returns the detailed comparison, and Core speaks it in general terms ("selection removed") when a value change of the control is waiting by then. Reason: Core judges and nothing is added. Limit: behind a flood of events the value change can arrive after the answer, and the detailed report is spoken.
- Typed characters that waited behind a stuck entry are echoed when it is given up, late and together, as NVDA echoes its queued characters. Reason: no rule is needed.
- The caret event a key raises costs one more caret read, for its selection comparison, as in NVDA. Reason: dropping it would miss a selection change something else made.
- NVDA's whole limiter is applied to the MSAA entries waiting in Core's event queue, beside the merge (see "Flood limiters and the merge").
- A progress bar that a built-in module's rule excludes, such as the Settings sound meters, costs one read the first time it is seen, and Core keeps the verdict until the bar's window is destroyed. Reason: otherwise the meter beeps many times a second.
- A built-in module does not read across its application's boundary; an exception is decided with the extension build.
- Entries already in the event queue when the secure desktop comes up run as NVDA's do: a focus that was waiting is spoken after the switch. Reason: no added rule.
- The single record of an outpost generation's life (M6.2) is built together with the event-queue build, since both change the same code.

Still to be verified, before M8.7 relies on it: whether UIAccess reaches elevated windows in a standard user's session. Microsoft's documents disagree; [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md), "M8.7, UIAccess and secure screens", say what to verify.

## Left to later milestones

- M5 build B: the bound on a module's command, modules asking for extra events, a hook's speech after the default announcement, and the extension surface's missing effects and reads.
- M7: the helper's loading and how long an application may wait on it, composed text, and the helper's tells.
- M8.3: which settings a profile carries, the say-all trigger.
- M8.7: UIAccess, the secure instance and the lock-screen rules beyond what this design adds.
- M12, M13, M14, M18: Remote's capture mode, applications' own object models and window-message text, where the maths engine runs, braille.
- Their own milestones: whether report focus speaks the application's live focus or the committed focus, and the control protocol's next version.

## Core's code is async functions (Dickson, 2026-11-14)

Core's logic is written as async functions run on Core's thread, from the event queue and focus build, under five rules: only a request to a reader can be awaited; each request names the type of its answer, which the queue decodes once; every request gets exactly one outcome, so no paused function outlives its entry; the recorder's checkpoint is the state at the front entry's start plus the inputs since, so replay stays exact; and a state dump shows each entry, the answers it has had and the request out. Reason: a feature's handling reads top to bottom like NVDA's and keeps its code in one place, while the dumps and exact replay the project relies on are kept. Limits: a copied fact held across a wait is not caught by the compiler and is checked in review; the checkpoint copies Core's state at entry boundaries, whose cost grows with the application. The trial and its figures are in [the build notes](https://github.com/Neurrone/verbatim/blob/main/docs/design/readers-tell-core-asks.md).

## To try (Dickson, 2026-11-13)

- One recorded tape of an application's events and tree, replayed both to a simulator and to real Windows, so that NVDA is captured against the same application; for the design of the speech-level tests.

## Before the build, and the order of building

- Measured first: the background-event backlog and the keyboard, with every change kind hooked desktop-wide, idle with File Explorer open, a background console flood and an MSAA flood, counting the listener's and Core's wake-ups, processor time, and the delay from a held key's press to Core taking its report. If the budget is missed, the ruled fallback is measured before anything else is built. The same measurement decides whether count caps are needed at the listener's limiter.
- Also before: the cost of starting a listener reading thread after its application went idle; the property changes following the focus and its ancestors brings in File Explorer and Settings; the lag between a focus and its state changes across the two routes.
- In the first build: the live hang check's own time, and the below-the-lock-screen test while locked.
- Builds, one changing state at a time: the fast speech-level tests first; then the event queue and focus as one build; then changes and announcements; then keys and text; then terminals; then M5's design corrections and build A.

## What would reopen this

A measured fact that removes one of its reasons: a way for a reader to act on an event in Core's order without asking Core, a way for a key to be ordered with an application's reads outside Core, or a measurement showing that every background event reaching Core misses the budget even with the ruled fallback.
