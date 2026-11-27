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

The requests an outpost answers: Core's batches (`outpost::batch`),
among them the focus reads, a `FocusRead` of the element the listener
read by its route (`outpost::completion`), of one MSAA candidate, or of
the application's current focus, and the tests before them; a node's properties,
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
kind, and Core asks the outpost for what it reads.

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
`verbatim_win32::provider::served_by`, local calls), and the focus read (`read::checked_focused_element`) reads no focused element in another
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
(`read::chain`) stops below the boundary, a step that reaches
another process's window anyway is not read further, the MSAA walk does
not step from a window's own window object into a parent window it reads
as plain (`read::reads_as_plain`) and does not read a parent an
`accParent` named in one, and a focus route through one is not followed
(`Context::focus_route`). The accepted exception is object navigation's
raw-view step (a `Related` step by the ordinary calls when no program
runs), which reads such a neighbor through UIA before
`read::corrected_backend` switches it to MSAA.

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
(`store::element`, under "A node's element" below), while a walk that fails, a window lookup
timed out or refused among them, fails the query (`focus_path::find`),
since a slow application has not shown the element gone. The walk has no
time budget: it runs until it ends, unless Core cancels its request and
its worker is abandoned. A
listener's selection is reported with what its event's cache holds
(`verbatim_uia::ListenerSelection`), since a search of a subtree enters
any child window another process serves ([ADR 0005](../adr/0005-calls-that-reach-another-application.md), "A node whose element is gone"). The supervisor's `App` table holds each application's
process open while it has a record, so its pid is never reused while an
outpost or reading thread serves it. Registrations scope windows' subtrees
by their elements (`Scope::Subtrees`), with no connection by handle.

## The arbitrator

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
- Both class lists are `verbatim_model::class`'s, written from
  `docs/parity/backends.md` ("The class lists the per-window verdict
  reads").

## Running an outpost

The outpost's API is in its rustdoc (`cargo doc -p verbatim-outpost
--no-deps --open`); its messages are `verbatim_model::protocol`'s, and the
framing, the shared clock and `peek` are `verbatim_process::message`'s
([verbatim-process](verbatim-process.md)).

- `run_pipe` runs one on Core's inherited overlapped pipes, watching its
  target, or with none as a spare. `Outpost::assign` sets the application once and says `Ready`; it installs no hook (the console's is
  `Context::hook_console`, through `verbatim_ia2::event_thread::Nudger::hook`).
  ([ADR 0001: Focus](../adr/0001-focus.md), "Spare outposts"). It serves until
  `Shutdown` or the end of the command pipe, then shuts down cleanly, and a
  thread waiting on the target sends `TargetExited`.
- `run_attach` watches a pid, asks for its focus through the request driver
  (`driver::Driver`, a `FocusRead` of the application's focus naming its
  main window), and prints each outbound
  message as the model's JSON on stdout: the development mode.
- `OutpostOptions::remote_operations` (`verbatim_process::arguments`) says whether a UIA focus's ancestry
  may be read with a remote operation (on; `--classic-uia` turns it off).
- A test in its own process builds an `Outpost` that watches from
  construction, with a sender thread in place of the I/O thread, and may
  replace its readers of the focused element, the foreground window, the
  focus window and the observation clock.
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
- The worker's and subscription callbacks' failed cached reads
  (`verbatim_uia::calls`) are logged at shutdown, and a test may observe
  each event heard once queued, and the calls each of Core's requests
  made before its answer is sent.

## Inside an outpost

(`docs/design/communication.md`, section 6.4;
`books/dev/src/explanation/design/ownership.md`.)

- The mailbox (`outpost::shell::Mailbox`): the console host's hook
  callbacks (`CONSOLE_SUBSCRIPTIONS`), the subscription's callbacks,
  Core's commands, and the workers' commits and ends only append to it,
  under its one lock, and return. A UIA callback keeps the cached parts and
  window handle, and parks its element in the store (`store::Parked`),
  the entry carrying the key. Every event a callback takes in
  (`Context::take_in`) carries its window's state, read as it arrives
  with calls to Windows only (`Context::window_state`): the event's own
  window, or for an element that is no window, its own nearest window
  (`outpost::sender_window`, under "Core's requests" below); hung or
  ghosted with its report
  (`verbatim_model::hung::WindowState::Hung`), or responsive with its
  thread, thread 0 when there is no window to read. The reducer sends
  the report (ADR 0003, "A frozen window").
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
  hung window: Core judges a hang, and every event heard is handled and
  told, a hung window's included. An event's check before its first call
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
- The focus-following subscription (`outpost::followed`): Core's `Follow` names the committed focus's node, its
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
  event queue judges every other event. Nothing is dropped at planning for a hung window: the worker
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
  ([verbatim-outpost-logic](verbatim-outpost-logic.md), "Core's requests").
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

## Text and dialog text

- `text` holds the outpost's text sources, public so mockapp drives
  them, and re-exports `verbatim_reader_rules::text`, the `TextSource`
  trait and the algorithms over it (the verbatim-reader-rules crate
  guide). The source is
  `uia::UiaText` over a text pattern (positions are `UiaPos`; `new` takes
  the element for remote operations; `remote`, `details`, `support`, and
  `known_support` say whether caret reads try a remote operation, which
  formatting they read, and the `TextSupport` known before and after). An
  edit control's text is the MSAA executor's (`verbatim_ia2::text`, the
  verbatim-ia2 crate guide), which the outpost's `text_reads::EditFacts`
  gives the node's ids and a copy of its text facts, committed after
  each read.
  `fused` runs a run of a batch's text steps on a UI Automation node as
  one segment of the program builder (`fused::run`) and a wait's poll
  (`fused::poll`); `InOneGo` gives a focus's caret read in one go
  through `fused::caret_line` for UI Automation and none for an edit
  control, which is read by parts; `fused::named` lists the anchors of the
  positions a run of steps names, which a copy must hold, from the
  batch's one list (`Batch::anchors`).
- A dialog's own text is gathered by `verbatim_reader_rules::dialog`'s
  rule over `verbatim_ia2::dialog::DialogObject` and
  `verbatim_uia_rops::dialog::UiaDialog`; the outpost reports it as the
  dialog's description.

## Implementation notes

### The current focus

- A `FocusRead` of the application's focus (its executor's arm) is the
  focused control (`hwndFocus`, through its window's backend) with
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
  reading, is logged once, by `remote_fell_back` (`verbatim-uia-rops`'s
  one fallback log) and `read_classically`,
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
  there; the focus read (`read::checked_focus_window`) treats the frame as nothing focused.

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
- An MSAA focus is one candidate per batch, its `Msaa` subject acquired
  once with NVDA's child-0-on-a-list redirect and the `accFocus` redirect
  (`focus_candidate`); a menu start with neither, at its event's address
  (`menu_start_candidate`), as NVDA builds it. The outpost reports facts
  and judges none: a `Test` reads the focused state over the object and
  its ancestors (`FocusedWithin`, read live) and, for the identity check,
  the ids it was reached at and what it reads as, each read alone
  (`FocusCandidate::property`) in the matcher's order and stopping at the
  first that decides; a menu start's role is a `Read`. Core's conditions
  decide whether its `FocusRead` runs; a redirected focus names the
  child's ids as its event ids, as NVDA builds the redirected object with
  the child's id as its event's.
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
- A UIA navigation step (`read::related_step`) is one `Related` step of
  the program builder from the node's element, one program, but by calls
  from a top-level window (a program's walk ends there) or from an
  element only UI Automation's proxies inside the outpost serve, with
  remote operations off, or in a window marked classic. A kept element
  that answers gone is let go of, and the element its route finds is
  stepped from by calls, or the node is answered gone.
- A dialog's own text (`read::describe_dialogs`) is given as the
  description of every dialog in the chain below the ancestors Core
  already knows, and of a dialog focus, while descriptions are read. A UIA
  dialog the listener read, whose element the outpost does not hold, is
  found by its route, or by the route of the answer being built, so it
  has its text on its first read.

### A node's element

Every reader of a UI Automation node's element finds it one way
(`outpost/store/element.rs`): the element the store keeps, else the one
the node's route finds, which is kept for the node from then on. A focus
is minted with no element when neither its route's walk nor the
application's focused element finds it, and every ancestor a completion
mints from the listener's read has none, so a reader that took the kept
element alone would answer such a node not there while its route finds
it.

- `element::as_kept` takes the kept element with no call, or walks the
  route: the store's for the node, else, for a dialog's text, the route
  of the read in progress. Text reads, programmed steps (the controller
  relation, the first selected item, a walk down), a step's element, a
  dialog's element, a navigation step and `Follow`, for its node and its
  extra object (`element::reference`), take it.
- `element::refreshed` refreshes the kept element with the read's
  request, which proves it still answers; one that answers gone is let
  go of and the route walked. A kept element that fails for another
  reason stays kept and the read fails.
- A kept element is let go of only while it is still the one kept
  (`element::let_go`), so a worker abandoned on a hung application,
  whose refresh fails late, never evicts the element a later worker kept.
- The store's own lookup of the kept element and its eviction are
  private to the store and this module. `element::kept` is the one
  look at the kept element with no call and no search, for judgments of
  the kept element itself: its own window, whether a completion keeps
  one for an ancestor, whether one under a reused runtime id is stale.
- A completion keeps the element it found, and the route of every node
  it minted, before it answers Core, so a node Core names next is found
  by one or the other. A focus it did not find has its text not read in
  that answer (`FocusCaret::NotRead`), with no second walk of the route
  its walk just missed.

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
  at the next release. A released node answers `Gone`, a step naming a
  forgotten anchor not there, by that anchor. Only the newest list is
  kept.
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
([verbatim-model](verbatim-model.md), "Themes"): a batch by its steps'
property sets (`batch::node_details`), an event's own reads, which
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
is read only for the details the step names (`TextDetails`) and
attributes the control supports,
learned from its answers and kept per node (`TextSupport`); spelling and
grammar annotation types are never learned, since Notepad answers "not
supported" without annotations, so the caret read asks a line first
(`books/dev/src/reference/crates/verbatim-uia-rops.md`, "The caret read"). `IsHidden` is never
read.

### Text and caret

A batch's text steps (`outpost/batch.rs`, `text_reads`, `text`) read a
node's text and hold reads behind the anchors Core names, released as
anchors are:

- A run of consecutive text steps (`Caret`, `ReadText`, `ReadRange`,
  `CompareSelection`, `Select`, `Locate`, `MoveBySentence`) on one
  subject that resolves to a UI Automation node is one segment of the
  program builder (`text_reads::fused_steps`,
  `text::fused::run`), the batch's own steps with the node's text as
  their resolver (`TextResolver`): a position Core names, a point of an
  earlier segment's result and a baseline are resolved to the positions
  held, a unit to the provider's with the formatting its details name,
  and each result is made the
  model's, a `Caret`'s points held as anchors and its read held as the
  next selection baseline (`NodeText::hold_caret`), with the selection's
  text a `CompareSelection` of the run read, a `ReadText`'s chunks minted
  with the formatting its details name, the control's support of each
  attribute learned as the caret read learns it. A node whose text cannot
  be opened answers each step of the run, under its condition, as the
  opening answered (no text, not there, failed). An edit control's text
  steps run one at a time by its messages (`text::by_calls`, `text::ops`).
  A step the text refuses answers its refusal as the step's outcome
  (no text, no such unit, unsupported), and an anchor no longer held
  answers not there, naming the anchor lost.
- A `Caret` compares nothing; a `CompareSelection` compares the selection
  of the `Caret` it names with a baseline anchor's range as it now is
  (through UI Automation a range follows the edits), or with an earlier
  `Caret`'s selection, in specific terms with each part's text, nothing
  when both are empty, and with the whole new selection beside it, plus
  an edit control's selection offsets. A baseline no longer held answers
  not there.
- A focus's caret read (`held::read_focus`, inside `FocusRead`): the
  caret, its line with the formatting the read's `details` name, and the
  selection with its text, held as the next baseline, compared with
  nothing; `NoText` for a node with no text, `NotRead` when the read
  failed and for a terminal, whose baseline read is its first.
- `MoveBySentence` on an edit control (`text::sentence`), run whole: from the starting
  point, the caret's paragraph is read, the next or previous sentence
  found in it by Unicode's rules, the paragraph beside it read when the
  sentence is not in it, and the caret moved; past the last sentence the
  caret goes to the text's end, and at the start it stays with the first
  sentence; the next baseline held. A starting point no longer held is
  not there, by its anchor. UI Automation's text has no sentence unit,
  and its executor refuses the move as no such unit.
- `Visible`, `Move`, `FindText` and `ReadColours`, and a whole or
  trimmed `ReadText` and a trimmed `ReadRange`, the steps Core's terminal
  reader reads and searches a terminal by
  (`verbatim_core::terminal::reader`), run in the UI Automation executor
  as every text step does (`text::fused`): `Visible`'s ends and a move's
  start and landing held as anchors, a trimmed text made by
  `text::trim::unpadded_utf16` with each line's padded length, a search's
  matches and a colour read's runs answered as read. The MSAA executor
  refuses them as unsupported.
- A batch whose first step is `WaitForCaret`, a caret key's wait, waits
  in the reducer's table (`verbatim-outpost-logic`'s `watch`), whose
  polls run as programs of their own (`batch::poll`): through UI
  Automation each poll is one program, the check of the caret, its line
  and the word at it against the starting point followed by the batch's
  later reads, gated on the caret or the selection having moved or the
  bound (`text::fused::poll`); an edit control's check is made by parts
  (`text::check`), and the later steps run after it
  ([verbatim-outpost-logic](verbatim-outpost-logic.md), "A caret key's
  wait"). The starting texts the effect test compares come with the wait,
  from Core.

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
  newer ones, and a step naming one is not there, by that anchor.
- Chunks are at most 64 KB of UTF-8, cut between grapheme clusters; UIA
  chunks carry the range's `Culture`. A `ReadText` moves first when
  asked, by whole units, saying how far, and reads up to 32 units, up to
  32 K UTF-16 code units, marking the last chunk; a missing unit is
  refused as no such unit (`StepRefusal::NoSuchUnit`). A `ReadRange`
  reads up to 1 MB; a `Select` selects, or moves the caret with two
  equal points, answering when it began; a `Locate` gives a character's
  screen position, or an offset and length where the text counts by
  offsets.
- A point is found as a `PointFrom` (caret, selection end, text end, kept
  position, or text after one, matched in the provider) and read from.
- The caret read for a focus (`text::held`, `text::InOneGo`)
  and report focus's text summary (the `Read` arm's) read through UI Automation
  one segment of the program builder (`text::fused::caret_line`): a
  `Caret`, a `ReadText` of the line at the caret with the formatting the
  read names and no language, and a `ReadRange` of the selection, run
  only when text is selected (a `Check::Found` on the `Caret`), so an
  empty range, whose text some providers fail to give, is never read
  (Claude, 2026-11-22), from
  which the selection's start in the line and whether it starts at the
  text's start are found; an edit control's, or one with no UI Automation
  client, is read by parts (Claude, 2026-11-22). Formatting travels as `FormatRun`s: bold at weight 700 or more, underline and
  strikethrough as `LineStyle`, size as "11.0 pt", colors by NVDA's named
  hues (`text/color.rs`, ported from NVDA's `colors.py`), `BulletStyle`,
  and links.

Core alone judges a caret key and a selection
([verbatim-core](verbatim-core.md), "The caret key on the event queue" and
"Caret tells and Core's text facts"): the outpost keeps no note of a key and makes
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

The hidden-dialog check Core puts before a focus read (an MSAA
candidate's, the current focus's, a route's) is a `Test` of the
top-level window's own object through MSAA, the MSAA executor's window
arm (`verbatim_ia2::batch`, `Object::Window`):
whether it is shown, a local call; only for a hidden one its client
object's role, two counted calls (`verbatim_ia2::acquire::client_role`),
answered absent when the read fails (Dickson, 2026-11-19). The window of
`WindowOf::Focused` is the top-level window of the window with the
keyboard focus, taken once per batch (Claude, 2026-11-22). The outpost
judges nothing of it: Core postpones a hidden dialog to its show. Tests:
verbatim-ia2's `batch::hidden_root_tests`, the check's cost pinned at no call for a
shown root and two for a hidden one.

Each `CoreToOutpost::Request` is one program on the worker
(`outpost/asked.rs`), answered once with one of the five outcomes: a
batch (`outpost/batch.rs`). A window with no verdict in force in Core is probed first,
once per batch, and the probe carried in the answer. Each node read
carries its nearest window's facts (`NodeWindow`, read from Windows with
no call into the application) and its properties by the step's set.

Each operation is implemented once per backend, inside its executor:
every step on one backend's subject is an executor's arm, the UI
Automation executor's (`verbatim_uia_rops::batch`) or the MSAA
executor's (`verbatim_ia2::batch`), reached through the outpost's
resolvers, which resolve subjects and convert answers. Chaining across
backends is the dispatcher's alone, and is itself one implementation:
`read::chain` with its window walk-on for ancestors, `read::navigate`
for a step whose neighbour is read through its own window's backend,
`asked::children` for the children of a node MSAA does not list, and
`read::activate_walk` for an activation's walk up the parents. A second
implementation of an operation is the one a later session copies
(Dickson, 2026-11-22). Each executor's `FocusRead` arm reads the
focus's caret last; through UI Automation the outpost's resolver opens
the node's text and makes the report and baseline from the caret
program's answer (`text_reads::focus_caret`).

`ReadColours` is the one operation for terminals alone, and its arms are
the UI Automation executor's (Dickson, 2026-11-22): the terminals answer a
stretch whose last cell differs as one colour, and their format unit
stops only at rows, so no read of formatting gives a row's colours.

A terminal's full read, its history read and its command search are
batches of the generic operations (`Visible`, `Move`, `FindText`,
`ReadColours`, a trimmed read and a condition comparing two points),
written by Core's terminal reader
(`verbatim_core::terminal::reader`) and run by the executors like any
batch.

What still runs outside the executors, counted at the one-path change's
end (2026-11-22), is the dispatcher's chaining across backends (four
functions, above) and, with no UI Automation client, a caret wait's check
by parts (`text_reads::watch`'s fallback, `text::check` over
`TextSource`). `UiaText` implements `TextSource` for that fallback, for
`chunk_from`, which mints a read chunk's anchor with the text it holds,
and for `InOneGo`'s bound: these are its last uses, kept because the
check by parts and the chunk's anchor are `verbatim_reader_rules`' one
implementation over any text, which the MSAA executor shares.

A batch is parsed once as it arrives and refused whole when it does not
parse. A batch whose MSAA subject's window has a UI Automation verdict
is refused whole `ReadThroughUia`, and one whose `Route` subject's window
has an MSAA verdict `ReadThroughMsaa`, before any step runs. Each
subject is resolved once, at its first use: a node held or an earlier
result's, an MSAA object acquired once with or without NVDA's redirect,
a window by handle, top-level window, class path
(`outpost/class_path.rs`), focus or foreground, of the outpost's
application and shown when `visible` asks (the shell's windows looked up
without the proxy rule, `any_window_element`), and the focus window,
taken before any call that can block. Each step runs under its gate
(`Step::gate`):

- `Read`, `Test`, `IsOneOf` and `Activate` on a UI Automation node, a
  `Read` or `Test` of a window read through UI Automation, a `Read` of a
  listener's route, and every `Follow`, run through the UI Automation
  executor's call arms
  (`verbatim_uia_rops::batch`, its crate guide), resolved by
  `outpost/uia_object.rs`: the element filled or found again by its
  route, the properties the outpost holds (its MSAA ids, whether its
  window is shown, its window's class, a tree view item's children), a
  property of the element read in full, the text a text summary reads
  and the summary made of what it read (`text::summary_of`), and the
  focus registration.
- `Read`, `Test`, `Related`, `Descendants`, `IsOneOf` and `Activate` on
  an MSAA node or an acquired MSAA object, and a `Read` or `Test` of a
  window read through MSAA, run through the MSAA executor
  (`verbatim_ia2::batch`, the verbatim-ia2 crate guide, "The MSAA
  executor"), resolved by `outpost/msaa_object.rs`: the properties the
  outpost holds, a snapshot's values, the window facts the focused-state
  test asks, the backend a reached object is read through, and the
  children of a node MSAA does not list, by the dispatcher's navigation
  on that node's backend (`asked::children`).
- `FocusRead`, and a `Test` of the focus, run through the executors'
  `FocusRead` arms (`verbatim_uia_rops::batch::focus`,
  `verbatim_ia2::batch::focus_read` and `test_focus`): the focus
  window's backend picks the arm (`outpost/focus.rs`), resolved by
  `outpost/uia_focus.rs` and by `outpost/msaa_object.rs`'s
  `MsaaFocusRead`. `focus_paths_agree` (mockapp) compares their answers
  with remote operations on and off on every fixture `focus_reports`
  uses.
- `Read` of a node reads its snapshot, an acquired MSAA object's each
  property alone (`FocusCandidate::property`). Through either backend,
  those no snapshot holds are read apart: `Suggestions`
  by one refresh reading the node's `ControllerFor` live, absent for a
  node that is neither editable text nor a search field;
  `HasValuePattern` and `ComboBoxList` by one refresh, the latter by
  `GetComboBoxInfo` on the node's window when the refresh finds NVDA's
  classic .NET Framework Windows Forms combo box, both UI Automation's
  alone; `WindowClass` from the node's own or nearest window, with no
  call into the application; and `TextSummary`, for report focus, by a
  caret read of the node's text, the selected text when there is any,
  else the caret's line, holding no baseline (the UI Automation `Read`
  arm's, an edit control's by `text::held::summary`). A
  dead element answers not there. A lone `Read` of a listener's kept
  element is the listener's.
- `Test` lazily in its matcher's order, each property read once and kept
  in its result in the order read. A `Test` of the application's focus,
  the check after a menu end, is its executor's arm: it reads an MSAA
  focus's focused state over it and every ancestor up to the desktop,
  and the focus in full only when it has it; the dispatcher keeps what
  it read for the `FocusRead` after it.
- `Related`: parent, siblings and first child by object navigation
  (`read::navigate`; through UI Automation a `Related` step of the
  program builder, `read::related_step`); a UI Automation node's last
  child in one such step, taken as a step to the first child is, an
  MSAA node's the last of its children, failed when the list ends at a
  gone child; and the controller-for relation and the first selected
  item through the UI Automation executor alone
  (`uia_object::programmed`), since an MSAA object controls nothing. A UI Automation node that is dead or gone answers not there,
  except that `Parent` answers the parent of the element's window
  (NVDA's fall back, `known_window_of`): the nearest window recorded in
  the store when the node was built, and its parent read by the object
  `window_object` chooses as NVDA does, the desktop carried as
  `NodeDetails::desktop_window` for Core to name.
- `Ancestors` by each backend's executor's `Ancestors` arm, the UI
  Automation walk chained above where it ends by the window walk-on
  (`read::chain`, the dispatcher's chaining across backends, which a
  focus's ancestors take too), cut at the first ancestor Core already knows, its
  end answered as the walk's.
- `Descendants` of a UI Automation node by the program builder's walk
  (`verbatim_uia_rops::batch::run_batch`), its matchers judged between
  runs; of an MSAA object by the MSAA executor's children walk
  (`verbatim_ia2::batch::descend`), at most 1000 children of a node: an
  MSAA object's children as its object lists them
  (`verbatim_ia2::acquire::children`), each through the backend its
  window uses, and any other node's by object navigation. A child gone
  during the walk keeps its place, read as a disconnected object reads,
  the children after it are still read, and the walk is incomplete, as
  it is wherever a node's children are not read or a navigation's list
  ends at a gone child. The tree dump is a `Descendants` of the main window's own
  object read through UI Automation, the shell's lists one of the
  taskbar's windows by class path in the control view.
- `IsOneOf` by NVDA's equality for the equal node and by address for the
  others.
- `Activate` goes to the issuing registry: UIA's patterns through the
  executor's arm, or MSAA's `accDoDefaultAction`. An activation tries
  the node, then up to 1,024
  parents (`ACTIVATION_PARENT_HOPS`), stopping at a node it has visited
  before, by runtime id or by an MSAA object's address where it was
  acquired at it (`Store::address_of`), and answers "nothing activated"
  past either.
- `FocusRead` of an MSAA candidate (the MSAA arm): its object
  not at its address is not there; it reads it in full with its
  ancestors up to the first Core already knows.
- `FocusRead` of the application's focus (`Subject::Focus`, the focus
  window's executor's arm): the focus-now read, its walk up through either
  API stopping at the first ancestor Core already knows. The window with
  the keyboard focus is taken first, before any call that can block,
  the request's probe included, as NVDA's fake focus takes it when its
  timer fires.
- `FocusRead` of a route (`Subject::Route`, the UI Automation arm's
  `route`, resolved by `outpost/completion.rs`):
  the outpost's own copy of the element is found by the route, which
  goes on, when the listener's walk met an ancestor Core already knows,
  from that ancestor: a window's own element, or the route kept for its
  node (`FocusPath::above`); a text focus whose walk the listener cut,
  which no route finds, as the application's focused element when its
  runtime id is the focus's. An element kept under the focus's runtime id
  that is gone or lost the keyboard focus is evicted with its text, and
  the answer says the id was reused; a focus's event registration is
  moved to the element on the registering thread and waited for, keeping
  the node `Follow` named, then the node's states are read; the
  ancestors are the listener's reads, which the route carries
  (`Route::read`), completed above as a focus read's are; a `Read` of a
  route, a selection's item, is the UI Automation executor's arm, which
  answers the listener's read of the element its resolver finds and
  mints alone (`completion::resolve_route`). Where the listener's walk
  met an ancestor Core already knows, the route's end names it, and the
  completion makes no read to find it.
- `Follow` (`outpost/followed.rs`): what it names, the node,
  its group and any extra object, or nothing, is recorded when the
  worker takes it, then the registration is moved on the registering
  thread to that node alone, the extra object given the value group, each
  element kept or found by its route ("A node's element" above), and
  waited for; a node whose element neither is answers not there, with
  nothing registered. A console host's terminal takes the extended group
  without its text and caret events, and is the console terminal the
  console's events are routed to. A batch of `Follow` steps alone still
  queued is replaced by a newer one, answered "ended or stopped"
  (`Asks::push`). The registering thread is the only one that registers:
  the reducer's own subscription moves are dropped.
- The text operations, under "Text and caret" above.

Tests: `mockapp`'s `batch_readers`, `focus_reports`, `controller_for`
and `executors_agree`; verbatim-ia2's `batch::hidden_root_tests`.

Besides answering Core:

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
- A property change and a caret tell carry the acceptance facts of their
  sender's own window, as NVDA judges a property change by its sender's
  nearest window (`UIAHandler/__init__.py`, the property-changed
  handler): its handle when it is a window, else the nearest window
  recorded for its node as the node was built, else, for the extra
  object `Follow` names, the followed node's (`outpost::sender_window`).
  A completion records the focus's window before it adds the focus's
  registration, so a change on a new focus, which the registration
  delivers before Core's `Follow` names it, is judged by its own window,
  not by the old focus's.
- `WindowDestroyed`: the queued batches whose MSAA subjects name the
  window are answered not there at once (`Asks::destroyed`), with the
  waits on it; every other request stays queued, whatever window it was
  asked on. The reducer owes the window a forget, and the windows
  destroyed in one drain of the mailbox are forgotten by one program
  (`Work::Forget`, `worker::forget`) ahead of every later program: each
  window's classic-read mark, and its MSAA nodes with their text facts,
  so a reused handle inherits none. A followed node's registration is
  Core's to end, with a `Follow` naming nothing, which runs after the
  forget. `Outpost::holds` says whether the store holds a node, for the
  harness's test of the forget.

### Requests from outside the event queue

Core asks the tree dump and the taskbar's two lists from outside the
event queue, each a batch (`verbatim_core::outside` builds them and
reads their answers):

- The tree dump: a `Descendants` of the attention application's main
  window read through UI Automation, raw view, capped at depth 64 and
  4096 nodes, saying whether incomplete; an application read only
  through MSAA dumps as UI Automation's view of it. Locations are read
  only when asked, since a title bar's are asked of the UI thread
  (`WM_GETTITLEBARINFOEX`).
- The notification area (`TrayNotifyWnd`, with the overflow flyout) or
  the taskbar (`Shell_TrayWnd` without the notification area, pruned by
  its class name): a `Descendants` of each window by class path in the
  control view, matching buttons, from which Core lists every named,
  on-screen button as a `ShellItem` (name and runtime id), or, for the
  position of a chosen item, the matching buttons as `ShellCandidates`
  with whether every button was read; Core picks (`verbatim-gui`'s
  `refind`). A list is complete or nothing: a walk the node cap or a
  failed read cut, or a taskbar window not there, fails the list; an
  overflow flyout not shown is not there and adds nothing. Core bounds
  each by its due time, as every request.

### Selection and notification events

The outpost raises none of these: selections, UI Automation notifications
and toasts are the listener's tells, which Core's event queue judges
([verbatim-core](verbatim-core.md), "Focus, events and objects", its
announcements). What Core reads for a
controlled selection, an element of the focus's ControllerFor relation or
inside one, is a batch's `Related` controller-for and a `Descendants` of
every node it found, one program where the window allows.

## Messages to Core

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
remote and classic: the read finished and its answer dropped, the four UIA registrations removed,
the pipe closed, mockapp still answering; and a pong while the worker is
held; it waits on the shutdown's observation point, so only `cargo xtask
faults` builds it), `crates/verbatim-supervisor/tests/shutdown_fallback.rs` (a stand-in
that never exits is reported killed at its limit), and
`crates/verbatim-outpost/tests/target_exit.rs` (`TargetExited`, then a
clean exit). The end-to-end suite checks every child exited with code 0.

## Terminals

The outpost reads a focused terminal as Core asks, by the batches Core's
terminal reader writes, and Core's terminal model aligns each read and
applies the reading rules (`books/dev/src/explanation/design/terminal-model.md`,
"Where it runs"; [verbatim-core](verbatim-core.md), "Core's terminal
model"). The outpost judges nothing a read says and keeps nothing of a
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
- Core's full read of a terminal is a batch of generic text steps, one
  program by remote operations, the colours Core asks included once a
  read has found the screen's size (the verbatim-uia-rops crate guide,
  "The colours"), or the same steps by calls
  (`verbatim_core::terminal::reader`); the outpost keeps nothing of the
  terminal but the anchors it holds for Core. A caret key's starting
  read holds the key's starting point with its caret step, so it is one
  round trip. Output reads are the worker's own after the event queue's
  requests; a read waits for the worker's current request, up to a
  search's length.
- Keys and reading settings are Core's, and so is whether a read takes
  colours. A caret key's starting and final reads in a terminal are
  window batches of Core's caret key entry, the starting read's caret
  step holding the key's starting point.
- Windows Terminal's reads never wait for a frame. Windows Terminal raises
  no change for writes of blank lines only, so such a scroll is read at
  the next change.
- When a terminal's focus is reported, the subscription moves to it and the
  worker waits for the move (`Registration::settle`) before the first
  read. The console host's console events are hooked when `Follow` first
  names it ("The console host's events", above).
- History and the command search: Core's history read and its search for
  a command mark's line are batches too (the reader's `history_batch` and
  `search_batch`), counted from the start anchor Core names, and Core
  picks the match. A start anchor released while a request names it is
  held until the answer; one that no longer exists ends the request as
  the anchor lost.
- The listener ignores every UIA notification from a terminal
  (`verbatim_uia::map::ignores_notifications`), as NVDA's terminal overlays
  do, or output would be spoken twice.
- Tests: `text/trim/tests.rs` checks the one-pass conversion
  (`text::trim::unpadded_utf16`) over `verbatim-term-sim`, and
  `conversion_measure.rs` holds its measurement (`measure_conversion`,
  ignored); mockapp's `terminal`, `terminal_reading` and
  `terminal_reader` tests drive a real outpost through its request
  driver.
