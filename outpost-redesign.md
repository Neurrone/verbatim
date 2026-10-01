# Outpost redesign, 2026-10-01

This is the agreed design and implementation plan for the outposts, the
focus listener, the supervisor, the app's handling of outpost traffic, and
the reducer changes that follow from them. It was worked out with Dickson
on 2026-10-01 while reading the reducer and outpost code in phase 1 of
`handoff-2026-09-02.md`, then reviewed adversarially against
`audit-2026-09-02.md`, the five additional findings in the handoff, the
handoff's hot-path notes, and decisions D13 to D15.

Nothing here is implemented yet. It replaces the contents of the handoff's
old phase 4 (review and simplify Core-outpost coordination) and pulls the
D14 attention model forward from M4. Implementation waits until the
harness repairs (handoff phase 2) have landed, so that every step can be
verified live as it is built.

## Why redesign rather than repair

The outpost code works most of the time, but its correctness rests on
many independent mechanisms, each added to fix one live failure, none of
which knows about the others:

- snapshot version numbers, checked by the reducer;
- observation timestamps, checked by the reducer for focus events;
- the outpost's announce generation counter;
- the supervisor's spawn generation counter;
- the newest-wins slots that hold facts during a spawn;
- the reducer's latest-navigation field;
- the reducer's identical-focus suppression;
- the app's foreground pid gate.

Changing one of them tends to break a case another was covering. On top of
that, the audit found that blocking work happens under the supervisor's
map lock and on the outpost's command thread (audit items 1 and 6), that
the outpost starts unbounded threads (item 18), and that outposts make
presentation decisions such as retrying nameless windows and shaping menu
events so the reducer's duplicate suppression drops one of two paths. The
redesign keeps the process model of D9 and D13 and replaces the mechanisms
inside it.

Three findings from the 2026-10-01 reading were not in the audit:

- The version check cannot be repaired usefully. Every hop after an
  outpost's outbound queue is single-threaded and in order (the outpost's
  writer thread, the pipe, Core's reader thread for that outpost, the
  app's queue, the reducer thread), so the only reordering it ever
  detects is the race between taking a number and queueing the event
  inside the outpost. Dropping an event because a later-numbered one
  arrived first also loses independent events such as notifications, and
  the response, a re-read of the focused node, is unrelated to most event
  kinds. The mechanism is removed rather than fixed.
- The outpost's UIA property, selection, and notification subscriptions
  are attached to the target application's top-level windows that exist
  when the outpost starts (`install_property_registration` and its two
  siblings in `runtime.rs`). A dialog or window opened later receives no
  UIA events at all. MSAA hooks are process-wide and unaffected.
- Each outpost numbers its nodes from 1 (`Outpost::new`). A restarted
  outpost hands out the same numbers for different elements, so a node id
  the reducer still holds from the old outpost can resolve to an
  unrelated element in the new one. An activation could press the wrong
  control.

## Principles

1. Responsibilities are split three ways. Outposts and the listener listen
   for accessibility events and answer queries. The supervisor manages
   processes and routes messages. The reducer decides what to speak,
   which events to accept, and what attention is. A decision about
   presentation never lives in an outpost.
2. Follow NVDA wherever it has an answer. The outpost and listener live in
   a Windows-specific GPL crate and may port NVDA's intake code directly.
   Behavior destined for the reducer (the window rules, name-change
   speech, the entered-menu rule, event acceptance) is platform-neutral
   and must be written into `docs/parity.md` from NVDA's behavior first,
   then implemented from that description, never translated from NVDA
   source (the provenance rule in `CLAUDE.md`). The NVDA file references
   in this document establish behavior; they are not a licence to port
   into `verbatim-core`.
3. Each correctness rule has exactly one owner, and the document names it.
4. Every queue has a stated overload policy, and no thread that others
   depend on ever waits on one.

## Identities

- Outpost id: names one outpost process incarnation. The supervisor
  assigns a fresh one at every spawn and never reuses one; its existing
  spawn counter provides the numbers. Core attaches the id to each
  message according to the pipe it arrived on, never from the message
  body.
- Node id: an outpost id plus a number issued by that outpost. An id from
  a replaced outpost can never name a node in its successor.
- Request id: names one query and records who asked, the reducer or the
  control plane.
- Message position: each outpost numbers its outgoing messages in the
  order its writer puts them on the pipe. Its only use is the acknowledged
  position in "nodes I hold" (below). Nothing uses it to decide whether an
  event is out of date.
- Window handle: the identity used to compare windows across processes,
  because it means the same thing in every process. Node ids are per
  outpost and cannot be compared across outposts.
- Application pid: carried on every event as information about its
  source, for attention and logs. Routing uses the outpost id.

## The focus listener

One process for the desktop, as in D13, under the same never-block rule:
it never calls into another process.

It subscribes to:

- MSAA focus, foreground, and menu-popup events for every process (as
  today);
- the desktop-wide UIA focus subscription (as today);
- new: `EVENT_SYSTEM_ALERT` for every process (D14);
- new: desktop-wide UIA subscriptions registered on the desktop root for
  the events NVDA registers globally under its selective registration on
  Windows 11 (`_registerGlobalEventHandlers`,
  `nvda/source/UIAHandler/__init__.py`): selection, notifications, live
  regions, menu opened, and range value changes.

Each event becomes a fact carrying the owning pid, the trace id, the
observation time (used for the latency record only), and what the event
itself delivered: a raw MSAA address, or a UIA element's cached
properties.

New: the listener coalesces its outgoing facts with NVDA's UIA limiter
rule before sending them, one waiting entry per element and event kind
where a newer entry replaces the older and moves to the back
(`nvda/nvdaHelper/local/UIAEventLimiter/`). Without this, a flood of
desktop-wide events from one busy process would pass through the listener
and Core unthrottled before any outpost could merge it.

Which facts may start an outpost for a process that has none: focus,
foreground, menu, notification, and alert. Selection and range value
facts for a process without an outpost are dropped; a background progress
bar option, if added later, would extend this list.

## Inside an outpost

An outpost watches one application for its whole life, as in D9. It has
these parts.

1. Intake. Callbacks only add an event to the queue and return. They
   never call into the application and never take a lock the worker
   holds. Three sources feed it: the outpost's own MSAA hooks for its
   process (value, name, state, selection, and new: window destruction,
   `EVENT_OBJECT_DESTROY`); the focus-following UIA subscriptions; and
   facts the supervisor routes from the listener.
2. The queue. One queue for the application, holding both events and
   queries from Core, with NVDA's limiter rules
   (`nvda/source/IAccessibleHandler/orderedWinEventLimiter.py` and the UIA
   limiter):
   - one waiting entry per object and event kind; a newer one replaces it
     and takes its place at the back;
   - a batch is everything that accumulated while the worker handled the
     previous batch;
   - per batch, keep the newest 4 focus events and the newest 10 other
     events per application UI thread, a UIA event's thread being found
     from its cached window handle with a local call (events with no
     window share one bucket);
   - the focused object's events are always kept (NVDA's
     `alwaysAllowedObjects`);
   - events from a window the system reports as hung (`IsHungAppWindow`
     on the cached handle, a local call) are dropped before any read, as
     NVDA's `_shouldSkipEventForHungWindow` does.
3. The worker. One thread takes entries in order and finishes each before
   starting the next. It is the only thread that calls into the
   application. For an event it decides the window's backend, reads what
   the event needs, and sends the result; for a query it does the query
   and sends the reply. Because there is one worker, events and replies
   leave the outpost in the order their entries joined the queue. This is
   NVDA's model, one thread doing all the work, with one such thread per
   application. It replaces the announce lane, the two-thread query pool,
   the per-probe threads, the announce-poll thread, and the background
   window retry. Workers run in COM's multithreaded apartment, so held
   objects can be used by a replacement worker without wrapping; confirm
   how `verbatim-ia2` initialises COM when implementing.
4. The watchdog. One thread that watches the worker's deadline. If a call
   hangs past it, the watchdog abandons the worker (it cannot be stopped
   safely), starts a replacement that continues with the rest of the
   queue, answers the stuck query "abandoned" if it was a query, and
   counts the abandoned worker. An abandoned worker that eventually
   returns exits at once, publishes nothing, and lowers the count (audit
   item 2, additional finding 2).
5. The reader. Receives commands from Core. It answers pings itself, so a
   busy or hung worker never makes the outpost look dead.
6. The writer. Numbers messages and writes them to the pipe. Pongs and
   Ready always go ahead of ordinary messages, so heartbeat delivery
   never waits behind event traffic. The queue from the worker to the
   writer is bounded; when it is full the worker waits, which is safe
   because the intake queue's own limits bound what accumulates behind
   it.

Parallel queries within one application are deliberately given up for
now. MSAA calls, and UIA calls to most Win32 and WinUI providers, are
answered on the application's UI thread one at a time anyway, so little is
lost. The known cost is waiting behind a long query in the same
application, chiefly the diagnostic tree dump (up to five seconds). If
that matters, the first fix is cancelling or splitting long diagnostic
queries, not adding threads.

### Arbitration

UIA or MSAA is decided per window, as NVDA's `_isUIAWindowHelper` does:
NVDA's own process excluded, the good class list, the bad class list,
then the `UiaHasServerSideProvider` probe on the worker. The decision is
kept for the window's lifetime and discarded when the window is
destroyed, as D15 specifies. NVDA's 500 ms cache was added to throttle a
check made on every event (NVDA commit 9b4f5d29a), not because answers go
stale. One assumption must be checked live during step 4: that a window's
answer does not change after creation, in particular on the Start search
box, the XAML control whose UIA facts were dropped as non-UIA in July.
The class lists and any per-application overrides are data sent by Core
in Configure. The dead `Arbitrator::resolve_with` is removed (audit
item 25).

### Reading events

- A UIA focus fact is resolved with one `focused_element` call, compared
  against the fact's runtime id. A mismatch means focus has already moved
  and the newer fact will arrive; the stale one is dropped. There is no
  runtime-id search (handoff hot-path notes).
- Each element is resolved once per event and reused for the window, the
  ancestors, and the selected child.
- A focus event carries the focused node, its ancestors, its selected
  child for lists and tab controls, and window facts: its top-level
  window handle (`GetAncestor` with `GA_ROOT`), its root owner
  (`GA_ROOTOWNER`), and whether it or its root is topmost. All three are
  local calls. Every other event carries the same window facts for the
  window it concerns.
- Hidden-frame suppression for Verbatim's own hidden main window stays in
  the outpost, unchanged in substance.
- The MSAA window snapshot moves into `verbatim-ia2` (audit item 23), and
  one navigation-direction type replaces three (item 24).

### Focus-following UIA subscriptions

Following NVDA's selective registration on Windows 11
(`_createLocalEventHandlerGroup` and `addLocalEventHandlerGroupToElement`
in `nvda/source/UIAHandler/__init__.py`), most UIA property changes (name,
value, toggle state, expand and collapse) are subscribed on the focused
element and its ancestors only. The worker moves the subscription when it
reports a new focus, without waiting for Core. This replaces the
subscriptions on the top-level windows that existed at spawn, and with it
the missing events for windows opened later. Removing a UIA subscription
waits for its callbacks to finish, which is why intake callbacks must
never take a lock the worker holds.

### Menus

NVDA's menu handling is intake logic in its MSAA handler
(`pumpAll`, `processMenuStartWinEvent`, `processFakeFocusWinEvent`, and
`_fakeFocus` in `nvda/source/IAccessibleHandler/__init__.py`), so it
lives in the worker:

- Menu opening: within a batch, focus and foreground events are handled
  first and the menu event last. If a focus event was handled in the
  batch and focus is already on a menu or menu item, the menu event is
  ignored. If its object is not a popup menu, it is ignored. Otherwise it
  becomes a focus on the popup menu. The reducer therefore never receives
  a separate menu event, only a focus.
- Menu closing: if no focus event for the application follows within 50
  milliseconds, the outpost reads the actual focus and reports it as a
  focus event. A timer adds a check-focus entry to the queue so the
  worker never sleeps.

### Held objects and "nodes I hold"

The reducer holds a small, known set of node references: the focus, its
ancestors, the last selection, the navigator, and pending queries. After
each input the shell sends each outpost the subset that belongs to it
when that subset changes, together with the acknowledged position: the
position of the last message from that outpost the shell has handled,
including messages it dropped before the reducer saw them.

The outpost keeps live objects (UIA elements and MSAA objects) for
exactly the nodes in the set, plus every node it issued after the
acknowledged position, which the reducer may not have seen yet. It
releases everything else, and answers a query for a released node with
"gone", never with a guess. This is NVDA's rule, where an element lives
as long as an object referring to it does, and it replaces the
clear-at-2,048 element cache and the identity maps that never shrink
(audit item 19).

MSAA is handled as NVDA does it:

- the outpost holds the actual MSAA object for each held node, so
  navigation, activation, and re-reads reach the object that was
  announced, not whatever now sits at its address;
- an incoming MSAA event, which always arrives as an address, is matched
  to a held node by NVDA's comparison order (`_isEqual` in
  `nvda/source/NVDAObjects/IAccessible/__init__.py`): child ids, the
  same COM object, IAccessible2 unique ids within a window, event
  addresses, MSAA identity strings, then location, role, and name;
- IAccessible2 unique ids are used wherever an application supplies them;
- a window's entries are dropped when the window is destroyed, so a
  reused window handle never inherits them;
- positional child ids in simple list controls remain a known limitation
  that NVDA shares.

## The supervisor

The supervisor stays inside Core.

- One lifecycle owner thread makes every lifecycle decision and owns the
  per-application records (starting, ready, ended) and the listener
  record. Reader threads, the heartbeat timer, and the idle timer only
  report facts to it. The shared map lock goes away.
- Launching a process blocks, so a helper thread does it and reports back.
  The owner marks the application starting before the launch, so a
  second fact arriving meanwhile is held instead of starting a second
  outpost (audit item 14).
- Each outpost has a writer thread in Core with a bounded queue. The
  supervisor never writes to a pipe while making a decision. When the
  queue is full: a routed fact replaces any older fact still waiting for
  the same object and kind; a query fails at once; a "nodes I hold"
  update replaces any older one; pings always get through. Core's thread
  count is two per outpost.
- Facts that arrive while an outpost is starting are held in arrival
  order, merged with NVDA's per-object rule, and released in that order
  on Ready (audit item 13). This replaces the four fixed slots.
- Crash: when an outpost's pipe closes, the owner ends it. It spawns a
  replacement at once only if the application holds attention;
  otherwise the next fact for the application starts one. After three
  crashes within a minute it stops respawning, logs a fault, and tries
  again at the next foreground change to that application.
- Hang: the heartbeat pings every three seconds. Nine seconds without a
  pong ends the outpost. Eight abandoned workers also end it, except
  while the application's own windows are reported hung, when a new
  outpost would hang the same way.
- Retirement: an application that has not held attention for two minutes,
  is not Core's own process, and in which Core holds no nodes, has its
  outpost ended.
- Every ending, for any reason including Core's own exit, closes the
  outpost's job handle. The Shutdown message is removed. An outpost keeps
  nothing worth saving.

## The app shell

- The request table, owned by the reducer thread, in its own small module
  of `verbatim-app`. Every query is recorded with its request id and
  outpost id. The first outcome removes the entry and goes to whoever
  asked; any later outcome is dropped. Core creates the outcome itself
  for a query to an outpost that has ended ("gone"), for a writer queue
  that is full ("failed"), and for every query still outstanding when an
  outpost ends ("gone"). This is the single owner of "exactly one outcome
  per query" (additional findings 2 and 5).
- Control-plane queries, such as the tree dump, are handed to the reducer
  thread through its command channel with a channel for the answer,
  instead of writing to the outpost and waiting in a separate locked slot.
- The live-outpost set. Messages from an outpost that has ended are
  dropped before the reducer sees them.
- The reducer thread never blocks. Every handoff from it, to outpost
  writers, to speech, or to the control plane, fails or replaces instead
  of waiting. The app's inbound queue has no fixed capacity; the limits
  at the outposts and the listener bound what flows into it. Core's reader
  threads therefore never wait, and an outpost's pongs are always read.
- After each input the shell derives two views from the reducer's state
  and sends their changes: the nodes held per outpost, to each outpost,
  and the application holding attention, to the supervisor.
- The foreground pid gate is removed. Acceptance is the reducer's, below.

## The reducer

### Removed

- The version map, the staleness check, the staleness re-read, and its
  fetch path. `SnapshotVersion` leaves the model and the protocol.
- The focus observation-time check and its two exceptions (late windows
  and ancestors). The observation time stays on inputs for the latency
  record and is not read by the reducer.
- The exclusion of windows from entered containers. A named window is
  announced on entry, as in NVDA, and the documented divergence in
  `docs/parity.md` goes.
- Most of the pending-fetch record: replies carry their query kind, so
  the reducer keeps only the latest navigation's request id.
- The foreground pid gate, which moves here as the attention model.

### Added

- Windows, following NVDA (`processForegroundWinEvent` and
  `doPreGainFocus`): a foreground change is a focus on the window. It is
  ignored when its window handle equals the current focus's top-level
  window handle, which covers both a window that is already the focus
  and focus already inside it. Window handles are used rather than node
  ids because the same window has different node ids in different
  outposts, as with a Settings page whose frame belongs to
  `ApplicationFrameHost.exe`, and because an ancestor walk that times out
  returns no ancestors to compare against. A window that has no name when
  focus enters it is not announced later.
- Name changes: when the focused node's name changes, the new name alone
  is spoken, queued rather than interrupting, as NVDA's
  `event_nameChange` does. This covers a window that is itself the focus
  and receives its name late.
- Entered menus: when a menu, menu bar, or menu item is newly entered as
  an ancestor, current speech stops and the menu is not announced, as
  NVDA's `event_focusEntered` does.
- Attention (D14, amended): the reducer keeps the process and top-level
  window that most recently received focus. Each event is classified
  against it from the window facts the outpost attached. Attended: the
  attention window and anything with the same top-level window, anything
  sharing its root owner, topmost windows, and the `Windows.UI.Core`
  case. Accepted from anywhere as background: UIA notifications, toast
  alerts, tooltips and notification bars, and configured progress bars.
  Everything else is dropped. Background events never move focus or the
  navigator, are spoken queued, and are capped per source. NVDA tests
  "inside the foreground window" with a parent-child check between two
  windows; comparing top-level windows and root owners is meant to cover
  the same cases, and must be checked against the Settings and Explorer
  scenarios.
- Outpost ended: the reducer keeps the focus's copied data (role, name,
  value, states, ancestors' names and roles) but treats its node ids as
  dead, clears the navigator and the latest navigation if they belonged
  to that outpost, and receives "gone" from the request table for any
  pending query. Navigation commands then do nothing until focus is
  re-read.
- The silent re-read: when the focus-now reply from a replacement outpost
  (or after a listener restart) arrives, it is compared with the kept
  copy. If role, name, value, states, and the ancestors' names and roles
  all match, the reducer takes the new node ids without speaking.
  Otherwise it announces as usual.
- A function exposing the nodes the state holds, grouped by outpost, and
  one exposing the application holding attention, for the shell's
  derived views.

### Unchanged

Identical-focus suppression stays in the reducer: only Core sees focus
across applications, and an outpost doing it alone would wrongly suppress
a return to its application after a visit elsewhere. Selection,
notification, value, and state handling, navigation with the
latest-navigation rule, report object, activate, and the review cursor
are unchanged.

Two parity gaps found on the way are separate items, not part of this
work: NVDA speaks description changes on the focus, which Verbatim does
not observe yet; and Verbatim interrupts speech for a value change while
NVDA's `event_valueChange` appears to queue it, which needs checking.

## Messages

From the listener to Core: focus facts (foreground, MSAA focus, UIA
focus, menu popup), the desktop-wide UIA events, alerts, Ready, Pong, and
Fault.

From Core to an outpost:

- Deliver: a routed listener fact, which joins the queue.
- Query: request id, kind, and arguments. Kinds: focus now (window,
  focused control, ancestors, selected child, window facts), read a node,
  one navigation step, activate, ancestors of a node, tree dump.
- Cancel, by request id.
- Nodes I hold, with the acknowledged position.
- Configure: the arbitration override, class lists, per-application data.
- Ping.

From an outpost to Core:

- Ready.
- Event: focus (with ancestors, selected child, and window facts),
  selection, value change, name change, state change, notification,
  alert. A foreground arrives as a focus on the window.
- Reply, exactly one per accepted query, with one of five outcomes:
  done (with the result); gone (the node is no longer reachable); failed
  (with a reason); not started (cancelled or expired while queued, no
  side effects); abandoned (started and passed its deadline, so side
  effects such as an activation may already have happened).
- Pong, with the abandoned-worker count.
- Fault.

From the supervisor to the app: outpost started (outpost id, pid) and
outpost ended (outpost id, reason: retired, exited, or killed). Every
other outpost message reaches the app labelled with its outpost id.

Framing stays newline-delimited JSON through `write_message` and
`read_message`, which remain the seam for a bulk path in M6.

## Ordering guarantees

- Events and replies for one application reach the reducer in the order
  their entries joined that application's outpost queue.
- Two events observed within about a millisecond of each other by
  different intakes (the listener and the outpost's own hooks) can reach
  the queue in either order. NVDA has the same property, with separate
  WinEvent and UIA intake threads feeding one main queue.
- There is no ordering between applications; attention decides what
  matters.
- Every message from an outpost reaches the app before that outpost's
  "ended" notice when it exits on its own. When it is killed, a few
  messages may follow the notice and are dropped by the live-outpost set.

## Lifecycle

- Core startup: the supervisor spawns the listener and Core's own outpost.
  The app reads the foreground window itself (a local call) and the
  supervisor spawns that application's outpost. On Ready the supervisor
  releases held facts, then Core sends a focus-now query; the reply
  reflects the current focus and is spoken. No retries are needed: a
  nameless window is not announced later (NVDA's rule), a control that
  takes focus later raises its own focus event, and the focus-now query
  covers focus that settled before the listener existed. The announce
  poll is removed (audit item 22).
- Listener restart: on a closed pipe or nine seconds without a pong the
  owner ends and respawns it. Once the new listener is Ready, the app
  reads the current foreground itself, rather than trusting the last one
  it knew, makes sure that application has an outpost, and sends a
  focus-now query, applied silently if it reads the same. Desktop-wide
  UIA events from the gap are lost; the guide must say so.
- Outpost crash: the reader forwards everything the outpost wrote, then
  reports the closed pipe. The owner sends "ended, exited". The request
  table fails the outpost's outstanding queries as "gone", including any
  still in its writer queue. The reducer drops its references. A
  replacement follows the crash rules above, and gets a focus-now query
  if its application holds attention.
- Outpost hang: a hung call is handled inside the outpost by the
  watchdog. A wholly unresponsive outpost is killed by the owner, which
  sends "ended, killed" at once; the rest follows the crash case.
- Retirement: the owner ends the outpost by closing its job handle and
  sends "ended, retired".

## Amendments to ratified decisions

These go into `docs/architecture.md` when the corresponding work lands,
as the handoff's housekeeping rule says.

- D13: the announce poll is removed rather than kept as a fallback. The
  listener additionally holds the desktop-wide UIA subscriptions and the
  alert hook, coalesces its facts, and may start outposts only for the
  fact kinds listed above. Outposts lose their window-scoped UIA
  subscriptions in favour of focus-following ones.
- D14: event acceptance is classified by the reducer, not the outposts.
  Outposts attach window facts read with local calls (top-level window,
  root owner, topmost); the reducer compares them with its attention
  record, so attention is never broadcast to outposts. The foreground is
  no longer announced separately; it is a focus on the window, as in
  NVDA. The attention model is implemented in this redesign instead of
  at M4.
- D15: unchanged in substance. The arbitration decision gets the window's
  lifetime, the element is resolved once per event, and the worker
  records the stage times the ledger needs (routed, worker start, backend
  decided, read, ancestors read, sent).
- Architecture section 3: trees are no longer described as versioned
  snapshots; node ids are per outpost incarnation.

## How the review findings were resolved

The adversarial review of this design on 2026-10-01 found these problems,
each now resolved above:

1. Windows compared by node id fail across outposts and when an ancestor
   walk returns nothing. Resolved by comparing top-level window handles.
2. Menu handling was undefined, and with the timestamp check removed, a
   menu fact released after the focus on its first item would move focus
   back to the menu. Resolved by NVDA's menu rules in the worker and by
   releasing held facts in arrival order.
3. A Core reader waiting on the app queue would stop reading its pipe and
   get a healthy outpost killed. Resolved by the rule that the reducer
   thread never blocks and the app queue has no fixed capacity.
4. Pongs could wait behind events in the outpost's writer. Resolved by
   sending pongs and Ready first.
5. Desktop-wide events had no limiter. Resolved by coalescing in the
   listener.
6. Desktop-wide events could start outposts for every process. Resolved
   by the list of fact kinds allowed to start one.

## Audit and handoff coverage

Resolved by this design:

- Audit items 1, 2, 5, 6, 13, 14, 16, 18, 19, 22, 23, 24, and 25.
- Audit item 15 changes meaning: a dead focus is no longer cleared by a
  re-read. The next focus event replaces it, and a navigation from a dead
  node is answered "gone".
- Additional findings 1, 2, and 5.
- The handoff's old phase 4 steps 1 to 7 and its hot-path items for the
  outpost (resolve once, no runtime-id search, window-lifetime verdicts).
- Carried-forward notes: the announce poll's retry budget and the
  clear-at-2,048 element cache both disappear.

Needed for verification here: audit item 21 (mockapp's stubbed
`accSelection`), the rapid focus churn scenario, the cold-start loop, and
the once-missed slider value change.

Outside this work: audit items 3, 4, and 17 and additional finding 3
(speech, handoff phase 4); audit item 7 (GUI port); audit items 8 to 12
and 20 and additional finding 4 (harness, handoff phase 2); audit item 26
(doc drift, fixed as the guides are rewritten).

## Implementation steps

Each step is independently reviewable, verified as the working agreements
require, and committed separately. The harness repairs land before step
1, so live verification is available throughout.

1. Reducer and model. Write the new behavior into `docs/parity.md`
   first. Then: node ids carry their outpost id; versions and the
   timestamp check removed; the NVDA window rules; name-change speech;
   the entered-menu rule; outpost-ended handling and the silent re-read;
   the attention model with window facts; the derived views of held nodes
   and attention. Deterministic tests for each, retiring the tests listed
   below. Current outposts stamp a placeholder outpost id and window facts
   until steps 3 and 4 land.
2. The app: the request table, the live-outpost set, control-plane
   queries through the reducer thread, the never-block rule, the
   foreground pid gate removed, and the derived views sent out.
3. The supervisor: the lifecycle owner and launch helper, writer threads
   with their overload policy, held facts in arrival order, the crash
   limit, ending by job handle, and attention-driven respawn and
   retirement.
4. The outpost: one worker and a watchdog, intake that only enqueues,
   NVDA's queue rules and batch limits, the hung-window check, the menu
   rules, window facts, UIA focus resolution by one call, destroy hooks,
   request ids with cancellation and the five outcomes, pongs first, and
   the cleanups of audit items 23 to 25. The announce lane, the poll, and
   the probe and retry threads are deleted.
5. The listener: desktop-wide UIA subscriptions, the alert hook,
   coalescing, and the rule for which facts may start an outpost. The
   outposts switch to focus-following UIA subscriptions.
6. Held objects: "nodes I hold" with the acknowledged position, held MSAA
   objects, NVDA's comparison order, and window-destroy cleanup.
7. Live verification and documentation: mockapp's selection support; the
   rapid focus churn scenario recreated; the cold-start loop; the
   Settings, Explorer, and Start search scenarios; the arbitration
   lifetime check; the crate guides for outpost, app, core, and model;
   and the architecture amendments above.

Reducer tests to retire or change in step 1, each confirmed against what
it covered before removal:

- removed: `out_of_order_version_triggers_fetch_for_focused_node`, the
  three `fetch_completed_*` staleness tests other than the unknown query
  id one, and the six observation-time tests from
  `a_stale_window_focus_is_spoken_but_never_moves_focus` through
  `a_zero_observation_always_proceeds`;
- reversed: `a_named_window_ancestor_is_never_announced_as_context` and
  `property_changed_name_on_focused_node_updates_silently`;
- rewritten: the two replay tests, whose script relies on a stale
  version, and the committed replay fixture, regenerated because recorded
  inputs change shape.
