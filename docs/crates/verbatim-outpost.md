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
  `SupervisorToOutpost`: `SetBackendOverride` (forces one backend for every
  window of the target, or restores normal arbitration), `DeliverFact` (a
  focus fact the listener captured, routed to this outpost — a UIA focus
  element's cached snapshot parts, an MSAA focus or menu-popup address, or a
  foreground window — carrying the listener's own trace id and observation
  timestamp so the latency timeline starts at the OS event), `Query` (a
  request id and a `Query`: `FocusNow`, `Navigate` with a model `QueryKind`,
  `Activate`, `Ancestors`, or `DumpTree`), `Cancel` (withdraws a query that
  has not started), and `Ping`. There is no shutdown message: Core ends a
  child by closing its job handle. `OutpostToSupervisor`: `Ready`, `Event`
  (trace id, observation timestamp, backend, the event window's
  `WindowFacts`, normalized event), `Reply` (exactly one per accepted query,
  echoing its request id, with a `QueryOutcome`: `Done` with a
  `QueryResult`, `Gone` when the node is no longer reachable, `Failed` with
  a reason, `NotStarted` when it was withdrawn before it ran, or `Abandoned`
  when it started and passed its deadline, so side effects such as an
  activation may already have happened), `Pong` (echoes the ping's sequence
  number and reports how many abandoned workers have not yet returned, which
  the supervisor watches), `Fault`, and `FocusFact` (sent only by the
  listener). A `FocusNow` answer carries the application's foreground
  window and its facts, when it holds the system foreground, and its focused
  control with ancestors, selected child, and window facts. A
  `ListenerFact` strips to a pid-less `DeliveredFact` once the supervisor
  has routed it. Node ids arrive from Core stamped with this outpost's id;
  the outpost looks them up with the stamp cleared (`NodeId::unstamped`).
  `OutpostToSupervisor::assign_outpost` is the stamp Core applies to every
  node id in a message. Framing is newline-delimited compact JSON via
  `write_message` and `read_message`.
- `Arbitrator` — NVDA's per-window backend decision (`_isUIAWindowHelper`):
  `verdict(hwnd, class)` answers from the good class list, the bad class
  list, a forced override from `SetBackendOverride`, or a kept probe result,
  and `None` when only the `UiaHasServerSideProvider` probe can decide; the
  worker then probes and calls `record_probe`. A probed verdict is kept for
  the window's lifetime and dropped by `forget` when the window is destroyed,
  as decision D15 specifies; NVDA's 500 ms cache throttles a check it makes on
  every event and is not there because answers go stale. Both class lists are
  lifted from NVDA and pinned by unit tests naming their exact NVDA source
  locations, so a future NVDA sync is a diff of two lists: the bad list is
  `badUIAWindowClassNames` in `nvda/source/UIAHandler/__init__.py`, and the
  good list concatenates `goodUIAWindowClassNames` from the same file with the
  Windows 11 shell tuple from the Explorer app module's `isGoodUIAWindow`
  (`nvda/source/appModules/explorer.py`).
- `Outpost`, `run_pipe`, `run_attach` — the per-application outpost (the
  `outpost` module; outpost redesign, "Inside an outpost"). Its parts:
  - Intake (`outpost::intake`): the MSAA hook callbacks (`APP_SUBSCRIPTIONS`:
    value, state, name, selection, menu end, and destroy, for the fixed pid),
    the focus-following UIA property callback, and the reader's routed facts
    (focus, foreground, menus, and the listener's desktop-wide selections,
    notifications, and alerts) and queries only add an entry to the queue and
    return. A UIA callback captures the element's cached parts, its cached
    window handle, and an agile reference; it never calls into the
    application.
  - The focus-following UIA property subscription, following NVDA's
    selective registration on Windows 11: name, value, and state changes on
    the focused element and its ancestors only. The worker moves it each time
    it reports a focus, without waiting. This replaces the subscriptions on
    the top-level windows that existed at spawn, under which a dialog or
    window opened later received no UIA events at all.
  - The queue applies NVDA's limiter rules: one waiting entry per object and
    kind, a newer one replacing it and moving to the back; a batch is
    everything that accumulated while the worker handled the previous one;
    per batch the newest 4 focus events and the newest 10 other events per
    application UI thread are kept, the focused object's events always;
    events from a window the system reports hung (`IsHungAppWindow`) are
    dropped before any read; and within a batch only the newest foreground
    change and the newest focus are handled, with the newest menu opening
    last.
  - The worker (`outpost::worker`): one thread takes entries in order and
    finishes each before the next. It is the only thread that calls into the
    application, so events and replies leave in the order their entries
    joined the queue. It replaces the announce lane, the query pool, the
    announce poll, the probe threads, and the late window retry.
  - The watchdog abandons a worker whose call passes its deadline (an event
    400 ms, a focus 1.5 s, a navigation or activation 400 ms, a focus-now
    query 2 s, an ancestor walk or tree dump 5 s), answers the stuck query
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
  JSON lines to stdout, the standalone dev mode.
- `run_listener` — the focus-listener runtime (decisions D13 and D14;
  outpost redesign, "The focus listener"): sets up the writer, announces
  `Ready`, and installs the desktop-global `FocusRegistration`, the global
  MSAA hooks (`LISTENER_SUBSCRIPTIONS`, pid zero: focus, foreground,
  menu-popup, and alert), and desktop-wide UIA subscriptions for the events
  NVDA registers globally on Windows 11: an element selected, a menu
  opened, and notifications. Each event becomes a `FocusFact` (a
  `ListenerFact`: the owning pid and a `DeliveredFact`) built entirely from
  cached and hang-safe local reads; a foreground event whose window is no
  longer the foreground is dropped before it is sent. Outgoing facts are
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
  app hears that the listener was replaced through
  `OutpostMessage::ListenerReplaced`, and of each incarnation through
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
  - Process creation (`process`): each child is spawned suspended into a
    kill-on-close job with a 200 MB memory cap and resumed, inheriting only
    its own two pipe ends and its log handle through a
    `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, so two launches running at once can
    never keep each other's pipes open.

Implementation notes:

- Spawning (`Supervisor`): the outpost is created suspended with two
  anonymous pipes whose child ends are the only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost. A per-application outpost's command
  line carries `--target-pid`, fixing the watched application for its whole
  life; the listener's carries `--listener` and no pid.
  Each child is spawned with `STARTF_USESTDHANDLES` and an inheritable,
  append-mode file handle as its standard error (and output), so the outpost's
  and listener's own `tracing` output — which otherwise had no subscriber and
  went nowhere — lands in a per-role log file (`logs/outpost-<target pid>.log`,
  `logs/listener.log`, next to the outpost executable). The outpost binary
  installs a stderr `tracing` subscriber at startup for exactly this; the E2E
  harness fetches these logs alongside the timeline and stderr, so a silent
  outpost is readable after the fact instead of theorized. Best-effort: a
  failed log open leaves the child unredirected, never unspawned.
- Foreground changes (the worker): a foreground fact is reported at once,
  named or not, as a focus on the window, since the foreground change is what
  moves the reducer's attention; the reducer does not speak a nameless
  foreground window, and nothing announces it later. The window's own
  accessible object is read: UIA via `element_from_handle`, MSAA via the
  `OBJID_WINDOW` object (not `OBJID_CLIENT`, which reads back as role
  "client" — confirmed live against Windows 11 Notepad, whose window
  announcement read "Untitled - Notepad, unknown" until this was fixed). A
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
  windows whether `GetGUIThreadInfo`'s active window is it or contains it),
  all local calls.
- A UIA focus fact is resolved with one `focused_element` call compared
  against the fact's runtime id. A mismatch means focus has already moved
  and the newer fact will arrive, so the stale one is dropped; there is no
  runtime-id search. The element in hand serves the window, the ancestors,
  and the selected child. An MSAA focus fact is read with NVDA's
  child-0-on-a-list redirect (`snapshot_from_focus_event`).
- Menus (the worker), following NVDA's MSAA handler: within a batch, focus
  and foreground events are handled first and the menu opening last. If a
  focus in the batch already put focus on a menu or menu item, the menu
  opening is ignored; if its object is not a popup menu, it is ignored;
  otherwise it becomes a focus on the popup menu, so the reducer never
  receives a separate menu event. When a menu closes (`EVENT_SYSTEM_MENUEND`
  or `EVENT_SYSTEM_MENUPOPUPEND`) and no focus event follows within 50
  milliseconds, the worker reads the real focus and reports it; a timer
  thread queues that check, so the worker never sleeps.
- Window destruction: an `EVENT_OBJECT_DESTROY` for a window drops its kept
  arbitration verdict and its MSAA nodes, so a reused handle is probed
  afresh and never inherits them.
- Held objects (outpost redesign, "Held objects"): both registries keep
  every node the outpost reports, with the live UIA element or MSAA object
  behind it where it has one.
  Each message that carries node ids (an event, or a reply with a result:
  `OutpostToSupervisor::carries_nodes`) takes the next position when the
  worker publishes it, and every node issued or looked up since the last
  publish is recorded as reported at that position. Core's reader counts
  the same messages and hands the app each message's position. The app
  sends `SupervisorToOutpost::NodesHeld` with the node numbers the reducer
  holds in that outpost and the position of the last message it has
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
  and UIA notifications emit `NormalizedEvent::Notification`. The reducer
  announces both ([verbatim-core](verbatim-core.md)).
- `OutpostMessage::Event` carries the target pid, the outpost id, the
  message's position, and the boxed `OutpostToSupervisor` payload: the
  replies grew the message enum well past the lifecycle notices, and boxing
  keeps every channel send small. `Supervisor::send_nodes_held` queues a
  `NodesHeld` list for one incarnation.
