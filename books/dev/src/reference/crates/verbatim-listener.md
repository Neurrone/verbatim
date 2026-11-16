# verbatim-listener

The focus listener process, `verbatim-listener.exe` (decision D13; ADR
0003, "Readers tell, Core asks"): one permanent process holding the
desktop-global UI Automation registration and the global MSAA hooks,
telling Core of every event it hears and answering Core's requests on what
it kept (`docs/design/readers-tell-core-asks-build1.md`, "The listener in
this build"). Core's supervisor ([verbatim-supervisor](verbatim-supervisor.md))
starts it with `--listener` and its arguments
(`verbatim_process::arguments::ListenerArguments`; `--hooks-in` is read and
not used), and replaces it when it ends. What it decides without a call,
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
  (`verbatim_listener_logic::kept`): swept at each tell past 60 seconds or
  beyond the newest 64, released at the application's exit, and those
  read by a request that is not a focus read released as the read ends. A
  focus read's element is kept, read and not yet named, until the next
  focus request names the readers' last focus's token, which keeps that
  one and lets every other read element go, across applications. Every
  element let go is released as a job on its application's reading
  thread, since a release can call into it.
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
  (`focus_read::CallLog`).
- `focus_read::FocusReader` is the unasked focus read that mockapp's
  integration harness still drives as its stand-in for the listener,
  until that harness moves onto the request driver; the listener process
  does not use it.
- `focus_read::read_object` is the listener's answer to Core's generic
  read of an element, the read a reading thread makes for a kept element,
  for a test that stands in for the listener holding the element itself;
  `focus_read::read_object_observed` is the same read, telling a test the
  name of each call before it makes it, so the test can change the
  application's windows between two of them.

## Progress bars

The listener is the one owner of every progress bar's changes,
foreground and background, classic and UI Automation
(`docs/design/progress-bars.md`), and Core judges whether each is heard.
A UIA bar's range value change arrives in the desktop-wide group with its
range cached; `bar_percent` drops anything but a progress bar, with no
call, as it does UIA's proxy for a classic window, and the reading thread
of the bar's application reads its place and its nearest window, once per
change and memoised by runtime id, and sends a`BarChange`. A classic
window's `EVENT_OBJECT_VALUECHANGE` reaches the global hook. A window of
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
  object, and a hung or ghosted window. A tell carries the event's object,
  its raising thread, the process that serves the window (a console
  window's and a destroyed window's the process of that thread), that
  process's executable name, read once per process held open
  (`Applications`), the input desktop's openness for a switch, and
  acceptance's window facts, the window's own read as the event arrives
  and the foreground's as the tell is placed. Every tell, and the idle
  barrier's answer, is placed under one lock (`Tells`), behind a
  foreground event whose window `GetForegroundWindow` does not yet name
  until it names it or 20 ms pass, on a threadpool timer armed only while
  the hold keeps tells. A menu's or the switcher's end, a desktop switch
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
  later focus or foreground event that finds it in front, unless a newer
  foreground event replaces it (`foreground::ForegroundWatch`). The
  console's events stay unhooked (`verbatim_ia2::suspend_kinds`) unless
  it starts with a console window in front, until one of that window's
  events names its thread.
- It handles `Ping`, `ApplicationExited` (which ends the application's
  check thread and moves its reading record to the exited generations,
  then answers `ExitHandled`), `IdleQuery`, `Request`, `Cancel` and
  `Shutdown`; `HookInstalled`, `HookRemoved` and `LiveReread` change
  nothing, as no application's events are handed over and no unasked read
  is kept. It makes no remote operation, so takes no `--classic-uia`.
- The idle barrier's answer goes behind every event delivered to the
  `WinEvent` thread before it, every job queued for the reading threads,
  every event handed to the check threads, and every tell placed before
  it; abandoned threads do not count.
- `listener::capture` and `listener::capture_selection` are what the
  notification and element-selected callbacks capture of their event's
  element, and `listener::bar_percent` and `listener::read_classic_bar`
  what the progress path reads, public so mockapp's tests make them as the
  listener does.
