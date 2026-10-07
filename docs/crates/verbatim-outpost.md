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
  (`calls`, a `CallCounts`). A caret key's reply also says when its wait
  for evidence ended (`awaited_at_us`) and how many of its calls the wait
  made (`awaited_calls`, a part of `calls`), which the worker learns from
  the wait's `CaretSignal::awaited`; the listener's `FocusFact`, Core's
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
  has not started), and `Ping`. There is no shutdown message: Core ends a
  child by closing its job handle. `OutpostToSupervisor`: `Ready`, `Event`
  (trace id, observation timestamp, backend, the event window's
  `WindowFacts`, normalized event), `Reply` (exactly one per accepted query,
  echoing its request id, with its `EventTiming`, default for a query
  withdrawn or abandoned, and a `QueryOutcome`: `Done` with a
  `QueryResult`, `Gone` when the node is no longer reachable, `Failed` with
  a reason, `NotStarted` when it was withdrawn before it ran, or `Abandoned`
  when it started and passed its deadline, so side effects such as an
  activation may already have happened), `Pong` (echoes the ping's sequence
  number and reports how many abandoned workers have not yet returned, which
  the supervisor watches), `Fault`, and `FocusFact` (sent only by the
  listener). A `FocusNow` answer carries the application's foreground
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
  a window that has one. Both class lists are
  lifted from NVDA and pinned by unit tests naming their exact NVDA source
  locations, so a future NVDA sync is a diff of two lists: the bad list is
  `badUIAWindowClassNames` in `nvda/source/UIAHandler/__init__.py`, and the
  good list concatenates `goodUIAWindowClassNames` from the same file with the
  Windows 11 shell tuple from the Explorer app module's `isGoodUIAWindow`
  (`nvda/source/appModules/explorer.py`).
- `Outpost`, `run_pipe`, `run_attach` — the per-application outpost (the
  `outpost` module; outpost redesign, "Inside an outpost"). Its parts:
  - Intake (`outpost::intake`): the MSAA hook callbacks (`APP_SUBSCRIPTIONS`:
    value, state, name, selection, and destroy, for the fixed pid),
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
    of the object it moves to keep their place. NVDA reads another
    object's event with one call and judges it against the focus when it
    runs; the outpost reads a whole snapshot, and the first focus in a new
    File Explorer window waited 2.6 seconds behind such reads while
    Explorer built the window (`docs/performance.md`, "A focus behind other
    objects' events").
  - The worker (`outpost::worker`): one thread takes entries in order and
    finishes each before the next. It is the only thread that calls into the
    application, so events and replies leave in the order their entries
    were planned: the order they joined the queue, but for the events a
    focus change overtakes. It replaces the announce lane, the query pool, the
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
    abandoned count, and exits.
  - The reader (`Outpost::handle_command`, driven by `run_pipe`) answers
    pings itself, withdraws a cancelled query that has not started with a
    `NotStarted` reply, and queues everything else.
  - The writer (`outpost::outbound`) sends pongs and `Ready` ahead of
    ordinary messages, which wait in a bounded queue.
  `run_pipe` is the production mode over inherited pipe handles; `run_attach`
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
  everything it does with it. `Outpost::settle` waits until the worker has
  handled everything queued, follow-ups included, the focus-following
  subscriptions have made every move asked of them, and every message
  published has been written to the pipe: the evidence those tests wait on
  before they read what the application answered, and before they assert
  the outpost said nothing more. The worker and its reads (`worker.rs`, `read.rs`), the
  intake, the writer, the protocol, and the supervisor's owner, policy, and
  writer modules forbid `unsafe` code: every UIA and MSAA read goes through
  the backend crates' safe wrappers, and what remains `unsafe` in the crate
  is window-manager calls (`outpost/window.rs`, arbitration's class
  reads), the event thread's message loop, process creation, and the
  inherited pipe handles.
- `text` (milestone M4) — the outpost's side of the text protocol
  (`docs/crates/verbatim-model.md`, "The text protocol"), public so
  mockapp's tests drive it as the worker does. `perform(source, anchors,
  op, signal)` answers one `TextOp` over a `TextSource`, a backend's view
  of one node's text in its own positions and UTF-16: `uia::UiaText`, over
  a text pattern, where a position (`UiaPos`) is one end of a text range,
  and `edit::EditText`, over an edit control's messages, where it is an
  offset; `UiaText::new` takes the element too, which a remote operation
  starts from, and `remote` and `fetches` say whether caret reads try a
  remote operation and which formatting they read. `caret_report` reads
  the caret's line and the selection, for `CaretMoved`, with the line's
  formatting when asked (the report after a focus), and remembers when
  its read finished. `Anchors` keeps one backend's anchors, by node, numbered
  from a counter both of an outpost's backends share; `NodeText` is one
  node's, and its `position_at` mints a position at a backend position
  without a call, as the worker keeps an active text position change's
  range (`UiaPos::start_of`). `CaretSignal` is a caret key's wait: whether a caret event
  arrived, waiting for one, and the clocks (an `Instant` for the wait
  and Unix milliseconds for when a caret was read), so the unit tests run
  on fake clocks. Details under "Text" below.
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
- `run_listener` — the focus-listener runtime (decisions D13 and D14;
  outpost redesign, "The focus listener"): sets up the writer, installs
  the desktop-global `FocusRegistration`, the global
  MSAA hooks (`LISTENER_SUBSCRIPTIONS`, pid zero: focus, foreground,
  menu-popup, menu and switcher end, and alert), and desktop-wide UIA
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
  starts the focus listener at once; `ensure_spawned(pid)` starts an outpost
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
  and `Ended` with a reason (exited, killed, or retired).
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
  - Crash: when an outpost's pipe closes, the owner ends it (`Ended`,
    exited) after the reader has forwarded everything it wrote. It is
    replaced at once only if its application holds attention; otherwise the
    next fact for the application starts one. After three crashes within a
    minute (`CRASH_LIMIT`, `CRASH_WINDOW`, the pure `CrashHistory`) it is
    not replaced until the next foreground change to that application. An
    application that has itself exited is left alone.
  - Hang: every child is pinged every three seconds; nine seconds without a
    pong ends it (`Ended`, killed). Eight abandoned workers end an outpost
    too, except while its application's windows are reported hung
    (`IsHungAppWindow`), when a replacement would hang the same way (the
    pure `wedge_decision`). A kill is followed by the crash rules.
  - Retirement: an outpost whose application has not held attention for two
    minutes, which is not Core's own, and in which the reducer holds no
    nodes, is ended (`Ended`, retired) by the sweep every 30 seconds (the
    pure `retirement_decision`).
  - Every ending closes the child's job handle, which kills it if it is
    still running; there is no shutdown message. When the owner kills or
    retires an outpost, `Ended` is sent at once and anything the child wrote
    afterwards follows it, which the app drops.
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
- Foreground changes (the worker): before the first entry of a batch that
  holds a foreground fact, the worker waits up to 250 ms, checking every
  10 ms with local calls, for that fact's window to become the system's
  foreground window (`Intake::next` names the window), as NVDA holds back
  event handling after a foreground event (issue 3831). Then a
  foreground fact is reported at once, stamped with the time its window
  was confirmed as the foreground rather than the time Windows raised the
  event, which comes before the change completes (`docs/parity.md`,
  "Stale focus events"),
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
  `ancestors_unknown`, and a queued `Item::ResolveFocus` follow-up (up to
  three attempts, while the focus is unchanged) finds the element and
  moves the focus-following subscription to it. When the focused element
  read is in another application, the fact is out of date and dropped.
  When it is another element of this application, the focus has most
  likely moved on (NVDA 2027.1 drops a focus event whose element no longer
  has the keyboard focus), but an application still starting can answer
  with a stand-in, so the fact is held back and reported only if a
  follow-up finds its element focused after all. For an
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
  read with NVDA's child-0-on-a-list redirect (`snapshot_from_focus_event`) and accepted
  only when the object or an ancestor has the focused state.
- Ancestors (`read::uia_remote_enrichment`, `read::uia_enrichment`,
  `read::msaa_enrichment`): the walk stops at the first ancestor in the
  previous focus's chain (the tracking state's `chain`) and splices the
  rest of that chain in, as NVDA does; the remainder is read within
  `ENRICHMENT_BUDGET`, two seconds (UIA calls inside it wait no longer
  than that), after which the ancestors are reported unknown. Object
  navigation's ancestor query reads the whole chain.
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
  worker publishes it, and every node issued or looked up since the last
  publish is recorded as reported at that position. Core's reader counts
  the same messages and hands the app each message's position. The app
  sends `SupervisorToOutpost::NodesHeld` with the node numbers the reducer
  holds in that outpost, the text anchors it holds there (milestone M4),
  and the position of the last message it has
  handled, whenever that set changes and also every 256 messages, so an
  outpost whose held set stays the same still releases what it reported
  meanwhile. The worker then releases every node not held that was
  reported at or before the acknowledged position; a node reported later,
  or not yet reported, is kept, since Core may not have seen it. The
  released nodes leave both registries under the watch lock, and their
  objects are dropped after it is released, so a worker that replaces an
  abandoned one can never report a node that is about to vanish. A query
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
  read only for the indications that are on: the annotation types for
  spelling and grammar errors, the font's name and size, its weight,
  italic, and underline style, and the color, each with its own detail.
- Text (milestone M4, `text` and the worker's `text_reads`). A `Query::Text`
  is answered `QueryResult::Text` with whatever the protocol answers,
  `NoText` and `Gone` among them, so Core hands every answer to the reducer
  unchanged; its deadline is two seconds. The node's backend comes from the
  registry that issued it: a UIA node has text when its element has a text
  pattern, fetched once per node (`TextPattern2` where the provider has it,
  for the caret) and kept until the node is released, and a node whose
  element has none answers `NoText`; an MSAA node has text when it is the
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
    remembers. Anchors Core holds (`NodesHeld`'s `anchors`, set as the
    list arrives, before the worker sees it) are kept; any other is
    forgotten once 64 newer ones were minted for the node, and a request
    naming it is answered `AnchorLost`. A released node's anchors and text
    patterns go with it.
  - Chunks are at most 64 KB of UTF-8, cut at a character boundary; offsets
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
    but none, the size as "11.0 pt", and the color by the nearest of
    NVDA's named hues, saturations, and brightnesses ("dark red",
    `text/color.rs`, ported from NVDA's `colors.py`). A character's
    formatting covers the character.
  - A caret key's wait (`AwaitCaret`) follows NVDA's caret scripts: it
    reads the caret, then waits for evidence, polling every 10 ms between
    caret events, for up to 100 or 300 ms, and answers with the caret's
    line, the watch's unit at the caret (a character cut from the line,
    any other unit read), and the selection's changes. Through UIA each
    read of the wait is the whole caret read above, one round trip
    remotely, so the read that finds the evidence is the answer, with
    nothing more to read. The evidence is the
    caret no longer where it was known to be, the characters either side
    of the caret changed from what was known, the text at the caret
    changed after a Delete, or the selection changed. Where it was known
    to be is the newest caret this outpost reported for the node from a
    read that finished before the key was pressed (the watch's
    `pressed_at_ms`, stamped by the keyboard hook on the same Unix
    millisecond clock as `observed_at_ms`), which can be newer than Core's
    (an earlier key's late caret event, or a paste's), else Core's. A
    caret read at or after the key's time is never the baseline: the
    application's caret event for this very key can reach the outpost, and
    be reported, before Core's request does, and judging against it would
    find no evidence. The outpost remembers its last eight reports per
    node for this. The characters matter
    because a provider's positions follow edits (a deleted character takes
    the known position with it) and the application may have handled the
    key before the request arrived; only the caret's neighbors count, since
    a line can wrap anew with no key at all. A caret event alone only wakes
    the wait, unless nothing knew the caret: it can be the application's
    late report of something earlier. The selection's changes are worked
    out by comparing endpoints, as the contract says; through UIA the
    caret read works them out and reads their text in the same round trip
    when the selection moved (`CaretRead::changes`), and the edit controls
    work them out call by call.
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
  - Through UIA, every request but the caret wait is also one round trip
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
  text pattern) or the caret cannot be read, `NoText` is sent instead, and
  Core speaks the focus's value in place of its line. The worker then follows its caret:
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
  answer reads it after the event) is dropped. Every caret event also
  counts for a caret key's wait, on the event and callback threads.
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

## Terminals (milestone M4 item 9)

The `terminal` module (public, so mockapp's tests drive it as the worker
does) finds a focused terminal's new output by an anchored diff of its
text (`phase6-design.md`, "How the outpost finds new lines"). Terminal
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

- The worker keeps, per terminal node, a `Terminal`: an anchor (a range at
  the start of the last line read) and a `Memory` (that line's text and
  the line before it, the fingerprint, and the last lines read without
  padding, the screen as last seen). It is forgotten when the node is
  released.
- When a terminal gains the focus, after its caret report, the worker reads
  it afresh as a baseline, which speaks nothing: output from before the
  focus arrived is not new.
- A `Text_TextChanged` event for the focus, when it is a terminal, runs
  `terminal::read` in place of reporting `TextChanged` (events that arrive
  while a read is in progress are coalesced by the intake into one more
  read, one waiting entry per element). It is one remote program
  (`verbatim_uia_rops::terminal_tail`), with the classic fallback behind
  the same entry point; a window whose elements cannot be imported is read
  classically from then on, as for the focus ancestry. `after_anchor`
  turns the read into a `TerminalOutput`: the anchor's line compared
  character by character with what it held (grown: the text added;
  rewritten: from the start of the word where it first differs; shorter:
  nothing), and the lines after it, all of them up to the read limit, or,
  when more follow, the first ones (`TerminalOutput::head`, so a flood's
  start is heard) and the last ones, each up to the read limit, with those
  between counted (`Skipped::Count`). A rewrite under
  a blank line is not trusted, since a blank line matches too easily. The
  anchor's line found above the anchor (`Found::Moved`) is compared the
  same way, since the last line read is often the one output was still
  being written to. A line that grew reports, as `LineChange::uncertain`,
  how much of the white space it gained it already had at the same place:
  its padding, or its own trailing spaces, which cannot be told apart, so
  Core matches typing after a prompt's trailing space.
- A read the text changed under (`settled` false: the line above where it
  started read differently at its end, or the one read of its lines did
  not end with the last line and the one before it read on their own)
  finds nothing and keeps the memory and the anchor, and the text change
  that disturbed it causes the next read. When the text scrolled beneath
  the read (`scrolled`: the line above where it started changed), lines
  went by unread instead: the read says "skipped lines" without a count
  (`Skipped::Uncounted`) and remembers its own last lines and last line
  as the anchor, so the next read starts from there; a set-aside read that
  read no lines (the fingerprint was found on the last line) keeps the
  fingerprint it had, which still names that line. An unsettled read that
  did not find the fingerprint is not set aside but read afresh, as a
  settled one is: counted from an anchor a full scrollback keeps on its
  last row, it would read no lines, and a fingerprint of two empty lines
  then kept every later read of the flood searching all 256 lines in
  vain (measured live in the console host, `docs/performance.md`, "A
  terminal flood"). Live, a flood in
  Windows Terminal's full scrollback kept every read from settling until
  it ended; kept back at an anchor from before it, a second, identical
  flood's end matched the first's and nothing was spoken. Before reads
  were checked at all, lines read one by one during such a flood came
  back twice or out of order.
- When the fingerprint is not found, the anchor no longer compares with
  the text (a full-screen program switched screens), or the anchored read
  found nothing new after the anchor (a full-screen program redrawing a
  line above it), the terminal is read afresh from the end of its
  document (one program, which gets the document range from the element
  itself, `TailStart::Text`), and `after_fresh` compares the lines read with the screen
  last seen: when the old screen's end reappears in the new one (its last
  line as it was or grown since; at the new one's start when both hold as
  many lines and the text scrolled, further down when the old one holds
  fewer, as a set-aside read's does), what that line gained and the lines
  after it, taking the latest place it reappears; otherwise the lines that differ in place, preceded
  by `Skipped::Uncounted` ("skipped lines") when no line kept its place
  and the text holds more lines than were read (the scrollback overflowed
  past the search). A redraw with the same text finds nothing, and
  nothing is sent.
- Lines lose their trailing padding, every trailing character with
  Unicode's `White_Space` property (`verbatim_text::trim_padding`), and are
  cut to `MAX_TERMINAL_LINE_BYTES`.
- Each tail read logs its cost at debug (`terminal tail timing`: anchored
  or fresh, the path, microseconds, calls, where the fingerprint was
  found, settled and scrolled), as does each text change's read (`terminal
  read timing`, with its wait in the queue) and each caret read (`caret
  read timing`).
- `TerminalOutput` is sent only when something changed, as
  `NormalizedEvent::TerminalOutput`.
- How many of the newest lines a read takes comes from Core,
  `SupervisorToOutpost::TerminalLines` (`SrState::terminal_read_lines`, as
  many as the flood policy's limits keep, 30 by default), sent when an
  outpost starts and whenever it changes, beside `Fetches`.
- Windows Terminal's UIA notifications with the activity id
  `TerminalTextOutput` from a terminal control are ignored by the
  notification handling (`is_terminal_output_notification`), as NVDA's
  terminal overlays ignore them, or every line would be spoken twice; a
  terminal's other notifications, and other controls' with that activity
  id, are reported as usual.
- The unit tests (`terminal/tests.rs`) run `read_new`, the logic the UIA
  read goes through, over simulated padded rows behind the `TailSource`
  trait: appended lines and a grown prompt, more output than a read takes,
  a line rewritten in place and shortened, a redraw with the same text, a
  full scrollback shifting beneath the anchor, a shift past the search, a
  cleared screen, the alternate screen and a redraw inside it, and padding
  of any `White_Space`.
