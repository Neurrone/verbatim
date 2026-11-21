# verbatim-listener

The focus listener process, `verbatim-listener.exe` (decision D13; ADR
0003, "Readers tell, Core asks"): one permanent process holding the
desktop-global UI Automation registration and the global MSAA hooks,
telling Core of every event it hears and answering Core's requests on what
it kept (`docs/design/readers-tell-core-asks-build1.md`, "The listener in
this build"). Core's supervisor ([verbatim-supervisor](verbatim-supervisor.md))
starts it with `--listener` and its arguments
(`verbatim_process::arguments::ListenerArguments`), and replaces it when
it ends. What it decides without a call,
its check threads' calls, its kept elements, the reading threads'
schedule, the tells' drops and the foreground watch, is
[verbatim-listener-logic](verbatim-listener-logic.md); the class verdict
is [verbatim-reader-rules](verbatim-reader-rules.md)'s. The reasons for
this shape are in `books/dev/src/explanation/architecture.md`, section 1
("The focus listener (D13)").

## Tells, checks and Core's requests

- Shape. The registration and group are shaped as
  `verbatim_uia::ListenerEvent` (`Part::shaped`): NVDA's base cache request
  with `IsDialog` and `IsTogglePatternAvailable` added
  (`books/dev/src/explanation/design/uia-shapes.md`, decision 6), so the
  role is refined as every other read refines it. The element-selected
  part alone is shaped as `verbatim_uia::ListenerSelection`, which adds the
  item's value, states and details, its states told with the event.
- The callbacks make no cross-process call. A UI Automation event
  received before the subscriptions are installed, with no process, or
  whose sender's cached window the listener's one readability reader
  answers hung or ghosted is dropped there, as NVDA's UIA handlers drop
  it (`_shouldSkipEventForHungWindow`); a sender with no cached window is
  not tested. A terminal text area's notifications and a window opened
  that is no toast candidate are dropped by their cached class and
  automation id. Every other event goes to its application's check thread (`listener::checking`,
  `Checks`), which makes the calls NVDA's handler for the event's kind
  makes at delivery (`verbatim_listener_logic::check`), with its own UI
  Automation client, then places the tell under the check threads' lock
  and the tells' lock, so no tell of an ended application is sent and the
  idle barrier's answer follows the tells of every event handed before it.
  A check ends at a window read through MSAA, as NVDA's handler returns
  when its native-element test fails. A thread exits after 30 seconds with
  nothing to check; at shutdown one inside a call is let go. A progress
  bar's change keeps no element: its tell names it by its cached runtime
  id alone (`EventObject::Followed`), and the check thread lets the
  element go at the end of its check.
- Kept elements (Table 6). Each told element is kept behind the token its
  tell names, in `Kept`, one record per application
  (`verbatim_listener_logic::kept`), until Core's held view leaves it out
  (`CoreToListener::ElementsHeld`). The listener's generation, given on
  its command line (`--generation`), is in every view and every request
  naming a kept element: one naming another generation is dropped, or
  answered "not there". The writer numbers each message carrying a token
  as it writes it, in pipe order, in its backlog (`Waiting`, its
  `Reported` record, under the writer's lock, taken after the kept
  elements' lock), and a view releases only an element whose tell was
  written at or before the position it acknowledges. A read that is not
  a focus read releases its element as it ends; a focus read's element is
  kept, read, unless the last view rules it out; a request cancelled for
  the foreground lets the kept reference of the element it reads go.
  Every element a view or the application's exit lets go is released as
  a job on its application's reading thread, which has a UI Automation
  client; never under a lock or on a callback thread, since a release can
  call into the application.
- Requests (`listener::asked`). Core's `CoreToListener::Request` is the
  generic read of the object an event names (`Request::ReadObject`); any
  other request, a relation, or descendants are refused unsupported, and a
  node is none of the listener's. A kept element is read on its
  application's reading thread: the element afresh, its name and class
  from its event's cache, as NVDA's object for the event reads them,
  except a selection's item (`Purpose::Selection`), whose name is the one
  read afresh in the same cache request, by Dickson's ruling of
  2026-10-20 ([changes and
  selection](https://github.com/Neurrone/verbatim/blob/main/docs/parity/changes-and-selection.md),
  "Selection in a list the focus controls"), the
  focused state added for a focus read, a menu item's legacy checked
  state; for a read that walks, its ancestors by the classic walk up to
  the first Core already knows (a window by its own element's runtime
  id), the desktop root or the process boundary, with the element's route
  (`Route`: each element read with its runtime id, its own window and
  whether it is a control and content element, and where the chain goes
  on: at the known ancestor it met, naming it, at the process boundary or
  at the root, from a window after a failed step or at a ghost's or hung
  application's window, or by plain windows after the depth limit of
  64). Each node read carries its nearest window's facts (`NodeWindow`,
  `verbatim_win32::window_facts::node_window`) and the properties of the
  request's set; the listener answers none for a keyboard focus or an
  object's text, which are the check's and the outpost's. An MSAA object
  at an event's address is read for the set asked of its name, role,
  value, states and place, any other set refused, on the reading thread of
  the window's process (`verbatim_ia2::acquire::read_at_event`): the
  desktop window's name, and a classic progress bar's role, value, states
  and place, which Core asks at its value change. A token released, or
  never kept, is answered not there at once.
- Reading threads (`reading`, the shell of
  `verbatim_listener_logic::schedule`): one per application process
  generation, Core's requests ahead of releases, an idle
  thread letting its record go after 30 seconds. Each request is answered
  once (`ListenerToCore::Answer`): by its read, or for one ended unrun
  cancelled, not there at its process's exit, or dropped unanswered at
  shutdown. Core's `CoreToListener::Cancel` drops a queued request,
  answered cancelled; a running one is abandoned for the foreground, with
  a fresh thread for what waits and its late result discarded, within
  `ABANDONED_LIMIT` (two) abandoned threads, past which jobs wait for the
  first to return, and left alone for any other reason, its answer sent
  late. A thread makes its calls with no lock held; the schedule and the
  threads' wakes sit under one lock, so a request's answer goes ahead of
  the idle barrier's answer its end lets run.
- Calls. Every call waits for the application, up to UIA's transaction
  timeout. A call over a second is logged at info
  (the private `call_log` module's `CallLog`).
- `read_object` is the listener's answer to Core's generic read of an
  element, the read a reading thread makes for a kept element, for a test
  that stands in for the listener holding the element itself, and
  `read_msaa_object` its read of an MSAA object at an event's address;
  `read_object_observed` is the same read, telling a test the name of
  each call before it makes it (`CallObserver`), so the test can change
  the application's windows between two of them.

## A window's show

The listener tells Core of a window object's show only while its window
is the window in front (`GetForegroundWindow` as the hook's callback
runs), as an ordinary `Show` tell on the window's client object, whatever
its class: Core reads a dialog it postponed at that show
(`books/dev/src/explanation/design/focus.md`, "A dialog read once it is
shown"). The rule is the hook's (`verbatim_ia2`'s `shown_in_front`), one
local call for each window object's show; the listener keeps no request,
waiter or record for it.

## Progress bars

The listener reads no bar unasked; Core judges every bar's change at its
turn and asks what it needs ([ADR
0003](../adr/0003-readers-tell-core-asks.md), "Progress bars"). A classic
bar's `EVENT_OBJECT_VALUECHANGE` is told like any other `WinEvent`, and
Core asks the listener to read the object at its address (role, value,
states and place), an ordinary request on the application's reading
thread. A UIA bar's range value change arrives in the desktop-wide group
shaped as `verbatim_uia::ListenerProgress`, its range, rectangle and
off-screen state cached; the callback's `bar_of` keeps only an on-screen
progress bar whose provider is its own, not UIA's proxy for a classic
window, and whose range is a number, and parses its percentage and the
centre of its rectangle, (0, 0) with none, with no call. The bar is then
handed to its application's check thread as every UIA event is
(`UiaProgress`, NVDA's automation-event handler's calls: the nearest
window, its probe and its acceptance facts) and told with the percentage
and place (`Payload::Progress`).

Limit: a bar read of an application that hangs during the read holds
that application's reading thread until the call returns, so its later
requests wait up to Core's due time; other applications have their own
threads.

## Public API

`run_listener` is the listener runtime (`docs/design/communication.md`,
section 6.3).

- Its main thread is its I/O thread (completion port,
  `THREAD_PRIORITY_HIGHEST`); every thread that produces a message writes
  it itself, encoded into the sender's one buffer. While a write is in
  flight, pongs, `Ready` and faults go first; waiting tells merge one per
  object, kind and raising thread; answers and reports go in the order
  they are made.
- It installs the global MSAA hooks (`LISTENER_SUBSCRIPTIONS`). Every
  event becomes a tell (`ListenerToCore::Tell`, the module
  `listener::tells`) unless NVDA's local drops take it
  (`verbatim_listener_logic::tell`): the window of 0 or an invalid one
  mapped or dropped, a destruction of anything but a window, Windows Live
  Messenger's hidden window, a show of neither a client nor a custom
  object, and a hung or ghosted window. A window's destruction is no tell:
  it goes as `ListenerToCore::Destroyed` with the window, the process of
  the raising thread, or none when that thread cannot be opened (a
  ghost's, raised on the desktop window manager's thread, or an exited
  thread's), and the time, so none is dropped for its process; it is
  placed in the tells' order through the foreground hold (`Held::Destroyed`)
  and waits on the lane that keeps order and never merges. A tell carries
  the event's object, its raising thread, the process that serves the
  window (a console window's the process of that thread), that
  process's executable name, read once per process held open, in the
  listener's one record under the check threads' lock (`Applications`),
  the input desktop's openness for a switch, and
  acceptance's window facts, the window's own read as the event arrives
  and the foreground's as the tell is placed; a UI Automation focus, menu
  opening, element selected or window opened has all of them read by its
  check thread where NVDA judges acceptance (`Call::Acceptance`,
  `PendingWindow::Whole`). Every tell, and the idle
  barrier's answer, is placed under one lock (`Tells`); an MSAA tell and
  the idle barrier's answer wait behind a foreground event whose window
  `GetForegroundWindow` does not yet name until it names it or 20 ms
  pass (`verbatim_listener_logic::foreground::Pending`), on a thread
  timer of the `WinEvent` thread armed only while the hold keeps tells
  (`verbatim_ia2::event_thread::ThreadTimer`), and a UI Automation tell
  is sent at once (`Tells::pass_hold`). Only the `WinEvent` thread steps
  the pending foreground, on an event or the timer; the idle barrier's
  answer, placed from another thread, releases nothing. A menu's or the switcher's end, a desktop switch
  and the desktop window's name change are tells like any other; their
  delays are Core's. Through one `Registration`, the focus handler
  (`Subscription::Focus`) then element selected, menu opened,
  notifications, window openings and range values, as one group on the
  desktop root, and says `Ready` only once all are in.
- For every foreground event whose window `GetForegroundWindow` names on
  the `WinEvent` thread, whatever its process, Program Manager's and the
  taskbar's included, the listener sends `Foreground` (window, its root
  owner by `GetAncestor` with `GA_ROOTOWNER`, the process holding
  attention, the console host's for a console window whose thread an
  event named, when the callback observed the event, the event's thread,
  whether a console host window, integrity level by
  `verbatim_win32::integrity::process_integrity`), none replacing another,
  and once its hooks are in, the window then in front
  (`Nudge::ForegroundStart`). An event whose window is not in front is
  the pending foreground, reported with the observation of a later event
  that finds it in front, or with its own when the hold's timer finds
  it, unless a newer foreground event replaces it. Each `WinEvent` steps
  the pending foreground with its tell under the tells' lock
  (`on_win_event`), and the window a step finds in front is reported
  before the tells that step releases, so Core takes it as the
  foreground before it judges them; the watch
  (`foreground::ForegroundWatch`) then hears of it, which unhooks the
  consoles' events, on the `WinEvent` thread. The step judges the event
  on one read of the window in front (`Tells::front`). Each fact of the
  event's windows is read where it is needed, its window's class not at
  all when the hook's filter read it; every read of the path goes
  through the event's reads (`EventReads`). The foreground's part of a
  tell's facts is read live under the tells' lock as the tell is placed.
  The console's events stay unhooked (`verbatim_ia2::suspend_kinds`) unless
  it starts with a console window in front, until one of that window's
  events names its thread.
- It handles `Ping`, `ApplicationExited` (which ends the application's
  check thread and moves its reading record to the exited generations,
  then answers `ExitHandled`), `IdleQuery`, `Request`, `Cancel`,
  `ElementsHeld` and `Shutdown`. `run_listener` takes the ignored
  processes, held open for its life. It makes no remote operation, so
  takes no `--classic-uia`.
- The idle barrier's answer goes behind every event delivered to the
  `WinEvent` thread before it, every job queued for the reading threads,
  every event handed to the check threads, and every tell placed before
  it; abandoned threads do not count. An answer made before the listener
  has said `Ready` is held until `Ready` is sent and goes behind it, so a
  barrier that follows the listener's restart passes only once its UIA
  subscriptions are installed.
- `listener::CheckEvent` (its `new` and `into_tell`), `listener::cached_of`,
  `listener::check_event`, `listener::Pending::finish`,
  `listener::msaa_tell` and `listener::written_together` are the check
  thread's capture, cached properties, check and tell assembly, the MSAA
  tell with its local drops, and the writer's merge of waiting tells,
  with the window reads and the clock as parameters, public so mockapp's
  driver runs the listener's own code.
- `listener::capture` and `listener::capture_selection` are what the
  notification and element-selected callbacks capture of their event's
  element, and `listener::bar_of` what the progress callback makes of a
  bar's event, public so mockapp's tests make them as the listener does.
