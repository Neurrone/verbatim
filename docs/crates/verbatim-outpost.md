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
and the receiving outpost mints the `NodeId` from it. The announce poll
(`AnnounceFocus`/`run_announce`) demotes to a fallback for the listener's
own respawn gap and for a window that exists before it has a readable name.

Public API:

- `protocol` — the wire vocabulary the supervisor and each outpost speak.
  `SupervisorToOutpost`: `SetBackendOverride` (forces one backend for every
  window of the target, or restores normal arbitration — the old
  `Configure`'s backend-override half), `AnnounceFocus` (the announce-poll
  fallback, decision D13; a synthetic top-level-window-then-focused-control
  announcement, still used for the supervisor's startup target and to
  re-announce across a listener respawn), `DeliverFact` (a focus fact the
  listener captured, routed to this outpost — a UIA focus element's cached
  snapshot parts, an MSAA focus or menu-popup address, or a foreground
  window — carrying the listener's own trace id and observation timestamp so
  the latency timeline starts at the OS event), `Fetch`, `Ping`, `DumpTree`
  (walk the target's tree from its top-level window), `AncestorChain` (the
  chain of ancestors of a node, outermost first, as `NodeSnapshot`s, capped
  at 64 hops), `Navigate` (one step from a node — parent, next or previous
  sibling, or first child, the protocol's own `NavigateDirection`),
  `Activate` (invoke the node's activation action), `Shutdown`.
  `OutpostToSupervisor`: `Ready`, `Event` (trace id, observation timestamp,
  backend, snapshot version, normalized event), `FetchReply`, `Pong` (echoes
  the ping's sequence number and reports the outpost's current `QueryPool`
  parked-thread count — recovery ladder rung 2's bounded garbage — so the
  supervisor's heartbeat can judge rung 3's wedge-kill decision from the same
  message that proves the outpost is still answering at all), `DumpTreeReply`
  (a `DumpedTree` — the root `verbatim_model::TreeNode` plus whether the walk
  was truncated — or a human-readable failure reason), `AncestorChainReply`,
  `NavigateReply` (whose success payload is a `NavigateOutcome`: a found
  snapshot, or a first-class `NoNeighbor` distinct from an error — a root's
  missing parent is not a failure), `ActivateReply`, `Fault`, `FocusFact`
  (sent only by the listener: a `ListenerFact` — the pid to route to plus the
  captured address — with the trace id and timestamp the listener stamped at
  observation). A `ListenerFact` strips to a pid-less `DeliveredFact` once
  the supervisor has routed it. The three M3 query
  pairs are deliberately outpost-protocol-only rather than carried by the
  reducer-facing `Fetch`: none of them re-reads one already-known node's
  own snapshot (the one thing `QueryKind::NodeSnapshot` answers), and each
  needs input `Query`'s node-id-only shape does not carry. Framing is
  newline-delimited compact JSON via `write_message` and `read_message`.
- `Arbitrator` — NVDA's per-window backend decision:
  `resolve_with(hwnd, class, probe)` walks the ladder (good class list, bad
  class list, then the injected probe), caches verdicts per window handle
  for 500 ms, and supports a forced override from `SetBackendOverride`.
  The probe is a closure so tests fake it. Both class lists are lifted
  from NVDA and pinned by unit tests naming their exact NVDA source
  locations, so a future NVDA sync is a diff of two lists: the bad list is
  `badUIAWindowClassNames` in `nvda/source/UIAHandler/__init__.py`, and
  the good list concatenates `goodUIAWindowClassNames` from the same file
  with the Windows 11 shell tuple from the Explorer app module's
  `isGoodUIAWindow` (`nvda/source/appModules/explorer.py`): taskbar,
  input switcher, Task View and snap layouts, and the systray overflow —
  the roadmap's shell window-classification rules as generic policy.
- `QueryPool` — the deadline-guarded workers. Every cross-process call carries
  a deadline (the founding rule of architecture section 1), request/response or
  fire-and-forget: `run(deadline, work)` blocks the caller up to the deadline
  and abandons the call on expiry (the worker stays parked, a counter
  increments, and a replacement spawns — recovery ladder rung two, since a
  thread blocked in a hung app's COM call cannot be safely killed);
  `submit_deadline(deadline, work)` is fire-and-forget whose result nobody
  awaits, but a single watchdog thread tracks each job's deadline and applies
  the identical abandonment on expiry (park the worker, spawn a replacement,
  same warning). The old unbounded `submit` is gone: it let one hung
  acquisition block a worker forever *without* bumping the parked count, so the
  wedge policy was blind to it and every replacement a later `run` timeout
  spawned immediately picked the next hung job off the shared backlog — the
  pool poisoned itself to zero capacity and every announcement silently failed.
  Workers lazily own their own `Uia` client.
- `Outpost`, `run_pipe`, `run_attach` — the per-application runtime.
  `Outpost::new(writer, target_pid)` installs the process-scoped property,
  value, state, and selection subscriptions (`APP_SUBSCRIPTIONS`) for the
  fixed pid and announces `Ready` — but no focus registration and no focus
  or menu-popup hooks, which are the listener's now (decision D13); focus
  arrives instead as a `DeliverFact`, enqueued onto a per-outpost **announce
  lane** — one long-lived thread (spawned in `Outpost::new`) draining a FIFO of
  jobs, running each to completion before the next, so announcements emit in
  arrival order, which is the listener's observation order (the pipe and the
  supervisor's newest-wins flush preserve it). A foreground fact enqueues the
  window job; MSAA-focus, menu, and UIA-focus facts enqueue their
  `run_msaa_fact`/`run_uia_fact` bodies. The jobs still do their blocking work
  on the deadline-guarded query pool; only their sequencing is serialized. This
  is what guarantees NVDA's window-then-focus order: the window announcement is
  spoken before the control it precedes rather than racing it on a separate
  thread and losing to the reducer's last-observation-wins rule (the failure
  three of three cold presses showed). The window job (`window_announce_job`)
  holds the lane for a bounded nameless-retry — `WINDOW_LANE_ATTEMPTS` (3)
  attempts spaced `WINDOW_LANE_INTERVAL` (200 ms), so the lane is held at most
  ~600 ms plus read time; if the window is named within that it announces in
  order, and if still nameless the lane must move on (it must never starve the
  control), so the job hands the remaining `ANNOUNCE_RETRY` budget to a
  background thread (`background_window_retry`) that emits the window late —
  out of order but not lost, which the reducer's window carve-out speaks
  without moving focus. The announce generation still aborts a superseded
  window job. The `AnnounceFocus` poll fallback (`run_announce`) keeps its own
  thread, off the lane.
  The one difference from a self-hooked event is arbitration: a fact resolves a
  *real* verdict inline (`resolve_fact_verdict` — a cached verdict, else a
  `has_server_side_provider` probe on the deadline-guarded pool), because a lane
  job is allowed to block where an event-thread callback is not. So exactly one
  backend announces every fact deterministically — a UIA window's UIA fact
  delivers and its MSAA fact drops, an MSAA window's the reverse — with no
  cold-case duplicate and no provisional announcement from a genuinely UIA
  window. The first-ever fact for a window pays one probe (bounded by the query
  deadline) before announcing; only a probe that times out falls back to the
  old provisional behavior (the MSAA fact proceeds, the UIA fact drops), so a
  hung window degrades to that contract rather than silence. `run_pipe` is the
  production mode
  over inherited pipe handles; `run_attach` watches a pid directly, immediately
  announces its focus by poll, and prints outbound messages as JSON lines to
  stdout, the standalone dev mode. The live pid-scoped hooks (value, state,
  name, selection) keep the non-blocking provisional cross-filter on the event
  thread, where blocking is still forbidden.
- `run_listener` — the focus-listener runtime (decision D13): sets up the
  outbound writer, announces `Ready`, installs the desktop-global
  `FocusRegistration` and the global MSAA hooks (`LISTENER_SUBSCRIPTIONS`,
  pid zero), and forwards each event as a `FocusFact` built entirely from
  cached and hang-safe local reads. It answers `Ping` with a `Pong` (parked
  count always zero — it never blocks on a cross-process call), exits on
  `Shutdown`, and ignores everything else. It holds no per-application
  state, so a crash respawns into full capability instantly.
- `Supervisor` — `new(events_tx)` (which also spawns the focus listener into
  its dedicated slot), `note_foreground(pid)` (the announce-poll fallback:
  spawn if this pid has no outpost yet, otherwise send the existing one an
  `AnnounceFocus`; used for the startup target and listener-respawn
  recovery, no longer every foreground change), `ensure_spawned(pid)` (warm
  an outpost without touching foreground tracking — used once, at Core
  startup, for Core's own pid; see its doc comment), `send_to(pid,
  command)`. Focus facts from the listener are routed by `route_fact`: for a
  foreground fact it records the new foreground and emits
  `OutpostMessage::ForegroundChanged(pid)` before delivering, so the
  reducer's stale-event gate has the new foreground by the time the fact's
  own event arrives; for every fact it ensures the target outpost exists
  (spawning it if needed) and delivers the fact, or — if the outpost has not
  sent `Ready` yet — queues it newest-wins per category, flushed when `Ready`
  is intercepted. The categories are foreground, MSAA focus, UIA focus, and
  menu-popup, with separate slots for the two backends' focus facts
  deliberately: both can report the same focus and the app outpost's per-window
  verdict decides which announces, so both must survive the spawn. Which fact
  is the one that announces depends on the window's backend — a UIA window's
  UIA fact, an MSAA window's MSAA fact — so a shared slot that let one overwrite
  the other could keep the wrong one and drop it against the verdict, silencing
  the control (found live for a UIA search box that fires no MSAA focus event
  at all). Flush order is foreground, MSAA focus, UIA focus, menu-popup. That
  queueing
  policy is the pure `PendingFacts` type, unit-tested like `idle_decision` and
  `wedge_decision`. State is a map keyed by target pid,
  genuinely N-ready now: a reader thread per outpost forwards messages into
  the channel as `OutpostMessage::Event(pid, message)`, respawning on end
  of stream only if that pid's map entry still has the same generation
  *and* the watched application's process is itself still alive (checked
  via `OpenProcess`/`GetExitCodeProcess`) — otherwise the entry is dropped
  and Core is told via `OutpostMessage::Retired(pid)`. A background sweep
  (every foreground change, plus a coarse 30-second timer) retires any
  outpost whose application has not held foreground for two minutes
  (`IDLE_RETIREMENT`, risk R2's memory-use mitigation), skipping whichever
  pid currently holds foreground; retirement removes the map entry *before*
  sending `Shutdown`, which is what makes the ordinary respawn path
  correctly do nothing for a deliberate retirement instead of resurrecting
  it.
- Wedge detection and kill-and-respawn (recovery ladder rung 3, completing
  the M2-era respawn-on-crash path): a dedicated heartbeat thread pings
  every live outpost every three seconds (`PING_INTERVAL`) and, from each
  `Pong`, records the outpost's last-answered time and reported
  parked-thread count. The pure policy function `wedge_decision` — given a
  last-pong time, now, and a parked count, decide kill or not, and why —
  is unit-tested in isolation from the ping/kill I/O, the same split
  `idle_decision` uses for idle retirement. An outpost is declared wedged,
  and killed and respawned, if either: no pong has arrived for three
  consecutive ping intervals (`MISSED_PONG_THRESHOLD`, tolerating one slow
  tick before concluding the outpost has actually stopped answering), or
  its last reported parked-thread count reached 8 (`PARKED_THREAD_KILL_THRESHOLD`
  — each parked thread is roughly a megabyte of stack and a handle, so 8 is
  already several megabytes of garbage and evidence of repeated hangs, not
  one isolated slow call). The kill itself reuses the same job object every
  outpost is spawned into: removing the map entry drops its job handle, and
  the kill-on-job-close limit set at spawn turns that into an immediate
  kernel-level kill, so no message needs to reach an outpost that is by
  definition not reliably answering. Both the kill and the subsequent
  respawn are generation-checked exactly like the crash path, so a kill
  decision computed a moment earlier cannot race a retirement or a natural
  respawn that already replaced the entry, and the killed process's late
  pong or end-of-stream is generation-mismatched against the replacement
  and ignored. Every kill logs at warn level with the target pid, the
  outpost's own pid, and the reason (`"missed heartbeats"` or `"parked
  threads"`), for a flight-recorder-plus-stderr investigation to grep for.
  The focus listener is supervised by this same machinery (decision D13),
  but from a dedicated slot rather than the per-pid map: it is spawned at
  startup (a failed initial spawn is retried on the heartbeat interval),
  pinged and wedge-killed by the same policy (its parked count is always
  zero, so only the missed-heartbeat rule can fire), and respawned on
  end-of-stream — after which the supervisor fires one synthetic announce for
  the current foreground to cover the gap during which no facts flowed. Being
  outside the per-pid map, the idle sweep structurally never touches it and
  no listener entry ever reaches Core's status mirror.

Implementation notes:

- Spawning (`Supervisor`): the outpost is created suspended with two
  anonymous pipes whose child ends are the only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost. A per-application outpost's command
  line carries `--target-pid`, fixing the watched application for its whole
  life; the listener's carries `--listener` and no pid. `spawn` itself no
  longer writes an `AnnounceFocus` (decision D13): a fact-routing spawn wants
  the fact to do the announcing, and the two callers that still want the poll
  — the startup target and a listener-respawn recovery — write it themselves.
  Each child is spawned with `STARTF_USESTDHANDLES` and an inheritable,
  append-mode file handle as its standard error (and output), so the outpost's
  and listener's own `tracing` output — which otherwise had no subscriber and
  went nowhere — lands in a per-role log file (`logs/outpost-<target pid>.log`,
  `logs/listener.log`, next to the outpost executable). The outpost binary
  installs a stderr `tracing` subscriber at startup for exactly this; the E2E
  harness fetches these logs alongside the timeline and stderr, so a silent
  outpost is readable after the fact instead of theorized. Best-effort: a
  failed log open leaves the child unredirected, never unspawned.
- Foreground announcements (`AnnounceFocus`, `run_announce`, the announce-poll
  fallback): the outpost announces the top-level foreground window, then the
  focused control, both sharing one retry budget — up to ten attempts across
  roughly five seconds. The window step stops retrying once it
  succeeds (or is deliberately skipped, when every top-level window is
  Core's own hidden frame); the control step keeps going until it succeeds
  or the attempts run out. A single deadline-guarded attempt for the window
  step was tried first, reasoned as safe because Windows raises the
  foreground event only once the application's top-level window already
  exists — true in the common case, but live testing against the VM under
  load found `EnumWindows` and `GetForegroundWindow` can still race a
  window's own creation closely enough to miss it on the very first
  attempt, losing the window announcement outright with no later chance to
  recover it; the window step now retries for exactly that reason. It
  reads the window's own accessible object specifically: UIA via
  `element_from_handle`, MSAA via a direct `OBJID_WINDOW` query (not
  `OBJID_CLIENT`, which is what `DumpTree`'s walk starts from and which
  reads back as role "client", unmapped to anything nameable — confirmed
  live against Windows 11 Notepad, whose window announcement read "Untitled
  - Notepad, unknown" until this was fixed). The window itself is located
  by `GetForegroundWindow`, deliberately not `GetGUIThreadInfo`'s
  `hwndFocus`: the latter can legitimately name a non-top-level descendant
  that still belongs to the target process (Windows 11 Notepad hosts its
  text area in its own child `hwnd` distinct from the frame), which made an
  earlier version of this code read the edit control's own snapshot instead
  of the window's. The control step's retries answer a different race —
  the second focus-timing race `docs/roadmap.md`'s M2 section names, the
  announce poll and this query racing the target process's own control
  creation — using `GetGUIThreadInfo`'s `hwndFocus` specifically,
  since that question ("what control is focused") is genuinely different
  from "what is the top-level window". The whole loop runs off the command
  loop on its own thread so `Ping` and `Fetch` stay responsive during the
  retry window; a per-outpost generation counter, bumped on every
  `AnnounceFocus`, is compared before every attempt and before every
  emission, so a superseding announce (a rapid re-foreground, or several in
  Notepad's own bursty startup events) aborts a stale retry loop rather
  than letting it starve real event acquisition or emit late.
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
  suppressed by hwnd coincidence), the UIA focus callback (reusing the
  window handle the arbitration filter already resolved, rather than
  resolving it twice), and the synthetic focus query (`focused_snapshot`
  treats the hidden frame as "nothing focused", so a caller retrying on
  `None` naturally retries past it). The top-level-window announcement's
  own window search additionally skips hidden-frame windows when choosing
  among a process's top-level windows, so resolution lands on a real window
  (a popup menu, a dialog) instead. The fact-announce paths
  (`run_uia_fact`, `run_msaa_fact`, the window job) use a stricter check that
  also walks to the window's top-level ancestor (`GetAncestor` `GA_ROOT`,
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
- Event flow (runtime): the event thread hosts the WinEvent hooks,
  installed once for the fixed target pid before the message loop starts
  (never rebound — a second live `WINEVENT_OUTOFCONTEXT` hook set on a
  thread that already has one has been observed to permanently kill
  WinEvent delivery on that thread for the rest of the process, which decision
  D9's one-pid-per-outpost-for-life design sidesteps entirely rather than
  risking); UIA registration lives on its own thread; both funnel through
  the cross-filter so exactly one backend survives per window. Because
  every Win32 and wx control is its own window handle, per-window
  arbitration is per-control there, while a WinUI top level resolves once
  for its whole subtree. Trace IDs are minted when the OS event first
  arrives, snapshot versions increment per emitted event, and each event
  carries its observation timestamp for the latency ledger.
  - MSAA side (`handle_msaa_event`): the event's own hwnd is exact — MSAA
    events always carry the real window, never an inferred one — so the
    filter just arbitrates it directly. A UIA verdict drops the MSAA event;
    a non-UIA verdict or no verdict yet delivers it, scheduling a probe in
    the no-verdict case. Delivering on an unresolved verdict is what makes
    dropping safe on the UIA side below: MSAA is the backend of record
    whenever arbitration has not yet decided.
  - UIA side (`uia_passes_filter`): most elements that raise UIA events are
    not windows themselves — a menu item or a list item is a descendant of
    one — so the cached native window handle is usually 0. Attribution
    resolves the window in three tiers: the cached handle when the element
    is itself a window, otherwise `verbatim_uia::nearest_window_handle`
    (NVDA's `getNearestWindowHandle`, one cross-process call that walks up
    to the nearest ancestor with a real handle), and only if that itself
    fails, the window holding keyboard focus as a last resort; finding no
    window at all keeps the event, since there is nothing to arbitrate on.
    Once a window is attributed, a UIA verdict delivers, a non-UIA verdict
    drops, and no verdict yet drops while scheduling a probe — symmetric
    with the MSAA side's delivery in that case, because the MSAA hook for
    the same logical element carries the event instead. This replaced an
    M1 heuristic that used the keyboard-focus window unconditionally, which
    is wrong for a popup menu: a popup never takes keyboard focus, so the
    heuristic found the menu's *owner* window while the MSAA event for the
    same menu item carried the popup window itself, and the two backends
    could both defer on their two different windows, losing the
    announcement entirely (found by the M2 E2E suite against the VM). The
    heuristic's compensation was to keep every event on an unresolved
    verdict rather than risk that silence, at the cost of occasional
    duplicate announcements. `nearest_window_handle` resolves a popup menu
    item straight to the popup window itself — the same window the MSAA
    event for that item carries — so both backends now arbitrate on one
    shared hwnd and the compensation is no longer needed.
- `DumpTree` (runtime): answered on a query-pool thread guarded by a five
  second deadline (`QueryPool::run`), the same pattern the foreground
  announcement's queries use, so a hung target abandons the call rather
  than wedging the outpost. Finds the target's currently active top-level
  window the same way the announcement's window step does (`GetForegroundWindow`,
  falling back to its first non-hidden-frame top-level window), arbitrates
  its backend, then walks it: UIA via `Uia::walk_tree`, a raw-view
  `IUIAutomationTreeWalker` driven with the same cache request as every
  other UIA read, so no step of the walk blocks on an uncached property;
  MSAA via `verbatim_ia2::acquire::walk_tree` (rooted at `OBJID_CLIENT`,
  unlike the window announcement's `OBJID_WINDOW` query — a tree dump wants
  the client subtree, not the window's own accessible object), recursing
  through `AccessibleChildren` since this backend has no cache requests to
  prefetch with. Both walkers share the same caps — depth 64, node count
  4096 across the whole walk — and report whether either cap cut the walk
  short.
- `AncestorChain`, `Navigate`, and `Activate` (runtime): each answered on
  a deadline-guarded query-pool thread exactly like `DumpTree`
  (`AncestorChain` shares its five-second deadline, since it chains up to
  64 per-hop round trips; `Navigate` and `Activate` use the standard
  single-query deadline), dispatched to whichever backend's registry knows
  the node id — the same dispatch a `Fetch` re-read uses. UIA hops go
  through the raw-view tree walker with the base cache request; MSAA
  through `accParent`, `accNavigate`, and `accDoDefaultAction`.
- Selection and notification events (runtime): the outpost installs
  `verbatim-uia`'s `SelectionRegistration` and `NotificationRegistration`
  alongside the focus and property registrations, and the MSAA hook set
  includes the four selection WinEvents. Both backends' selection events
  emit `NormalizedEvent::SelectionChanged` (the selected node's full
  snapshot) and UIA notifications emit `NormalizedEvent::Notification`,
  all through the same per-window arbitration cross-filter as every other
  event. The reducer announces both since M3: selection changes under a
  focused selection container, and notification display strings at the
  priority their processing hint implies ([verbatim-core](verbatim-core.md)).
- `OutpostMessage::Event` boxes its `OutpostToSupervisor` payload: the M3
  replies grew the message enum well past the bare-pid `Retired` variant,
  and boxing keeps every channel send small.
