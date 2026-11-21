# verbatim-outpost

The per-application outpost process, `verbatim-outpost.exe`: one outpost
per application, many at once, each watching its application for its whole
life, answering Core's requests of the shared list
(`verbatim_model::request`) and telling Core what its `Follow`
registration hears ([ADR 0003: Readers tell, Core asks](../adr/0003-readers-tell-core-asks.md)). Core's supervisor ([verbatim-supervisor](verbatim-supervisor.md))
starts, watches and ends it. Its reducer, the pure state machine every
remembered fact of its application is kept in, is
[verbatim-outpost-logic](verbatim-outpost-logic.md); the rules it shares
with the listener are [verbatim-reader-rules](verbatim-reader-rules.md),
and the window queries [verbatim-win32](verbatim-win32.md). The reasons for
this shape are in `books/dev/src/explanation/architecture.md`, section 1 ("Outposts", "The focus
listener (D13)"), in [How focus works](../../explanation/design/focus.md)
and in [ADR 0001: Focus](../adr/0001-focus.md).

An outpost's application is set once: by `--target-pid` on its command
line, or, for a spare, by the supervisor's `Assign`. Its `WinEvent` hook
installs once, when it has its application, its UIA registration once
during construction, and there is no rebind path. Outposts stay alive when
their application loses the foreground.

## How focus reaches an outpost

The focus listener is `verbatim-listener.exe`, with no target. It
holds the one desktop-wide UIA focus registration, the desktop-wide UIA
event group, and the global MSAA hooks, and tells Core of each event
([verbatim-listener](verbatim-listener.md)). Core judges every tell in
its event queue and asks a reader for what it reads: the listener for a
UI Automation focus's element and route, since an element cannot be
handed to another process, and the application's outpost for the
rest, by a request of the shared list (`CoreToOutpost::Request`), each
answered once (`OutpostToCore::Answer`). An outpost hears nothing of the
desktop but its console host's events and its `Follow` registration, and
calls into its application only for a request.

The requests an outpost answers: the completion of a UI Automation focus
from the listener's route (`Request::CompleteUia`, `outpost::completion`),
one MSAA candidate read to the depth Core asks (`MsaaRun`), the
application's current focus (`CurrentFocus`), a node's properties,
navigation, children and finds, a node's text (`Text`), an activation,
the registration that follows the committed focus (`Follow`), a tree dump
and the shell's items. At the start, and after an outpost or the
listener is replaced, Core asks for the current focus.

### The console host's events

No outpost hooks a `WinEvent` but the console host's: its event thread
hooks the console's update and caret events (`CONSOLE_SUBSCRIPTIONS`),
scoped to the console host's process, which Windows scopes by the
event's thread. The outpost hooks them once, the first time `Follow`
names a terminal in a console's window (`FollowGroup::Terminal { console:
true }`, `Context::hook_console`), whatever binary hosts the console
(`conhost`, `OpenConsole` behind Windows Terminal, or a test's stand-in),
and before it answers that `Follow`, so every console event raised after
Core's baseline read is heard. A hook that cannot be installed is a
`Fault`. The listener tells Core of every other
kind, and Core asks the outpost for what it reads
(`docs/design/readers-tell-core-asks-build1.md`).

### Console windows and frames of another process

A console window is one application in its console host's outpost, as
NVDA gives it and its text area the client's app module. The listener sends each
`WinEvent` for a `ConsoleWindowClass` window to the process of the event's
thread, the console host (`verbatim-listener`'s `tells::routed_process`, by which Core's reads of a window's MSAA object are routed too), dropping one whose
thread cannot be opened, so Core asks the console host's outpost of the
window, which answers a current-focus request for it. A focus answer
carries its application's name, read once per process and kept with the
process held open (`outpost::ApplicationNames`).

An outpost takes a window as its application's when the application serves
its UIA (`verbatim_win32::provider::served_by`, through
`verbatim_win32::provider::served_by`, local calls), and `read::focused_control_in` reads no focused element in another
process's window (`read::control_window`). `GetFocusedElementBuildCache`
reads the keyboard focus again itself and can return an element of the
application the focus has just moved to
(`experiments/getfocusedelement-after-the-focus-moves/`), so its request
caches the element's process (`Request::with_process`), and an element
whose cached process is not the outpost's application, the console host
for a console, or that has none cached, is read no further: no
enrichment, walk, subscription or report, `None` answered, and a debug
line naming both processes. Each read is logged at debug with what asked
for it (`read::FocusRead`). The one connection the call makes into
the other application cannot be prevented.

Every window a request names is first checked as its application's
(through `served_by`): another process's window is neither probed, read
nor reported, with a debug line naming it, since any process can raise a
`WinEvent` naming another's window, and the probe, or a console window's
check by handle, would call into that process. Core routes a console
client's request to its host through `verbatim_win32::provider`'s
`provider_process` and `console_host_of`.

A focus whose top-level window belongs to another process, as the Settings
page inside `ApplicationFrameHost`'s frame, is read by the listener up to
the window below the boundary, and the outpost completes the chain with
that window's parent or owner, and every window above it to the desktop,
read from Windows' own state with no call into the other process
([ADR 0001: Focus](../adr/0001-focus.md), "Object identity"; the decision
of 2026-10-29). A plain
window (`read::plain_window`) is read from Windows' own state alone: role
window, named by `InternalGetWindowText`, unavailable when its style
disables it, never focused and never invisible, kept under its window
object's address. A window `read::plain_windows_from` reads, above the
process boundary, past the depth limit, or after a failure, that is not a
ghost's or a hung application's is read the same way but described as
NVDA's MSAA client for it (`read::client_pane`): role pane, marked
`top_level_client` when its style has `WS_SYSMENU`, so Core speaks the
Settings frame, or a top-level window the listener's read did not reach,
by its title, and takes a child window between as structure; a ghost's or a
hung application's stays a plain window. The ghost and hung checks are
three local calls per window, and the style is read once
(`window::window_style`). The outpost connects to, probes, and walks up into only its
application's windows (`read::serves`): its UIA walk
(`read::uia_ancestors`) stops below the boundary, a step that reaches
another process's window anyway is not read further, the MSAA walk does
not step from a window's own window object into a parent window it reads
as plain (`read::reads_as_plain`) and does not read a parent an
`accParent` named in one, and a focus route through one is not followed
(`Context::focus_route`). The accepted exception is object navigation's
raw-view step (`verbatim_uia_rops::navigation_step_classic` when the
program is not run), which reads such a neighbor through UIA
before `read::corrected_backend` switches it to MSAA.

Where a walk ends below the desktop, the chain goes on from a window
(`read::window_onward`), as NVDA's `Window._get_parent` does: after an
MSAA `accParent` that fails or names nothing, a UIA step that fails or
times out, or a walk that fails before it reads anything
(`read::onward_after_failure`), the parent window of the outermost
object's nearest window, never that window itself; after a window read
through the other API, that window; above the boundary, the parent or
owner. Each window is read by its own object as NVDA chooses it, its UIA
element when the outpost judges it UIA, its MSAA client otherwise, and
walked up by that backend; a window another process serves and every
window above it, a ghost's or a hung application's window, and a window
whose MSAA client cannot be acquired is a plain window, the chain going on
from the parent window of the last two. A window above the boundary is
marked a stand-in (`NodeDetails::above_boundary`, by
`read::plain_windows_from`), the same object as its own application's
entry for it. A window read through UIA whose element cannot be read
loses the focus (`read::FocusLost`): the program reports nothing, as
NVDA's unguarded `ElementFromHandleBuildCache` ends its event. In debug
builds every focus chain the outpost sends is checked to hold one entry
per window before it is published. Every UIA walk, the listener's
classic walk (`FocusQuery::plain`, judged with the listener's
window readability reader) and the outpost's
(`read::uia_walk_leaves`), checks the window each step reaches, and a
ghost's or a hung application's window other than the focus's nearest one
ends the walk there, the parent in it not kept, as NVDA makes that one
step and then reads the window as a plain window: the listener names it
to go on from (`AncestorsEnd::ContinueFrom`), and the outpost goes on from
it. The check is asked before the outpost's verdict for the window, so a
hung window is never probed. A remote program cannot judge a window
between its steps, so a current-focus request whose focus has such a window
above its own (`read::plain_window_above`, local calls by
`verbatim_win32::provider::raw_view_parent_window`) reads its ancestors by the classic
walk instead. The outpost's walks have no time limit: a listener read cut at its depth limit
(`AncestorsEnd::PlainFrom`), and a walk cut at its depth limit of 64, are
completed by plain windows from the outermost element's nearest window,
included, and no continuation is cut by time. Whether a
window can be read is the system's judgement
(`verbatim_win32::window::readability`, a `Readability`), which a
test that cannot wait five seconds for Windows to judge an application
hung replaces (`Outpost::set_window_readability_reader`, a
`verbatim_win32::window::WindowReadabilityReader`).

### Starting an outpost and asking for the focus

Core starts an outpost for an application when the listener's
foreground report names it, and for Verbatim's own process at the start
(`QueueEffect::StartOutpost`, `crates/verbatim-core/src/event_queue/outposts.rs`);
a request for an application with no outpost waits for its start. Node
identity never crosses a process: the outpost mints each `NodeId` it
answers. Nothing polls: at startup, and after an outpost or the listener
is replaced, Core asks for the current focus.

An application usually gets a spare ([ADR 0001: Focus](../adr/0001-focus.md),
"Spare outposts"): the supervisor keeps two outposts started with `--spare`, with
everything made but its application, which say `SpareReady`. A start takes the
oldest, ready or not, and sends `Assign`; the spare sets its application,
waits on the application's exit, and
says `Ready`, the supervisor holding the application's facts meanwhile.
Spares start once Core's own outpost is ready or has ended; a given spare
is replaced at once. Spares are pinged but never asked in idle rounds.
No outpost is ended for being idle.

Every outpost failure takes one path, Core's generation record
(`crates/verbatim-core/src/supervisor/record.rs`, `failed`): the process
exits or crashes, its console hook fails (a `Fault`), its launch fails, it misses three pings, or it piles up abandoned
workers. The failure is logged with its last fault, as an error (heard)
for a spare or before `Ready`, a warning otherwise. If the application has
exited, that is its end. Otherwise the application keeps its record, held
facts, and any idle query, and gets the next spare or a new outpost: the
first failure at once, further ones after a backoff (`policy::Backoff`) of
1, 2, 4, 8, 16, then 30 seconds, counted again once an outpost stayed up a
minute or the application comes to the foreground
(`verbatim_core::supervisor::BACKOFF`). The
requests out to the failed generation are answered "reader ended", and
those waiting for the application go to its next one. Spares' failures
are replaced with their own backoff.

### Who calls into an application

Only an application's own outpost sends it a remote operation or makes a
new UIA connection to its windows by a call of Verbatim's, with the
exceptions [ADR 0005](../adr/0005-calls-that-reach-another-application.md) states in its limits: UIA's client
side's connections to a parent across the boundary, and the focus-read and
walk-up races (`books/dev/src/explanation/design/listener-without-connections.md`, section 2),
so no lock is needed around them. No node is searched for by runtime id:
a UIA node whose kept element is gone, or that has none, and whose focus
route's walk finishes without it is answered gone
(`read::resolve_uia_element`), while a walk that fails, a window lookup
timed out or refused among them, fails the query (`focus_path::find`),
since a slow application has not shown the element gone. The walk has no
time budget: it runs until it ends, unless Core cancels its request and
its worker is abandoned. A
listener's selection is reported with what its event's cache holds
(`verbatim_uia::ListenerSelection`), since a search of a subtree enters
any child window another process serves (the study's instance 3). The supervisor's `App` table holds each application's
process open while it has a record, so its pid is never reused while an
outpost or reading thread serves it. Registrations scope windows' subtrees
by their elements (`Scope::Subtrees`), with no connection by handle.

## Public API

### The protocol

The messages are `verbatim_model::protocol`'s, and the framing, the shared
clock and `peek` are `verbatim_process::message`'s
([verbatim-process](verbatim-process.md)).

### The arbitrator

`Arbitrator` is NVDA's per-window backend decision (`_isUIAWindowHelper`)
as the outpost reads a window. It holds the configuration: the class
lists and the classic walks' clock. `class_verdict(&WindowClasses)`
answers from the good class list, the Windows 11 shell rule, the bad
class list or the override, and `None` when only the probe can decide; a
program then probes the window with the raw probe
(`verbatim_uia::raw_probe`) through `arbitration::uses_uia`. No
connection follows a probe
(`books/dev/src/explanation/design/listener-without-connections.md`, decision 2). The listener
makes no verdict; it skips a read only by
`verbatim_reader_rules::class::class_verdict`, the class lists alone, from
the same tables and code, which the arbitrator's own lists are built from.

- Core's backend cache is the one lasting keeper of a window's verdict
  ([ADR 0004](../adr/0004-focus-validity.md)): a request carries Core's
  verdict for its window, or "probe first". The outpost keeps what it
  probes only for the request it probes in
  (`arbitration::RequestVerdicts`, held by the request's context and
  dropped with it), so a window a walk crosses is probed at most once per
  request, and a "probe first" request reports only a probe it made,
  begun when that probe began on the observation clock. A window Core is
  never told of costs one probe per request that crosses it.
- `verbatim_win32::window::window_classes(hwnd)` reads a window's
  `WindowClasses` (`verbatim_reader_rules::class`): the raw class, the
  class normalized as NVDA does (`verbatim_model::class::normalize`), and the root
  ancestor's class.
- `post_probe_check` (`verbatim-reader-rules`) names NVDA's check before using a provider: a list
  view's Windows Forms origin, or a console's formatted text, read from
  the window. A provider that fails it is read through MSAA.
- A timed-out probe is an unknown verdict (`arbitration::Verdict::Unknown`),
  which the worker reads through MSAA for the rest of its request. A
  probe on an abandoned worker sends nothing and keeps nothing.
- `Outpost::set_observation_clock` sets the clock the outpost's own
  events and the probes it reports are stamped by, for a test that moves
  that time on.
- Both class lists are `verbatim_model::class`'s, written from
  `docs/parity/backends.md` ("The class lists the per-window verdict
  reads").

### The outpost

`Outpost`, `run_pipe`, and `run_attach` are the per-application outpost.

- `run_pipe(pipe_in, pipe_out, target_pid, options)` runs one on Core's
  inherited overlapped pipes, watching `target_pid`, or for `None` as a
  spare. `Outpost::assign` sets the application once and says `Ready`; it installs no hook (the console's is
  `Context::hook_console`, through `verbatim_ia2::event_thread::Nudger::hook`).
  ([ADR 0001: Focus](../adr/0001-focus.md), "Spare outposts"). It serves until
  `Shutdown` or the end of the command pipe, then shuts down cleanly, and a
  thread waiting on the target sends `TargetExited`.
- `run_attach` watches a pid, asks for its focus through the request driver
  (`driver::Driver`, a `Request::CurrentFocus` of its main window, as Core
  asks at its start), and prints each outbound
  message as the model's JSON on stdout: the development mode.
- `OutpostOptions::remote_operations` (`verbatim_process::arguments`) says whether a UIA focus's ancestry
  may be read with a remote operation (on; `--classic-uia` turns it off).
- For tests in their own process: `Outpost::new` and its siblings watch
  from construction and panic when the hook fails, with a sender thread in
  place of the I/O thread; `with_options`; `with_focused_element_reader` (a
  `FocusedElementReader` in place of `Uia::focused`);
  `set_foreground_reader` (a `verbatim_win32::window::ForegroundReader` in place of
  `GetForegroundWindow`, for acceptance and the window a focus entered);
  `set_focus_window_reader` (a `FocusWindowReader` for focus-now); `set_observation_clock` (a `verbatim_process::clock::ObservationClock`
  for the outpost's own events' stamps and the probes it reports).
- `Outpost::settle` posts a barrier (`Item::Barrier` with
  `Asker::Settle`) and waits until the reducer reaches it with no program
  in flight, everything queued before it handled and a noted caret read,
  then, on the caller's thread, until the subscription has made every move
  asked (`Registration::settle`), every published message is written
  (`Outbound::flush`), and, with the `sync-marker` feature, which only
  mockapp's tests turn on, the event thread has taken every nudge and
  every `WinEvent` raised before it. The
  reducer answers it as it is reached (`Effect::Settled`), waiting for no
  open caret watch or held copies of the hook's, as
  Core's round does; it divides batches as the round does, and waits for
  nothing that arrived after it. The shell keeps its waiters in the order
  they asked and answers the first at each settle reached, since barriers
  are never merged or reordered; at shutdown the intake drops them and the
  shell its waiters, so the call returns. It covers only what has
  reached the outpost.
- `Outpost::failed_reads` gives the worker's and subscription callbacks'
  failed cached reads (`verbatim_uia::calls`), also logged at shutdown.
- `Outpost::observe_heard` hands a `HeardObserver` each event the outpost's
  own handlers and hook take in, as a `Heard`, once queued.
- `Outpost::observe_request_calls` hands a `RequestCallsObserver` the
  calls into the application each of Core's requests made, by its
  number, before its answer is sent, for a test that pins a request's
  calls.

Inside an outpost (`docs/design/communication.md`, section 6.4;
`books/dev/src/explanation/design/ownership.md`):

- The mailbox (`outpost::shell::Mailbox`): the console host's hook
  callbacks (`CONSOLE_SUBSCRIPTIONS`), the subscription's callbacks,
  Core's commands, and the workers' commits and ends only append to it,
  under its one lock, and return. A UIA callback keeps the cached parts and
  window handle, and parks its element in the store (`store::Parked`),
  the entry carrying the key.
- The reducer ([verbatim-outpost-logic](verbatim-outpost-logic.md)), a
  pure state machine on a thread of
  its own, run by its shell (`outpost::shell`): it holds Core's requests
  queued for the worker, the intake's order, the caret keys' watches, and
  the program in flight with the worker generation in charge. Inputs come in mailbox
  order; effects go out: dispatch a program, send a message, release,
  abandon a worker. It names no Windows type, holds no application object
  and makes no call. A program has no deadline and the outpost judges no
  hang: a worker is abandoned only when Core cancels the request it runs
  because the foreground moved away. A release's answer is given at once,
  as the reducer's next input, before the shell takes another message. `cargo xtask ci`
  reads every file of its crate by the pure rule
  (`xtask/src/ownership.rs`). The shell waits on the mailbox's alarm
  (`verbatim_process::alarm::Alarm`): an event set by an append while it waits, and a
  high-resolution timer set to the reducer's next timer, so no watchdog
  thread or threadpool timer is kept, and each timer
  costs one wake. Every lock its thread takes is a short hold with no
  call: the mailbox's, the backlog's with the store's inside it for a
  release, the store's for a commit's writes, and a worker's charge.
- What the shell and the workers guarantee the reducer, which its
  generated test's generator relies on and gives no more than
  (`books/dev/src/reference/crates/verbatim-outpost-logic.md`): the shell ticks
  the reducer after every drain of the mailbox, not only when a timer is
  due; each of the three clocks of `Now` never goes back; an abandoned
  worker still posts its program's finish; a
  program's posts arrive in the order posted, and its finish exactly once;
  Core's request numbers are unique for the outpost's life; entries
  queued behind the close still reach the reducer; and the shell gives the
  reducer only a commit it accepts.
- Locks (`outpost::lock`): every lock the crate declares is a
  `Lock<T>`, a mutex whose `lock` is `#[track_caller]` and, in debug
  builds, records in the thread's slots where it was taken
  (`verbatim_win32::held_locks`); its `LockGuard` empties the slot when
  dropped. The crate's `clippy.toml` disallows `std::sync::Mutex`, so no
  lock escapes the record. A call into an application made while a slot
  is filled fails the debug-build check: every call the backends count,
  every agile reference and the drop of the store's `Removed` check. The
  worker's catch around a program (`worker::catching`) and UIA's around
  its event callbacks end the process when the panic is the check's.
- The worker installs a call gate on its thread (`verbatim_win32::calls`),
  bound to its charge, in release builds as in debug ones, and uninstalls
  it as its thread ends: once its generation is abandoned, every counted
  call fails with `CallError::Abandoned` and makes no call, which no
  caller reads as a fact about the application. Nothing is refused for a
  hung window: Core judges a hang. An event's check before its first call
  (`Worker::readable`) still drops an event of a window that cannot be
  read, by the outpost's reader. The pong reports the abandoned workers
  that have not returned (`parked_count`), one value (`Lock<usize>`) written when a
  worker is parked or returns and read once per ping.
- Programs (`outpost::worker`): the worker generation in charge takes one
  program at a time from its inbox, Core's request, a watch's check or
  final read, an entry's handling, or a release's drop, with a copy of
  the store's text facts taken as it starts (`Store::text_facts`), and
  posts each commit (`commit::Effects`) to the mailbox, applying its text
  facts' changes to its own copy, so what it reads after is what the
  store will hold. The shell applies a commit only while its request is
  live, the program in flight of the generation in charge, and refuses
  any other whole: first the
  writes only it makes, to the store and its table of the elements the
  subscription follows (`commit::Writes`), the program's text writes
  among them, then the reducer's change. The inbox
  is a plain channel of jobs the worker waits on; the shell lets go of an
  abandoned generation's sender, which ends its worker once it returns.
  The worker keeps nothing between programs. A worker the system cannot
  start, the first at `Shell::start` or a replacement, leaves its
  generation with none (`worker::spawn` returns the error, logged at
  error level), and the outpost keeps running: each dispatch tries once
  to start the generation's worker, warm as the first was, and with still
  none answers its program as failed ("the outpost failed handling the
  query") and keeps what the shell let go of, whose release can call into
  a hung application, for the next worker to drop. `worker::spawn` carries
  the `ThreadStart` fault point under the `faults` feature
  ([verbatim-faults](verbatim-faults.md)), which mockapp's `failure_paths`
  tests use.
- The focus-following subscription (`outpost::followed`;
  `docs/design/readers-tell-core-asks-build1.md`, "Changes on objects"
  and Table 10): Core's `Follow` names the committed focus's node, its
  event group and any extra object, or nothing, and the outpost keeps that
  node's registration, adding it when absent, and removes every other. The
  completion of a UI Automation focus adds the new node's registration
  while keeping the one `Follow` named, so the old focus stays followed
  until Core commits the new one. Core alone decides the group: the
  completion gives the new node the base group, unless it is the node
  `Follow` names, which keeps the group and extra object Core named, so a
  repeated focus or a re-take on a followed terminal leaves its text
  changes coming. The base group holds name, value, and state changes and
  `Text_TextSelectionChanged`; the extended group, which Core names for
  Word's document and Windows Terminal's text area alone
  (`docs/parity/backends.md`, "Which focus takes text-change events"),
  adds `Text_TextChanged`; the active text position changed event is not
  registered until browse mode (M7) acts on it.
  Each settled move is logged at debug level as "the focus registration
  moved", with what made it and the worker's wait (`elapsed_us`). The
  console host's text area takes the
  group without its text events (`Group::ExtendedWithoutTextEvents`), and
  the console's caret events are its caret events
  (`books/dev/src/explanation/design/wake-ups.md`, item 15). The extra
  object, the parent of a focus that is a `SensitiveSlider`'s thumb, is
  given the value group (`Group::Value`), since Core speaks its value
  changes while its thumb is the focus (`docs/parity/focus-context.md`,
  "A slider that focuses its thumb"). Registrations are never on the
  elements' descendants (`docs/parity/backends.md`, "UIA event
  registration"). The registering thread makes every registration
  (`verbatim_uia::Registration`); `RemoveAllEventHandlers` only at
  shutdown. What the registration hears is told to Core
  (`OutpostToCore::Tell`), its element parked in the store
  (`store::Parked`) for Core's later request to name.
- The queue (`verbatim-outpost-logic`'s intake): what the outpost still
  hears, its `Follow` subscription's UI Automation events and a console
  host's console events, with Core's lists of held nodes and idle
  queries, in arrival order, one entry per object and kind, a newer
  replacing it at the back (the console host's events per window and
  kind). A batch is what accumulated during the previous one. Core's
  event queue judges every other event
  (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
  build"). Nothing is dropped at planning for a hung window: the worker
  tests a UIA event's own window first as it handles it
  (`IsHungAppWindow`, a window of 0 passing, as NVDA's test passes a
  sender with no cached window), before any window lookup or read, and a
  dropped event's parked element and range go with it.
- The worker generation in charge is the only one calling into the
  application, one program at a time, so tells and answers leave in the
  order made. NVDA's acceptance test is Core's
  (`crates/verbatim-core/src/focus/acceptance.rs`), on the window facts a
  tell carries. A program takes the backend
  crates' per-thread call counts into each published `EventTiming`; a
  program that publishes nothing logs its calls (`books/dev/src/reference/performance.md`,
  "Cancelled traces").
- Abandonment is the reducer's, and only for Core's cancel of the request
  a program runs because the foreground moved away: a program has no
  deadline and the outpost judges no user moving on. It marks the
  generation dead in the worker's charge (`worker::Charge`), answers the
  request "abandoned", and starts a replacement. A returning abandoned
  worker has its commits refused, posts that it returned, and exits. The
  shell alone counts the abandoned workers that have not returned, one
  more as it carries out `Effect::Abandon` and one fewer at each return,
  which no longer reaches the reducer, and publishes the count the I/O
  thread answers pings with.
- A focus is answered once its walk returns, in full, however long the
  walk takes: no walk is published at a bound.
- Requests: each program is a request (the generation and a request
  number the reducer gives), whose `RequestContext` carries the store's
  handle; the program in flight's request is live until it ends or its
  worker is abandoned. A program's text writes
  go to the store only with a commit the reducer accepts, as every other
  write of the shell's does ("Window destruction and held objects", text
  on copies). A terminal read works on a
  copy of its entry, put in place only by its commit.
- Core's requests wait in the reducer's `Asks` and are each answered
  once: the program's answer, ended "abandoned" for a program whose worker
  was abandoned, ended "failed" for a program that ended without
  answering, and ended "cancelled" for a `Cancel` before dispatch
  (`docs/design/readers-tell-core-asks-build1.md`, Table 14).
- Time limits in an entry ([ADR 0001: Focus](../adr/0001-focus.md), "Time
  limits"): UIA
  calls end at `verbatim_uia::UIA_CONNECTION_TIMEOUT` (2 s) or
  `UIA_TRANSACTION_TIMEOUT` (10 s); the continuation of a listener read's
  chain, an MSAA focus's ancestor walk and a focus read's walk have no
  limit of their own; a dialog's text, UIA's as MSAA's, is gathered in
  its program on the worker's own client, as NVDA gathers it inside its
  focus event; tree view, list view, and edit messages use
  `SendMessageTimeoutW` with no limit, sent whatever the window's state;
  an `IAccessible` call has no limit. Core bounds each request by its due
  time.
- The I/O thread, the main thread (`verbatim_process::transport::ChildIo`,
  at `THREAD_PRIORITY_HIGHEST`), waits only on its completion port, hands
  each command to `Outpost::handle_command` (which answers pings itself and
  appends everything else, a `Cancel` among them, to the mailbox), and
  finishes pending writes.

The workers, their reads, the reducer, the shell, the writer, the protocol, and the
supervisor's owner, policy, and writer modules forbid `unsafe`; what remains
is window-manager calls, the event thread's message loop, process creation,
and the inherited pipe handles.

### Text and dialog text

- `text` is the outpost's side of the text protocol
  (`books/dev/src/reference/crates/verbatim-model.md`, "The text protocol"), public so mockapp
  drives it. `perform(source, anchors, op)` answers a `TextOp` over a
  `TextSource`, a backend's view of one node's text in UTF-16:
  `uia::UiaText` over a text pattern (positions are `UiaPos`; `new` takes
  the element for remote operations; `remote`, `details`, `support`, and
  `known_support` say whether caret reads try a remote operation, which
  formatting they read, and the `TextSupport` known before and after), or
  `edit::EditText` over an edit control's messages (offsets; a rich edit's
  word is the control's own, `EM_FINDWORDBREAK`, and a plain edit's the
  line broken by Uniscribe, `verbatim_ia2::uniscribe`, with NVDA's two
  letters appended, so a run of one punctuation mark is one word; a
  character is Uniscribe's in every edit control, as NVDA's
  `EditTextInfo` forces both; the plain edit
  control alone answers `TextSource::measure`, a point's offset and the
  text's length, so a location there is answered `OffsetLocation`, and a
  rich edit's is answered `Location`, as NVDA reads a rich edit through
  its text object model).
  `caret_report_from` reads the caret and its line; `held` reads what a
  caret key and a caret tell ask (`held::starting_read`,
  `held::read_selection`, `held::perform_placed`), and `check` a watch's
  checks. `Anchors` keeps a backend's anchors by node for a reader that
  holds them for the whole read, a test or mockapp, until
  `forget_anchors`; `NodeText` is one node's,
  borrowed from it or from the crate's `NodeTextCopy`, the copy a
  program's text call works on (`position_at`, `reported_point`). Both hold the reducer's `NodeAnchors` with each
  position a `Slot`: the source's position and the key it was kept under,
  so a call's result names a position it was given by the same key and
  only one it made is given a new one (`NodeTextCopy::finish`);
  `named_anchors(op)` lists the anchors of the positions an operation
  names, which a copy must hold.
- `dialog_text(&dialog)` gathers a dialog's own text by NVDA's rules
  (`docs/nvda/object-model.md`, "A dialog's own text") over a
  `DialogParent` whose children are `DialogObject`s: MSAA through
  `verbatim_ia2::dialog::DialogObject`, UIA through
  `UiaDialog::new(uia, request, element)` and `UiaObject`, each child read as
  a `verbatim_uia::DialogText` with its description whatever the theme
  (`books/dev/src/explanation/design/uia-shapes.md`, decision 5). `is_dialog(role)` says which
  roles have such text. The walk counts every child it fetches against
  `MAX_OBJECTS` (65,536) and stops at an object it visited before
  (`Visited`, by runtime id for UIA and by window, object id and child id
  for an MSAA window's client area, `verbatim_ia2::dialog::DialogObject::address`,
  the one object of the walk with an address of its own; Claude,
  2026-11-21), giving the text read up to either.

## Implementation notes

### The current focus

- The answer to Core's `CurrentFocus` request (`asked::current_focus`)
  is the focused control (`hwndFocus`, through its window's backend) with
  its ancestors up to one Core already knows, the selected item that is
  its value where it has one, and its caret (a console's text area reads as a nameless terminal,
  `read::console_text_area`); and, for a focus that met no ancestor Core
  knows, the foreground object, read after the focus as every focus
  answer reads it (`read::foreground_object_among`, the outpost's
  foreground reader, `GetForegroundWindow`, when the application holds
  the foreground), as NVDA reads `objectInForeground` after the focus's
  ancestry only when it shares nothing below the desktop with the old
  focus's (`eventHandler.py` 402 to 413). A foreground or focus window that
  is a ghost's or a hung application's is answered as a plain window, the
  focus with the plain windows above it, with no call, a deliberate
  difference from NVDA, which says nothing (Dickson, 2026-10-29). Through
  MSAA the focus is the focus window's client's `accFocus`, and an object
  it names in a window this application does not serve answers no focused
  control (`verbatim_ia2::acquire::focused_object`). There are no
  retries. Its UIA ancestry is one remote operation
  (`verbatim_uia_rops::focus_ancestry`); with remote operations off, or
  for a window whose element cannot be imported (a proxy, marked for the
  window's life), the per-hop walk is used. Every remote operation that
  falls back to the classic calls, and every window marked for classic
  reading, is logged once, by `remote_fell_back` and `read_classically`,
  to the `verbatim::remote_fallback` target, with the program, its error
  and the window, which fails an end-to-end scenario. Each UIA node it answers keeps
  a route to its element (`completion::keep_read_routes`), made from the
  raw view's chain the walk stepped through (`AncestorStops::visited`),
  unreported ancestors included: from the nearest element at or above it
  with a window of its own, or, where the walk met an ancestor Core
  already knows, from that ancestor's route. A later focus whose
  listener walk meets one of these nodes is found by that route, as one
  below an ancestor a completion minted is. Each walk on from a window
  (`read::window_object_onward`), on a focus-now read, a run's MSAA focus
  or a completion's chain above its route, records the routes of the
  nodes it mints the same way, from that window's element.
- An MSAA client object of a `WS_SYSMENU` window is marked
  `NodeDetails::top_level_client`, read as a pane and presented by its
  title, as NVDA's `isPresentableFocusAncestor` presents it.
- A plain window (`read::plain_window`), a window's pane read from local
  state (`read::client_pane`), and a foreground window answered from local
  data carry their handle in `NodeDetails::window`, as the readers' UIA
  window elements and MSAA window and client objects do, so Core takes the
  frame one outpost reads as its focus and another as a pane above its
  content as one container.
- Hidden-frame suppression (decision D9): Core's hidden 1x1 frame, marked
  with `verbatim_model::HIDDEN_FRAME_WINDOW_PROP`, is never announced: a
  reader's window facts say a window is the frame, and Core drops a focus
  there; `focused_control_in` treats the frame as nothing focused.

### Event flow

- The event thread hosts the console host's hook, never rebound: a second
  `WINEVENT_OUTOFCONTEXT` hook set on a thread that has one has been seen
  to stop delivery on it for the process's life. It hooks the console's
  events when `Follow` first names a console's terminal, and queues each
  as it comes.
- Trace ids are minted when a tell is made. `WindowFacts` (`window_facts`,
  local calls) give the top-level window and root owner, topmost style,
  `Windows.UI.Core` activity, and for a focus the window's class, the
  application's name (executable stem, lower-cased; the client's in a
  console window), and whether Alt is held.
- A UIA focus's answer carries the element's class
  (`NodeDetails::class_name`) and its parent's automation id
  (`NodeDetails::parent_automation_id`) for Core's per-application rules.
  A node read alone carries grid row and column when Core's request names
  cell coordinates, and a focus and a navigation step's neighbour their
  table (`NodeDetails::cell`), read in the remote
  program where one runs (`FocusQuery::is_cell`).

### Reading the focus

- A UI Automation focus is the completion of the listener's read from its
  route (`outpost::completion`): the outpost finds its own copy of the
  element by the route, mints node ids from the runtime ids the listener
  read, replaces the element kept under a reused runtime id, adds the
  focus's event registration and waits for it before it reads the node's
  states, and completes the chain above the route, through MSAA from a
  window it reads that way and above the process boundary by plain
  windows. Where the listener's walk ended at an ancestor Core already
  knows, the route's end names it, so the completion makes no read to
  find it. A focus whose element is in no tree, as Windows Terminal's tab
  focuses, is answered all the same.
- A focus with no window of its own whose walk up the listener cut below
  its nearest window, so that no element of its chain has a window of its
  own, is routed from that nearest window, which the route carries, down
  by every runtime id of the chain (`FocusPath::route_to`), which finds it
  when the chain's outermost element is that window's child.
- Any focus that no route finds is read as its application's focused
  element instead (`read::focused_element_in`, trigger
  `read::FocusRead::NoRoute`), and kept only when its runtime id is the
  focus's and its process is the outpost's own. It finds the element only
  while the element still has the keyboard focus, a limit.
- An MSAA focus is one candidate per request (`MsaaRun`), acquired with
  NVDA's child-0-on-a-list redirect and the `accFocus` redirect
  (`focus_candidate`); a menu start with neither, at its event's address
  (`menu_start_candidate`), as NVDA builds it. The outpost reports facts and judges none: a focus
  event's focused state over the object and its ancestors, read live; a
  menu start's role. It then reads it in full unless the depth is
  `Facts`, or `FullIfFocused` with the object not focused, even when it
  repeats the focus, since Core judges repeats; a redirected focus names
  the child's ids as its event ids, as NVDA builds the redirected object
  with the child's id as its event's. At `FullUnlessSame`, a focused
  object reached at the ids Core names whose identity reads as Core names
  it (`FocusCandidate::reads_as`) is answered `Reply::CandidateSame`, with
  nothing more read: the outpost computes the comparison as a fact against
  the values Core names, and Core decides the duplicate.
- A focus walk stops at the ancestors Core already knows, which its
  request names, comparing what it reads with them by NVDA's equality,
  and gathers the text of every dialog below them, which the focus
  enters (`read::describe_dialogs`).
- A walk that finds no parent, or whose step fails, below the root (or
  below a top-level window, for the remote program and `Uia::ancestors`)
  goes on from the parent window of its outermost element's nearest
  window (`Ancestry::failed_hop`, `AncestorWalk::Failed`,
  `verbatim_ia2::acquire::Walked::Onward`), and one cut by its depth limit
  is completed by plain windows (`AncestorWalk::Cut`, `Walked::Cut`), for
  a UIA walk from the parent window of its outermost element's window
  when that element is the window's own, so the window is not in the
  chain twice (`verbatim_win32::provider::plain_windows_start`); a walk that fails
  before it reads anything, or an MSAA node no longer reachable, goes on
  from the parent window of the focus's nearest window, read live
  (`read::onward_after_failure`).
- NVDA's focused-state test of an MSAA focus
  (`FocusCandidate::has_focused_state`) goes on past an `accParent` that
  fails or names nothing from the parent window's own object, chosen by
  `read::parent_window_for_focused_state` (the audit's difference 19): by
  the outpost's verdict, its MSAA client, read and walked up from, or its
  UIA element's focused state, one call; a ghost's or a hung window is a
  plain window, never focused, and the test goes on from its parent
  window; above the process boundary every window is plain, and the test
  ends with no call there.
- A menu item's legacy MSAA checked state is one more live call
  (`verbatim_uia::map::with_legacy_checked_state`), for its focus and for
  each state change the `Follow` group tells of it, read from the event's
  element (`verbatim_uia::map::with_legacy_checked_states`).
- A UIA navigation step (`read::uia_remote_step`) is one remote operation
  (`verbatim_uia_rops::navigation_step`) from the kept element, but classic
  from a top-level window (a program's walk ends there), with remote
  operations off, or in a window marked classic. A gone element is found
  by the focus's route and stepped from classically, or answered gone.
- A dialog's own text (`read::describe_dialogs`) is given as the
  description of every dialog in the chain below the ancestors Core
  already knows, and of a dialog focus, while descriptions are read. A UIA
  dialog the listener read, whose element the outpost does not hold, is
  found by the route of the answer being built, so it has its text on its
  first read.

### Menus

A menu's opening, start and end, and the Alt+Tab switcher's end, are
judged by Core (`crates/verbatim-core/src/focus/menus.rs`), which asks
the outpost for a menu start's object or the application's current focus
as NVDA's handlers read them.

### Window destruction and held objects

- Window destruction: `EVENT_OBJECT_DESTROY` for a window drops its
  classic mark, and its MSAA nodes.
- Held objects: the object store (`outpost::store`;
  `books/dev/src/explanation/design/ownership.md`) owns both
  registries, each node's text patterns and anchors, the terminals' kept
  ranges, and the caret tells' elements. Its API returns owned clones and
  owned removed objects (`store::Removed`), never a guard, and takes no
  closure. Walks issue and look up nodes through the request's
  `store::Nodes` handle (`Store::nodes_for(details)`,
  `RequestContext::nodes`), which implements the interning traits of
  `verbatim-uia` and `verbatim-ia2`, keeping an element a lookup replaced
  to drop as the handle drops, on the worker; a plain
  window and a foreground's client object read from local data are minted
  by window handle (`Nodes::window`, `Nodes::client`), as their MSAA keys.
  Every ordinary message is sent as a
  `store::Recorded`, which only the store makes, listing the nodes and
  anchors it names (`OutpostToCore::named`) and the element a caret tell
  names by its token; each message that carries them
  (`OutpostToCore::carries_nodes`) takes the next position as it is
  written, and what it names is recorded at it (`outbound::Carried`, in
  the numbering `verbatim_reader_rules::Numbered` the listener shares).
  The app writes `NodesHeld` (the reducer's nodes, text anchors and caret
  entries' tokens in that outpost and the last position handled) when the
  node set changed or after a focus report (`Supervisor::send_nodes_held`);
  any other change, a caret tell's token among them, waits in the
  writer's slot (`Supervisor::leave_nodes_held`) for the next command, a
  request or a ping within three seconds. When the reducer reaches it,
  between programs, its shell calls `Store::release`: under the sender's
  lock and then the store's, it frees a node, anchor or caret tell's
  element Core does not hold that either no message carried or was
  carried at or before that position with none waiting that carries it,
  by position and Core's view alone; with it go the node's text facts,
  text objects and terminal entry, and in the same hold of the store's
  lock a freed anchor's text facts and the UIA ranges no fact names any
  more. The reducer dispatches no program until the release's answer, so
  no program is given a vanishing node.
  The objects are dropped by a release
  program on a worker, never on the reducer's thread. A
  never-carried node of an ended request or an abandoned generation goes
  at the next release. A released node answers `Gone`, a forgotten anchor
  `AnchorLost`. Only the newest list is kept.
- Text on copies: each node's text facts are the store's
  (`reducer::TextFacts`, "Text and caret"), and a program reads them in
  its copy. A text call copies the part its operation names
  (`NodeAnchors::part`), clones the UIA ranges that part names from the
  program's own or from the store in one hold (`Store::ranges`), and works
  on the copy with no lock held (`text_reads::uia_call`, `edit_call`).
  What it changed is applied to the program's copy at once and carried,
  with the ranges it made under new keys and the anchors it minted or
  reported, to the program's next commit (`text_reads::put_uia`,
  `put_edit`, `ProgramText`); a program that ends with text writes no
  commit carried commits them alone. The shell makes them only for a
  commit the reducer accepts (`Store::write_text`): the facts' change is
  made, the ranges and anchors kept, and the ranges no fact names after
  the change let go of, but a change that adds to a node the store has
  released is not made. A refused commit's writes, with the terminal entry
  it made, are dropped by a worker.
  The other text writes, the patterns and what a text supports, a
  forgotten answer of no pattern, and a node's text forgotten for a new
  element, ride the same commits. A destroyed window's nodes have their
  text facts forgotten as the store frees them (`Store::forget_window`).

### What to read

Each program reads the node details its request names and nothing more
([verbatim-model](verbatim-model.md), "Themes"): a request of the shared
list by its property set (`asked::node_details`), a legacy query by the
details it carries, a tree dump every detail, an event's own reads, which
no request names, every detail (Core's theme decides what is spoken), and
a text read or a release none. The store's handle for the program carries
them, so the MSAA reads see them too (`Interning::details`). UIA requests
and remote fills are built from the gates of the `Node` and `Ancestor`
shapes
(`books/dev/src/explanation/design/uia-shapes.md`), one function per shape and use (`gates.rs`):
a node's place only for an element read alone (`node_gates`,
`focus_gates`, not `many_node_gates` or `ancestor_gates`, decision 9), and
an ancestor's description always (decision 4). MSAA reads skip what is
not named. The subscriptions' cache requests are fixed, so a listener's
selection is reported with every detail its cache holds. Text formatting
is read only for the details the request names (`TextDetails`, from the
text op) and attributes the control supports,
learned from its answers and kept per node (`TextSupport`); spelling and
grammar annotation types are never learned, since Notepad answers "not
supported" without annotations, so the caret read asks a line first
(`books/dev/src/reference/crates/verbatim-uia-rops.md`, "The caret read"). `IsHidden` is never
read.

### Text and caret

Core's `Request::Text` (`text_reads::answer_asked`, `text::held`) holds
reads behind ids Core names, released as anchors are (Table 16):

- `StartingRead`: the caret and the selection held as the starting point,
  with the key's unit where its `StartingUnit` says: at the caret (the
  line where the text has none), with the word at the caret for Delete,
  or, for Backspace and Control+Backspace, around the character before
  the caret, as NVDA's Backspace scripts read what they delete: the
  caret moved back a character and expanded to the unit, nothing at the
  text's start (`held::unit_before_caret`). Through UI Automation that
  unit is read in the caret read's own program (`CaretUnit::BeforeCaret`);
  an edit control moves back and reads it by its messages. It compares
  no selection.
- A focus's caret read (`held::read_focus`): the caret, its line with
  the formatting the focus request's `details` name, and the selection
  with its text, held as the next baseline, compared with nothing.
- `ReadSelection`, a caret tell's read: the caret and the selection,
  compared with the named baseline when the node holds it, as its range
  now is (through UI Automation a range follows the edits), in specific
  terms with each part's text, nothing when both are empty, and with the
  whole new selection beside it; an edit control's selection offsets; the
  next baseline held. A baseline no longer held is no baseline.
- `MoveCaret` and `Select` answer `CaretPlaced` and `SelectionPlaced` with
  the next baseline, the read the move kept, or a fresh read of the caret
  when it kept none.
- A focus answer's caret (`FocusCaret::Read`): the same read as
  `ReadSelection`, after the node's states, with the request's baseline;
  `NoText` for a node with no text, `NotRead` when the read failed and for
  a terminal, whose baseline read is its first.
- `MoveBySentence` (`text::sentence`), run whole: from the starting
  point, the caret's paragraph is read, the next or previous sentence
  found in it by Unicode's rules, the paragraph beside it read when the
  sentence is not in it, and the caret moved; past the last sentence the
  caret goes to the text's end, and at the start it stays with the first
  sentence; the next baseline held. A source with no sentence unit
  answers `UnsupportedUnit`, a starting point no longer held
  `AnchorLost`.
- `Watch`, a caret key's watch, waits in the reducer's table
  (`verbatim-outpost-logic`'s `watch`), whose checks and final read run as
  programs of their own (`text_reads::watch`, `text::check`): each check
  reads the caret, and the character or word at it, against the starting
  point, and the final read answers the key's unit at the effect or the
  bound (`docs/design/readers-tell-core-asks-build1.md`, "The watch" and
  Table 15).

- A UIA node has text when its element has a text pattern, fetched once and kept with the node. `NoText` is kept only
  until the node is next the focus or raises a caret or text event
  (`text_reads::forget_no_text`), since UIA answers a provider not yet ready
  as one with no pattern; a caret watch stays open on it, and a failed fetch
  answers `Unanswered`. An MSAA node has text when it is the client area of
  an edit control class (`Edit`, `RichEdit`, `RichEdit20`, `REComboBox20W`,
  `RICHEDIT50W`). A UIA terminal never reads a paragraph or a page.
- Anchors are minted at each chunk's start and a selection's ends; a
  `TextPosition` is resolved by moving forward over the chunk's UTF-16
  text, checking it, except positions the outpost reported, which it
  remembers. Anchors Core holds (in `text::HeldAnchors`, with its own lock,
  so the reader never waits on the worker) are kept; others go after 64
  newer ones, answered `AnchorLost`.
- Chunks are at most 64 KB of UTF-8, cut between grapheme clusters; UIA
  chunks carry the range's `Culture`. Reads move first when asked, by whole
  units, saying how far; a missing unit is `UnsupportedUnit`. `ReadRange`
  reads up to 1 MB; `Select` and `MoveCaret` select, `Select` answering the
  selection's change; `Location` gives a character's screen position;
  `ReadAhead` reads units up to 32 K UTF-16 code units and marks the last
  chunk.
- Through UIA every request but the caret watch is one remote operation
  where the provider runs them (`TextSource::read_units`, `range`,
  `point_location`, with `verbatim_uia_rops::text_units`, `text_range`,
  `text_location`), finding a `PointFrom` (caret, selection end, text end,
  kept position, or text after one, matched in the provider) and reading
  from it. Edit controls are read message by message.
- The caret read (`TextSource::caret_read`, `verbatim_uia_rops::caret_read`)
  reads, in one remote operation where possible, the caret and selection,
  whether they moved, the caret's line and the key's unit with offsets,
  and the formatting the request names. It
  travels as `FormatRun`s: bold at weight 700 or more, underline and
  strikethrough as `LineStyle`, size as "11.0 pt", colors by NVDA's named
  hues (`text/color.rs`, ported from NVDA's `colors.py`), `BulletStyle`,
  and links.

Core alone judges a caret key and a selection
(`docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
this build" and Table 17): the outpost keeps no note of a key and makes
no comparison Core did not ask for.

- Each node's text facts are the object store's: its anchors, the positions
  reported, its one last read (`LastRead`: the caret, its line and the
  selection with its text, and when the read began) and the reads held
  behind ids Core names. A position is an edit control's offset, or the
  key under which the store keeps a UIA range, so the facts hold no
  application object and an edit control's offsets cross between
  programs as they are. They change only by a program's commits
  (`TextChange`) and a release, and the store lets go of each UIA range
  no fact names any more as it makes the change. No rule of the reducer
  reads them, so the store holds them (Dickson's confirmed default,
  2026-11-20).
- A report's selection starts where Core can read before it: at the
  caret, in the caret's line, since a UIA caret is the selection's start,
  as NVDA's is (`CaretRead::selection_start`).
- `Text_TextSelectionChanged` of a followed node is told to Core as a
  caret tell, the event's element kept behind the token the tell names
  until Core's held view leaves the token out once it has handled the
  tell, for Core's caret read to adopt when the node answered no text pattern (`text_reads::adopt_told`);
  the outpost reads nothing for it. A console's update and caret events
  are told as a terminal's change tells. `Text_TextChanged` and the active
  text position change are told too; the system caret's events reach Core
  from the listener.

### Core's requests

A focus read Core asks with `HiddenRoot::Check` (an MSAA candidate's,
the current focus's, a completion's) first takes its window's top-level
window and whether it is visible, two local calls; only for a hidden one
does it read its client object's role, two counted calls
(`verbatim_ia2::acquire::client_role`), and a dialog's role, a dialog,
an alert or a property page, answers `Reply::HiddenDialog` with that
window's facts and nothing more (`asked::hidden_root`, Dickson,
2026-11-19). A hidden root of any other role is read as it is; a role
read that fails answers "not there". The outpost judges nothing of it:
Core postpones the dialog to its show, and asks `ReadAnyway` for a read
that must not check. Tests: `outpost::hidden_root_tests`, the check's cost
pinned at no call for a shown root and two for a hidden one.

Each `CoreToOutpost::Request` is one program on the worker
(`outpost/asked.rs`), answered once with one of the five outcomes, each
node read with its nearest window's facts (`NodeWindow`, read from
Windows with no call into the application) and its properties by the
request's set, from the snapshot the outpost's reads make. A window with
no verdict in force in Core is probed first, always, and the probe
carried in the answer, by every request: a completion, an MSAA run and a read of an MSAA object
consult the verdict themselves, and every other request probes its
window before it is handled.

- The shared list: `Properties` (the node's live read, made only when
  the set names a property a snapshot holds; the four no snapshot holds
  are read apart: `Suggestions` by one refresh reading the node's
  `ControllerFor` live, absent for a node that is neither editable text
  nor a search field; `HasValuePattern` and `ComboBoxList` by one
  refresh, the latter by `GetComboBoxInfo` on the node's window when the
  refresh finds NVDA's classic .NET Framework Windows Forms combo box;
  and `WindowClass` from the node's own or nearest window, with no
  call into the application), `Navigate`,
  `Children` (first child, then next siblings, at most 1000), `Find`
  (children, or descendants depth first, at most 1000 visited), `Text`
  (today's text answers) and `Activate`.
- `ReadObject` of an MSAA object at an event's address, refused "read
  through UI Automation" when its window is, or of a node it holds: its
  relation to the nodes Core names by NVDA's equality, its ancestors up to
  the first Core already knows, the walk stopping at a known node of this
  outpost, and with `descendants` its recursive descendants depth first,
  at most 1000. The relation to what the committed focus controls is
  read live from the focus's `ControllerFor` (`controlled_by`, one remote
  program where the window allows), the object itself (`Controlled`) or
  inside one (`InsideControlled`); an MSAA object, or a focus that is not
  a UI Automation node, controls nothing; a relation read that fails
  answers the whole read not there. A selection's relation falls
  back to its place among the nodes Core names.
- `MsaaRun`: one candidate, answered skipped when its window is read
  through UI Automation, not there when no object is at its address, else
  its facts and, to the depth asked, its full read with its ancestors up
  to the first Core already knows, or, at `FullUnlessSame`, the same as
  named.
- `CurrentFocus`: the focus-now read, its walk up through either API
  stopping at the first ancestor Core already knows. The window with the
  keyboard focus is taken first, before any call that can block, the
  request's probe included, as NVDA's fake focus takes it when its timer
  fires. It answers a `CandidateAnswer`; with `FullIfFocused`, the check
  after a menu end, an MSAA focus's focused state over it and every
  ancestor up to the desktop is read as a fact, and the focus read in full
  only when it has it.
- `CompleteUia` (`outpost/completion.rs`): refused "read through MSAA"
  when its window is; else the outpost's own copy of the element is found
  by the route, which goes on, when the listener's walk met an ancestor
  Core already knows, from that ancestor: a window's own element, or the
  route kept for its node (`FocusPath::above`); a text focus whose walk
  the listener cut, which no route finds, as the application's focused
  element when its runtime id is the focus's; an element kept under the
  focus's runtime id is judged stale when it is gone or lacks the keyboard
  focus, whether or not the focus's own was found; an element kept under its runtime id that is gone or
  lost the keyboard focus is evicted with its text, and the answer says
  the id was reused; a focus's event registration is moved to the
  element on the registering thread and waited for, keeping the node
  `Follow` named, then the node's states are read; the ancestors are the listener's reads, completed
  above as a focus read's are; a selection's item is answered as an
  object read, with its relation to the committed focus read live and its
  ancestors, its parent first. Where the listener's walk met an ancestor
  Core already knows, the route's end names it, and the completion makes
  no read to find it.
- `Follow` (`outpost/followed.rs`, Table 10): what it names, the node,
  its group and any extra object, or nothing, is recorded when the worker
  takes it, then the registration is moved on the registering thread to
  that node alone, the extra object given the value group (its element
  found again by its route when the outpost keeps none, as an ancestor
  the completion minted), and waited
  for; a node whose element the outpost no longer holds answers not
  there, with nothing registered. A console host's terminal takes the
  extended group without its text and caret events, and is the console
  terminal the console's events are routed to. A `Follow` still queued is
  replaced by a newer one, answered "ended or stopped"
  (`Asks::push`). The registering thread is the only one that registers:
  the reducer's own subscription moves are dropped.
- A UI Automation property change the registration delivers is told
  to Core (`OutpostToCore::Tell`, `Worker::property_tell`) with no
  judging: its kind, the element's runtime id (`EventObject::Followed`),
  whether it has the Text pattern, and the value it carried: the states,
  name or value from the event's cache; a help text change as a
  description change and a `ControllerFor` change as a suggestions
  change, each with no value, which Core asks for. A suggestions change
  also carries the element's automation id from the event's cache
  (`UiaKind::Property`), so Core plays the suggestions-closed sound for
  an element that is no longer the focus only when it is a suggestions
  field, with no call added.
- `WindowDestroyed`: the queued reads whose MSAA ids name the window, a
  read by ids and a run's candidate, are answered not there at once
  (`Asks::destroyed`), with the watches waiting on it; every other request
  stays queued, whatever window it was asked on. The reducer owes the
  window a forget, and the windows destroyed in one drain of the mailbox
  are forgotten by one program (`Work::Forget`, `worker::forget`) ahead of
  every later program: each window's classic-read mark,
  and its MSAA nodes with their text facts, so a reused handle inherits
  none. A followed node's registration is Core's to end, with a `Follow`
  naming nothing, which runs after the forget. `Outpost::holds` says
  whether the store holds a node, for the harness's test of the forget.

### Requests from outside the event queue

- `ShellItems` (`outpost/shell_items.rs`) reads the notification area
  (`TrayNotifyWnd`, with the overflow flyout) or the taskbar
  (`Shell_TrayWnd` without it) in the control view as `ShellButton`s, and
  answers every named, on-screen button as a `ShellItem` (name and runtime
  id). `ShellItemPosition { kind, item }` walks the same way and answers
  `ShellCandidates` with each matching button's `Place` and whether every
  button was read; Core picks (`verbatim-gui`'s `refind`). Both read only
  their application's windows, look the shell windows up without the proxy
  rule (`any_window_element`), have no time budget of their own, and read
  a failed step as no more items, logged as a warning. Core bounds each
  by its due time, as every request.
- `DumpTree { locations }` walks the foreground window (or the first
  visible top-level window): UIA with `verbatim_uia_rops::tree_dump`, MSAA
  with `verbatim_ia2::acquire::walk_tree`, capped at depth 64 and 4096
  nodes, saying whether incomplete. Locations are read only with
  `locations`, since a title bar's are asked of the UI thread
  (`WM_GETTITLEBARINFOEX`).
- `Properties` reads a node live for a review command, its table's row
  and column counts included when the set names them; a dead element
  answers not there. A UI Automation node that is dead or gone answers not
  there to `Navigate` too, except that `Parent` answers the parent of the
  element's window (NVDA's fall back, `known_window_of`): the nearest
  window recorded in the store when the node was built, and its parent
  read by the object `window_object` chooses as NVDA does, the desktop
  carried as `NodeDetails::desktop_window` for Core to name.
- `Navigate` and `Activate` go to the issuing registry: UIA's raw-view
  walker with the base cache, or MSAA's `accParent`, `accNavigate`, and
  `accDoDefaultAction`. An unknown or unreachable node answers not there.
  An activation tries the node, then up to 1,024 parents
  (`ACTIVATION_PARENT_HOPS`), stopping at a node it has visited before, by
  runtime id or by an MSAA object's address where it was acquired at it
  (`Store::address_of`), and answers "nothing activated" past either.

### Selection and notification events

The outpost raises none of these: selections, UI Automation notifications
and toasts are the listener's tells, which Core's event queue judges
(`docs/design/readers-tell-core-asks-build1.md`). What Core reads for a
controlled selection, an element of the focus's ControllerFor relation or
inside one, the relation and the search are one remote operation
(`verbatim_uia_rops::controlled_selection`) made on Core's request.

## Messages to Core## Messages to Core

The thread that sends a message writes it itself (`outpost::outbound`,
through the transport's `Sender`; `docs/design/communication.md`, section
4.3): at once with one `WriteFile` when no write is in flight, otherwise
into the sender's backlog, which the I/O thread writes as writes complete.
Nothing that sends ever waits, so the reducer can always abandon a worker.

- Pongs, `Ready`, `TargetExited` and `SpareReady` go ahead of everything
  waiting: `Outbound::urgent` takes only an `Urgent`, which carries no
  node, so nothing that carries nodes skips their numbering.
- Every other message keeps its order: nothing merges, and a flush
  (`Outpost::settle`'s) is answered once every earlier write has
  completed.
- What waits is bounded by Core's requests and the intake's limits.
  Messages carrying node ids are numbered as written (`Pending`).

The supervisor and the listener never wait to send either.

## The idle barrier

The harness's `AwaitIdle` (`verbatim-app`'s `idle` module;
`docs/design/communication.md`, section 6.7) is answered after a round in
which every child said nothing that came before the question was in
progress. The round is in band, so it covers pipes and backlogs.

- `Supervisor::query_idle` posts the round to the relay, which writes
  `IdleQuery` to the listener behind every earlier command; a missing or
  ending listener counts as having answered.
- The listener passes it through its `WinEvent` thread (`Nudger::nudge`)
  to its check threads and reading threads (`when_idle`), and answers once
  no event handed to a check thread before it is untold and no job queued
  before it is waiting or running, behind every earlier tell.
- The relay then asks every outpost in the same pass, telling the app which
  first (`IdleQueried`); outposts being launched are asked once every launch
  has reported, a starting outpost behind its held facts. The query gets
  through a full queue.
- An outpost queues it (`Item::Barrier` with `Asker::Round`), dividing
  what waits and ending its batch. The reducer reaches the query with no
  program in flight, so nothing before it is unhandled, and answers it
  (`Reducer::answer_idle`) as it is reached and after every input, once no
  caret key's watch is open or held by the program in flight. Entries
  after the query are not waited for, nor is a program handling one. An
  outpost that ends answers by its `Ended`.

## Shutting down

Every child ends by a clean shutdown, the kill being only the fallback
(`books/dev/src/explanation/architecture.md`, section 1, gives the reason): at Verbatim's exit
(`Supervisor::shutdown`, run while the exit sound plays, after the keyboard
hook is removed; no child starts afterwards), when a child is replaced, and
when its application exits.

The supervisor's side (`verbatim-supervisor`'s `retire`): it sends `Shutdown`, which
passes a full queue, closes the command pipe once it is written, and waits
for `SHUTDOWN_LIMIT`, 21 seconds. Exit code 0 is clean. A child still
running then is killed through its job (`Contained::kill`,
`verbatim_process::KILLED_EXIT_CODE`), logged as a warning with why it was
ended and counted `killed`, as is any other exit code; a failed wait ends in
a kill too, logged as an error and counted `killed_on_failure`.

The outpost's side (`Outpost::shutdown`), in order:

1. The mailbox closes, and the reducer drops what waits and dispatches
   nothing more.
2. The `WinEvent` hook and its thread go.
3. The subscription's groups are removed (`RemoveEventHandlerGroup`), each
   waiting for callbacks in progress.
4. Under the `faults` feature, the observation point
   `ShutdownWaitsForCalls` ([verbatim-faults](verbatim-faults.md)) is
   reached. The reducer's thread ends once the program in flight has ended, with no
   worker abandoned meanwhile, after handing what it let go of and the
   elements the subscription followed to the worker in charge to drop;
   then every worker, abandoned or not, and
   every bounded-client thread returns; no call is cut off, since UIA's
   timeouts end it.
5. Every node leaves the store, with its text patterns, anchors, and
   a terminal's kept start (`Store::release_everything`).
6. What waits is written, every write in flight completes
   (`ChildIo::finish`), and the pipe closes, since an exit would cancel a
   write (`docs/design/communication.md`, section 4.6).
7. The thread leaves COM's apartment. `run_pipe`, the process's exit
   path, then gives back the process's hold
   (`verbatim_uia::release_mta_usage`), ending the registering thread with
   its `RemoveAllEventHandlers` (`books/dev/src/reference/crates/verbatim-uia.md`,
   `Registration`); `Outpost::shutdown` does not, because the thread is
   the process's, shared by every outpost in it, as in a test process.

The outpost logs each step's time (`the outpost shut down`). The listener
removes its hooks and handlers, writes what waits, and leaves COM. Both
then end with `TerminateProcess` and exit code 0 (`books/dev/src/explanation/architecture.md`,
section 1). The limit outlasts any UIA call (`UIA_TRANSACTION_TIMEOUT`, ten
seconds, `UIA_CONNECTION_TIMEOUT`, two, for classic calls and `Execute` alike)
and a search of several; only an MSAA call into a hung application can
outlast it. A clean shutdown takes milliseconds: over one end-to-end run, 4
to 443 ms from the message to the exit, median 15.

Tests: `crates/mockapp/tests/shutdown.rs` (a shutdown during a held read,
remote and classic: the answer sent, the four UIA registrations removed,
the pipe closed, mockapp still answering; and a pong while the worker is
held; it waits on the shutdown's observation point, so only `cargo xtask
faults` builds it), `crates/verbatim-supervisor/tests/shutdown_fallback.rs` (a stand-in
that never exits is reported killed at its limit), and
`crates/verbatim-outpost/tests/target_exit.rs` (`TargetExited`, then a
clean exit). The end-to-end suite checks every child exited with code 0.

## Terminals

The `terminal` module (public, so mockapp drives it) reads a focused
terminal as Core asks and sends each read to Core, whose terminal model
aligns it and applies the reading rules (`books/dev/src/explanation/design/terminal-model.md`,
"Where it runs"; [verbatim-core](verbatim-core.md), "Core's terminal
model"); the outpost judges nothing a read says and keeps nothing of a
terminal between reads but the start anchors Core names, held as text
anchors until `NodesHeld` lets them go. A UIA element of class `TermControl` (Windows
Terminal) or `WPFTermControl` (Visual Studio), or a focus in a
`ConsoleWindowClass` window, is `Role::Terminal`, never by title. The
console host's text area is read through UIA; inside a remote operation its
provider gives no handle for the window, which the classic walk finds.

- A change is a tell. Every change signal of the terminal `Follow` names
  (the console host's `EVENT_CONSOLE_UPDATE_REGION`, `_SIMPLE`, `_SCROLL`,
  `EVENT_CONSOLE_LAYOUT` and `EVENT_CONSOLE_CARET`; Windows Terminal's UIA
  text changes and caret events) is sent to Core as one `Event` tell of
  kind `TerminalChanged` (`Worker::terminal_tell`), with the time it was
  observed. The outpost reads nothing for it. The intake's limiter merges
  the signals of the same window or element and kind that wait together
  (`a_windows_console_updates_merge_into_one_while_they_wait`). The outpost
  keeps no "changed, not yet read" flag: Core alone does.
- A read is one `TextOp::ReadTerminal(TerminalRead)` of Core's
  (`text_reads::terminal_read`, through `terminal::read`, `verbatim_uia_rops::terminal_snapshot`, classic fallback
  behind it): text from a screen above the start anchor Core names to the
  end in one `GetText`, the screen, the caret and its line, and colours when
  Core asks, in one remote operation. Every fact of the read is Core's
  (`terminal::ReadAsk`): the start anchor, the scope (`Fresh`, `Whole`, or
  `HistoryBlock`), the screen's height and width, and whether to take colours.
  The answer is `TextReply::TerminalRead` (`TerminalReadReply`): the
  `TerminalSnapshot` (`terminal::snapshot_of`: UTF-8 text without line
  padding, each line's padded length from one pass,
  `terminal::unpadded_utf16`, the caret's row and column, colours from
  `verbatim_uia_rops::ColourRun`, and when the read began), the start
  anchor the read minted at the top of its screen unless the screen shows
  no text where the last read's did (`verbatim_text::terminal_rows::shows_text`),
  when the read began on the shared clock, and the caret starting point
  when the request named a unit, held from the caret the read found
  (`text::held::starting_from`), so a caret key's starting read is one
  round trip. The caret read is a caret report inside the snapshot.
  Output reads are the worker's own after the event queue's requests; a
  read waits for the worker's current request, up to a search's length.
- Keys and reading settings are Core's, and so is whether a read takes
  colours. A caret key's starting and final reads in a terminal are
  `ReadTerminal` requests of Core's caret key entry.
- Windows Terminal's reads never wait for a frame. Windows Terminal raises
  no change for writes of blank lines only, so such a scroll is read at
  the next change.
- When a terminal's focus is reported, the subscription moves to it and the
  worker waits for the move (`Registration::settle`) before the first
  read. The console host's console events are hooked when `Follow` first
  names it ("The console host's events", above).
- History: `TextOp::TerminalHistory { above, whole, start, height }` reads rows
  above the start anchor Core names, padding kept, answered
  `TextReply::TerminalHistory` with the rows above the start and the read
  number whose start it is (`terminal::history`, `History`);
  `TextOp::TerminalFind` (`terminal::find`,
  `verbatim_uia_rops::terminal_find`) finds a command mark's line from the
  named start, answered `TextReply::TerminalFound`, and Core picks the
  match. A start anchor released while a request names it is held until
  the answer; one that no longer exists ends the request as the anchor lost.
- Each snapshot and read logs its cost at debug (`terminal snapshot
  timing`, `terminal read timing`).
- The listener ignores every UIA notification from a terminal
  (`verbatim_uia::map::ignores_notifications`), as NVDA's terminal overlays
  do, or output would be spoken twice.
- Tests: `terminal/tests.rs` checks the one-pass conversion over
  `verbatim-term-sim`; mockapp's `terminal`, `terminal_reading`
  and `terminal_snapshot` tests drive a real outpost through its request driver.
