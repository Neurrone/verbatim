# verbatim-outpost

The per-application outpost process, the focus listener, the Core-side
supervisor, and the private protocol between them (architecture sections 1
and 4, decisions D9 and D13, implemented in full — generalized from M1's
single instance to the many-concurrent-processes design at the end of M2,
then split so a dedicated listener detects focus and the per-app outposts
announce it).

An outpost's target application is fixed at spawn and never retargeted:
the pid arrives on its command line (`--target-pid`), hooks and UIA
registrations install once during construction, and there is no rebind
path. Outposts stay alive when their application loses foreground.

Focus detection is a separate process (decision D13). The focus listener —
the same `verbatim-outpost.exe` run with `--listener` and no target pid —
holds the one desktop-global UIA focus registration and the global MSAA
hooks for focus, foreground, and menu-popup opens (process id zero), under a
hard rule that it never makes a cross-process call: it reads only what each
event carries (a UIA element's cached properties, an MSAA event's raw
address) plus the hang-safe `GetWindowThreadProcessId`, and forwards each
captured focus fact to Core. The supervisor routes every fact to the target
application's own outpost, which acquires, arbitrates, enriches, and
announces it exactly as it does for the events it still hooks itself. Node
identity never crosses a process: the listener forwards a UIA runtime id,
and the receiving outpost mints the `NodeId` from it. There is no announce
poll: at startup, and after an outpost or the listener is replaced, Core
asks the application for its current focus with a single focus-now query.

Public API:

- `protocol` — the wire vocabulary the supervisor and each outpost speak.
  `EventTiming` records, in microseconds from `now_us`, when an event was
  observed, relayed to the outpost by Core, taken from the outpost's queue,
  and published to Core, and for a WinEvent how long before it was observed
  Windows raised it, and the cross-process calls the worker made for it
  (`calls`, a `CallCounts`). A caret key's reply answered by a later
  check of its watch also says when the evidence that answered it was
  taken from the queue (`awaited_at_us`) and how many of its calls the
  checks before it made (`awaited_calls`, a part of `calls`); the
  listener's `FocusFact`, Core's
  `DeliverFact`, and the outpost's `Event` and `Reply` carry it, and Core's
  latency ledger reads it.
  `SupervisorToOutpost`: `SetBackendOverride` (forces one backend for every
  window of the target, or restores normal arbitration), `DeliverFact` (a
  focus fact the listener captured, routed to this outpost — a UIA focus
  element's cached snapshot parts, an MSAA focus or menu-popup address, or a
  foreground window — carrying the listener's own trace id and observation
  timestamp so the latency timeline starts at the OS event, and its
  `EventTiming`), `Query` (a
  request id and a `Query`: `FocusNow`, `Navigate` with a model `QueryKind`,
  `Activate`, `Ancestors`, `DumpTree`, or `Text`, a node and a model
  `TextOp`, milestone M4's text protocol), `Cancel` (withdraws a query that
  has not started), `Ping`, and `Shutdown` (shut down cleanly and exit,
  under "Shutting down" below). `OutpostToSupervisor`: `Ready`, `Event`
  (trace id, observation timestamp, backend, the event window's
  `WindowFacts`, normalized event), `Reply` (exactly one per accepted query,
  echoing its request id, with its `EventTiming`, default for a query
  withdrawn or abandoned, and a `QueryOutcome`: `Done` with a
  `QueryResult`, `Gone` when the node is no longer reachable, `Failed` with
  a reason, `NotStarted` when it was withdrawn before it ran, or `Abandoned`
  when it started and passed its deadline, so side effects such as an
  activation may already have happened), `Pong` (echoes the ping's sequence
  number and reports how many abandoned workers have not yet returned, which
  the supervisor watches), `Fault`, `FocusFact` (sent only by the
  listener), and `TargetExited` (the outpost's application has exited,
  sent once, ahead of ordinary messages, so the supervisor shuts the
  outpost down). A `FocusNow` answer carries the application's foreground
  window and its facts, when it holds the system foreground, and its focused
  control with ancestors, selected child, and window facts, and the time
  the outpost began reading it, which orders it among the focus events. A
  `ListenerFact` strips to a pid-less `DeliveredFact` once the supervisor
  has routed it. Node ids arrive from Core stamped with this outpost's id;
  the outpost looks them up with the stamp cleared (`NodeId::unstamped`).
  `OutpostToSupervisor::assign_outpost` is the stamp Core applies to every
  node id in a message. Framing is newline-delimited compact JSON via
  `write_message` and `read_message`.
- `Arbitrator` — NVDA's per-window backend decision (`_isUIAWindowHelper`):
  `verdict(hwnd, &WindowClasses)` answers from the good class list, the
  Windows 11 shell rule (the root ancestor's class is a shell top-level
  window, and the window is not the Start button), the bad class list, a
  forced override from `SetBackendOverride`, or a kept probe result, and
  `None` when only the `UiaHasServerSideProvider` probe can decide; the
  worker then probes and calls `record_probe`. `WindowClasses::of(hwnd)`
  reads the window's raw class, its class normalized as NVDA normalizes it
  (`normalize_class_name`: NVDA's class map, and the Windows Forms and
  `ATL:` wrappers removed), and its root ancestor's class. When the probe
  finds a provider, `post_probe_check` names the check NVDA makes before
  using it (a console's formatted text, a list view's Windows Forms
  origin); a provider that fails it is recorded with `record_excluded` and
  is MSAA for the window's lifetime. A probe that finds a UIA
  provider is kept for the window's lifetime and dropped by `forget` when
  the window is destroyed, as decision D15 specifies. A probe that finds
  none is trusted for only `NEGATIVE_VERDICT_LIFETIME` (500 ms, NVDA's
  cache period) and then repeated: a busy or starting application answers
  the probe late or not at all, and the probe then reports no provider for
  a window that has one. The renewal is by time because no event says a
  window has begun to answer the probe. The time is the arbitrator's
  `Clock`, the system's unless `Arbitrator::set_clock` sets another, and
  `Arbitrator::now` reads it, for the worker's renewal of the verdicts an
  entry probed; `Outpost::set_arbitration_clock` sets an outpost's, for a
  test that counts calls and decides when a probe is made again. Both class lists are
  lifted from NVDA and pinned by unit tests naming their exact NVDA source
  locations, so a future NVDA sync is a diff of two lists: the bad list is
  `badUIAWindowClassNames` in `nvda/source/UIAHandler/__init__.py`, and the
  good list concatenates `goodUIAWindowClassNames` from the same file with the
  Windows 11 shell tuple from the Explorer app module's `isGoodUIAWindow`
  (`nvda/source/appModules/explorer.py`).
- `Outpost`, `run_pipe`, `run_attach` — the per-application outpost (the
  `outpost` module; outpost redesign, "Inside an outpost"). Its parts:
  - Intake (`outpost::intake`): the MSAA hook callbacks (`APP_SUBSCRIPTIONS`:
    value, state, name, selection, destroy, and a top-level window
    shown, for the fixed pid),
    the focus-following UIA property callback, and the reader's routed facts
    (focus, foreground, menus, and the listener's desktop-wide selections,
    notifications, and alerts) and queries only add an entry to the queue and
    return. A UIA callback captures the element's cached parts, its cached
    window handle, and an agile reference; it never calls into the
    application.
  - The focus-following UIA property subscription, following NVDA's
    selective registration on Windows 11: name, value, and state changes on
    the focused element only, since the reducer acts on no other element's
    changes (NVDA's scope also names the focus's ancestors, but UIA
    delivers no ancestor's event to it; `docs/parity.md`, "UIA event
    registration"). The worker moves it each
    time it reports a focus, without waiting. This replaces the
    subscriptions on the top-level windows that existed at spawn, under
    which a dialog or window opened later received no UIA events at all,
    and, since 2026-10-07, the registrations on each of the focus's
    reported ancestors.
  - The queue applies NVDA's limiter rules: one waiting entry per object and
    kind, a newer one replacing it and moving to the back; a batch is
    everything that accumulated while the worker handled the previous one;
    per batch the newest 4 focus events and the newest 10 other events per
    application UI thread are kept, the focused object's events always;
    events from a window the system reports hung (`IsHungAppWindow`) are
    dropped before any read; and within a batch only the newest foreground
    change and the newest focus are handled, with the newest menu opening
    last.
  - A focus change is never kept waiting behind reads of other objects
    (`intake::overtaken`, since 2026-10-07): in a batch that holds a
    foreground change or a focus, the events of objects that are neither
    the focus nor an object the change moves to (the focus's own object, a
    foreground window's window and client objects) are handled after it,
    and after any menu opening, in their own order; and a focus change that
    arrives while the worker is in the middle of a batch takes that batch's
    such events that have not started back into the queue, where the next
    batch puts them after it, the limits they already passed not applied
    again. Notifications, alerts, queries, and the events of the focus and
    of the object it moves to keep their place, and so does a state change
    on one of the focus's ancestors known at its own address whose changes
    are spoken (`Intake::set_judged`, since 2026-10-08): observed before
    the batch's first focus change, or in the same millisecond, it is
    handled before it, judged against the focus it was observed under,
    even when it was pushed after it, as a hook event can be after a focus
    the listener relays. The outcome then does not depend on batching:
    Core speaks it and then the new focus, or cuts it off when the focus
    moves into another top-level window or a menu, as NVDA, which handles
    events in order, does. NVDA reads another
    object's event with one call and judges it against the focus when it
    runs; the outpost reads a whole snapshot, and the first focus in a new
    File Explorer window waited 2.6 seconds behind such reads while
    Explorer built the window (`docs/performance.md`, "A focus behind other
    objects' events").
  - The worker (`outpost::worker`): one thread takes entries in order and
    finishes each before the next. A caret key's evidence is watched for
    between entries (under "Text" below), not waited on. It is the only
    thread that calls into the application, so events and replies leave in
    the order their entries were planned: the order they joined the queue,
    but for the events a focus change overtakes and a caret key's answer,
    which leaves when its evidence comes. It replaces the announce lane, the query pool, the
    announce poll, the probe threads, and the late window retry. Being the
    only such thread, it is where the calls are counted: the backend crates
    count each call on the thread that makes it, and the worker takes both
    counts when it publishes an event or a reply, which carries them in its
    `EventTiming`. Calls an entry makes without publishing, for an event it
    drops, are taken when the entry ends, logged, and belong to no trace
    (`docs/performance.md`, "Cancelled traces").
  - The watchdog abandons a worker whose call passes its deadline (an
    event, a focus, or a focus-now query 10 s, NVDA's normal watchdog
    timeout, since an application that is starting up can take seconds to
    answer a read that then succeeds; a navigation or activation 400 ms; an
    ancestor walk or tree dump 5 s), or, after half a second, once the user
    has moved to a window of the same application on another UI thread
    (the decision is `abandon_reason` and `next_check`, the window test
    `is_another_thread_of_its_application`, each unit-tested), answers the stuck query
    `Abandoned`, and starts a replacement that continues with the queue. An
    abandoned worker that returns publishes nothing, since publishing checks
    under the watchdog's lock that the worker is still in charge, lowers the
    abandoned count, and exits. Nothing is sent while that lock is held:
    publishing claims the entry's result under the lock and queues the
    message after releasing it, and the watchdog releases it before it
    queues an `Abandoned` answer, so the watchdog never waits on the
    writer.
  - The reader (`Outpost::handle_command`, driven by `run_pipe`) answers
    pings itself, withdraws a cancelled query that has not started with a
    `NotStarted` reply, and queues everything else.
  - The writer (`outpost::outbound`), under "Messages to Core" below.
  `run_pipe` is the production mode over inherited pipe handles, which
  reads Core's commands until Core asks it to shut down, or the command
  pipe ends, and then shuts the outpost down cleanly (`Outpost::shutdown`,
  under "Shutting down" below); it also tells Core when the target
  application exits, from a thread that waits on the application's
  process. `run_attach`
  watches a pid directly, asks for its focus, and prints outbound messages as
  JSON lines to stdout, the standalone dev mode. Both take an
  `OutpostOptions`, fixed for the outpost's life, whose one field,
  `remote_operations`, says whether a UIA focus's ancestry may be read
  with a remote operation (`Outpost::with_options`; `Outpost::new` uses
  the default, on). `Outpost::with_focused_element_reader` takes a
  `FocusedElementReader` too, the function the worker reads a UIA focus's
  element with, which is otherwise the system's keyboard focus
  (`Uia::focused_element`): mockapp's tests hand a real outpost a UIA
  focus in a window that does not have the keyboard focus, and measure
  everything it does with it. `Outpost::set_foreground_reader` gives the
  outpost a `ForegroundReader`, the function it reads the foreground
  window with when it records the window a focus entered, which is
  otherwise `GetForegroundWindow`: an MSAA state change on that window's
  own object, when the window was the foreground as the focus entered it,
  is not spoken as an ancestor's, as NVDA's foreground object takes its
  place (`docs/parity.md`, "A top-level window's state change"), and
  mockapp's windows, on desktops where none takes the foreground, are made
  the foreground this way. `Outpost::settle` waits until the worker has
  handled everything queued, follow-ups included, the focus-following
  subscriptions have made every move asked of them, and every message
  published has been written to the pipe: the evidence those tests wait on
  before they read what the application answered, and before they assert
  the outpost said nothing more. `settle` covers only what has reached the
  outpost: an application's event reaches a client after the call that
  raised it has returned, on UI Automation's threads or the hook thread.
  `Outpost::observe_heard` hands an observer (`HeardObserver`) each event
  the outpost's own UIA handlers and `WinEvent` hooks take in, as a
  `Heard` (a property change by id, a text selection change, a text
  change, an active text position change, or a `WinEvent` by kind), once
  it is queued for the worker; a test waits to hear the events it had
  mockapp raise, then settles. The worker and its reads (`worker.rs`, `read.rs`), the
  intake, the writer, the protocol, and the supervisor's owner, policy, and
  writer modules forbid `unsafe` code: every UIA and MSAA read goes through
  the backend crates' safe wrappers, and what remains `unsafe` in the crate
  is window-manager calls (`outpost/window.rs`, arbitration's class
  reads), the event thread's message loop, process creation, and the
  inherited pipe handles.
- `text` (milestone M4) — the outpost's side of the text protocol
  (`docs/crates/verbatim-model.md`, "The text protocol"), public so
  mockapp's tests drive it as the worker does. `perform(source, anchors,
  op)` answers one `TextOp` but a caret key's watch, which `check_caret`
  checks, over a `TextSource`, a backend's view
  of one node's text in its own positions and UTF-16: `uia::UiaText`, over
  a text pattern, where a position (`UiaPos`) is one end of a text range,
  and `edit::EditText`, over an edit control's messages, where it is an
  offset (a plain edit control's word is the line segmented by
  `verbatim-text`'s word rules, each word with the white space after it,
  a no-break space read as a space, and a carriage return or line feed a
  word of its own, as NVDA finds it); `UiaText::new` takes the element too, which a remote operation
  starts from, and `remote` and `fetches` say whether caret reads try a
  remote operation and which formatting they read; `support` gives what
  the control is already known to support of the text attributes
  (`TextSupport`), and `known_support` what it is known to support after
  the reads, for the caller to keep. `caret_report` reads
  the caret's line and the selection, for `CaretMoved`, with the line's
  formatting when asked (the report after a focus), and remembers when
  its read finished. `Anchors` keeps one backend's anchors, by node, numbered
  from a counter both of an outpost's backends share, and never forgets
  those in its `HeldAnchors`, the anchors Core holds, which both of an
  outpost's backends share too (`Anchors::sharing`); `NodeText` is one
  node's, and its `position_at` mints a position at a backend position
  without a call, as the worker keeps an active text position change's
  range (`UiaPos::start_of`). `check_caret` checks a caret key's watch
  with one read of the caret and answers `Watched::Answered` with the
  key's reply once there is evidence, or `Watched::Watching`; it never
  waits. `CaretSignal` is what a check knows besides the text: whether a
  caret event prompted it, and the clock for when a caret was read (Unix
  milliseconds), so the unit tests run on a fake clock. Details under
  "Text" below.
- `dialog_text` — a dialog's own text, such as a message box's question,
  gathered from its children by NVDA's rules (`docs/nvda/object-model.md`,
  "A dialog's own text"), public so mockapp's tests gather it as the
  worker does. `dialog_text(&dialog)` runs the rules over a
  `DialogObject`, the trait each backend implements: MSAA through
  `verbatim_ia2::dialog::DialogObject`, UIA through `UiaObject`, whose
  children come with their properties cached, one call per container.
  `is_dialog(role)` says which nodes have such text: a dialog, an alert,
  or a property page. A gathering looks at no more than `MAX_OBJECTS`
  objects; a dialog with more says nothing rather than part of its text.
- `supervisor::IgnoredProcesses` — processes Verbatim ignores entirely,
  named by the end-to-end harness in `VERBATIM_IGNORE_PIDS`
  (`IGNORE_PIDS_ENV`; `from_env`, `hold`, `contains`, `parse_pids`): the
  owner's own Windows Terminal. Each is held open as a `Target` is, so its
  pid names that process and no other for as long as Verbatim runs.
  `Supervisor::new` and `with_executable` take them; the owner starts no
  outpost for one (`ensure_spawned` answers `NotWatched`) and routes none
  of its facts, and passes their pids to the focus listener as
  `--ignore-pids`, which holds them open too and drops every fact from
  them before it is queued. `Supervisor::is_ignored` lets the app pass
  them over (`crates/verbatim-outpost/tests/ignored_process.rs`, and the
  listener's `a_fact_from_an_ignored_process_is_never_queued`).
- `run_listener` — the focus-listener runtime (decisions D13 and D14;
  outpost redesign, "The focus listener"): sets up the writer, installs
  the desktop-global `FocusRegistration`, the global
  MSAA hooks (`LISTENER_SUBSCRIPTIONS`, pid zero: focus, foreground,
  menu-popup, menu and switcher end, alert, and a tooltip window shown,
  forwarded as `DeliveredFact::Show`, whose help balloon the worker
  reports as an alert), and desktop-wide UIA
  subscriptions for the events NVDA registers globally on Windows 11: an
  element selected, a menu opened, and notifications, registered together
  as one event handler group on the desktop's root element, and only then
  announces `Ready`, so focus and menus are seen from the moment it does
  (announcing first lost a menu opened right after start, found
  2026-10-06). The event thread (`EventThread::spawn`) likewise returns
  only once its hooks are installed. Each event becomes a `FocusFact` (a
  `ListenerFact`: the owning pid and a `DeliveredFact`) built entirely from
  cached and hang-safe local reads (`uia_focus_fact` builds a UIA focus's,
  public so a test hands an outpost the fact the listener would). A
  foreground event is forwarded without
  checking the foreground: a starting application's window raises it before
  it actually becomes the foreground window, so the check waits for the
  outpost's worker, as NVDA's waits for its main-thread pump. Outgoing facts are
  coalesced with NVDA's UIA limiter rule, one waiting fact per element and
  kind (`DeliveredFact::key`; notifications are never merged), with pongs
  and `Ready` first. It answers `Ping` with a `Pong` and ignores everything
  else. It holds no per-application state, so a crash respawns into full
  capability instantly. Range-value changes and live regions, which NVDA
  also registers globally, are not subscribed yet: the reducer speaks values
  only for the focus, and live regions are milestone M6's.
- `Supervisor` (the `supervisor` module; outpost redesign, "The
  supervisor") — `new(events_tx)` starts the lifecycle owner thread, which
  starts the focus listener at once (`with_executable` launches every
  child from a given executable with a given shutdown time limit, for the
  test that launches a stand-in); `shutdown()` shuts every outpost and the
  listener down for Verbatim's exit, returning once every one has ended
  with a `ShutdownSummary`, how many of the children that ended during
  Verbatim's life shut down cleanly, had already exited or exited with an
  error, or had to be killed; `ensure_spawned(pid)` starts an outpost
  without asking it to report anything (used once, at Core startup, for
  Core's own pid, so its outpost is warm before the first gesture);
  `send_to_outpost(outpost_id, command)` queues a command for one outpost
  incarnation without waiting, failing at once with a `QueueError` when that
  incarnation has ended or its queue is full; and `note_views(attention,
  holding)` passes the views the app derives from the reducer state. The
  app hears that the listener is ready, and whether it replaced one that
  ended, through `OutpostMessage::ListenerReady`, and of each incarnation
  through
  `OutpostMessage`: `Started` before
  any of its messages, `Event(pid, outpost_id, message)` for each message,
  and `Ended` with a reason (exited, killed, retired, or target exited).
  - The lifecycle owner (`owner`) is one thread that makes every lifecycle
    decision and owns the per-application records and the listener record.
    Readers, the heartbeat and sweep timers (`crossbeam_channel::tick`
    inside the owner's `select!`), launch helpers, and the app report to it
    over one channel; there is no shared map lock. Launching blocks, so the
    owner records the application as starting and hands the launch to a
    helper thread, which starts the child's writer, sends `Started`, starts
    its reader, and reports back; a second fact meanwhile is held, never a
    second launch.
  - Each child has a writer thread with a bounded queue (`writer`, capacity
    64), so the owner never writes to a pipe while deciding and the reducer
    thread never waits on one. When the queue is full a ping still gets
    through, a routed fact replaces any waiting fact for the same object and
    kind (or the oldest waiting fact makes room), and anything else, a query
    above all, fails at once. Each child has a reader thread that stamps
    every node id with the child's outpost id, forwards messages to the app,
    and reports `Ready`, pongs, and the end of its pipe to the owner. Core's
    thread count is two per child plus the owner.
  - Facts that arrive while an outpost is starting are held in arrival
    order, a newer fact for the same object and kind replacing the older one
    and moving to the back (NVDA's limiter rule), and released in that order
    on `Ready` (the pure `HeldFacts`).
  - Targets: the owner identifies each application by a process it holds
    open (`process::Target`), opened when the application's first outpost
    is started and kept while the application has an outpost or a crash
    history. Windows never gives a process's id to another process while a
    handle to it is open, so the pid the owner knows, and passes to the
    outpost, names that application and no other for as long as it is
    held. No outpost is started or replaced for an application whose
    process has exited, or that cannot be opened to be held; the app hears
    `OutpostMessage::NotWatched`, so nothing waits for one. Until an
    application is held, the pid in a routed fact, or in the app's request,
    is trusted to name the process that raised the fact or holds the
    foreground, which was observed moments before it is opened.
  - Crash: when an outpost's pipe closes, the owner ends it after the
    reader has forwarded everything it wrote. If its application has
    exited, it ended with its target (`Ended`, target exited), is not
    replaced, and the application's process and crash history are let go.
    Otherwise it ended unexpectedly (`Ended`, exited), and it is replaced
    at once only if its application holds attention; otherwise the next
    fact for the application starts one. After three crashes within a
    minute (`CRASH_LIMIT`, `CRASH_WINDOW`, the pure `CrashHistory`) it is
    not replaced until the next foreground change to that application;
    the app hears `OutpostMessage::NotWatched` after the third crash, since
    it may be waiting for a replacement to ask for the focus, and again for
    every request to start one meanwhile (`ensure_spawned`), so a focus it
    wants from that application is dropped rather than awaited for ever
    (`crates/verbatim-outpost/tests/target_gone.rs`). The
    sweep lets go of crash histories of applications that have since
    exited.
  - Hang: every child is pinged every three seconds; nine seconds without a
    pong ends it (`Ended`, killed). Eight abandoned workers end an outpost
    too, except while its application's windows are reported hung
    (`IsHungAppWindow`), when a replacement would hang the same way (the
    pure `wedge_decision`). A kill is followed by the crash rules, an
    application that has exited included.
  - Retirement: an outpost whose application has not held attention for two
    minutes, which is not Core's own, and in which the reducer holds no
    nodes, is ended (`Ended`, retired) by the sweep every 30 seconds (the
    pure `retirement_decision`).
  - Target exit: an outpost whose application has exited says so
    (`TargetExited`), and the owner ends it (`Ended`, target exited); it is
    not replaced. The heartbeat also checks every held application, and
    ends the outpost of one that has exited the same way, for an outpost
    that could not wait on its application's process.
  - Every ending shuts the child down cleanly (`retire`, on a thread of
    its own, under "Shutting down" below): unless it has already exited,
    the child is sent `Shutdown`, its command pipe is closed once that is
    written, and it is killed through its job only if it has not exited
    within `SHUTDOWN_LIMIT`. When the owner ends an outpost for any reason
    but the end of its pipe, `Ended` is sent at once and anything the
    child wrote afterwards follows it, which the app drops.
  - The focus listener is supervised the same way from its own record: its
    facts go to the owner, which routes them; it is pinged and replaced when
    it stops answering or its pipe closes, and a replacement's `Ready` asks
    the attention application to report again, since facts were lost in the
    gap.
  - Process creation (`process`): each child is launched through
    [verbatim-process](verbatim-process.md), spawned suspended into a
    kill-on-close job with a 200 MB memory cap and resumed, inheriting only
    its own two pipe ends and its log handle through a
    `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, so two launches running at once can
    never keep each other's pipes open. The module itself keeps only the
    role's command line and log name and the two local process and window
    queries the owner uses.

Implementation notes:

- Spawning (`Supervisor`, through `verbatim-process`): the outpost is
  created suspended with two
  anonymous pipes whose child ends are the only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost. A per-application outpost's command
  line carries `--target-pid`, fixing the watched application for its whole
  life, and `--classic-uia` when `uia.remote_operations` is off in
  `settings.toml` (`Supervisor::new` takes the `OutpostOptions` every
  outpost is launched with, which `verbatim-app` reads from the setting at
  startup); the listener's carries `--listener` and no pid.
  Each child is spawned with `STARTF_USESTDHANDLES` and an inheritable,
  append-mode file handle as its standard error (and output), so the outpost's
  and listener's own `tracing` output — which otherwise had no subscriber and
  went nowhere — lands in a per-role log file in this Verbatim launch's own
  log directory (`logs\<Verbatim's pid>\outpost-<target image>-<target
  pid>.log` and `listener.log`, next to the outpost executable; the
  synthesizer host's `synth-<id>.log` lands there too). The app calls
  `verbatim_process::prepare_launch_logs` at startup, before it starts
  any child, which empties
  that directory if an earlier process with the same pid left it and
  keeps only the newest ten launch directories. The outpost binary
  installs a stderr `tracing` subscriber at startup for exactly this; the E2E
  harness fetches these logs alongside the timeline and stderr, so a silent
  outpost is readable after the fact instead of theorized. Best-effort: a
  failed log open leaves the child unredirected, never unspawned.
- Foreground changes (the worker): a batch that holds a foreground fact
  is handed out at once. A foreground fact whose window is not the
  system's foreground window when the worker handles it is dropped, as
  NVDA's `processForegroundWinEvent` drops it: measured live on
  2026-10-09, Windows raises the foreground event once the change is
  made, and the events that come before are stale or raised early by the
  application itself, such as File Explorer's as it creates its window,
  which the system's own event follows (`docs/parity.md`, "Stale focus
  events"). The intake held such a batch for up to 250 ms, checking every
  10 ms, until then. A foreground window not yet shown waits for its show
  event (`WinEventKind::WindowShown`, the outpost's own process-scoped
  hook, never dropped by the limiter) and is reported then, its name read
  then, or just before a focus inside it that comes first; a newer
  foreground change replaces it (`Tracking::unshown_foreground`;
  `phase6-design.md`, "File Explorer opened without the foreground
  right"). Otherwise a foreground fact is reported at once,
  stamped with the time its window was confirmed as the foreground, after
  its name is read, rather than the time Windows raised the event,
  named or not, as a focus on the window, since the foreground change is what
  moves the reducer's attention; the reducer does not speak a nameless
  foreground window, and nothing announces it later. The window's own
  accessible object is read: UIA via `element_from_handle`, MSAA via the
  window's client object (`OBJID_CLIENT`), as NVDA reads a foreground
  window, so a focus event on the client area that follows is the same
  node and is not announced again; a popup menu window (`#32768`) reads
  the same way, as role menu (`read::window_snapshot`). A
  window whose accessible object cannot be read yet (a freshly created
  msinfo32 window, found live) is reported from local window data, and a
  window whose accessible name is still empty takes its window text
  (`InternalGetWindowText`, which never sends the window a message). A
  report whose window is no longer the system's foreground window when it is
  ready to send is dropped, as NVDA's `processForegroundWinEvent` drops it.
- Focus-now (the worker): the answer to Core's focus-now query is the
  target's foreground window, when it holds the system foreground
  (`GetForegroundWindow`, never `GetGUIThreadInfo`'s `hwndFocus`, which can
  name a child window such as Windows 11 Notepad's text area), and its
  focused control (`hwndFocus`, through that window's backend) with its
  ancestors and selected child. There are no retries: a nameless window is
  not announced later, a control that takes focus later raises its own focus
  event, and the query covers focus that settled before the listener
  existed.
- Hidden-frame suppression (decision D9): Core's hidden 1x1 main frame is
  marked with the `verbatim_model::HIDDEN_FRAME_WINDOW_PROP` window
  property by `verbatim-gui` (see that crate's section) and must never be
  announced — it transits real focus during the prePopup show/raise/force-
  foreground dance and would otherwise read as a nameless "Verbatim"
  window with role unknown, the first of the two M2 focus-timing races.
  Every `FocusChanged` emission path checks the property (`GetPropW`, which
  tolerates any handle and never blocks) before emitting: the MSAA event
  path (scoped to `id_child == verbatim_ia2::CHILDID_SELF`, re-exported
  from that crate's `com` module for exactly this check, so a child
  element's event on some unrelated window is never accidentally
  suppressed by hwnd coincidence), the UIA focus path, and the focus-now
  query (`focused_control` treats the hidden frame as "nothing focused").
  The foreground and focus-now window choices skip hidden-frame windows. The
  checks also walk to the window's top-level ancestor (`GetAncestor` `GA_ROOT`,
  another hang-safe local read): the marker property sits on the frame window
  only, so a control that lives in its own child `hwnd` inside the frame — a
  wxWidgets panel has one — is not caught by reading the marker on that child
  alone, and was announcing as a bare "pane" until the ancestor check was added.
  A marked window counts only when it belongs to Core's process, the
  outpost's parent (read once from a process snapshot), since any process
  can set the property on its own windows. The frame is named once, on
  purpose: when a popup menu opens while the frame holds the foreground
  (Verbatim+V), the menu's focus is reported in the frame's window, with
  the frame's own snapshot, read as a foreground window is read, as its
  only ancestor, so the reducer says "Verbatim" and then "Context menu",
  as NVDA names the foreground window a menu opens from
  (`docs/parity.md`).
- `verbatim-gui`'s `force_foreground` (see that crate's section) injects a
  bare `VK_CONTROL` tap before attempting `SetForegroundWindow`: a gesture
  that arrived via the control plane (no physical input, as every E2E test
  and any future remote session sends) fails Windows' foreground-lock
  heuristic and falls back to the slow `AttachThreadInput` path, observed
  at roughly two seconds; the tap satisfies the heuristic directly, cutting
  that to roughly 150 to 450 ms. `VK_MENU` was tried and rejected — a lone
  Alt press activates menu bars and bounces foreground straight back.
- Event flow (the worker): the event thread hosts the WinEvent hooks,
  installed once for the fixed target pid before the message loop starts
  (never rebound — a second live `WINEVENT_OUTOFCONTEXT` hook set on a
  thread that already has one has been observed to permanently kill
  WinEvent delivery on that thread for the rest of the process, which
  decision D9's one-pid-per-outpost-for-life design sidesteps). Every event
  is arbitrated per window so exactly one backend reports it: an MSAA event
  for a UIA window is dropped, and a UIA event for an MSAA window. An MSAA
  event's window is exact; a UIA element that is not a window itself is
  attributed to its window by `nearest_window_handle` (NVDA's
  `getNearestWindowHandle`), on the worker, since it is a cross-process call.
  Because every Win32 and wx control is its own window handle, per-window
  arbitration is per-control there, while a WinUI top level resolves once
  for its whole subtree. Trace IDs are minted when the OS event first
  arrives, and each event carries its observation timestamp for the latency
  ledger and `WindowFacts` for the window it concerns (`window_facts`:
  top-level window and root owner from `GetAncestor`, the topmost extended
  style on the window or its top-level window, and for `Windows.UI.Core`
  windows whether `GetGUIThreadInfo`'s active window is it or contains it,
  and whether it is in `GetForegroundWindow`'s window by NVDA's test), all
  local calls.
- A UIA focus fact is reported from the fact itself, as NVDA builds the
  focus from the event's sender, and it is accepted only when its
  properties say the element has the keyboard focus. Its name and role
  are the event's, which NVDA reads from the sender's cache; its value,
  states, and details are read when the focus is handled, as NVDA fetches
  them then (`with_live_reads`; `docs/parity.md`, "How an outpost turns
  events into focus reports"), from the focused element the outpost reads
  (`live_focus_element`, waiting at most `FOCUS_READ_WAIT`, one second)
  to get its own live copy of the element, which also serves for the
  ancestors, the selected child, the element's window, and navigation.
  Without it the focus is emitted from the event alone, with
  `ancestors_unknown`, and nothing reads again: the element comes from
  the focus's own next event, keyed by its runtime id. Meanwhile the
  focus-following property subscription and the caret and text
  subscription listen in the focus's top-level window, else in each of
  the application's (`follow_focus_window`), and the first of their
  events that the focus raises itself brings its element
  (`adopt_focus_element`), which is kept under the focus's node and
  followed from then on, the subscriptions moving to it and, for a focus
  that may have text, its caret asked for, whose line Core then speaks;
  the event is then handled as any other, and the other elements' events
  are dropped. A focus event reads the element as any focus does, and
  Core takes the same node silently; a selection event of the focus reads
  the focused element once (`element_from_selection`) and keeps it when
  it is the focus's. Until 2026-10-09 a queued follow-up read the focused
  element once more, and the subscriptions listened nowhere, so a focus
  that raised neither a focus nor a selection event was never followed
  (mockapp's
  `a_focus_whose_element_was_not_found_is_followed_from_its_own_changes`). When the focused element
  read is in another application, the fact is out of date and dropped.
  When it is another element of this application, and the fact's element
  is a window of its own, that window's element is read
  (`own_element_focused`): if it is the fact's element and has the
  keyboard focus, read live, the focus is reported from it at once, as
  NVDA accepts a UIA focus event whose own element has the keyboard focus
  (`shouldAllowUIAFocusEvent`), where the outpost had held the focus back
  and read again (`phase6-design.md`, "Notepad at launch and the
  outposts' loose ends"). The console host's window is never taken this
  way: its element is the parent of the text area, reports the keyboard
  focus whenever the text area has it, and raises focus events around
  the text area's, which NVDA refuses whatever the element reports
  (`consoleUIAWindow.shouldAllowUIAFocusEvent` is false,
  `NVDAObjects/UIA/winConsoleUIA.py` lines 356 to 358 and 446 to 447);
  taken, it was announced as a second focus beside the text area's.
  Otherwise the focus has moved on to another element of the
  application since the event, and the fact is reported all the same, as
  its event said (since 2026-10-09; it had been dropped): NVDA judges a
  UIA focus event by its sender's keyboard focus as the event arrives
  (`shouldAllowUIAFocusEvent` in `NVDAObjects/UIA/__init__.py`, lines
  1632 to 1637, checked by
  `IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent` in
  `UIAHandler/__init__.py`, lines 948 to 953), which the fact's cached
  states carry, and the newer focus's own event follows. Its element is
  found in its window (`element_of_moved_focus`): by its runtime id, else
  by the name and position the event gave it, since Windows Terminal
  raises its tabs' focus events from elements that are not in its tree;
  its ancestors are read without requiring the keyboard focus
  (`FocusQuery::require_focus`). It is not kept with its element, so the
  focus-following subscriptions do not move to it; one found nowhere is
  gone and is dropped, as File Explorer's "Working on it..." is as a
  folder opens. The console host's window's own focus is still dropped.
  Every
  UIA focus of a batch is handled in turn, oldest first, as NVDA's UIA
  handler queues each one, and Core culls the speech of those that have
  expired (mockapp's
  `a_focus_that_moved_on_since_its_event_is_reported_as_the_event_said`).
  For an
  element with no window of its own the listener also sends the keyboard
  focus window it found when it captured the event (`focus_window`, the
  foreground thread's focus window when it belongs to the element's
  process, a local read); an element the outpost cannot resolve is
  reported with that window's facts, so Core judges it against the
  foreground window. That window is exact when the event is current; for
  a late event in an application with several windows it can be another
  of them. Intake also uses it as the entry's window, so a hung focus
  window drops the fact and the watchdog's moved-on check applies to it.
  Only a fact with neither is reported without window facts, arbitrated
  against this application's own focus window. An MSAA focus fact is
  acquired with NVDA's child-0-on-a-list redirect, and the redirect of any
  control's own focus to a focused child its `accFocus` names
  (`focus_candidate`), and checked as NVDA checks it before anything else
  is read: it is dropped as a duplicate when it names the address of the
  focus last reported, a whole object rather than a child by id, with no
  foreground change reported since (the focus may have been in another
  application then), counting as reported so no older focus of the batch
  is tried; and it is accepted only when the object or an ancestor has the
  focused state, read live. Only then is it read and its ancestors walked.
  When a control's focus was reported as its child, the child's own focus
  event, which the control raised in the same `SetFocus` call, is dropped
  while the child is still the focus (`Intake::take_redirected_focus`), so
  the child is reported once, as NVDA reports it.
- MSAA name, value, and state changes are spoken only for the focus, so
  the event's object is told from the focus by its identity
  (`EventObject::which_of`) before any property is read, and an object
  that is not the focus is not read. A state change that newly expands
  the focus, by the states the outpost last read for it, carries a
  `SysTreeView32` item's child count. A selection event is read in full,
  as before.
- Ancestors (`read::uia_remote_enrichment`, `read::uia_enrichment`,
  `read::msaa_enrichment`): the walk stops at the first ancestor in the
  previous focus's chain (the tracking state's `chain`) and splices the
  rest of that chain in, as NVDA does; the remainder is read within
  `ENRICHMENT_BUDGET`, two seconds (UIA calls inside it wait no longer
  than that), after which the ancestors are reported unknown. A walk
  that fails, the remote program by UIA's transaction timeout or the
  classic walk by a hop that timed out, also reports them unknown, never
  as an empty chain (`crates/mockapp/tests/remote_ops.rs`,
  `a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown`).
  Object navigation's ancestor query reads the whole chain.
- A dialog's own text (`read::describe_dialogs`). Before a focus is
  published, every dialog among its ancestors, and the focus itself, that
  is not in the previous focus's chain and has no description of its own
  is given its text as its description, while descriptions are read at all
  (the theme's description indication), so Core speaks it where NVDA does,
  after the dialog's name, role, and states and before the focused
  control. A dialog the focus stays inside is therefore read once: the
  splice carries its text along. A foreground report of a dialog gathers
  its text too and keeps it (`Tracking`'s `dialog`), and the focus that
  follows into it takes that rather than reading the dialog again; a
  focus-now answer gathers for its window and its focus's chain alike,
  reading a dialog that is both only once. A UIA dialog is read through the
  element the registry holds, each call waiting at most a second.
- A UIA focus's ancestry, by default, is one remote operation
  (`verbatim_uia_rops::focus_ancestry`, called by
  `read::uia_remote_enrichment` right after the focused element is read):
  one `Execute` checks the element's `HasKeyboardFocus` live, reads a list's
  or tab control's selected child, walks the raw-view parents with the
  base cache filled inside the provider until it meets a runtime id of the
  previous chain, and finds the element's nearest window, so the
  `NormalizeElement` call for the window is not made either. The parents
  become the chain through `Uia::ancestor_chain_from`, with the same
  presentable filter, the same stop at a window read through MSAA and the
  same continuation through MSAA from there, and the same splice; the
  program also stops at a known ancestor that is not reported (the
  previous focus itself, say), and the splice is made there. An element
  the program finds no longer focused holds the fact back as above.
  When the focus's runtime id already names a node, the program also
  reads whether the element that node stands for still has the keyboard
  focus (`FocusQuery::previous`; with remote operations off, one call of
  its own): an application can give a dead element's runtime id to a new
  one, and when the held element has lost the focus, or cannot be read,
  the registry gives the id a new node before the focus is reported
  (`NodeIdRegistry::reissue`; `docs/parity.md`, "Duplicate focus
  suppression"). A held element that is gone fails the whole program
  before it runs, and `verbatim_uia_rops::focus_ancestry` runs it once
  more without it. With
  remote operations off (`--classic-uia`), or for a window whose element
  could not be imported into a program (a client-side proxy, marked in the
  context for the window's lifetime and forgotten when it is destroyed),
  the per-hop walk (`read::uia_enrichment`) is used, as before. A program
  that fails otherwise is answered by the classic walk for that call and
  logged with the failing instruction's source line. The focus-now query
  reads its focus the same way. A steady-state UIA focus change costs two
  UIA calls, the focused element and the `Execute` (`docs/performance.md`).
  A focused menu item that no pattern makes checkable costs one more: its
  legacy MSAA checked state, read live for it alone
  (`verbatim_uia::map::with_legacy_checked_state`), as are the focus-now
  answer and a navigation step's neighbor.
- A UIA object-navigation step (`read::uia_remote_step`) is one remote
  operation (`verbatim_uia_rops::navigation_step`): from the element the
  registry keeps, without refreshing its cache first, the program finds
  the element's nearest window and takes the step, filling the neighbor's
  cache inside the provider. A step from a top-level window is taken the
  classic way, since a program's walk ends there while a tree walker goes
  on to the desktop and other applications' windows; so is any step with
  remote operations off or in a window marked as read classically. An
  element whose provider reports it gone is searched for by runtime id
  and stepped from classically, as before.
- Focus candidates: the intake keeps the three newest focus facts from
  each backend (`Planned::Focus`), and the worker handles them newest
  first, each under its own deadline, until one is reported, as NVDA's
  event pump falls back to an older focus event.
- Menus (the worker), following NVDA's MSAA handler: within a batch, focus
  and foreground events are handled first and the menu opening last. If a
  focus in the batch already put focus on a menu or menu item, the menu
  opening is ignored; if its object is not a popup menu, it is ignored;
  otherwise it becomes a focus on the popup menu, so the reducer never
  receives a separate menu event. The end of a menu
  (`EVENT_SYSTEM_MENUEND` or `EVENT_SYSTEM_MENUPOPUPEND`) or of the Alt+Tab
  switcher (`EVENT_SYSTEM_SWITCHEND`) is not an outpost's concern: the
  focus listener forwards every such end to Core 50 milliseconds later as
  `MenuOrSwitchEnded`, with the time it ended, and unless Core has applied
  a focus observed since then, Core asks the foreground application's
  outpost for its focus (`Query::FocusNow`).
- Window destruction: an `EVENT_OBJECT_DESTROY` for a window drops its kept
  arbitration verdict, its mark as read without remote operations, and its
  MSAA nodes, so a reused handle is probed afresh and never inherits them.
- Held objects (outpost redesign, "Held objects"): both registries keep
  every node the outpost reports, with the live UIA element or MSAA object
  behind it where it has one.
  Each message that carries node ids (an event, or a reply with a result:
  `OutpostToSupervisor::carries_nodes`) takes the next position when the
  worker queues it to be written, and every node issued or looked up since
  the last publish is recorded as reported at that position; a message
  merged away while it waits gives up its position, so the positions stay
  those Core counts (under "Messages to Core" below). Core's reader counts
  the same messages and hands the app each message's position. The app
  sends `SupervisorToOutpost::NodesHeld` with the node numbers the reducer
  holds in that outpost, the text anchors it holds there (milestone M4),
  and the position of the last message it has
  handled, whenever that set changes and also every 256 messages, so an
  outpost whose held set stays the same still releases what it reported
  meanwhile. The worker then releases every node not held that was
  reported at or before the acknowledged position; a node reported later,
  or not yet reported, is kept, since Core may not have seen it. Only the
  worker in charge releases, and the released nodes leave both registries
  under the watch lock and the writer's queue lock, and their objects are
  dropped after both are released, so a worker that replaces an abandoned
  one can never report a node that is about to vanish. A query
  for a released node answers `Gone`. Core's writer replaces a waiting
  list with a newer one and lets it past a full queue; the intake queue
  likewise keeps only the newest and never limits it.
- What to read (milestone M4, `phase6-design.md`, "Themes: one model for
  verbosity, speech, and sounds"). `SupervisorToOutpost::Fetches` carries
  the details the active theme wants read for each node
  (`SrState::fetches`); the app sends it to each outpost as it starts and
  again whenever the theme in use changes, and the reader applies it at
  once. Until it arrives, everything is read. A detail whose indication is
  off is not read: UIA reads in the worker build their cache requests, and
  the remote focus walk its property list, from
  `verbatim_uia::cached_properties`, which leaves out the description's,
  shortcut's, position's, and level's properties when those are off; MSAA
  reads skip `accDescription` and `accKeyboardShortcut` and the list and
  tree position reads through the MSAA registry, which holds the same
  setting. The cache requests of the UIA event subscriptions are fixed when
  they are registered and still ask for everything; the presentation stage
  drops what is off either way. Text formatting (milestone M4 item 7) is
  read only for the indications that are on, and among them only what the
  control supports: the annotation types for spelling and grammar errors,
  the font's name, its size, its weight, italic, and underline style (the
  font attributes; the underline style also for the kind of underline),
  the strikethrough style, the color, the background color, the bullet
  style, and the link attribute, each with its own detail. `IsHidden` is
  never read. What a control supports is learned from its answers and
  kept with its text patterns, by node, until the node is released
  (`text_support` in the worker's context): an attribute a line answers
  "not supported" for while its support is unknown is not asked again,
  and one answered otherwise is supported and no longer checked. The
  annotation types are never learned, since a provider (Windows 11
  Notepad) answers "not supported" for text without annotations; the
  caret read asks a line for them first instead
  (`docs/crates/verbatim-uia-rops.md`, "The caret read"). Nothing is
  chosen by application.
- Text (milestone M4, `text` and the worker's `text_reads`). A `Query::Text`
  is answered `QueryResult::Text` with whatever the protocol answers,
  `NoText` and `Gone` among them, so Core hands every answer to the reducer
  unchanged; its deadline is two seconds. The node's backend comes from the
  registry that issued it: a UIA node has text when its element has a text
  pattern, fetched once per node (`TextPattern2` where the provider has it,
  for the caret) and kept until the node is released, and a node whose
  element has none answers `NoText`. That answer is kept only until the
  node is next reported as the focus, or raises a caret or text event,
  which only an element with text does (`text_reads::forget_no_text`):
  UIA reports a provider that fails the request, as one not ready yet
  while its application starts, exactly as one with no pattern, and
  Windows 11 Notepad once answered so at launch, after which every caret
  key in it was silent. A caret key's watch on such a UIA node stays open
  rather than being answered `NoText`, so the application's caret event
  answers it; a fetch that fails with an error is not kept at all and
  answers `Unanswered` (mockapp's `refuse-text`, in
  `crates/mockapp/tests/caret_watch.rs`). An MSAA node has text when it is the
  client area of a window whose class, normalized by NVDA's class map, is
  an edit control's (`Edit`, `RichEdit`, `RichEdit20`, `REComboBox20W`,
  `RICHEDIT50W`), read through its messages, and any other MSAA node
  answers `NoText`, as MSAA has no text interface. A UIA terminal (by its
  class, or a focus in a `ConsoleWindowClass` window, the console host)
  never reads a paragraph or a page, which Windows Terminal reports as the
  whole buffer. The rules the module follows:
  - Positions. An anchor is minted at the start of every chunk sent and at
    each end of a selection; a `TextPosition` is resolved by moving forward
    from its anchor over the chunk's text converted to UTF-16, checking
    the text passed so a provider whose characters are code points or
    grapheme clusters lands right, except a position the outpost itself
    reported (a caret, a selection's end, a read's point), which it
    remembers. Anchors Core holds (`NodesHeld`'s `anchors`, set by the
    reader as the list arrives, before the worker sees it, in a set with a
    lock of its own, `text::HeldAnchors`, since 2026-10-08: the reader had
    taken the anchor stores' locks, which the worker holds while it reads
    text, and so waited for a read, pings included) are kept; any other is
    forgotten once 64 newer ones were minted for the node, and a request
    naming it is answered `AnchorLost`. A released node's anchors and text
    patterns go with it.
  - Chunks are at most 64 KB of UTF-8, cut between whole characters
    (grapheme clusters): a unit cut short by the byte limit ends at the
    last cluster boundary before it, and one the source read only in part
    loses its last cluster, which may have been cut, a surrogate pair's
    first half among them; a copied range and a selection's text are cut
    the same way. Offsets
    are converted from UTF-16 at character boundaries, a position inside a
    surrogate pair moving past it. UIA reads carry the range's `Culture`
    as one language run over the chunk (a mixed range carries none);
    caret reports carry none, to keep a caret move's calls down.
  - The caret through UIA (`TextSource::caret_read`, which `UiaText`
    implements with `verbatim_uia_rops::caret_read`) is read in one go: the
    caret and the selection, whether they moved from the baseline, the
    caret's line and the watch's unit with the caret's offset in each, and
    the formatting of the text to be spoken, in one remote operation where
    the window's provider runs them (the same choice and fallback as the
    focus walk and the terminal read, logged and remembered alike), and
    otherwise in its classic reads. The edit controls have no such read,
    and are read part by part as before, without formatting. Formatting is
    read for the report after a focus (the line, which Core speaks) and
    for a caret key's answer (the character, the word, or the line the key
    speaks; not a paragraph or a page, as NVDA reads no spelling errors
    when the caret moves by paragraph), never for the report after a typed
    character, which nothing speaks. It travels in the chunk as
    `FormatRun`s (byte ranges of its text), in the model's words: bold
    from a font weight of 700 or more, underlined from any underline style
    but none (and the style itself, a `LineStyle`, when the kind of
    underline is fetched), the strikethrough style as a `LineStyle`, the
    size as "11.0 pt", the color and the background color by the nearest
    of NVDA's named hues, saturations, and brightnesses ("dark red",
    `text/color.rs`, ported from NVDA's `colors.py`), the bullet style as
    a `BulletStyle`, and a link from any value of the link attribute. A
    character's formatting covers the character.
  - A caret key's watch (`AwaitCaret`) looks for the evidence NVDA's caret
    scripts wait for, without waiting (since 2026-10-08; it had polled the
    caret every 10 ms for up to 100 ms, 300 in a terminal, on the worker,
    and every other event of the application, a focus change among them,
    waited behind it). When the request arrives the worker ends the watch it
    replaces, then checks the new one with one read of the caret
    (`text_reads::check_watch`, `text::check_caret`). When the read finds
    evidence the key is answered at once with the caret's line, the watch's
    unit at the caret (a character cut from the line, any other unit read),
    and the selection's changes. Otherwise the watch stays open
    (`text_reads::OpenWatch`, the context's `caret_watch`, at most one),
    the request is released from the watchdog, and the worker returns to
    its queue. Each later caret event, text change, or text selection change
    of the watched node checks the watch again with one read, unless the
    event was observed before the caret was last read, which already saw
    it; the check that finds evidence answers the key, and the caret event's
    own report is not sent, the answer carrying the same caret. While a
    check reads, the watchdog treats the watch's request as the one running,
    so an abandonment answers it. A watch ends without evidence, answered
    `TextReply::WatchEnded`, for which Core says nothing, when the next
    caret key's watch replaces it, when a focus on another node is
    reported, or when it is `CARET_WATCH_BOUND` (10 seconds) old, which
    only frees it: the intake wakes the idle worker then (`Item::Wake`,
    `Intake::wake_at`), and nothing is spoken. So a key that moves nothing
    is silent (`docs/parity.md`, "Text, documents, terminals"), and a key
    whose application reports nothing when it moves the caret is silent
    too. Through UIA each check is the whole caret read above, one round
    trip remotely, so the check that finds the evidence is the answer, with
    nothing more to read. The evidence is the caret no longer where it was
    known to be, the characters either side of the caret changed from what
    was known, the text at the caret changed after a Delete, or the
    selection changed. Where it was known to be is the newest caret this
    outpost reported for the node from a read that finished before the key
    was pressed (the watch's `pressed_at_ms`, stamped by the keyboard hook
    on the same Unix millisecond clock as `observed_at_ms`), which can be
    newer than Core's (an earlier key's late caret event, or a paste's),
    else Core's. A caret read at or after the key's time is never the
    baseline: the application's caret event for this very key can reach
    the outpost, and be reported, before Core's request does, and judging
    against it would find no evidence. The outpost remembers its last eight
    reports per node for this. The characters matter because a provider's
    positions follow edits (a deleted character takes the known position
    with it) and the application may have handled the key before the
    request arrived; only the caret's neighbors count, since a line can
    wrap anew with no key at all. A caret event alone is evidence only when
    nothing knew the caret: it can be the application's late report of
    something earlier. The selection's changes are worked out by comparing
    endpoints, as the contract says; through UIA the caret read works them
    out and reads their text in the same round trip when the selection
    moved (`CaretRead::changes`), and the edit controls work them out call
    by call.
  - Which applications raise the evidence (measured 2026-10-08 with
    mockapp's real edit control): a Common Controls version 6 edit
    control, as a Windows Forms text box is with visual styles, raises
    `EVENT_OBJECT_TEXTSELECTIONCHANGED` whenever its caret moves, focused
    or not, which the hooks report; the classic edit control raises
    nothing for an unfocused caret move and, focused, only the system
    caret's hide and show (`EVENT_OBJECT_HIDE` and `EVENT_OBJECT_SHOW` on
    `OBJID_CARET`), which the outpost does not subscribe to, so its caret
    keys are silent. Windows 11 Notepad, Windows Terminal, and the console
    host, whose caret events come on their own schedule, need a live
    check.
  - Reads move first when asked: from the start of the unit containing the
    point, by whole units, never past the text's ends, saying how far they
    went; a document movement goes to the start or the end. A unit the
    source lacks is `UnsupportedUnit`; UIA has no sentence, and an edit
    control's sentence read is its paragraph, which is its line, for Core
    to split. `ReadRange` reads up to 1 MB for a copy, `Select` and
    `MoveCaret` select through the backend, and `Location` gives the
    screen position of the character at a point. `ReadAhead`, say-all's
    read, reads up to its count of units in one request, each the unit
    after the one before, stopping once 32 K UTF-16 code units are read,
    and marks the last chunk as the text's last when no unit follows it.
  - Through UIA, every request but the caret watch is also one round trip
    where the window's provider runs remote operations, with the same
    choice, fallback, logging, and marking as the caret read: a point
    named as the protocol names it (`PointFrom`: the caret, a selection's
    end, an end of the text, a kept position, or a position some text after
    a kept one, which is found inside the provider by matching that text)
    is found, moved from, and read in one program
    (`TextSource::read_units`, `range`, and `point_location`, which
    `UiaText` implements with `verbatim_uia_rops::text_units`,
    `text_range`, and `text_location`). A point found by matching text is
    remembered as one the outpost reported, so naming it again costs
    nothing. The edit controls keep their message-by-message reads.
- Caret and text events (the worker). A focus that may have text (through
  UIA an edit field, a document, or a terminal; through MSAA an edit
  control's client area) gets a caret report, queued just after the focus
  is published (`Item::CaretOf`), so the focus's own calls and speech are
  unchanged, and sent as `CaretMoved`. So does any other focus whose role
  is edit field, document, or terminal; when that report finds no text to
  read (an MSAA object that is not an edit control, a UIA element with no
  text pattern), `NoText` is sent instead, and Core speaks the focus's
  value in place of its line, as NVDA speaks the value of an object
  without navigable text. When the text could not be read now (its
  element is not known yet, the provider did not answer, or the caret
  read failed), nothing is sent: Core speaks the line when the caret is
  next reported, and never the value, which for a document is all of its
  text. A focused terminal's screen is read first, as the baseline of its
  output; when it is Windows Terminal's and every row of it is blank, a
  new window or tab with nothing written yet (one whose shell printed a
  notice first is not), `NoText` goes just before
  its `CaretMoved`, so Core says no line, as NVDA says none there, while
  the console host's says "blank" (`docs/parity.md`). The worker then
  follows its caret:
  a second focus-following UIA subscription, moved to the focus when it
  has text and to nothing otherwise, delivers `Text_TextSelectionChanged`,
  reported as `CaretMoved`, and `Text_TextChanged`, reported as
  `TextChanged`, and, in the same event handler group, the active text
  position changed event, reported as `ActiveTextPositionChanged` with the
  start of its range kept as a position in the focus's text (an anchor
  minted for it, no call), and dropped when its element is not a node the
  outpost knows or it carries no range, as NVDA's handler drops it; the
  intake keeps the newest one per element. For an edit control, the hooks' caret
  (`EVENT_OBJECT_LOCATIONCHANGE` on `OBJID_CARET`) and text selection
  events are reported as `CaretMoved`, and its value change as
  `TextChanged` without reading the control's whole text as its MSAA value.
  The intake keeps one waiting caret report per node, and a caret event
  observed before the worker last read that node's caret (a caret key's
  check reads it after the event) is dropped. A caret event, a text
  change, and a text selection change of the node a caret key's watch is
  open on check the watch first. A focus-now answer's control has its caret
  and text changes followed too (the subscription moved to it, and an
  edit control's caret events checked against a watch on it), since Core
  takes it as its focus and its caret keys are answered by those events;
  it gets no caret report and no property subscription.
- Queries (the worker): `DumpTree` walks the target's foreground window (or
  its first visible top-level window) through its backend — UIA via
  `Uia::walk_tree` with the base cache request, MSAA via
  `verbatim_ia2::acquire::walk_tree` from `OBJID_CLIENT` — capped at depth 64
  and 4096 nodes, reporting whether a cap cut it short. `Ancestors`,
  `Navigate`, and `Activate` dispatch to whichever registry issued the node:
  UIA hops through the raw-view tree walker with the base cache request, MSAA
  through `accParent`, `accNavigate`, and `accDoDefaultAction` on the object
  the registry kept for the node. A node no registry knows, or one that can
  no longer be reached, answers `Gone`.
- Selection and notification events: both backends' selection events emit
  `NormalizedEvent::SelectionChanged` (the selected node's full snapshot)
  and UIA notifications emit `NormalizedEvent::Notification`. A UIA
  selection inside an element the focused element names in its
  ControllerFor relation emits `NormalizedEvent::ControlledSelection`
  instead, carrying the focus's id, as NVDA reports a search suggestion
  (`Uia::controlled_descendant`). The reducer
  announces both ([verbatim-core](verbatim-core.md)).
- `OutpostMessage::Event` carries the target pid, the outpost id, the
  message's position, and the boxed `OutpostToSupervisor` payload: the
  replies grew the message enum well past the lifecycle notices, and boxing
  keeps every channel send small. `Supervisor::send_nodes_held` queues a
  `NodesHeld` list for one incarnation.

## Messages to Core

The outpost writes everything it says to Core from one thread
(`outpost::outbound`), from one queue, and nothing that queues a message
ever waits: the worker, the watchdog, the reader, and the thread watching
the target application only add to the queue and return. A watchdog that
waited behind a full queue, as it did when queuing waited for room under
the watch lock, could not abandon a hung worker.

- Pongs, `Ready`, and `TargetExited` go ahead of everything else, so a
  busy outpost is never mistaken for a dead one.
- While messages wait, they are merged by object and kind, as the intake
  and the supervisor merge what reaches the outpost and as NVDA's limiters
  do. An event replaces a waiting event of the same kind for the same node
  (for a property change, the same property; for a focus, the same kind of
  focus) and goes to the back. A terminal's new output is combined with
  its output still waiting (`terminal::combine`): every line in order (what
  either found above its last line taking its place in the stream), a
  change to a line still waiting putting the whole line in its place as
  Core does, two changes of the same line made one, and the flood
  policy's limits kept, at most the read limit in the head and as many in
  the newest lines, with the rest counted as skipped. Core then treats the
  combined output as any other.
- Answers to Core's requests are never merged and keep their order, and
  so do notifications, each of which says something of its own, and
  faults. A cancelled query's `NotStarted` answer takes its place among
  them too.
- Nothing is merged across a flush (`Outpost::settle`'s), so everything
  published before it has been written when it is answered.
- What waits is bounded by the nodes and kinds there are, by Core's
  requests (the supervisor's queue to the outpost holds at most 64), and
  by the intake's limits, without a bound of its own.
- The queue numbers the messages that carry node ids as Core counts them,
  in the order they are written, and records which nodes each may have
  reported. A message merged away gives up its position, so the messages
  after it come one place earlier than their recorded positions say,
  which only keeps their nodes a little longer.

The supervisor's side already never waits: each child's writer queue
fails or replaces a command when it is full (the `Supervisor` entry above),
the reader threads forward to unbounded channels, and the map of writers
is locked only to look one up, never across a push or a write. The focus
listener's own queue merges its facts by element and kind and never
waits either.

## Shutting down

Outposts and the listener are UI Automation clients of every application
Verbatim reads, and killing a client in the middle of a call or a remote
operation into an application is the leading suspect in a Windows Terminal
crash inside `UIAutomationCore.dll` (`phase6-design.md`, "Windows Terminal
crash of 2026-10-08"); it also happened to every application whenever
Verbatim exited, since ending a child used to mean closing its job. So
every way a child ends goes through a clean shutdown, and the kill is
only the fallback:

- Verbatim exiting: `Supervisor::shutdown`, which the app runs while the
  exit sound plays, after removing its keyboard hook, and waits for before
  it exits. No child is started from then on, and a child whose launch
  reports back afterwards is shut down at once.
- An outpost or the listener replaced or restarted: one that stopped
  answering pings, piled up abandoned workers, or closed its pipe, and a
  child launched after it was no longer wanted.
- An outpost retired, or ended because its application exited.

The supervisor's side (`supervisor::retire`): the child is sent
`Shutdown`, which passes a full command queue, its command pipe is closed
once everything before it is written, and the supervisor waits on the
process for `SHUTDOWN_LIMIT`. A child that exits with code 0 shut down
cleanly. One that has not exited by then is killed through its job
(`Contained::kill`, with `verbatim_process::KILLED_EXIT_CODE`), and the
kill is logged as a warning with why the child was being ended, and counted
in the summary; so is an exit with another code. The end-to-end suite
checks after every scenario that each outpost and the listener exited
with code 0 before Verbatim did.

The outpost's side (`Outpost::shutdown`), in order:

1. The intake closes: nothing new is taken, and what was waiting is
   dropped.
2. The `WinEvent` hooks are removed, with the thread that pumped them.
3. Both focus-following UIA subscriptions are closed: each client removes
   everything it registered (`RemoveAllEventHandlers`).
4. The worker finishes the entry in hand and every abandoned worker
   returns from its call. The watchdog ends first, so nothing more is
   abandoned. No call is cut off: a call into an application that does
   not answer is ended by UIA's own timeouts.
5. Every object held is released: every node leaves both registries, with
   its text patterns, anchors, and terminal memory.
6. The writer writes what is queued, the answer to the call that was in
   progress included, and closes the pipe.
7. The thread leaves COM's multithreaded apartment, and the process's
   hold on it is given back (`verbatim_uia::release_mta_usage`), so COM
   ends the apartment once every thread has left it; each worker and each
   subscription thread leaves it as it ends.

The outpost logs how long each step took (`the outpost shut down`). The
listener's shutdown removes its hooks, its desktop-wide focus handler, and
its other desktop-wide handlers, writes what is queued, and gives back
the apartment; it makes no call into an application, so nothing else can
be in progress. Both then end the process with `TerminateProcess` and
exit code 0 rather than returning from `main`, so no DLL's process-detach
code runs (`docs/architecture.md`, "Process lifetime").

The time limit, `SHUTDOWN_LIMIT`, is 21 seconds. Over the end-to-end
suite on the development machine (2026-10-08, 436 shutdowns), a clean
shutdown took from 4 to 443 milliseconds from the shutdown message to the
process's exit, as the supervisor logs it (`shut down cleanly`,
`elapsed_ms`): a median of 15 and a 99th percentile of 31, the slowest
the focus listener's once. The only thing that can make it take longer is a call in
progress, which the outpost lets finish. UIA ends a call to a provider
that never answers by its transaction timeout, 20 seconds, UIA's default,
which Verbatim leaves as it is; this bounds a classic call and a remote
operation's `Execute` alike (mockapp's `remote_ops` tests pin it), and the
connection timeout, 10 seconds, ends a call to a provider that cannot be
reached. So 20 seconds covers every call UIA ends by itself, and one more
second covers the shutdown's own work; waiting longer could only wait on
an MSAA call into a hung application, which nothing times out, and that
child is killed. The kill only ever happens while an application is not
answering, and Verbatim's exit then waits up to the limit, after the exit
sound.

Tests: `crates/mockapp/tests/shutdown.rs` tells a real outpost to shut
down while its read into mockapp is held at its first provider call, once
for a remote operation and once for classic reads, and checks that the
read's answer is sent, the outpost's four UIA event registrations on
mockapp's window are removed (counted by mockapp's fragment root through
`IRawElementProviderAdviseEvents`), the pipe is closed, and mockapp still
answers another client. `crates/verbatim-outpost/tests/shutdown_fallback.rs`
has a real supervisor launch a stand-in listener that never exits, and
checks that it is killed once its time limit has passed and that the kill
is reported in the shutdown's summary.
`crates/verbatim-outpost/tests/target_exit.rs` runs the real outpost
binary against a stand-in application, which it ends, and checks that the
outpost says `TargetExited` and, sent `Shutdown`, closes its pipe and
exits with code 0. The end-to-end suite checks every run's outposts and
listener as above.

## Terminals (milestone M4 item 9)

The `terminal` module (public, so mockapp's tests drive it as the worker
does) finds a focused terminal's new output by diffing its screen
(`phase6-design.md`, "Terminal reading by diffing the screen"). Terminal
behavior is generic, keyed by the control, never the window's title: a
UIA element of class `TermControl` (Windows Terminal) or `WPFTermControl`
(the terminal embedded in Visual Studio), or a focus in a
`ConsoleWindowClass` window (the console host), is `Role::Terminal`.
The console host's text area is read through UIA, as NVDA reads it on
current Windows (its provider reports text formatting, which arbitration
checks). Its focus comes from the host's own process, while
`GetWindowThreadProcessId` names the console's client as the window's
owner, and inside a remote operation the host's provider gives no native
window handle for the window; the classic walk to the nearest window
finds it.

- The worker keeps, per terminal node, a `Terminal`: a `Memory` (the
  screen's lines as last read, its top two rows as the provider gave
  them, which are the anchor, the text's first row, and whether it had
  history above it), a range at the start of that top row, its on-demand
  reading state (`terminal::reading`), and an answer to Core it owes. It
  is forgotten when the node is released.
- Every read is one remote program (`verbatim_uia_rops::terminal_screen`,
  with the classic fallback behind the same entry point), which also
  reads the caret and its line, as the worker reports it after the
  output (`text_reads::terminal_output`, `CaretMoved`), since a terminal
  raises no caret event for every character typed (the console host's
  come on a schedule of their own). The caret is stamped as read when the
  round trip began (`Context::caret_read_from`), so a caret event observed
  while it was in flight is not taken as already seen. The program reads
  the text pattern's first visible range in one call (the screen), its
  top two rows, whether the text starts where the screen starts (no
  history above it), and, given the anchor, finds the anchor's top row
  again (`docs/crates/verbatim-uia-rops.md`, "Layer 3: a terminal's
  screen"): at the range kept for it while nothing can have been
  discarded from the text (its first row reads as it did, or the old
  screen had no history above it), and otherwise by its text with
  `FindText`, the row padded in the console host, whose `FindText`
  matches padding (`Terminal::matches_padding`, set from the console
  window), at most `SEARCH_MATCHES` (20) matches. Found, the rows from it
  to the screen's top (moving by rows, which transfers no text) are how
  far the text scrolled; the row the old screen's last line was on is
  read as it is now; and the first rows past the old screen's lines,
  which went by unread, are read too, up to the read limit, so a flood's
  start is heard. Not found by its text, the rows of the whole text are
  counted. On a screen with no history above it, before the read and
  after it, the range stays on its row while a full-screen program
  scrolls its text through the rows (a pager moving a line), so there
  how far the text scrolled up is found from the text instead
  (`screen::alternate_scroll`): the scroll at which most of the old
  screen's lines that are not blank stand on the new screen as they
  were, when that is most of the lines the two share. A screen whose top row read differently from its text, or
  changed by the end of the read, or whose shift could not be found
  exactly (the text shrank, or grew and the screen's top was not found
  within the rows it grew by), was written to while it was read, and is
  not trusted (`settled` false). One whose view moved while it was read
  (`view_moved`) is trusted when its anchor was found: output scrolling
  the view leaves every row where it was, and the rows below the read are
  read next time. A footer below a scroll region in the console host is
  drawn a row lower as the view moves and its old row written over, so a
  read whose range was taken before the move gives the rows above where
  the footer was, without it: when the old screen's last line is no
  longer on its row, as it was or grown, and the read's lines do not end
  with it, it is a footer still on the screen below the rows read
  (`footer_below`), left out of the diff and remembered as the screen's
  last line, so the next read does not find it new. During a flood in
  the console host the view moves under nearly every read, so setting
  such reads aside left a flood above a footer unread until it ended,
  losing its first lines and the anchor (the case 9b2aa65 still set
  aside; `conhost_footer_overflow`).
- `read_new` turns a read into a `TerminalOutput` with the pure screen
  diff (`terminal::screen`). The old screen's lines that scrolled off its
  top are set aside, and the rest are lined up with the new screen: where
  the new screen still has the old last line in its place, as it was or
  grown, output was written at it and after it, so everything after it is
  new and the lines above it are compared on their own; otherwise the two
  are lined up by their longest common run of lines. Lines inserted are
  new, spoken whole; a line replaced in place speaks what changed of it
  (`screen::line_change`: what it gained at its end, or from the start of
  the word that changed, by Unicode's word rules with ICU's dictionaries
  for scripts written without spaces, and whole graphemes); a line that
  only lost text or gained only white space, and a spinner (one symbol
  replaced by another, nothing else), say nothing; lines only deleted say
  nothing. What is new above the old last line is `above`, the old last
  line's change is `changed` (or, when the caret was read on a line above
  it, a prompt above a status line, that line's change, the old last
  line's then coming after it), then the first unread lines (`head`), the
  rest counted (`Skipped::Count`), and the screen's new lines (`lines`).
  When the old screen scrolled away whole, its last line's change is
  worked out from the row it was on. When the anchor has left a history
  above both screens, the history overflowed: the skipped lines are
  "more than" the history's rows less the screen's lines
  (`Skipped::MoreThan`; `Skipped::Uncounted` when the history holds no
  more than the screen), and the screen's lines are new. When the anchor
  was not found and either screen had no history above it (the screen
  cleared, a full-screen program's alternate screen opened or closed),
  the screens are lined up by their common lines. Lines lose their
  trailing padding, every trailing character with Unicode's `White_Space`
  property (`verbatim_text::trim_padding`), and blank rows at the end of
  the screen are rows not yet written to (`Memory::unwritten`). A blank
  line on one of those rows, the first below the old screen's last line,
  is not inserted: it was blank before and is blank now, a row a program
  passed over (to draw a footer on the last row, or a screen cleared), not
  a line it printed (coherence review, Dickson, 2026-10-09;
  `screen::Unwritten`). Every other line counts, blank ones included.
- Rows and lines. A read counts rows (the shift, the rows that went by
  unread, the history's rows), and the text gives a line that wrapped onto
  more rows whole, so the diff compares lines. Each line's rows are known
  from its cells and the terminal's width, its top row's cells padding
  included (`screen::line_rows`, `Memory::rows`); the rows the old screen
  held (`Memory::rows_held`) are what the next read counts from, its last
  line's rows are read whole as the row it was on now (`old_last_row`,
  `ScreenQuery::last_rows`), and a shift in rows becomes the lines still
  on the screen (`Memory::after_scroll`), a wrapped line that straddles
  the screen's top being what is left of it, the new screen's first line.
  When the screen scrolled away whole and the row the old screen's last
  line was on now holds something else than that line, as it was or
  grown, that row is a line of its own, the first after the old screen
  (the first of `head`), not a change of that line: a footer's row a
  flood scrolled through is so a line of the flood.
- A line changed in place also reports what it gained where it changed
  (`LineChange::inserted`), which Core matches with typing held. The
  memory keeps each line twice: as read (`Memory::screen`) and as said
  (`Memory::said`), where a line that has only got shorter since keeps
  what it said before. A progress line cleared and written again, read
  half written, is so compared with what it said ("51%", not "oading
  51%"). Where the line's change since it was read differs, it goes
  with the change as `LineChange::since_read`: a line cut short by the
  user (Backspace, Escape) and typed again shows the typing only there,
  and a line that came back to what it said has nothing else to speak.
  Lines are taken as rewritten in place, first to first, only where at
  least as many new lines replace them; a screen cleared down to its
  prompt speaks the prompt as new. A lone line replaced by several is
  paired with the one sharing the longest start with it, so a footer
  redrawn below lines scrolled in above it is its own rewrite. When the
  screen scrolled away whole and its last line was a footer that stayed on
  the last row (the row it was on now holds something else, and the new
  last line shares a start with it), the footer is said only as it
  changed (`keep_footer`). A row erased whole is drawn again, so
  what it said is not kept for it. A screen replaced whole (none of its
  lines left on the new screen, nor grown or cut short there) is kept as
  the main screen (`Memory::main`): a full-screen program opened its
  alternate screen over it. When a later screen replaces the alternate
  one whole and starts with the main screen's lines (after as many of them
  as scrolled away, the last allowed to have grown), the main screen is
  back, and only what follows it is new. A screen the read found scrolled
  by a known shift is the same text moved on, however little of it is
  left, so neither: a flood in a new console host, whose screen had no
  history yet, scrolled its first screen away whole between two reads,
  which kept it as a main screen, and a later read whose first line
  started with that screen's last ("flood line 1972" after "flood line
  1") took itself for the main screen back and lost the skipped count.

- A line key (Up or Down Arrow) in a terminal is watched as in a text
  field (Dickson, 2026-10-09, `phase6-design.md`, "Terminal line keys as
  NVDA has them"): the caret moving is the evidence, and the answer is
  the caret's line. What the key made the program redraw elsewhere on
  the screen is spoken at once as output, through the screen diff. The
  watch is checked before each read of the terminal, the console host's
  own update events included, and again after each read that read the
  caret, since a terminal can move its caret after the text change that
  caused the read and raise no event for it (Windows Terminal under
  PSReadLine's menu).
- `terminal::keys::wrapped_removal` judges a key answered by where the
  caret landed (Escape) whose caret left its row: the screen's text gives
  a line that wrapped whole, so when the caret is on the same line of the
  screen and that line was cut short, the rest of it is the answer's
  `CaretReply::removed`; the next line still showing that rest is a
  redraw under way, and the watch stays open. The screen before the key
  is the terminal's memory when the read behind it ended before the key
  was pressed (`Terminal::screen_at`); otherwise the key is judged by
  the caret alone.
- On-demand reading (`terminal::reading`, `phase6-design.md`, "Terminal
  decisions"), a pure transition function the worker drives. Live, the
  default, a text change of the focused terminal (`Text_TextChanged`) is
  read at once and its output sent as `NormalizedEvent::TerminalOutput`,
  when it is not empty. Core's text requests change it:
  `TextOp::TerminalHold` (Core's queue is full: changes are only noted,
  answered `Done`), `TextOp::TerminalRead { hold }` (answered with what
  is new, read only if the text changed since the last read, then
  holding or live again), and `TextOp::TerminalCancel` (speech was cut
  off: read to the end, answered with the read for typing echo only,
  live again; a read written to meanwhile is still remembered). A read
  Core asked for that the terminal disturbed is owed, and the next change,
  which the writing raises, reads again and answers it. In the console
  host, the console's own `WinEvents` (`EVENT_CONSOLE_UPDATE_REGION`,
  `_SIMPLE`, `_SCROLL` and `EVENT_CONSOLE_LAYOUT`, merged into one per
  window while they wait) are its terminal's text changes as well as UIA's:
  in 2 of 14 runs of a 12,000-line write, UIA's text changes stopped
  reaching the outpost after a few reads of about 0.12 seconds, for the
  rest of the write, while its value and text selection `WinEvents` (about
  27,500) went on (measured 2026-10-08); why UIA's stopped is not known.
  NVDA's console support for UIA listens to UIA's text changes alone
  (`UIAHandler/__init__.py` 145 to 147 and 674 to 679, the "Text Area"
  automation id given them at 713 to 721); its `WinEvent` handling of the
  console (`winConsoleHandler.py` 84 to 87) is for the legacy console. A
  change observed before the last read began is covered by it and reads
  nothing (`Terminal::read_started_ms`). A terminal gaining
  the focus is read as a baseline, which speaks nothing, and reading is
  live.
- Each read logs its cost at debug (`terminal screen timing`: the path,
  microseconds, calls, the shift, the head's rows, the rows counted,
  whether it had history above it and settled, and its top row), as does
  the worker's handling of each event (`terminal read timing`, with the
  event, the action, its wait in the queue, and what it found).
- How many unread lines a read takes from their first comes from Core,
  `SupervisorToOutpost::TerminalLines` (`SrState::terminal_read_lines`, as
  many as the flood policy's limits keep, 30 by default), sent when an
  outpost starts and whenever it changes, beside `Fetches`.
- Windows Terminal's UIA notifications with the activity id
  `TerminalTextOutput` from a terminal control are ignored by the
  notification handling (`is_terminal_output_notification`), as NVDA's
  terminal overlays ignore them, or every line would be spoken twice; a
  terminal's other notifications, and other controls' with that activity
  id, are reported as usual.
- The unit tests: `terminal/screen.rs` holds the screen diff's table of
  cases (insertion at the end, in the middle and above a footer; deletion
  only; scrolling up and down; repeated identical lines; a selection
  list's marker; several changes in one block; blank lines; symbol-only
  lines) and the changed-word rule's (Chinese, Thai, combining marks, wide
  characters and tabs, spinners); `terminal/reading.rs` every state and
  event of on-demand reading; and `terminal/tests.rs` runs `read_new` over
  simulated padded rows behind the `ScreenSource` trait (output on a screen
  not yet full, a flood within the history and past it, a full history
  shifting beneath the screen, a command line that scrolled away, a line
  rewritten in place, a redraw, a cleared screen, the alternate screen, a
  disturbed read, padding of any `White_Space`, identical lines), and
  `terminal::combine`'s cases.
