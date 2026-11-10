# verbatim-outpost

The per-application outpost process, `verbatim-outpost.exe`: one outpost
per application, many at once, each watching its application for its whole
life, announcing the focus the focus listener
([verbatim-listener](verbatim-listener.md)) detects and answering Core's
queries. Core's supervisor ([verbatim-supervisor](verbatim-supervisor.md))
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
event group, and the global MSAA hooks. Its callbacks never make a
cross-process call: they read what each event carries and the hang-safe
`GetWindowThreadProcessId`, and forward each fact to Core. A UIA focus is
the exception: the listener judges and reads it on a thread for the
element's application (below). The supervisor routes every fact to its
application's outpost, which handles it as it handles its own hook's
events.

The listener installs its global MSAA hooks before its desktop-wide UIA
subscriptions, which a thread of their own installs (`Listener::start`),
because UIA reads the system's focus for the focus registration and waits
on the foreground application for it, up to about 6 seconds for one that
does not answer (`docs/design/cross-process-calls-study.md`, instance 2):
MSAA facts flow meanwhile, and the pipe thread answers the supervisor's
pings. That thread says `Ready` once the installation returns, whether or
not it succeeded, because the listener's `Ready` gates Verbatim's
readiness to take input, which the end-to-end harness waits for before a
scenario's first key, and no input may be taken before UIA focus events
can arrive. An event the
listener receives before those subscriptions are all installed is dropped
(`Outgoing::uia_event_process` in `crates/verbatim-listener/src/listener.rs`),
as NVDA drops events before its global handlers are registered; Verbatim's
start asks the foreground application's outpost for its focus instead
(`ShellCommand::FocusNow`), and a `WinEvent` focus whose UIA focus is so
dropped is lost, as NVDA loses it. The installation's end is logged
at info, with the time it took; the shutdown waits for it.

### The hand-over of an application's MSAA events

An outpost's own process-scoped hook takes in all its application's MSAA
events, focus, foreground, menu-popup, alert, and tooltip included, so they
reach it by one route in the order Windows raised them
([ADR 0001: Focus](../adr/0001-focus.md), "One route per application, and
the marker"). The listener forwards
those events only until the outpost's hook is in, so that an application's
first events start its outpost:

- An outpost tells its reducer the hook goes in, installs its hook, and
  says `Ready` once it is in.
- On `Ready` the supervisor sends the listener `HookInstalled` (application
  and generation). The listener reads the clock, forwards none of that
  application's facts raised after that tick, still forwards those raised
  at or before it, and queues `ForwardingEnded`, naming the tick, behind
  every fact queued; the supervisor relays it to that generation, or drops
  it if the generation has gone.
- Every `WinEvent` fact carries its raising thread and time
  (`WinEventStamp`). The reducer holds the hook's whole stream from the
  hook's install until `ForwardingEnded`, and handles the listener's
  copies as they come (`reducer::hold`;
  `books/dev/src/explanation/design/ownership.md`). At
  `ForwardingEnded` it drops the held copies of the kinds the listener
  forwards raised at or before the named tick, which the listener
  forwarded, and releases the rest in the hook's order, as one batch,
  ahead of anything the hook delivers later; a later `ForwardingEnded`,
  from a replacement listener, finds the hold released. If the listener's
  reads end first (`UiaFocusReadsEnded`), the hold is released, dropping
  the held copies raised at or before the newest listener copy handled. An
  outpost no listener forwards to (`run_attach`, or a test's,
  `Outpost::without_listener`) holds nothing. The hold costs only an
  outpost's start: a median of 20.6 ms, at most 591.3 ms over 126 starts.
- When an application's outpost generation ends, from any stage, or the
  supervisor declines to watch it, the supervisor sends the listener
  `HookRemoved`, and the listener forwards that application's events again.
  A replacement listener gets `--hooks-in`, forwards none of those
  applications' events, and first sends each such outpost a
  `ForwardingEnded`. Messages for the listener while it launches are held
  in order.
- A console window's events reach the console host's outpost by its own
  hook, which Windows scopes by the event's thread.

### A UIA focus, judged and read by the listener

The listener judges and reads each UIA focus from the event's own element,
since an element cannot be handed to another process; how is
[verbatim-listener](verbatim-listener.md)'s. The read reaches the outpost
as a `UiaFocusRead`, or its drop as a `UiaFocusDropped`, behind the
marker the listener sent first, and the outpost's reducer keeps the
intake's order ([verbatim-outpost-logic](verbatim-outpost-logic.md)). A
`WinEvent` focus copy in a window the outpost judges UIA is dropped
("MSAA focus dropped: UIA owns the window"), as NVDA's
`winEventToNVDAEvent` drops a native UIA window's event, so its UIA focus
alone reports it.

### Console windows and frames of another process

A console window is one application in its console host's outpost, as
NVDA gives it and its text area the client's app module. The listener sends each
`WinEvent` for a `ConsoleWindowClass` window to the process of the event's
thread, the console host (`verbatim-listener`'s `listener::routed_process`), dropping one whose
thread cannot be opened. The console host's outpost marks each event in a
console window with the client as its `application`, which Core takes as
the event's source, and answers a focus-now query for the window. A focus
change carries its application's name, read once per process and kept with
the process held open (`outpost::ApplicationNames`).

An outpost takes a window as its application's when the application serves
its UIA (`verbatim_win32::window::served_by`, through
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
for it (`read::FocusRead`: a focus-now query or a cut walk, below). The one connection the call makes into
the other application cannot be prevented.

Every window the worker judges or acts on for an event, whether its own
hook delivered the event or the listener named the window (a drop's
nearest window, a read's window, a selection's), is first checked as its
application's (`read::own_window_verdict`, through `served_by`): another
process's window is neither probed, read nor reported, with a debug line
naming it, since any process can raise a `WinEvent` naming another's
window, and the probe, or a console window's check by handle, would call
into that process. The supervisor checks that a
process it starts an outpost for serves the fact's window
(`check_served_by` in `crates/verbatim-supervisor/src/owner.rs`; a debug build fails). Core
routes a console client's focus-now query to its host through
`verbatim_win32::provider`'s `provider_process` and `console_host_of`.

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
raw-view step (`Uia::neighbor`), which reads such a neighbor through UIA
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
`FocusReader::set_window_readability_reader`) and the outpost's
(`read::uia_walk_leaves`), checks the window each step reaches, and a
ghost's or a hung application's window other than the focus's nearest one
ends the walk there, the parent in it not kept, as NVDA makes that one
step and then reads the window as a plain window: the listener names it
to go on from (`AncestorsEnd::ContinueFrom`), and the outpost goes on from
it. The check is asked before the outpost's verdict for the window, so a
hung window is never probed. A remote program cannot judge a window
between its steps, so a focus-now query whose focus has such a window
above its own (`read::plain_window_above`, local calls by
`verbatim_uia::raw_view_parent_window`) reads its ancestors by the classic
walk instead. The outpost's walks have no time limit but their program's
deadline: a listener read cut at its depth limit
(`AncestorsEnd::PlainFrom`), and a walk cut at its depth limit of 64, are
completed by plain windows from the outermost element's nearest window,
included, and no continuation is cut by time. Whether a
window can be read is the system's judgement
(`verbatim_win32::window::readability`, a `Readability`), which a
test that cannot wait five seconds for Windows to judge an application
hung replaces (`Outpost::set_window_readability_reader` and
`FocusReader::set_window_readability_reader`, each a
`verbatim_win32::window::WindowReadabilityReader`).

### Starting an outpost and asking for the focus

A fact for an application with no outpost starts one
(`policy::may_start_outpost`) unless it is a selection or a
notification other than the shell's window-snap results. Node identity
never crosses a process: the outpost mints the `NodeId` from the forwarded
runtime id. Nothing polls: at startup, and after an outpost or the listener
is replaced, Core asks for the focus with one focus-now query.

An application usually gets a spare ([ADR 0001: Focus](../adr/0001-focus.md),
"Spare outposts"): the supervisor keeps two outposts started with `--spare`, with
everything made but a hook, which say `SpareReady`. A start takes the
oldest, ready or not, and sends `Assign`; the spare sets its application,
reads the clock, installs its hook, waits on the application's exit, and
says `Ready`, the supervisor holding the application's facts meanwhile.
Spares start once Core's own outpost is ready or has ended; a given spare
is replaced at once. Spares are pinged but never asked in idle rounds,
sent end-of-reads markers, or retired.

Every outpost failure takes one path (`Owner::outpost_failed`): the process
exits or crashes, its hook fails (a `Fault`, then exit code 1 without
`Ready`), its launch fails, it misses three pings, or it piles up abandoned
workers. The failure is logged with its last fault, as an error (heard)
for a spare or before `Ready`, a warning otherwise. If the application has
exited, that is its end. Otherwise the application keeps its record, held
facts, and any idle query, and gets the next spare or a new outpost: the
first failure at once, further ones after a backoff (`policy::Backoff`) of
1, 2, 4, 8, 16, then 30 seconds, counted again once an outpost stayed up a
minute (`STAYED_UP`) or the application comes to the foreground. A ready
outpost's `Ended` and `HookRemoved` go at once; for one that failed before
`Ready` the app hears `Regenerated`, then the failed one's `Ended` if it
heard it start, then the next one's `Started`, which also answers an idle
round in its place. Spares' failures are replaced with their own backoff.

### Who calls into an application

Only an application's own outpost sends it a remote operation or makes a
new UIA connection to its windows by a call of Verbatim's, with the
exceptions `docs/design/cross-process-calls-study.md` lists: UIA's client
side's connections to a parent across the boundary, and the focus-read and
walk-up races (`books/dev/src/explanation/design/listener-without-connections.md`, section 2),
so no lock is needed around them. No node is searched for by runtime id:
a UIA node whose kept element is gone, or that has none, and whose focus
route's walk finishes without it is answered gone
(`read::resolve_uia_element`), while a walk that fails, a window lookup
timed out or refused among them, fails the query (`focus_path::find`),
since a slow application has not shown the element gone. The walk has no
time budget of its own: the program's deadline bounds it, and a walk
still under way at the deadline is abandoned with its program. A
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
([verbatim-process](verbatim-process.md)); the fact a `WinEvent` makes,
whichever hook took it, is `verbatim_ia2::fact_of_win_event`
([verbatim-ia2](verbatim-ia2.md)).

### The arbitrator

`Arbitrator` is NVDA's per-window backend decision (`_isUIAWindowHelper`),
each outpost's one verdict per window. It holds the configuration: the
class lists and the clocks. The verdicts probes
found are the reducer's state, `Verdicts` (`verbatim-reader-rules`),
which callers give it. `verdict(&verdicts, hwnd, &WindowClasses)` answers
from the good class list, the Windows 11 shell rule, the bad class list,
the override, or a kept probe, and `None` when only the probe can decide;
a program then probes the window with the raw probe
(`verbatim_uia::raw_probe`) through `arbitration::uses_uia`, and records
its answer through its `arbitration::Judgement`, the request's context
(`Arbitrator::record_probe` makes the same record for a test that probes
by hand). No connection follows a probe
(`books/dev/src/explanation/design/listener-without-connections.md`, decision 2). The listener
makes no verdict; it skips a read only by
`verbatim_reader_rules::class::class_verdict`, the class lists alone, from
the same tables and code, which the arbitrator's own lists are built from.

- `verbatim_win32::window::window_classes(hwnd)` reads a window's
  `WindowClasses` (`verbatim_reader_rules::class`): the raw class, the
  class normalized as NVDA does (`normalize_class_name`), and the root
  ancestor's class.
- `post_probe_check` (`verbatim-reader-rules`) names NVDA's check before using a provider: a list
  view's Windows Forms origin, or a console's formatted text, read from
  the window. A provider that fails is recorded as excluded
  (`Probed::Excluded`), MSAA for the window's lifetime.
- A "yes" is kept for the window's lifetime and dropped on destruction
  (`VerdictChange::Forget`, which the program handling the window's
  destroy event commits; decision D15). A "no" is kept for events observed until
  `NEGATIVE_VERDICT_LIFETIME` (500 ms, NVDA's cache period) after the
  probe's end, since a busy application answers late. A timed-out probe is
  an unknown verdict (`arbitration::Verdict::Unknown`), which the worker
  reads through MSAA and keeps as "no" the same way (`Probed::TimedOut`); a
  verdict kept for the window's lifetime meanwhile stays.
- A kept verdict is compared with the observation of the event it judges,
  never with when the worker handles it: each worker entry is judged at
  its event's observation, or, for a program with no event, now at each
  judgement (`Judgement::judged_at`), and a verdict judges the
  events observed from its probe's beginning, stamped with the observation
  of the judgement that probed, until its keep ends. A verdict judges no
  event observed before its probe began; each window keeps its previous
  verdict for those (`Verdicts::kept_at`), one being enough behind the
  intake's marker barrier.
- A program reads its own copy of the verdicts (`Facts::verdicts`, an
  `Arc` the reducer replaces on change, so a copy costs a reference count).
  Each probe is posted as it returns, as a message of its own
  (`Posted::Verdict`, `Input::Verdict`, taken even after the mailbox
  closes, as a commit is), and applied to the program's copy, which it
  judges by for the rest of its walk. The reducer records a probe only
  from the worker generation in charge, but a timed-out probe and a destroyed window's
  forgetting from any generation, so a window is not probed again more
  often than NVDA probes it. A probe committed before its worker is
  abandoned stays recorded. When the record changes the reducer publishes
  the new snapshot to the mailbox's push-time copy (`Effect::Verdicts`),
  and after it, for a probe that found a provider, nudges the event thread
  to decide its hook again (`Effect::Narrow`).
- `lasting_verdict` answers only a verdict that lasts the window's
  lifetime.
- `Clock` (`verbatim_process::clock`), `Arbitrator::set_clock`, and
  `Arbitrator::now` time the classic
  walk budgets; `Outpost::set_clock` sets it for tests that count calls. The keeps'
  ends and an entry with no event are stamped by the `ObservationClock`
  (`Arbitrator::set_observation_clock`), the run's shared clock, which
  `Outpost::set_observation_clock` sets, with the stamps of the outpost's
  own events, for a test that moves that time on.
- Both class lists, `verbatim_reader_rules::class`'s, are lifted from
  NVDA and pinned by unit tests there naming their sources (`badUIAWindowClassNames` and `goodUIAWindowClassNames` in
  `nvda/source/UIAHandler/__init__.py`, and `isGoodUIAWindow` in
  `nvda/source/appModules/explorer.py`), so an NVDA sync is a diff.

### The outpost

`Outpost`, `run_pipe`, and `run_attach` are the per-application outpost.

- `run_pipe(pipe_in, pipe_out, target_pid, options)` runs one on Core's
  inherited overlapped pipes, watching `target_pid`, or for `None` as a
  spare. `Outpost::assign` sets the application once: the clock, the hook on
  the `WinEvent` thread (`verbatim_ia2::event_thread::EventThread::hook`), and `Ready`. When the hook
  fails it reports a `Fault`, sends no `Ready`, and exits with code 1
  ([ADR 0001: Focus](../adr/0001-focus.md), "Spare outposts"). It serves until
  `Shutdown` or the end of the command pipe, then shuts down cleanly, and a
  thread waiting on the target sends `TargetExited`.
- `run_attach` watches a pid, asks for its focus, and prints each outbound
  message as the model's JSON on stdout: the development mode.
- `OutpostOptions::remote_operations` (`verbatim_process::arguments`) says whether a UIA focus's ancestry
  may be read with a remote operation (on; `--classic-uia` turns it off).
- For tests in their own process: `Outpost::new` and its siblings watch
  from construction and panic when the hook fails, with a sender thread in
  place of the I/O thread; `with_options`; `with_focused_element_reader` (a
  `FocusedElementReader` in place of `Uia::focused`);
  `set_foreground_reader` (a `verbatim_win32::window::ForegroundReader` in place of
  `GetForegroundWindow`, for acceptance and the window a focus entered);
  `observe_foreground_programs` (a `ForegroundProgramObserver`, told on the
  worker the window of each foreground change a program is about to
  handle, so a test can hold the worker there);
  `set_focus_window_reader` (a `FocusWindowReader` for focus-now and the
  moved-on judgement); `set_observation_clock` (a `verbatim_process::clock::ObservationClock`
  for the outpost's own events' stamps and its verdicts' keeps);
  `set_handling_deadline` (a deadline in
  place of the reducer's `HANDLING_DEADLINE`, 11 seconds, for the
  programs dispatched after it, so a test that waits out a worker's
  abandonment waits less; Verbatim never calls it); and `without_listener`, for an outpost
  no focus listener forwards to, whose hook's copies are never held.
- `Outpost::settle` posts a barrier (`Item::Barrier` with
  `Asker::Settle`) and waits until the reducer reaches it with no program
  in flight, everything queued before it handled and a noted caret read,
  then, on the caller's thread, until the subscription has made every move
  asked (`Registration::settle`), every published message is written
  (`Outbound::flush`), and the event thread has taken every nudge. The
  reducer answers it as it is reached (`Effect::Settled`), waiting for no
  open caret watch or held copies of the hook's, as
  Core's round does; it divides batches as the round does, and waits for
  nothing that arrived after it. The shell keeps its waiter by number, and
  checks after every transition that the waiters are the settles waiting
  in the intake (`Reducer::settles`); at shutdown the intake drops them
  and the shell its waiters, so the call returns. It covers only what has
  reached the outpost.
- `Outpost::failed_reads` gives the worker's and subscription callbacks'
  failed cached reads (`verbatim_uia::calls`), also logged at shutdown.
- `Outpost::observe_heard` hands a `HeardObserver` each event the outpost's
  own handlers and hook take in, as a `Heard`, once queued.

Inside an outpost (`docs/design/communication.md`, section 6.4;
`books/dev/src/explanation/design/ownership.md`):

- The mailbox (`outpost::shell::Mailbox`): the hook callbacks
  (`APP_SUBSCRIPTIONS`), the subscription's callbacks, routed facts,
  Core's commands, and the workers' commits and ends only append to it,
  under its one lock, and return. Two kinds of entry are dropped as they
  are pushed, judged on copies of the reducer's marks that only the shell
  writes, from the reducer's effects (`Marks::drops_on_arrival`): a held
  console's updates and the system caret's events while the focus has
  caret events of its own. The event thread's drop of an MSAA event of a
  window known to use UIA reads the mailbox's copy of the reducer's
  verdicts (`Mailbox::verdicts`). A UIA callback keeps the cached parts and
  window handle, and parks its element, and an active text position's
  range, in the store (`store::Parked`), the entry carrying the key.
- The reducer ([verbatim-outpost-logic](verbatim-outpost-logic.md)), a
  pure state machine on a thread of
  its own, run by its shell (`outpost::shell`): it holds the intake's
  order (`reducer::intake`), the reports Core may hold
  (`reducer::held`), the follow state (`reducer::follow`), the hand-over's
  hold, the arbitrator's verdicts (`reducer::verdicts`), each node's text
  facts (`reducer::text`), the program in flight as its work in flight
  (`verbatim_reader_rules::in_flight::InFlight`: the worker generation in
  charge, the deadline and the moved-on rule) with its hang mark, the
  record of hung windows, and the queries not yet answered. Inputs
  come in mailbox order; effects go out: dispatch a program, send a
  message, release, move the subscription, abandon a worker, set a timer.
  It names no Windows type, holds no application object and makes no
  call; what it needs of Windows' own state it asks for by an effect
  (`Effect::AskJudge`, the check of the program's window, answered
  `Input::HangSeen` when it is hung and `Input::Judged` with whether the
  user moved on; `Effect::CheckFront`, the check of the window in front
  while a listener read's marker waits, answered `Input::HangSeen` or
  `Input::FrontReadable`; `Effect::JudgeTold`, the told window judged,
  answered `Input::ToldJudged`; `Effect::CheckHung`, a window a commit
  named hung confirmed, answered `Input::HangSeen` when it is), which the shell's `apply` answers at once, as
  the reducer's next inputs, before it takes another message, and
  what the answer asks is carried out before anything asked after the
  question, as a release's answer is. `cargo xtask ci`
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
  Core's request ids are unique for the outpost's life; entries queued
  behind the close still reach the reducer; the shell gives the reducer
  only a commit it accepts; a program that answers a query commits that
  query's reply as its one message; no program arms a walk; a read under
  a key's note commits its effect only to the note named by that note's
  end; only the owed
  work commits it done; and only a caret read of an edit control's client
  area is cut at its value change check.
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
- Hung windows (`outpost::worker`, `outpost::shell`): each program's window is
  named in its worker's charge, keyed by its request, with no call into
  the application: the window its entry names, at its start; else a
  query's node's own window (the window an MSAA node is kept under, or a
  UIA element's whose runtime id is `[42, hwnd]`), the application's focus
  window for a focus read and a node of no window of its own
  (the foreground thread's focus window when that thread is the
  application's, else the top-level window of the focus the outpost
  expects Core to hold), a tree dump's application's main window
  (`window::main_window_of`), a shell item query's taskbar window
  (`shell_items::taskbar_window`), the owed work's and the console poll's
  node's. The shell answers the reducer's `AskJudge` by that window, read
  under the charge's lock: hung or ghosted by the outpost's reader, which
  is a hang seen, told to Core once per hang and abandoning nothing, then
  moved on, which the reducer ignores for a tree dump and a shell item
  query, as they are not about the focus and are bounded by their deadline
  alone; with no window named, the program stays. While a listener
  read's marker has waited half a second, nothing is in flight behind it,
  so the shell answers the reducer's `CheckFront` from the window in
  front (`Context::foreground_window`, by the outpost's foreground
  reader): a ghost is mapped to the hung window it stands in for
  (`verbatim_win32::window::hung_window_from_ghost`), since the ghost's
  process is the desktop window manager's, and a window the application
  serves is judged by the readability reader, all by local calls. An
  event's check before its first call (`Worker::readable`), a query's of
  its named window, and a window message's (through the call gate) all ask
  the same reader; a window found hung is named in the program's commit as
  hung, and a query refused so is answered `Abandoned` with no call, so a
  query on a hung window parks no worker. A listener read is not tested:
  its program reads the focus, waiting for the application call by call
  within its deadline. A focus read is not refused: its check of the focus
  window answers no focused control in a window that cannot be read. A
  commit names the window its program found hung at a check, if any
  (`Change::found`). The worker installs a call gate on its thread
  (`verbatim_win32::calls`), bound to its charge, in release builds as in
  debug ones, and uninstalls it as its thread ends: once its generation is abandoned, every counted call fails
  with `CallError::Abandoned` and makes no call, which no caller reads as
  a fact about the application. The shell keeps each parked worker's
  reason by generation (`Posted::Returned` carries the generation), and
  the pong reports those parked on a window seen hung, by their program's
  hang mark (`parked_on_hung`), apart from the rest (`parked_count`),
  which alone the supervisor's limit counts. The two counts are one value
  (`Lock<Parked>`), written together when a worker is parked or returns
  and read once per ping, so each parked worker is counted once in a pong.
- Programs (`outpost::worker`): the worker generation in charge takes one
  program at a time from its inbox, an entry's handling, the owed work, a
  release's drop, or the finish of a caret
  read cut at its value change check ("Text and caret"), with copies of
  what it
  reads
  (`reducer::Facts`: the reports, the follow state, the marks, the
  verdicts), and posts
  each commit (`commit::Effects`) to the mailbox, applying its pure part
  to its own copies (`reducer::apply`), so what it reads after is what the
  reducer will hold. The shell applies a commit only while its request is
  live, the program in flight of the generation in charge, and refuses
  any other whole: first the
  writes only it makes, to the store and its table of the elements the
  subscription follows (`commit::Writes`), the program's text writes
  among them, then the reducer's change. A
  program receives too the caret events waiting when it was dispatched
  (`reducer::WaitingCaret`), which a caret key's check asks after. The inbox
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
- The focus-following subscription ([ADR 0001: Focus](../adr/0001-focus.md),
  "Focus-following subscriptions"): one registration with NVDA's two groups. The base group, given every
  UIA focus whose element the outpost has, holds name, value, and state
  changes and `Text_TextSelectionChanged`; the extended group, for a focus
  with text read through UIA, adds `Text_TextChanged` and the active text
  position changed event. The console host's text area takes the group
  without its text events (`Group::ExtendedWithoutTextEvents`), and the
  console's caret events are its caret events
  (`books/dev/src/explanation/design/wake-ups.md`, item 15). Each is on the element of every
  UIA report whose focus node the outpost has not released, the newest
  one's moved as each focus is followed and the older ones' kept until
  their release (`FollowState::kept_groups`), never on the elements'
  descendants (`docs/parity/backends.md`, "UIA event registration"). An
  event is attributed to its node by its cached runtime id, and a caret
  or text event of an older report's element is read for that node, which
  Core targets; a terminal's output is read for every terminal with a
  follow entry, which keeps its entry until its node is released. A focus
  whose parent is a UIA slider of class `SensitiveSlider`
  (`verbatim_core::focus_rules::is_sensitive_slider`) has that parent
  given the value group too (`Group::Value`, value and range value
  changes alone, through `retarget_with`), since Core speaks its value
  changes while its thumb is the focus (`docs/parity/focus-context.md`,
  "A slider that focuses its thumb"); the listener's read keeps no
  ancestor's element, so the worker finds the slider by the focus's path
  (`sensitive_slider_of`, `element_by_focus_path`) and keeps it under
  the parent's node. Elements
  arrive as `Cached<PropertyEvent>` or `Cached<TextEvent>`, parked in the
  store (`store::Parked`) and resolved as `Kept` by the program that takes
  them. The shell moves the subscription as the reducer's follow state
  asks (`reducer::Subscription`), by its tokens for the elements it keeps:
  add the group to the new focus, then `RemoveEventHandlerGroup` from the
  old;
  `RemoveAllEventHandlers` only at shutdown. A move to what is already
  followed makes no call; a focus read through MSAA is followed by nothing.
- The queue ([How focus works](../../explanation/design/focus.md), "One MSAA
  focus"; the rules are in
  `books/dev/src/explanation/architecture.md`, "Outposts"): one entry per object and kind, and for
  an MSAA event other than a menu opening per raising thread (a menu opening
  per address; the console host's events per window and kind), a newer
  replacing it at the back. A batch is what accumulated during the previous
  one, in arrival order, but a focused terminal's caret event ahead of its
  text change goes just after the change, whose read reads the caret
  (`caret_after_update`). Per batch the newest 4 MSAA focus events and 10
  other MSAA events per thread are kept (the focus's own events beyond the
  limit, a foreground change by either). Nothing is dropped at planning
  for a hung window: the worker tests a UIA event's own
  window first as it handles it (`IsHungAppWindow`, a window of 0
  passing, as NVDA's test passes a sender with no cached window), before
  any window lookup or read, so an entry whose window became hung while
  it waited is dropped too, and a dropped event's parked element and
  range go with it; an MSAA event's window is tested by `readable`.
- Runs: each run of MSAA focus and foreground events is tried newest first,
  each try a program under its own deadline, until one is reported, at the
  run's newest place, as NVDA's `pumpAll` does (`ends_a_run`); the reducer
  dispatches the next try when a try's program ends unreported, and the
  replacement of a worker abandoned in a try tries the next. Three
  focus candidates per backend are kept (`Planned::Focus`). A foreground
  change removes a waiting focus on its client object from the same thread
  not raised after it. A run whose events all carry raise times is tried in
  that order (`in_raised_order`), ties in the order they arrived. UIA
  focus events are handled at their own places, the newest menu opening
  per backend last.
- Markers: nothing that arrives after a UIA focus's marker is planned until
  the read, its drop, or the end of reads resolves it, as NVDA's UIA events
  pass through one flusher thread. When the read's ancestors include the
  focus last reported, that old focus's own state changes queued after the
  marker are planned ahead of the read (`old_focus_events_first`), since a
  provider moving the focus into a child raises the old focus's change
  first but it comes by another route. The check compares runtime ids.
- The worker generation in charge is the only one calling into the
  application, one program at a time, so events and replies leave in
  planned order but for menu openings, a run's older focuses, and a caret
  key's answer. Before any call a program applies NVDA's acceptance test
  (`verbatim_reader_rules::acceptance`; `docs/parity/focus-events.md`,
  "Event acceptance"): `accepts_event`, a pure function of the event, its
  window, `WindowQueries` (local calls in
  `verbatim_win32::window::SystemWindows`), and `RequestedEvents`; for an
  MSAA focus and a foreground change the test was made on the reducer's
  thread as the entry was taken in, and the program uses the result its
  entry carries (`reducer::Front`);
  its `foreground_relation` goes in the window facts
  (`verbatim_core::focus_rules::ForegroundRelation`). It takes the backend
  crates' per-thread call counts into each published `EventTiming`; a
  program that publishes nothing logs its calls (`books/dev/src/reference/performance.md`,
  "Cancelled traces").
- Deadlines and abandonment are the reducer's: an entry has the handling
  deadline (`HANDLING_DEADLINE`, 11 s); a focus read, and the report of a
  listener's read, the focus read's (`FOCUS_READ_DEADLINE`, 13 s); a shell
  query the due time it carries (`budget` in `verbatim-outpost-logic`). A program on a window also has its worker abandoned once the user
  has moved to another UI thread of the same provider process, judged
  when the half-second grace ends and every half second after until the
  deadline, as NVDA's watchdog judges, by an ask the shell answers with
  `verbatim_win32::window::is_another_thread_of_its_application` against
  the window with the keyboard focus, by the outpost's focus window
  reader; the same check tells a hang of the window and abandons nothing
  for it; a foreground fact judges nothing. It marks the generation dead in the worker's charge
  (`worker::Charge`), answers the query the program was running
  `Abandoned`, and starts a replacement. The shell's warning does not name the call
  the worker was in: only the worker's thread knows it. A returning abandoned worker has
  its commits refused, posts that it returned, and exits. The shell alone
  counts the abandoned workers that have not returned, one more as it
  carries out `Effect::Abandon` and one fewer at each return, which no
  longer reaches the reducer, and publishes the count to an atomic the
  I/O thread answers pings with.
- A focus is reported once its walk returns, in full, however long the
  walk takes within its program's deadline: no walk is published at a
  bound. The owed work, a terminal's first read from the caret read with
  its report, is a program the reducer dispatches before anything else,
  once per worker generation, under a deadline of its own with no
  moved-on window.
- Requests and their tags: each program is a request with a mint tag of
  its own (`store::Tag`, the generation and a request number the reducer
  gives), which its `RequestContext` carries with the store's handle; the
  program in flight's request is live until it ends or its worker is
  abandoned. A program's text writes
  go to the store only with a commit the reducer accepts, as every other
  write of the shell's does ("Window destruction and held objects", text
  on copies). A terminal read works on a
  copy of its entry, put in place only by its commit.
- Queries: each query the outpost takes has a reply token
  (`verbatim_model::protocol::Responder`) in the shell, keyed by request
  id, consumed by the one answer the reducer sends: the reply, `Abandoned`
  at the deadline, `Failed` for a program that ended without answering,
  `NotStarted` for a `Cancel` before dispatch. A token dropped unanswered answers `Failed`. In debug builds
  the shell asserts after each transition that its tokens are the queries
  the reducer records.
- Time limits in an entry ([ADR 0001: Focus](../adr/0001-focus.md), "Time
  limits"): UIA
  calls end at `verbatim_uia::UIA_CONNECTION_TIMEOUT` (2 s) or
  `UIA_TRANSACTION_TIMEOUT` (10 s); the continuation of a listener read's
  chain, an MSAA focus's ancestor walk and a focus read's walk have no
  limit but their program's deadline; a dialog's text, UIA's as MSAA's, is gathered in
  its program on the worker's own client, bounded by the program's
  deadline, as NVDA gathers it inside its focus event; tree view,
  list view, and edit messages use `SendMessageTimeoutW`, refused unsent
  by a hung or ghosted window as NVDA's `SMTO_ABORTIFHUNG` refuses them,
  none being in the `WM_APP` to `0xBFFF` range NVDA exempts; an
  `IAccessible` call is bounded by the entry's deadline.
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
  the element for remote operations; `remote`, `fetches`, `support`, and
  `known_support` say whether caret reads try a remote operation, which
  formatting they read, and the `TextSupport` known before and after), or
  `edit::EditText` over an edit control's messages (offsets; a word is the
  line segmented by `verbatim-text`'s rules, as NVDA finds it; the plain edit
  control alone answers `TextSource::measure`, a point's offset and the
  text's length, so a location there is answered `OffsetLocation`, and a
  rich edit's is answered `Location`, as NVDA reads a rich edit through
  its text object model).
  `caret_report`, `caret_report_since` (with `ReportAsk` and
  `ReportedCaret`), and `caret_report_checking` read the caret for
  `CaretMoved`. `Anchors` keeps a backend's anchors by node for a reader
  that holds them for the whole read, a test or mockapp, until
  `forget_anchors`, with `note_content_changed`; `NodeText` is one node's,
  borrowed from it or from the crate's `NodeTextCopy`, the copy a
  program's text call works on (`position_at`, `forget_selection`,
  `reported_point`). Both hold the reducer's `NodeAnchors` with each
  position a `Slot`: the source's position and the key it was kept under,
  so a call's result names a position it was given by the same key and
  only one it made is given a new one (`NodeTextCopy::finish`);
  `named_anchors(op)` lists the anchors of the positions an operation
  names, which a copy must hold. `check_key` checks a caret key's watch
  with one read against its baseline (`CheckKey`, `LastRead`) and
  answers `KeyChecked`: the read and whether it shows an effect, that the
  caret moved while it was read, or that it failed.
- `dialog_text(&dialog)` gathers a dialog's own text by NVDA's rules
  (`docs/nvda/object-model.md`, "A dialog's own text") over a
  `DialogParent` whose children are `DialogObject`s: MSAA through
  `verbatim_ia2::dialog::DialogObject`, UIA through
  `UiaDialog::new(uia, request, element)` and `UiaObject`, each child read as
  a `verbatim_uia::DialogText` with its description whatever the theme
  (`books/dev/src/explanation/design/uia-shapes.md`, decision 5). `is_dialog(role)` says which
  roles have such text; more than `MAX_OBJECTS` objects gives no text.

## Implementation notes

### Foreground changes and focus-now

- A batch with a foreground fact is handed out at once. A foreground fact
  whose window is not in front at the outpost's one test is dropped, as
  NVDA's `processForegroundWinEvent` drops it, since Windows raises its own
  event once the change is made (`docs/parity/focus-events.md`, "Stale
  focus events"). The test is made on the reducer's thread as the reducer
  takes the entry from the mailbox (`Context::front_test`, carried on the
  entry as `reducer::Front`), never on the worker, which can wait behind a
  call into a held application; an MSAA focus's acceptance test is made
  there too ([ADR 0001: Focus](../adr/0001-focus.md), "The foreground and
  the window title"). It counts among its thread's other events and among focus events
  in the batch limits, so later focus events do not crowd it out.
- A foreground fact in front is reported, whatever the focus: NVDA's two
  tests against its last queued focus are Core's, which refuses a
  foreground report of the window its focus is in. In a focus group, a
  reported change whose window holds a report's line lets the older entry
  be tried, and every report of the group carries its number and rank
  (`verbatim_model::FocusGroup`), so Core takes the older entry only when
  it refused the change; a reported change of any other window ends the
  group, as a reported focus does.
- It is reported at once as a focus on the window, stamped when its test
  found it in front, never before its observation. The window's own object is read (`window_element`, or
  `OBJID_CLIENT`), so a focus on its client area is the same node; a popup
  menu window (`#32768`) reads as a menu (`read::window_snapshot`). An
  unreadable window is reported from local data, and an empty name takes
  the window text (`InternalGetWindowText`). A window that leaves the front
  during its read is still reported, as NVDA makes no test after its
  object is made; Core judges it by its foreground record.
- A foreground event for a UIA window is dropped, as NVDA drops every MSAA
  event for one; the window is announced from the focus's ancestry
  ([ADR 0001: Focus](../adr/0001-focus.md), "The foreground and the window
  title"). A console window's title
  comes from its window element among the listener's ancestors.
- An MSAA client object of a `WS_SYSMENU` window is marked
  `NodeDetails::top_level_client`, read as a pane and presented by its
  title, as NVDA's `isPresentableFocusAncestor` presents it.
- A plain window (`read::plain_window`), a window's pane read from local
  state (`read::client_pane`), and a foreground window reported
  from local data, carries its handle in `NodeDetails::window`, as the
  readers' UIA window elements and MSAA window and client objects do, so
  Core takes the frame one outpost reports as its focus and another as a
  pane above its content as one container.
- Focus-now answers the foreground window, when the application holds the
  foreground (the outpost's foreground reader, `GetForegroundWindow`, never
  `hwndFocus`), and the focused control (`hwndFocus`, through its window's
  backend) with its ancestors, complete to the desktop, selected child,
  and caret (`read_focus_text`; a console's text area
  reads as a nameless terminal, `read::console_text_area`). A foreground or
  focus window that is a ghost's or a hung application's is answered as a
  plain window, the focus with the plain windows above it, with no call, a
  deliberate difference from NVDA, which says nothing (Dickson,
  2026-10-29). Through MSAA the focus is the focus window's client's
  `accFocus`, and an object it names in a window this application does not
  serve answers no focused control
  (`verbatim_ia2::acquire::focused_object`). There are no retries.
- A focus query says why it is asked (`Query::ReadFocus { purpose }`).
  For a fake focus (`FocusPurpose::Fake`), an MSAA control is read only
  when it or an ancestor has the focused state, NVDA's
  `shouldAllowIAccessibleFocusEvent` test on what its fake focus queues
  (`FocusedObject::has_focused_state`, the walk a focus event's object
  gets); one that fails makes the whole answer empty, no window and no
  control (`read::FocusReadMiss::Unfocused`). A UIA control has no test,
  and a fake focus with no control is no input whatever its window, so the
  window is not tested. Every answer's window facts carry the
  application's name, a console window's client's, and whether Alt is
  held (`worker::focus_answer_facts`), as a reported focus's do, so Core's
  rules for an application's windows, the Alt+Tab switcher's among them,
  apply to an answer too.
- Hidden-frame suppression (decision D9): Core's hidden 1x1 frame, marked
  with `verbatim_model::HIDDEN_FRAME_WINDOW_PROP`, is never announced. Every
  `FocusChanged` path checks the property (`GetPropW`) on the window and its
  root (`GetAncestor` `GA_ROOT`): the MSAA path (only for `CHILDID_SELF`),
  the UIA path, and focus-now (`focused_control_in` treats it as nothing
  focused). A marked window counts only in Core's process, the outpost's
  parent. A popup menu opened while the frame is in front (Verbatim+V) is
  reported with the frame's snapshot as its only ancestor, so the reducer
  says "Verbatim" and then "Context menu" (`docs/parity/focus-announcement.md`,
  "Focus announcement content and property order").

### Event flow

- The event thread hosts the hook, never rebound: a second
  `WINEVENT_OUTOFCONTEXT` hook set on a thread that has one has been seen to
  stop delivery on it for the process's life.
- Every event is arbitrated per window, so one backend reports it. The event
  thread drops an MSAA event for a known UIA window before queuing it, from
  its record of windows (`arbitration::EventWindows` in a
  `verbatim_ia2::WindowCache`), asking the arbitrator with the mailbox's
  push-time copy of the verdicts only until `lasting_verdict`; a verdict a
  program probed reaches it one transition of the reducer later, which
  lets at most a few more MSAA events through for the program to drop. An
  entry goes on destruction or when the handle is
  found owned by another thread, and all go after an override change. A UIA
  element that is not a window gets its window from `nearest_window_handle`
  on the worker.
- The event thread narrows the hook (`outpost/narrowing.rs`;
  `books/dev/src/explanation/design/wake-ups.md`, "Narrowing the outpost's hook", which states
  what can be lost): while the focus window and foreground window are both
  UIA, the value, state, name, description, selection, and text selection
  events are unhooked (`narrowing::NARROWED`), hooked again once the focus,
  a menu opening, or the foreground is in an MSAA or undecided window.
  Those windows come from the hook's own events, or, until it delivers one,
  from nudges for routed facts (`verbatim_ia2::event_thread::Nudger`); the shell nudges
  when the reducer records a UIA verdict (`Effect::Narrow`).
- Trace ids are minted when an event arrives. `WindowFacts` (`window_facts`,
  local calls) give the top-level window and root owner, topmost style,
  `Windows.UI.Core` activity, NVDA's foreground test, and for a focus the
  window's class, the application's name (executable stem, lower-cased; the
  client's in a console window), and whether Alt is held.
- A UIA focus's snapshot carries the element's class
  (`NodeDetails::class_name`) and its parent's automation id
  (`NodeDetails::parent_automation_id`) for Core's per-application rules.
  A node read alone carries grid row and column when the theme reports cell
  coordinates (`gates::node_gates`), and a focus, its selected child and a
  navigation step's neighbour their table (`NodeDetails::cell`), read in
  the remote program where one runs (`FocusQuery::is_cell`).

### Reading the focus

- A UIA focus is reported from the listener's read (`Worker::uia_focus`),
  which the outpost does not test against the foreground: the listener made
  that test before it read ([ADR 0001: Focus](../adr/0001-focus.md), "The
  listener's checks, ported from NVDA's focus handler"). The outpost's
  acceptance test covers MSAA events and the other UI Automation events,
  not a focus. Its backend is decided. Its runtime id keeps its node; when the
  listener read that the earlier element under it lost the focus or is
  gone, the report carries that evidence (`FocusChanged::earlier`) and its
  commit evicts the element kept for the node, if still kept
  (`commit::Stale`, `Store::evict_element_if`), not moving the
  subscription to it; the node's text is dropped from the program's
  copies and the store before the caret read, while the entry's request
  is live (`text_reads::forget_text`), so the caret read takes the new
  element's pattern, and the commit drops the node's terminal entry and
  ends a caret key's watch on the node (`Change::replaced`). The
  subscription's target is compared by node, group, element and slider
  element, each element by the shell's token for it, which the shell keeps
  while it is followed, so a node or slider whose element was replaced is
  followed again. When the node still keeps the earlier element because the
  focus's own element was not found, the caret read reports the text as not
  read (`FocusText::NotRead`), so Core speaks neither the text nor the value
  (`Worker::read_focus_text`). Its
  ancestors are the listener's chain, filtered, spliced at the previous
  chain, and completed above as the listener's walk ended
  (`read::uia_chain_from_read`). Every UIA
  focus is handled, oldest first; Core culls expired speech.
- The outpost then finds its own copy by the chain's runtime ids
  ([ADR 0001: Focus](../adr/0001-focus.md), "The outpost's own element"),
  after the report for a
  focus without text and before it for one that may have text: one
  `ElementFromHandle` and one remote operation
  (`verbatim_uia_rops::element_by_path`, or the classic walk within a
  second). It keeps the element under the focus's node and follows it. The
  chain is kept (`outpost::focus_path`) for the controller relation, a
  dialog's text, and queries. A focus whose element is in no tree, as
  Windows Terminal's tab focuses, is reported all the same.
- A focus with no window of its own whose walk up the listener cut below
  its nearest window, so that no element of its chain has a window of its
  own, is routed from that nearest window, which the read carries
  (`UiaFocusRead::window`), down by every runtime id of the chain
  (`FocusPath::route_to`), which finds it when the chain's outermost
  element is that window's child. A text focus whose chain holds only the
  focus (`FocusPath::is_cut_at`) and that this does not find is read as
  its application's focused element instead
  (`read::focused_element_in`, trigger `read::FocusRead::CutWalk`), with
  the focus read's checks and process-id check, and keeps it when its
  runtime id is the focus's (`Worker::element_after_cut_walk`). It
  finds the element only while the element still has the keyboard focus,
  a limit; a focus without text is not looked for this way.
- While the focus's value is its selected item
  (`UiaFocusRead::value_from_selection`), a selection of one of its children
  (found by one `FindFirstBuildCache`, `verbatim_uia::SelectedItem`) is
  reported as `ValueChanged` on the focus (`Worker::selection_as_value`)
  unless the item has the keyboard focus. NVDA's rules for a
  `Windows.UI.Core` combo box with `Value` and the .NET Windows Forms combo
  box are not built (`docs/parity/changes-and-selection.md`).
- A UIA selection fact's `focus_window` is the window its acceptance is
  tested with. Notifications are not tested, as NVDA tests none, nor are a
  text focus's caret, text, and active text position events.
- An MSAA focus is acquired with NVDA's child-0-on-a-list redirect and the
  `accFocus` redirect (`focus_candidate`), read in full even when it
  repeats the focus, since Core judges repeats, and accepted only when it
  or an ancestor has the focused state, read live. A focus in a window of
  a process the outpost inherits as ignored (`IGNORE_PIDS_ENV`) is not
  reported, and every focus report carries window facts.
  A child's own focus event from the same `SetFocus` as its control's
  redirected one is dropped (`Followed::redirected`).
- The reports (`reducer::held`;
  `books/dev/src/explanation/design/ownership.md`):
  every focus or foreground report whose focus node the outpost has not
  released, added at a program's commit and removed by a release, one
  per node and line. Core's focus line is always among them. With each
  the outpost keeps its MSAA address, whether a simple child, the window in
  front as it was entered, and the text each dialog of its line was
  given. The program reports the evidence (`InFront`: a foreground
  change's window, or any other focus's top-level window when it was read
  as the foreground window), and the rule runs as the report is recorded
  (`Held::entered_in_front`), on the program's copy and in the reducer
  alike: a foreground change sets it to its window, any other focus
  inherits it from the newest report in its top-level window, and
  otherwise it is the read. The two agree at each commit, since only the
  program in flight and a release with no program in flight write the
  reports. The newest is `expected()`. A report of
  the newest report's node whose own evidence says the same element still
  has the focus leaves the states last read as they were. The followed
  focus (`Followed`'s `node`, `role`, `window`, `chain`) is the newest
  report of a control, which the caret, text and terminal output follow.
- MSAA name, value, and state changes are read for the nodes the outpost
  has not released, and Core targets what is spoken
  (`focus_rules::change_target`): name, description, and value for a
  report's focus, state for any node of a report's line or carried by a
  message still waiting (`Outbound::waiting_nodes`). The object is
  identified (`EventObject::which_of`, against
  `focus_rules::change_candidates`) before any read, and only the named
  property is read, with its role for a carried node no line has. A newly expanded
  `SysTreeView32` item carries its child count; a progress bar's value
  change reads role, states, value, and location (`EventObject::progress`).
  A selection on a focus or ancestor is a state change
  (`focus_rules::selection_target`); any other is read only inside a
  focused selection container (`focus_rules::is_selection_container`).
- Ancestors (`read::uia_remote_enrichment`, `read::uia_enrichment`,
  `read::msaa_enrichment`) stop at the previous chain and splice it. A walk that finds no parent, or whose step fails,
  below the root (or below a top-level window, for the remote program and
  `Uia::ancestors`) goes on from the parent window of its outermost
  element's nearest window (`Ancestry::failed_hop`, `AncestorWalk::Failed`,
  `verbatim_ia2::acquire::Walked::Onward`), and one cut by its depth
  limit is completed by plain windows (`AncestorWalk::Cut`, `Walked::Cut`),
  for a UIA walk from the parent window of its outermost element's window
  when that element is the window's own, so the window is not in the
  chain twice (`verbatim_uia::plain_windows_start`);
  a walk that fails before it reads anything, a remote program by UIA's
  transaction timeout among them, or an MSAA node no longer reachable,
  goes on from the parent window of the focus's nearest window, read live
  (`read::onward_after_failure`). The
  listener does not stop its next read at a focus whose walk did not reach
  the top. Object navigation's ancestor query reads the whole chain.
- NVDA's focused-state test of an MSAA focus
  (`FocusCandidate::has_focused_state`) goes on past an `accParent` that
  fails or names nothing from the parent window's own object, chosen by
  `read::parent_window_for_focused_state` (the audit's difference 19): by
  the outpost's verdict, its MSAA client, read and walked up from, or its
  UIA element's focused state, one call; a ghost's or a hung window is a
  plain window, never focused, and the test goes on from its parent
  window; above the process boundary every window is plain, and the test
  ends with no call there.
- A focus-now's UIA ancestry is one remote operation
  (`verbatim_uia_rops::focus_ancestry`): keyboard focus, selected child,
  parents with the base cache, and nearest window, made into a chain by
  `Uia::ancestors_from`. With remote operations off, or for a window whose
  element cannot be imported (a proxy, marked for the window's life), the
  per-hop walk is used; any other failure falls back for that call and is
  logged with the failing instruction's line.
- A menu item's legacy MSAA checked state is one more live call
  (`verbatim_uia::map::with_legacy_checked_state`).
- A UIA navigation step (`read::uia_remote_step`) is one remote operation
  (`verbatim_uia_rops::navigation_step`) from the kept element, but classic
  from a top-level window (a program's walk ends there), with remote
  operations off, or in a window marked classic. A gone element is found
  by the focus's route and stepped from classically, or answered gone.
- A dialog's own text (`read::describe_dialogs`) is given as the
  description of every dialog in the chain no report's line is already
  inside (`focus_rules::already_entered`), and of a dialog focus, without
  one, while descriptions are read; a dialog a report's line is inside
  carries the text that report gave it, with no call, so Core, which
  speaks it when its own line enters the dialog, always has it. A
  foreground report's (`FollowState::dialog`) serves the focus that
  follows. A UIA dialog the listener read, whose element the outpost does
  not hold, is found by the path of the report being built
  (`Worker::focus_path`, before its commit), so it has its text on the
  report's first read.
- A name, description or value change no key places among a report's
  nodes is matched to the followed focus by NVDA's equality against the
  event's own address (`EventObject::is_equal_to`), as NVDA directs a
  change event to the focus object it equals: a client of role window is
  kept under its window object's key, and so is reached only this way.

### Menus

Within a batch, focus and foreground events are handled first and the menu
opening last, as in NVDA's MSAA handler: ignored if a focus already landed on
a menu or menu item, or if its object is not a popup menu, and otherwise
reported as a focus on the popup menu. The end of a menu or of the Alt+Tab
switcher is the listener's (`MenuOrSwitchEnded`); Core then asks the
foreground application for its focus unless a newer focus was applied.

### Window destruction and held objects

- Window destruction: `EVENT_OBJECT_DESTROY` for a window drops its verdict,
  its classic mark, and its MSAA nodes.
- Held objects: the object store (`outpost::store`;
  `books/dev/src/explanation/design/ownership.md`) owns both
  registries, each node's text patterns and anchors, the terminals' kept
  ranges, and every node's and anchor's mint tag, the requests that issued
  or looked it up. Its API returns owned clones and owned removed objects
  (`store::Removed`), never a guard, and takes no closure. Walks issue and
  look up nodes through the request's `store::Nodes` handle
  (`Store::nodes_for(tag)`, `RequestContext::nodes`), which implements the
  interning traits of `verbatim-uia` and `verbatim-ia2` and tags each node
  with the request's tag in the same hold of the store's lock, keeping an
  element a lookup replaced for its holder to drop as the handle drops,
  on the worker; a plain
  window and a foreground's client object read from local data are minted
  by window handle (`Nodes::window`, `Nodes::client`), as their MSAA keys.
  The lines a walk copies are tagged too: the previous focus's chain it may
  splice from (`read::held_prefix`, `Store::held_prefix`), which keeps only
  the entries above the outermost one no longer held, and the reports'
  MSAA lines it compares with (`read::msaa_candidates`,
  `Store::tag_copied`). Every ordinary message is sent as a
  `store::Recorded`, which only the store makes, listing the nodes and
  anchors it names (`OutpostToCore::named`); each message that carries
  node ids (`OutpostToCore::carries_nodes`) takes the next position as it
  is written, and what it names is recorded at it (`outbound::Carried`).
  The app writes `NodesHeld` (the reducer's nodes and text anchors in
  that outpost and the last position handled) when the node set changed
  or after a focus report (`Supervisor::send_nodes_held`); any other
  change waits in the writer's slot (`Supervisor::leave_nodes_held`) for
  the next command, a request or a ping within three seconds. When the
  reducer reaches it, between programs, its shell calls `Store::release`
  with the live requests' tags: under
  the sender's lock and then the store's, it frees a node or anchor only
  when none of its tags is live and either no message carried it or one
  was written at or before that position, none waits that carries it, and
  Core does not hold it; with it go the node's text and terminal entry,
  and the reducer is given the released nodes and anchors before it
  dispatches any program, dropping their reports, focus-following groups
  and text facts, so no program is given a vanishing node; it then has the
  store let go of the freed anchors' UIA ranges (`Effect::ForgetRanges`).
  The objects are dropped by a release
  program on a worker, under a deadline, never on the reducer's thread. A
  never-carried node of an ended request or an abandoned generation goes
  at the next release. A released node answers `Gone`, a forgotten anchor
  `AnchorLost`. Only the newest list is kept.
- Text on copies: each node's text facts are the reducer's
  (`reducer::text`, "Text and caret"), and a program reads them in its
  copies. A text call copies the part its operation names
  (`NodeAnchors::part`), clones the UIA ranges that part names from the
  program's own or from the store in one hold (`Store::ranges`), and works
  on the copy with no lock held (`text_reads::uia_call`, `edit_call`).
  What it changed is applied to the program's copies at once and carried,
  with the ranges it made under new keys and the anchors it minted or
  reported, to the program's next commit (`text_reads::put_uia`,
  `put_edit`, `ProgramText`); a program that ends with text writes no
  commit carried commits them alone. The shell makes them only for a
  commit the reducer accepts (`Store::write_text`): the ranges are kept,
  the anchors tagged with the commit's request, and the facts' change
  given to the reducer, but a change that adds to a node the store has
  released is not made. A refused commit's writes, with the terminal entry
  it made, are dropped by a worker.
  The other text writes, the patterns and what a text supports, a
  forgotten answer of no pattern, and a node's text forgotten for a new
  element, ride the same commits. A destroyed window's nodes have their
  text facts forgotten with the program's next commit.

### What to read

`Fetches` carries the details the theme wants (`SrState::fetches`;
[verbatim-model](verbatim-model.md), "Themes"), sent at start and on theme
changes; until it arrives everything is read. UIA requests and remote
fills are built from the gates of the `Node` and `Ancestor` shapes
(`books/dev/src/explanation/design/uia-shapes.md`), one function per shape and use (`gates.rs`):
a node's place only for an element read alone (`node_gates`,
`focus_gates`, not `many_node_gates` or `ancestor_gates`, decision 9), and
an ancestor's description always (decision 4). MSAA reads skip what is off.
The subscriptions' cache requests are fixed, whatever the theme, so the
details of a listener's selection that the theme does not report are taken
out of its report (`gates::leave_out_unreported`). Text formatting is
read only for indications that are on and attributes the control supports,
learned from its answers and kept per node (`TextSupport`); spelling and
grammar annotation types are never learned, since Notepad answers "not
supported" without annotations, so the caret read asks a line first
(`books/dev/src/reference/crates/verbatim-uia-rops.md`, "The caret read"). `IsHidden` is never
read.

### Text and caret

`Query::Text` is answered `QueryResult::Text`, `NoText` and `Gone` among
the answers, within two seconds (`text`, the worker's `text_reads`).

- A UIA node has text when its element has a text pattern (`TextPattern2`
  where offered), fetched once and kept with the node. `NoText` is kept only
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
  and the formatting to be spoken: for the report after a focus and a caret
  key's report (not a paragraph or page), never a typed character's. It
  travels as `FormatRun`s: bold at weight 700 or more, underline and
  strikethrough as `LineStyle`, size as "11.0 pt", colors by NVDA's named
  hues (`text/color.rs`, ported from NVDA's `colors.py`), `BulletStyle`,
  and links.

A caret key is read and reported for Core, which alone judges it
(`docs/design/caret-selection-rework.md`):

- A key's reads come from a note on its node (`reducer::KeyNote`), set as a
  before read's request (`TextOp::ReadCaret` with a before-key `ReadPurpose`)
  or a one-way `CoreToOutpost::KeyNote` arrives and ended by the clock, the
  key's final read or Verbatim's own move; it knows nothing of the key's
  progress. A note past its end counts as absent where it is read.
- While a note lives, each caret event and text change of the node is read
  with one read (`text_reads::note_read`, `text::check_key`), which fetches
  the key's unit, compares with the node's last read and reports it as
  `CaretMoved` carrying `ReadFacts`. A read showing the key's effect
  narrows the note to `CaretOnly`, which ends the extra reads of text
  changes while caret-event reads keep their facts until the note ends. A
  terminal's read under a note reports only an effect, and its own reading
  rules say the rest.
- The final read, `ReadPurpose::Final`, answers Core with the same
  comparison. Core reports nothing for a key's progress: it passes the key
  on, counts its bound, and judges each report's facts.
- The console host's caret poll (`TerminalReads::Poll`; a documented workaround,
  `books/dev/src/explanation/design/key-holding.md`, "The console host's
  caret poll"): the console host raises caret events only on its cursor's
  blink timer, so in its focused text area the note's reads run every 5 ms
  for 50 ms from when the before read ended or the note arrived, until a
  read shows the key's effect.


Selections no key made (`docs/parity/text.md`, "TextInfo-equivalent
layer"):

- Each node keeps its one last read (`LastRead`): the caret, its line and
  the selection with its text, and when the read began, but a terminal's,
  which keeps no selection. The reducer holds it among the node's text
  facts (`reducer::NodeAnchors`, in `reducer::TextFacts`), with its
  anchors, the positions reported, and the note that the text changed. A position is an edit control's offset, or the
  key under which the store keeps a UIA range, so the facts hold no
  application object and an edit control's offsets cross between programs
  as they are. They change only by a program's commits
  (`reducer::TextChange`: a text call's part put back, but for an anchor
  it copied that a release freed meanwhile; a selection forgotten; a value
  change noted; a node's text forgotten), and the reducer has the store
  let go of each UIA range no fact names any more
  (`Effect::ForgetRanges`). A caret event's report of a focus
  that is not a terminal (`text_reads::CaretFor::Event`) compares with it and sends `selection_changes`; through UIA in
  the same round trip, comparing nothing when both are empty
  (`CaretRequest::previous_collapsed`). It is forgotten at a new focus, and
  replaced by what the outpost made when it selects or moves the caret for
  Core (`NodeText::keep_selection`, `RangeRead::selection_after`). Each
  change is compared once, by whichever read sees it first.
- A report's selection starts where Core can read before it: in the caret's
  line, or an earlier line sent as `CaretSelection::start_line`; through
  UIA, where the caret can be the selection's other end, the caret read
  finds the start (`CaretRead::selection_start`).
- A comparison is in general terms (`text::content_changed`,
  `text::general_changes`: ends only, `content_changed`, and the new
  selection's text in `new_selection`) when the node's note is set (NVDA's
  `hasContentChangedSinceLastSelection`, set by a classic edit control's
  value change, `text_reads::note_value_change`, and cleared by each
  comparison and by a caret event of the control that a caret read made
  after it already covered, as NVDA's comparison at a caret event that
  moved the selection would,
  `text_reads::note_caret_event_counted`); when the kept selection's
  text no longer matches what its ends hold (through UIA,
  `CaretRequest::read_selection_text`, `KeptSelection::text`,
  `CaretRequest::read_previous_text`); or when the control's value change
  waits once the caret is read, for the Windows Forms text box. That check
  is the reducer's, so the read is cut in two programs there
  (`text::Checked::Cut`, `text::caret_report_checking`,
  `text::check_key_cut`), only for an edit control's client area with a
  selection in the compared one or the new one: the read part syncs with
  the `WinEvent` thread (`text_reads::value_change_cut`, `Nudger::sync`,
  which raises a `verbatim_ia2::SyncMarker` event, since a posted message
  can overtake a `WinEvent`) and ends, carrying its offsets, the caret's
  line if read, the selection compared with, the text's length if read
  (`EditText::length_read`), the entry's timing and its calls not yet sent
  (`reducer::Cut`) with its program's end. Every value change raised
  before the sync is then ahead of that end in the mailbox; the reducer
  looks for one of the control's client area among its entries not yet
  dispatched (`Intake::value_change_waiting`, `Hold::held`) and dispatches
  the finish at once (`Work::Finish`, `Worker::finish`), under the read
  part's deadline and query, which makes the comparison and the report or
  the answer (`text::finish_report`, `text::finish_answer`) with the read
  part's calls in its message. So "x" typed over a selection in Notepad
  says "selection removed" where NVDA says "x unselected". What the design
  weighed, and what may come between the two parts, is in
  [its design](../../explanation/design/ownership.md).
- Two waiting caret reports merge with both reports' changes in order
  (`Merge for OutpostToCore` in `verbatim_model::protocol`); one with
  changes never merges across a reply or with a report in general terms.
  A merged change of states that leaves the node expanded keeps the older's
  child count when the newer read none, and a merged focus report keeps
  the stronger evidence of the element held before (`earlier`).

Caret and text events:

- A focus that may have text, or has an edit field, document, or terminal
  role, has its caret read before it is reported (`Worker::read_focus_text`)
  and carried in `FocusChanged`'s `text` (`docs/nvda/speech.md`, "What an
  object with text says"), with the selected text when something is
  selected. No text gives `FocusText::NoText` (Core speaks the value);
  unreadable text gives `FocusText::NotRead` (Core speaks neither). A caret
  read that hangs holds the report until the deadline.
- `Text_TextSelectionChanged` is `CaretMoved`, `Text_TextChanged` is
  `TextChanged`, and the active text position change is
  `ActiveTextPositionChanged`, its range's start kept as an anchor without a
  call, dropped for an unknown element or no range. An edit control's caret
  (`EVENT_OBJECT_LOCATIONCHANGE` or `EVENT_OBJECT_SHOW` on `OBJID_CARET`)
  and text selection events are `CaretMoved`, its value change
  `TextChanged`.
- For a UIA focus, the system caret's events are its caret events
  (`Worker::system_caret_of_uia_focus`), since some UIA documents raise
  none, until the focus raises a UIA caret event of its own, or from the
  start in a console host terminal; then the intake drops them
  (`Intake::drop_system_caret`) until the next focus, edit controls' caret
  events with them. NVDA's IAccessible2, Word, and Scintilla exceptions are
  not built (`docs/parity/text.md`).
- The intake keeps one caret report per node and drops a caret event older
  than the node's last caret read. A held terminal's caret event is noted
  (`Item::NotedCaret`, handed out by `Intake::when_idle`), and the console
  host's updates for it are dropped as pushed
  (`FollowState::absorbed`) while no watch is open, the mark
  cleared before a terminal request, a caret key, or a new focus.
- A focus-now answer's control has its caret read with it, is recorded
  among the reports (`reducer::held`), as NVDA's fake focus is, and then
  becomes the followed focus (`Worker::follow_answer`, as `emit_focus`
  does): another node's watch ends, the intake is told through the
  follow change it commits, and the tracking takes it, unless it is
  already followed under the same node, which is followed again, not
  adopted. Verbatim+Tab's answer (`FocusPurpose::Report`), which Core
  only speaks, is none of these: its caret is not read, Core asking for
  its line, and it is neither recorded, adopted nor followed, so the
  followed chain and the events judged against it are unchanged.

### Queries

- `ShellItems` (`outpost/shell_items.rs`) reads the notification area
  (`TrayNotifyWnd`, with the overflow flyout) or the taskbar
  (`Shell_TrayWnd` without it) in the control view as `ShellButton`s, and
  answers every named, on-screen button as a `ShellItem` (name and runtime
  id). `ShellItemPosition { kind, item }` walks the same way and answers
  `ShellCandidates` with each matching button's `Place` and whether every
  button was read; Core picks (`verbatim-gui`'s `refind`). Both read only
  their application's windows, look the shell windows up without the proxy
  rule (`any_window_element`), have no time budget of their own, and read
  a failed step as no more items, logged as a warning. The reducer
  abandons either query at the `answer_by_us` it carries, Core's deadline
  for the list dialog's wait less its reply margin, fixed when the dialog
  asked, and answers one it reaches at or after its `answer_by_us` with
  nothing, never giving it to the worker: a slow taskbar's whole list is
  shown once it answers, and one that does not finish in time gives none.
- `DumpTree { locations }` walks the foreground window (or the first
  visible top-level window): UIA with `verbatim_uia_rops::tree_dump`, MSAA
  with `verbatim_ia2::acquire::walk_tree`, capped at depth 64 and 4096
  nodes, saying whether incomplete. Locations are read only with
  `locations`, since a title bar's are asked of the UI thread
  (`WM_GETTITLEBARINFOEX`).
- `ReadNode` reads a node live for a review command, its table's row and
  column counts included (`Node`'s gated `GridCounts`); a dead element
  answers `Gone`. A UI Automation node that is dead or gone answers `Gone`
  to `Navigate` too, except that `Parent` answers the parent of the
  element's window (NVDA's fall back, `known_window_of`): the nearest
  window recorded in the store when the node was built, and its parent
  read by the object `window_object` chooses as NVDA does, the desktop
  carried as `NodeDetails::desktop_window` for Core to name.
- `Ancestors`, `Navigate`, and `Activate` go to the issuing registry: UIA's
  raw-view walker with the base cache, or MSAA's `accParent`,
  `accNavigate`, and `accDoDefaultAction`. An unknown or unreachable node
  answers `Gone`.

### Selection and notification events

Selections emit `NormalizedEvent::SelectionChanged` and UIA notifications
`NormalizedEvent::Notification`. A UIA selection of an element the
focus names in its ControllerFor relation, or inside one, emits
`NormalizedEvent::ControlledSelection` with the focus's id, as NVDA reports
a search suggestion; the relation and the search are one remote operation
(`verbatim_uia_rops::controlled_selection`). A UIA selection or
notification goes through its application's reading thread behind a marker
(`DeliveredFact::UiaMarked`), which finds a non-window sender's nearest
window; one with none is dropped, but for the shell's window-snap
notifications. A waiting notification is replaced by a newer one from the
same element with the same kind, processing, and activity id, as NVDA's
UIA limiter keys it.

A Windows toast (`DeliveredFact::UiaToast`) goes through the same reading
thread and is reported as `NormalizedEvent::Alert` with
`AlertOrigin::ToastWindowOpened`, its identity a hash of its runtime id
computed here, for Core's repeat rule (`docs/parity/alerts.md`, "Toasts").
MSAA alert and show events from a native UIA window are dropped by the
arbitrator's verdict for the window.

### The foreground window's progress bars

Core says where attention is (`CoreToOutpost::Front`), and the reducer's
`BarWatch` follows it, one value ended with its generation: none, finding
the window's element, or subscribed. Its table is in
`docs/parity/changes-and-selection.md`, "Progress bars that are not the
focus". `Work::FindBar` is one call on a worker (the window's element, by
`Uia::any_window_element`); a commit's `bar_found` gives the element a token
in the shell, and a program's ending without one, or its abandonment, is the
lookup's failure. The window's element is followed with
`Group::RangeValue` over its subtree, holding only the range value change;
the callback drops every role but progress bar from the event's cache, and
a progress bar's change takes the property path, reporting
`ProgressChanged` after one read of its place on the screen. Its token is
the shell's one token for the element (`Elements::token`), and
`follows` includes it, so neither subscription drops the other's.

The ancestry cache ([verbatim-core](verbatim-core.md), "The ancestry
cache"). The followed chain is the outpost's ancestry cache, saved with the
window text of each window entry (`Followed::titles`); `Worker::focus_chain`
gives a walk nothing to stop at when a saved title reads otherwise now, or
when the read answers a live re-read. Every join to the cache,
`read::join_cache`, notes the entries it took
(`RequestContext::note_cached`), and so do the listener's cached facts;
`emit_focus` computes the report's `first_cached_ancestor` from the
entries it emits, after its filters, by its `FocusSource`. A listener read
carrying `answers` keeps its report's verdict and joins no cache; an MSAA
live re-read (`CoreToOutpost::LiveReread`, planned as
`DeliveredFact::MsaaLiveReread`) reacquires the object at the last MSAA
report's event address (`Origin::Msaa`). The reducer records the
live re-read each program's entry answers (`Current::live_reread`), cleared
by the commit that sends its report or the listener's `UiaLiveRereadGone`;
one the program ends, panics or is abandoned without answering, its object
gone or its report dropped, is answered `OutpostToCore::LiveRereadGone`
there, the one answer path. A newer focus of the same element never
replaces a waiting answer in the intake. A foreground change makes its window the window every
older report in it entered in front (`Held::reported`), and the
selection-as-value mark is each held report's own
(`Held::value_from_selection`).

## Messages to Core

The thread that sends a message writes it itself (`outpost::outbound`,
through the transport's `Sender`; `docs/design/communication.md`, section
4.3): at once with one `WriteFile` when no write is in flight, otherwise
into the sender's backlog, which the I/O thread writes as writes complete.
Nothing that sends ever waits, so the reducer can always abandon a worker.

- Pongs, `Ready`, `TargetExited` and `SpareReady` go ahead of everything
  waiting: `Outbound::urgent` takes only an `Urgent`, which carries no
  node, so nothing that carries nodes skips their numbering.
- Waiting events merge as `Merge for OutpostToCore` declares, placed by
  `merge_into` among the messages since the last flush marker: by node and
  kind (the same property, or the same kind of focus), the newer at the
  back, but not past a caret report with selection changes that has a
  reply after it, and never a terminal's read.
- Replies, notifications, and faults never merge and keep their order.
- Nothing merges across a flush (`Outpost::settle`'s) or an idle answer, and
  a flush is answered once every earlier write has completed.
- What waits is bounded by the nodes and kinds, Core's requests, and the
  intake's limits. Messages carrying node ids are numbered as written
  (`Pending`).

The supervisor and the listener never wait to send either.

## The idle barrier

The harness's `AwaitIdle` (`verbatim-app`'s `idle` module;
`docs/design/communication.md`, section 6.7) is answered after a round in
which every child said nothing that came before the question was in
progress. The round is in band, so it covers pipes and backlogs.

- `Supervisor::query_idle` posts the round to the relay, which writes
  `IdleQuery` to the listener behind every earlier command; a missing or
  ending listener counts as having answered.
- The listener passes it through its `WinEvent` thread (`Nudger::nudge`) to
  the reading threads (`Readers::when_idle`), which answer once no event
  queued before it is waiting or being read, behind a menu end still in
  its grace and every earlier fact.
- The relay then asks every outpost in the same pass, telling the app which
  first (`IdleQueried`); outposts being launched are asked once every launch
  has reported, a starting outpost behind its held facts. The query gets
  through a full queue.
- An outpost queues it (`Item::Barrier` with `Asker::Round`), dividing what waits and ending
  its batch; a noted caret goes just ahead (`Intake::push_ahead`), carrying
  its event's timing, so its read is skipped only when a caret read since
  that event covers it. The reducer reaches the query with no program in
  flight, so nothing before it is unhandled, and answers it
  (`Reducer::answer_idle`) as it is reached and after every input, once the
  hook's copies are no longer held at the outpost's start, no
  caret key's watch is open or held by the program in flight. Entries after the query are not waited for, nor is a
  program handling one. An outpost that ends answers by its `Ended`.

## Shutting down

Every child ends by a clean shutdown, the kill being only the fallback
(`books/dev/src/explanation/architecture.md`, section 1, gives the reason): at Verbatim's exit
(`Supervisor::shutdown`, run while the exit sound plays, after the keyboard
hook is removed; no child starts afterwards), when a child is replaced, and
when an outpost is retired or its application exits.

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
terminal and sends each read to Core, whose terminal model aligns it and
applies the reading rules (`books/dev/src/explanation/design/terminal-model.md`, "Where it runs";
[verbatim-core](verbatim-core.md), "Core's terminal model"); the outpost
judges nothing a read says. A UIA element of class `TermControl` (Windows
Terminal) or `WPFTermControl` (Visual Studio), or a focus in a
`ConsoleWindowClass` window, is `Role::Terminal`, never by title. The
console host's text area is read through UIA; inside a remote operation its
provider gives no handle for the window, which the classic walk finds.

- Per terminal node the worker keeps a `Terminal`: the next read's start
  (`Snapshot::next_start`) and whether its screen showed text, the screen's
  size as Core told it (`TextOp::TerminalShape`, `Terminal::shape`), whether
  colours are due, the reading state (`terminal::reading`), the last read's
  times, the caret last sent, and a hash of the last read sent.
- A read is one snapshot (`terminal::read`, through
  `verbatim_uia_rops::terminal_snapshot`, classic fallback behind it): text
  from a screen above the start to the end in one `GetText`, the screen,
  the caret and its line, and colours when due, in one remote operation.
  Core may ask for the whole document or a first read's history block
  (`TextOp::TerminalReadNow`). `terminal::snapshot_of` makes Core's
  `TerminalSnapshot`: UTF-8 text without line padding, each line's padded
  length (one pass, `terminal::unpadded_utf16`, into new buffers),
  the caret's row and column, colours from `verbatim_uia_rops::ColourRun`,
  and when the read began. The start is kept unless the screen shows no text
  where the last kept read's did (`verbatim_text::terminal_rows::shows_text`).
  The caret read is a caret report inside the snapshot
  (`text_reads::terminal_read`).
- A live read is sent as `NormalizedEvent::TerminalSnapshot` unless
  identical to the last sent but for times and caret (`Terminal::sends`), a
  read that waited for Core's queue excepted; an answer to Core is always
  sent (`TextReply::TerminalSnapshot`). Reads never merge while waiting,
  since each starts where the last said, and a whole document goes as one
  message.
- Core asks for colours with every key that reaches the terminal
  (`TextOp::TerminalColours { from_ms, until_ms }`); reads beginning by the
  latest end asked take them (`Terminal::colours`), and if the last read
  ended after the key without them, one is read at once. Each read carries
  its end (`ended_ms`).
- Keys and reading settings are Core's. A held caret key's before read is,
  in a terminal, a snapshot whose caret answers it; the watch's evidence
  then reads the terminal, and its answer says nothing.
- The console host's console events (`EVENT_CONSOLE_UPDATE_REGION`,
  `_SIMPLE`, `_SCROLL`, `EVENT_CONSOLE_LAYOUT`, merged per window) are its
  text changes, and its `EVENT_CONSOLE_CARET` reads the terminal too;
  Windows Terminal's UIA text changes and caret events read it. A change
  observed before the last read began is covered (`Terminal::covers`, in
  microseconds; `ReadAnswer::started_us` is the read's start, which the
  snapshot's event is stamped with). Reads on Windows Terminal's events
  never wait for a frame; the before read of a held key, prompted by no event, can
  cost that key one software-rendered frame. Windows Terminal raises no change for writes of
  blank lines only, so such a scroll is read at the next event.
- When a terminal's focus is reported, the subscription moves to it and the
  worker waits for the move (`Registration::settle`) before the first read,
  which is sent even if identical (`Terminal::focused`).
- On-demand reading (`terminal::reading`, a pure transition function): live
  by default; `TextOp::TerminalHold` only notes changes;
  `TextOp::TerminalRead { hold }` answers with a read taken as one that
  waited, or `TextReply::TerminalUnchanged`; `TextOp::TerminalCancel` goes
  live and reads only if the text changed; `TerminalReadNow(Fresh)` reads
  whatever the state. Each read is answered from one locked state of the
  buffer.
- History: `TextOp::TerminalHistory { above, whole }` reads rows above
  Core's memory, padding kept, answered `TextReply::TerminalHistory` with
  the rows above the start and the read number whose start it is
  (`terminal::history`, `History`); `TextOp::TerminalFind`
  (`terminal::find`, `verbatim_uia_rops::terminal_find`) finds a command
  mark's line, answered `TextReply::TerminalFound`, and Core picks the
  match. Reads are numbered (`TerminalSnapshot::read`); an unsent read's
  number is the last sent one's.
- Each snapshot and read logs its cost at debug (`terminal snapshot
  timing`, `terminal read timing`).
- The listener ignores every UIA notification from a terminal
  (`verbatim_uia::map::ignores_notifications`), as NVDA's terminal overlays
  do, or output would be spoken twice.
- Tests: `terminal/reading.rs` covers every reading state; `terminal/tests.rs`
  drives `Terminal::sends` over `verbatim-term-sim` and checks the one-pass
  conversion, with an ignored `measure_conversion` that times the
  conversion, the read's event written as the outpost writes it, and
  Core's parse of that frame, in a release build (`books/dev/src/reference/performance.md`,
  "A terminal's reads in Core"); mockapp's `terminal` and
  `terminal_reading` tests drive a real outpost.
