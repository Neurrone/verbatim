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
  automation id. Every other event but a progress bar's range-value
  change goes to its application's check thread (`listener::checking`,
  `Checks`), which makes the calls NVDA's handler for the event's kind
  makes at delivery (`verbatim_listener_logic::check`), with its own UI
  Automation client, then places the tell under the check threads' lock
  and the tells' lock, so no tell of an ended application is sent and the
  idle barrier's answer follows the tells of every event handed before it.
  A check ends at a window read through MSAA, as NVDA's handler returns
  when its native-element test fails. A thread exits after 30 seconds with
  nothing to check; at shutdown one inside a call is let go.
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
  Every element a view lets go is released on its application's check
  thread while one runs (`CheckJob::Release`), which checked it and has a
  UI Automation client, else as a job on its reading thread; at the
  application's exit, on its reading thread; never under a lock or on a
  callback thread, since a release can call into the application.
- Requests (`listener::asked`). Core's `CoreToListener::Request` is the
  generic read of the object an event names (`Request::ReadObject`); any
  other request, a relation, or descendants are refused unsupported, and a
  node is none of the listener's. A kept element is read on its
  application's reading thread: the element afresh, its name and class
  from its event's cache, as NVDA's object for the event reads them, the
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
  is read only for its name at the event's address
  (`verbatim_ia2::acquire::name_at_event`), the desktop window's name, on
  the reading thread of the window's process. A token released, or never
  kept, is answered not there at once.
- Reading threads (`reading`, the shell of
  `verbatim_listener_logic::schedule`): one per application process
  generation, Core's requests ahead of bar reads and releases, an idle
  thread letting its record go after 30 seconds. Each request is answered
  once (`ListenerToCore::Answer`): by its read, or for one ended unrun
  cancelled, not there at its process's exit, or dropped unanswered at
  shutdown. Core's `CoreToListener::Cancel` drops a queued request,
  answered cancelled; a running one is abandoned for the foreground, with
  a fresh thread for what waits and its late result discarded, within
  `ABANDONED_LIMIT` (two) abandoned threads, past which jobs wait for the
  first to return, and left alone for any other reason, its answer sent
  late. A thread makes its calls with no lock held; the schedule, the
  threads' wakes and the alarm sit under one lock, so a request's answer
  goes ahead of the idle barrier's answer its end lets run. The watchdog
  thread sleeps on an alarm set only while a bar read runs.
- Calls. Every call waits for the application, up to UIA's transaction
  timeout. A call over a second is logged at info
  (the private `call_log` module's `CallLog`).
- `read_object` is the listener's answer to Core's generic read of an
  element, the read a reading thread makes for a kept element, for a test
  that stands in for the listener holding the element itself;
  `read_object_observed` is the same read, telling a test the name of
  each call before it makes it (`CallObserver`), so the test can change
  the application's windows between two of them.

## Progress bars

The listener is the one owner of every progress bar's changes,
foreground and background, classic and UI Automation
(`docs/design/progress-bars.md`), and Core judges whether each is heard.
A UIA bar's range value change arrives in the desktop-wide group with its
range cached; `bar_percent` drops anything but a progress bar, with no
call, as it does UIA's proxy for a classic window, and the reading thread
of the bar's application finds its nearest window, memoised by runtime
id, then reads its place, once per change, and sends a `BarChange`; a bar
that reports no rectangle is placed at (0, 0), as NVDA places one whose
location it cannot read, and only an off-screen bar is dropped. A classic
window's `EVENT_OBJECT_VALUECHANGE` reaches the global hook.

While background progress bars are not reported, the listener skips the
read of a value change whose window the acceptance test refuses, as NVDA's
acceptance drops a background value change before it makes the object
(Dickson, 2026-11-19), judged by Core's own function
(`verbatim_core::accepts`, with background bars not reported), so the
rule is written once; Core still judges every tell and every
`BarChange` against its own setting. The setting is the listener's
configuration (`listener::configuration`): `BackgroundBars`, reported or
not, from its launch argument (`--background-bars`), then replaced by
each `CoreToListener::Configuration` the I/O thread handles, held by a
monitor in `Outgoing` with one setter and one getter, neither running a
caller's code; the `WinEvent` thread, the UI Automation callback and the
reading threads read it as they judge. A classic value change is judged
on the `WinEvent` thread right after `knowledge.drain()`, before any lock,
job or call, on the facts of its own tell as it was sent in this event's
step (`Told`, which `tell_win_event` returns: `Tells::place_seen` and
`Tells::foreground` give the facts of the tell whose trace id is the
event's, whatever else the step releases): one sent is read while
background bars are reported, and otherwise only when acceptance takes
its window; one the foreground hold keeps is read, its facts not yet
read; one not told is never read, so Windows Live Messenger's hidden
window is sent no `WM_GETOBJECT` (`reads_value_change`). The tell is kept
either way, so Core's acceptance, its limiter's count and the caret's
count of a waiting value change are unchanged. A UIA bar with its own
window is judged in the callback, after the hang check and before it is
queued, on its window's facts read then with local calls and none into
the application (`admits_bar`); a windowless bar is judged on the reading
thread once its nearest window is found, before its place is read
(`uia_bar_place`). While the setting is reported, neither path reads a
window fact for the gate. Limits: a change judged before Core's command
reaches the I/O thread is judged under the value one message old, so
right after the setting is turned on one change of each background bar
may go unread (accepted by Dickson, 2026-11-19); and an off-screen
windowless bar's first change costs its window lookup, in both states. A window of
a class NVDA names as a bar, visible, is read by its window messages. Any
other is judged on the event thread by `value_change.rs`, the one record of
what each application's windows and objects were found to be, which
nothing else reads or writes, so an event dropped there takes no lock and
makes no call. A window with a provider NVDA uses is dropped; a window with
no answer has one job that judges it (its class lists, then the raw probe
and the checks after it, as the outposts' arbitrator does) and reads the
event's object if it has none; a change arriving while that first answer is
awaited is dropped, as Dickson decided. A kept "no" past its 500 ms is
asked again, and its old answer used meanwhile. A window with no provider
goes on to the object's memo: known not to be a bar it is dropped with no
call, known to be one its value is read through MSAA, and otherwise its role
is read first. A reading thread's answers come back over an unbounded
channel, drained at the start of each value change's callback. A job
carries a token whose drop tells the record the window is no longer being
asked, so a job dropped on any path answers it. The record holds up to 64
applications, the least recently used forgotten first, as no exit is
reported for a process with no outpost, and an answer is kept with its
window's owning thread, so a reused handle has none. The last MSAA focus's
object is exempt from the limits below, as NVDA's is.

Limits: a console window with a provider is answered as using UIA with no
check, because the check needs a UIA connection or a held element and the
listener makes no connection and holds none on this path, so an old
console with an incomplete provider differs from NVDA's test
(`nvda/source/UIAHandler/__init__.py` 1344 to 1345); its value changes
reach no bar. A bar's read, classic or UI Automation, runs behind Core's
requests on its application's reading thread, has a deadline of 1 second
and does not count towards `ABANDONED_LIMIT`, so it cannot starve a
request; it has its own count of abandoned reads still in their calls, at
most `BAR_READS_ABANDONED_LIMIT` (2) per application: at 2, the
application's waiting and further bar changes are dropped with no thread
started, clearing as each abandoned call returns. A focus read of the
application drops its waiting classic value changes. Neither path starts
an outpost or asks one. An outpost's own hook keeps the value change for
text and reports no bar.

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
  process's executable name, read once per process held open
  (`Applications`), the input desktop's openness for a switch, and
  acceptance's window facts, the window's own read as the event arrives
  and the foreground's as the tell is placed; a UI Automation focus, menu
  opening, element selected or window opened has all of them read by its
  check thread where NVDA judges acceptance (`Call::Acceptance`,
  `PendingWindow::Whole`). Every tell, and the idle
  barrier's answer, is placed under one lock (`Tells`); an MSAA tell and
  the idle barrier's answer wait behind a foreground event whose window
  `GetForegroundWindow` does not yet name until it names it or 20 ms
  pass, on a threadpool timer armed only while the hold keeps tells, and
  a UI Automation tell is sent at once (`Tells::pass_hold`). A menu's or the switcher's end, a desktop switch
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
  (`NUDGE_FOREGROUND_START`). An event whose window is not in front is
  kept as the watch's pending window and sent with the observation of a
  later event that finds it in front, unless a newer
  foreground event replaces it (`foreground::ForegroundWatch`). Each
  `WinEvent` feeds the watch before its tell is placed (`on_win_event`),
  so the report that a window came to the foreground reaches Core before
  the tells it decides, the foreground event's own and those the hold
  releases with it; the watch and the hold judge the event on one read
  of the window in front (`Tells::front`). The facts of the event's
  windows are read once for the event (`EventWindows`, over
  `verbatim_win32::window_facts::ReadOnce`), its window's class not at
  all when the hook's filter read it, and shared by the watch, the tell
  and the progress path; every read of the path goes through it
  (`EventReads`). The foreground's part of a tell's facts is read live
  under the tells' lock as the tell is placed, and a value change's hang
  judged again by the listener's one judge after its tell is sent. The console's events stay unhooked (`verbatim_ia2::suspend_kinds`) unless
  it starts with a console window in front, until one of that window's
  events names its thread.
- It handles `Ping`, `ApplicationExited` (which ends the application's
  check thread and moves its reading record to the exited generations,
  then answers `ExitHandled`), `IdleQuery`, `Request`, `Cancel`,
  `Configuration` (the listener's configuration, above) and `Shutdown`.
  `run_listener` takes the launch's `report_background_progress_bars`
  with the ignored processes, set before any value change is handled. It makes no remote operation, so takes no `--classic-uia`.
- The idle barrier's answer goes behind every event delivered to the
  `WinEvent` thread before it, every job queued for the reading threads,
  every event handed to the check threads, and every tell placed before
  it; abandoned threads do not count. An answer made before the listener
  has said `Ready` is held until `Ready` is sent and goes behind it, so a
  barrier that follows the listener's restart passes only once its UIA
  subscriptions are installed.
- `listener::capture` and `listener::capture_selection` are what the
  notification and element-selected callbacks capture of their event's
  element, and `listener::bar_percent`, `listener::uia_bar_place` (with
  the gate for a windowless bar's window as its last argument) and
  `listener::read_classic_bar` what the progress path reads, public so mockapp's tests make them as the
  listener does.
