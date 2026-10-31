# verbatim-outpost

The per-application outpost process, the focus listener, the Core-side
supervisor, and the private protocol between them: one outpost per
application, many at once, with a dedicated listener that detects focus and
the outposts announcing it. The reasons for this shape are in
`docs/architecture.md`, section 1 ("Outposts", "The focus listener (D13)")
and in `docs/design/focus-pipeline.md`.

An outpost's application is set once: by `--target-pid` on its command
line, or, for a spare, by the supervisor's `Assign`. Its `WinEvent` hook
installs once, when it has its application, its UIA registration once
during construction, and there is no rebind path. Outposts stay alive when
their application loses the foreground.

## How focus reaches an outpost

The focus listener is `verbatim-outpost.exe --listener`, with no target. It
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
(`Outgoing::uia_event_process` in `crates/verbatim-outpost/src/listener.rs`),
as NVDA drops events before its global handlers are registered; Verbatim's
start asks the foreground application's outpost for its focus instead
(`ShellCommand::FocusNow`), and a `WinEvent` focus whose UIA focus is so
dropped is read by its outpost at the gap record's bound when the outpost
judges the copy's window UIA (below). The installation's end is logged
at info, with the time it took; the shutdown waits for it.

### The hand-over of an application's MSAA events

An outpost's own process-scoped hook takes in all its application's MSAA
events, focus, foreground, menu-popup, alert, and tooltip included, so they
reach it by one route in the order Windows raised them
(`docs/design/focus-pipeline.md`, section 12.4). The listener forwards
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
  `docs/design/ownership-outpost-reducer.md`, "The hand-over's hold"). At
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
- When a ready outpost ends, the supervisor sends the listener
  `HookRemoved`, and the listener forwards that application's events again.
  A replacement listener gets `--hooks-in`, forwards none of those
  applications' events, and first sends each such outpost a
  `ForwardingEnded`. Messages for the listener while it launches are held
  in order.
- A console window's events reach the console host's outpost by its own
  hook, which Windows scopes by the event's thread.

### A UIA focus, judged and read by the listener

A UIA element cannot be handed to another process, so the listener, which
receives the event's own element, judges and reads a UIA focus
(`docs/design/focus-pipeline.md`, sections 12, 16, and 18;
`docs/design/listener-without-connections.md`, sections 2 and 3).

- Shape. The registration and group are shaped as
  `verbatim_uia::ListenerEvent` (`Part::shaped`): NVDA's base cache request
  with `IsDialog` and `IsTogglePatternAvailable` added
  (`docs/design/uia-shapes.md`, decision 6), so the role is refined as
  every other read refines it. `ListenerEvent::parts` gives the role, name,
  and focused state. The element-selected part alone is shaped as
  `verbatim_uia::ListenerSelection`, which adds the item's value, states,
  and every detail but its rectangle, what Core speaks of an item selected
  in a focused list, so the selection is reported with no call (Dickson,
  2026-10-29).
- The callback makes no call. It sends the application a marker
  (`DeliveredFact::UiaFocusMarker`) and queues the element as an
  `Agile<ListenerEvent>` for the application's reading thread
  (`focus_read::Readers`): one per application, created on its first event,
  ended once its queue is read after `HookRemoved`
  (`Readers::outpost_ended`) or at shutdown.
- The thread judges and reads each event in order
  (`focus_read::FocusReader::read`; `check_fact` for a selection or
  notification). A hung window or the console window's own element is
  dropped before any call; so is a focus whose nearest window's class is on
  NVDA's MSAA class list (`focus_read::MSAA_CLASS`), by the class verdict
  every outpost's arbitrator gives (`arbitration::class_verdict`), unless a
  backend override forcing UIA holds for its application
  (`SetBackendOverride` with its `pid`, `FocusReader::forces_uia`); the
  listener makes no other backend verdict, no probe and no console check,
  which are its outpost's. A shell window that only stages a switch is read
  and Core drops it. The listener makes no duplicate test and keeps no
  judgement of Core's (`docs/design/ownership-one-judge.md`, "The
  listener"): a focus that passes the other checks is read in full, and
  Core judges a repeat. A menu opening is compared (`CompareElements`) with
  the reading thread's own last read (`LastFocus`) while that read is
  current for it: nothing moved away from the application since that
  read's event was queued (`read_schedule::QueuePlace::current_since`,
  the `Arrival`'s `place` and `left_at`), moving away being another
  application's UIA focus or menu opening queued, another process's MSAA
  focus forwarded by the `WinEvent` thread (`Readers::msaa_focus`), or a
  foreground event of another top-level window than the last read's, or
  than the read under way once it knows its window, one during a read
  whose window is unknown counting (`Readers::foreground_changed`). An
  opening UI Automation equates with it is judged as that focus, with its
  window; a failed comparison drops the opening. Then the acceptance test
  against the foreground window (`outpost::acceptance`, shared with the
  outposts) on the element's own or nearest window
  (`verbatim_uia::nearest_window_handle`); the outpost does not test a UIA
  focus again.
- The read (`verbatim_uia_rops::focus_ancestry_classic`) makes every call
  on an element the listener holds, with no remote operation and no
  connection: whether the element with the same runtime id as the previous
  focus still has the keyboard focus, has lost it, or is gone
  (`UiaFocusRead::same_as_last`, a `verbatim_model::EarlierElement`); value,
  states, and details afresh; a list's or tab control's selected child, or
  for a combo box without `Value` or a looping selector the first selected
  item as its value (`UiaFocusRead::value_from_selection`), left out when
  its read times out or finds it gone; the raw-view
  ancestors with their window handles, up to the first the thread knew or
  the desktop root, never across the process boundary
  (`verbatim_uia::parent_across_process_boundary`); and a menu item's
  legacy MSAA state. Where the walk ends below the top it names where the
  chain goes on (`UiaFocusRead::ancestors_end`, a `protocol::AncestorsEnd`),
  since it connects to no window: after a step to a parent that fails or
  finds none, the parent window of its outermost element's nearest window
  (`ContinueFrom`, `verbatim_uia::outermost_nearest_window`), as NVDA's
  parent read falls back to the window hierarchy; after the walk's bound
  or its depth limit of 64, that nearest window, from which the chain is
  completed by plain windows (`PlainFrom`).
  Each read carries its walk's bound (`UiaFocusRead::bound_us`), which
  the outpost's continuation of the chain ends by. It reuses, from where a later
  read meets it, only a chain that reached the top. A read that fails
  reports the focus from its event, with the ancestors read before the
  failure and plain windows above them (`PlainFrom`), from the parent
  window of the focus's own window when it is one and nothing was read.
  A test can be told
  each call the classic walk is about to make
  (`FocusReader::observe_calls`, a `CallObserver`), to change the
  application between two of them. A step that reaches a ghost's or a
  hung application's window ends the walk there, named to go on from
  (`ContinueFrom`). Each read is logged at debug with the sender's runtime
  id, class, framework (read only with debug logging on), and window.
- The read (`DeliveredFact::UiaFocusRead`, whose `focus` says focus or menu
  opening) or the drop (`DeliveredFact::UiaFocusDropped`, whose `focus`
  says the same) follows the marker through verbatim.exe, never coalesced
  or dropped. Every drop carries its answer to its outpost's gap record
  (`answer`, a `protocol::GapAnswer` from `focus_read::gap_answer`), placed
  in the top-level window of the window its read found, the cached window
  or the nearest window (`Reading::outcome`'s `placed`, one local
  `GetAncestor`), or in none when it found neither. A focus whose nearest
  window's lookup failed with `UIA_E_TIMEOUT` is dropped with no other
  call, its reason chosen by `focus_read::nearest_timeout_reason`: after
  `focus_read::NEAREST_FAILED_FAST_BOUND` (a second) or more,
  as timed out (`focus_read::NEAREST_TIMED_OUT`), which answers the gap
  record in any window; sooner, as failing fast
  (`focus_read::NEAREST_FAILED_FAST`), which leaves the record open, so
  the outpost's gap read at the bound reads the focus. An application
  whose lookups keep failing fast has every focus spoken through the gap,
  up to a second late, and the focuses between two gap reads not spoken.
- Budget and bound. NVDA's checks have `READ_BUDGET` (2 seconds) from
  the read's start, asked before each call after the first (`Budget`).
  Past it the read makes no calls but still makes every check that needs
  none, the class-list skip among them, taking the keyboard focus from the
  cached `HasKeyboardFocus` (also when a live read fails by UIA's
  transaction timeout); a menu opening's comparison is skipped. The walk
  after the checks has its bound, measured
  from the event's observation (`Budget::bound_walk`, the time the event
  waited counted by the reader's observation clock,
  `FocusReader::set_observation_clock`): `MOVED_ON_GRACE` (500 ms) when
  the focus's top-level window is not that of the previous focus the
  listener observed, which the reading thread places in order
  (`FocusReader::last_top`) and which is always elsewhere after another
  application's focus (the schedule's `last_focus`), and `READ_BUDGET`
  otherwise. A walk past its bound is reported by `read_at_bound`: the
  focus as read so far, the ancestors read, and `PlainFrom` the outermost
  one's nearest window. A call over a second is logged at info
  (`FocusQuery::before_call`).
- Publication at the bound. Once a read has passed every check, its
  thread shares its progress with the watchdog (`ReadProgress` in
  `WatchedRead`: the event's cached facts, the facts read so far through
  `FocusQuery::progress`, the ancestors read, the nearest window and the
  bound) and sets the watchdog's alarm to the bound (`on_arm`). A read
  whose call is still under way at the bound is published by the watchdog
  from that progress through the same `read_at_bound`, its chain
  completed by plain windows by local calls on the watchdog's thread
  (`WatchedRead::prepare_publish`), and claimed (`WatchedRead::claim`) as
  the schedule decides, so its thread's late result is discarded when it
  returns. The thread is not abandoned for this: it
  reads its application's next event when its call returns, is abandoned
  only at `READ_DEADLINE` or by the moved-on rule, and so counts against
  `ABANDONED_LIMIT` only then, when no second outcome is sent. A read
  still in its checks at the bound is left to those rules. A menu item's
  legacy MSAA state, read after the walk, is published as the walk ended
  should it be the call under way (`ReadProgress::finished`). A published
  read is no longer a pending focus for menu openings, nor in progress for
  the idle barrier. `ReadingThreads` runs the reading threads and their
  watchdog in a test's process, outcomes sent to a channel, so a mockapp
  test can watch the watchdog meet the bound.
- Watchdog. A read past `READ_DEADLINE`, 8 seconds (the budget, the
  5-second transaction timeout, and a second), is abandoned;
  the timeout bounds each provider call, not an operation
  (`docs/uia-bugs.md`). A focus's or menu opening's read is also abandoned
  after `MOVED_ON_GRACE` (half a second) once the window in front belongs
  to its application on another UI thread
  (`outpost::window::front_is_another_thread_of_its_application`, by
  provider process, `verbatim_uia::provider_process`), as NVDA's watchdog
  cancels a slow call. The window watched (`focus_read::WatchedRead`) is the
  cached handle or the nearest window once found (`WatchedRead::learn`), and
  the front is judged only with evidence: a foreground event from the
  listener's hook (`Readers::foreground_changed`) or the front already
  elsewhere when the window became known (`WatchedRead::judge_front`). An
  abandoned read is claimed, the event is reported dropped, and a new
  thread takes later events; when its thread returns, the schedule answers
  `ForgetLast` and the thread forgets its last read
  (`Reading::forget_last`, `FocusReader::forget_last`), a focus Core never
  got, which no later read compares with or reuses. With `ABANDONED_LIMIT`
  (two) threads still in calls the application gets no new thread and its
  events wait, and the first that returns reads them; `READ_DEADLINE` after
  that every waiting and later event is dropped until a thread returns.
  The watchdog sleeps on an alarm (`outpost::alarm::Alarm`, as an
  outpost's reducer does): a read sets its timer as it starts, to its
  deadline, or to the end of its grace when a foreground event came while
  it waited, and again to its bound once it has passed its checks, each
  only when no sooner time is set (`alarm_by`); a read that returns with
  none other under way cancels it. A read whose window is known from the
  event's cache has the window in front judged as it starts, on its own
  thread (`WatchedRead::known`). So a read that returns before its bound
  wakes the watchdog not at all.
- The schedule (`read_schedule`). Every decision above of which thread
  reads which event, when a read is abandoned or published, which waiting
  events are dropped, which idle waiters run, and where each application
  was last moved away from, is the pure `Schedule::step`, an input in
  (`ScheduleInput`: `Push`, `Next`, `Returned`, `Tick`, `Foreground`,
  `MsaaFocus`, `OutpostEnded`, `WhenIdle`, `SpawnFailed`, `Stop`) and its
  effects out, in order (`ScheduleEffect`: `Outcome`, `Spawn`, `Wake`,
  `Claim`, `ForgetLast`, `Alarm`, `RunIdle`, `Job`), with time as
  microseconds from the shell's start and no clock, lock or call of its
  own. `focus_read::Readers` is its shell: it keeps the readers' lock, the
  threads and the condition variable each waits on, the watchdog's alarm,
  the idle waiters' closures and the reads under way (`WatchedRead`), and
  applies each step's effects under the lock. A tick (`TickRead`) carries
  what the shell found of each read: its window, the front's judgement,
  whether the user moved on, asked only when `Schedule::moved_on_wanted`
  says so, and the read as it would be published once its bound has
  passed; an answer for a read whose window changed since judges nothing
  and has the watchdog look again at once. Each event's marker gets
  exactly one answer: `Readers::push` takes the answer owed for it
  (`Reading::Owed`, the listener's `MarkerOwed`, a
  `verbatim_model::protocol::Responder`), which the shell keeps by marker
  beside the schedule, never in it, and the event's outcome consumes; one
  dropped unanswered, as an event still waiting when the readers stop is,
  sends the marker's drop, leaving the gap record open. Debug builds
  assert after every step that the owed markers are exactly those the
  schedule holds (`Schedule::markers`). The menu end's grace is the pure `GraceState::step`
  in the same module (`GraceInput`: `Ended`, `Fired`, `IdleAnswer`,
  `Close`; `GraceEffect`: `SendEnd`, `SendIdleAnswer`, `SetTimer`), driven
  by the listener's threadpool timer. `cargo xtask ci` reads the module
  against an allow-list (`xtask/src/ownership.rs`), which also keeps any
responder out of it.
- When the listener is replaced, the supervisor tells every outpost its
  reads have ended (`DeliveredFact::UiaFocusReadsEnded`).

The gap record (`reducer::gap_record`, in the reducer's intake;
`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated fact",
the gap's evidence; `docs/design/listener-without-connections.md`,
decision 11). UI Automation sometimes delivers an application's focus late
or never while its `WinEvent` focus comes, so the outpost keeps, beside the
verdict that dropped its own copy, one record: a focus copy it dropped
because it judged the window UIA, with its window, top-level window
(`window::top_level_of`), stamp, and the bound's due time, on one clock.

- Opening. The worker opens it when it drops a focus copy, its own hook's
  or the listener's forwarded before the hook is in, because its verdict
  on the copy's window is UIA (`Worker::copy_dropped_for_uia`, its
  program's commit carrying a `GapNote`, applied by
  `Intake::gap_copy_dropped`). It looks back first: an answer kept for the
  copy's top-level window or for any window, observed up to 100 ms before
  the copy or after it (`gap_record::ANSWERED_BEFORE_US`), stands for it,
  and nothing opens; otherwise a drop for no window above the sender
  observed so has the focus read at once in the copy's place, stamped with
  the copy's observation, the reducer's next program. A copy handled through MSAA opens nothing.
- Restamps. While it is open, a later copy the worker drops for UIA in its
  top-level window, or a foreground change into that window the worker
  accepts (after its acceptance, front, repeat and readability tests and
  before its window's verdict), gives it that event's observation and
  window, its bound unchanged (`GapRecord::copy_dropped`,
  `GapRecord::accepted`). The stamp only moves forward: an older event, as
  in a focus run tried newest first, neither restamps nor answers it.
- Answers. Each UIA focus's outcome is applied as it resolves its marker
  (`Intake::push`, with no program dispatched): a focus's read placed in
  its own window's top-level window, or its nearest window's, one local
  `GetAncestor`, and a drop by the answer it carries. An answer for the
  record's top-level window, or for any window, observed up to 100 ms
  before its stamp or after it, closes it, and is kept, the newest per
  top-level window, pruned when the window is destroyed
  (`Intake::gap_window_destroyed`), and the newest for any window. A drop
  for no window closes the record it answers and takes its marker's place
  as the record's read (`Item::GapRead`), and is kept as the newest
  no-window stamp. A drop for no keyboard focus, one replaced, and any
  other drop that could not establish the focus leave it open, as does
  anything but a focus. A newer copy accepted in another top-level window,
  through either backend, or an accepted foreground change into one,
  answers it. Every answer is final.
- The bound (`gap_record::BOUND_US`, one second from the first copy) is
  one of the reducer's timers, set when the record opens and cancelled
  when it is answered (`TimerAction`), in the order the record changes,
  since one thread changes it; the shell's timed wait ends at it. When it
  passes, `Item::GapBound` is pushed into the intake, behind every marker
  queued before it. Handled with the record open and its bound passed by
  the observation clock (`Intake::gap_bound_reached`), the record closes
  and is read; a bound reached early is set again for the rest. A test
  sets a timer of its own (`Outpost::set_gap_timer`, behind the
  `GapTimer` trait) in place of the reducer's, and reaches the bound
  itself (`Outpost::gap_bound_reacher`).
- The read (`Worker::gap_read`, `read::gap_focus_read`, trigger
  `read::FocusRead::Gap`) is the focus-now read through UIA only, made
  once: refused with local calls when the foreground thread's focus window
  is not the application's, is Core's hidden frame, cannot be read, or is
  one the outpost judges not UIA; and refused after
  `GetFocusedElementBuildCache` when the focus window, taken again with
  `GetGUIThreadInfo`, has changed to one the outpost does not judge UIA
  without a probe (`arbitration::known_to_use_uia`), since a focus that
  moved into an MSAA window meanwhile comes back as UIA's MSAA proxy for
  it, and a proxied list item has no window handle to judge. It is
  reported as a UIA focus, stamped with the record's stamp, logged at info
  with both windows, whether a listener UIA focus had reached the outpost
  before the stamp (`uia_focus_before`) and whether the record's window was
  gone (`sender_gone`); a late UIA focus of the same element is reported
  under the read's node, which Core takes as a duplicate.
- The idle barrier's answer waits while the record is open
  (`Reducer::answer_idle`); a drop that closes it as its marker resolves,
  leaving the worker nothing to handle, pushes `Item::Wake` while a round
  waits, so the answer still goes.

The verdicts. The outpost's verdict is the one verdict per window; the
listener makes none (`docs/design/focus-pipeline-rework.md`, "Who owns
each duplicated fact", the verdicts). A listener's read in a window the
outpost judges not UIA, by the read's nearest window, judged at the read's
observation, is dropped (`Worker::uia_focus`), its answer having already
been applied to the gap record.

### Console windows and frames of another process

A console window is one application in its console host's outpost, as
NVDA gives it and its text area the client's app module
(`docs/design/focus-pipeline.md`, section 5.4). The listener sends each
`WinEvent` for a `ConsoleWindowClass` window to the process of the event's
thread, the console host (`listener::routed_process`), dropping one whose
thread cannot be opened. The console host's outpost marks each event in a
console window with the client as its `application`, which Core takes as
the event's source, and answers a focus-now query for the window. A focus
change carries its application's name, read once per process and kept with
the process held open (`outpost::ApplicationNames`).

An outpost takes a window as its application's when the application serves
its UIA (`window::served_by`, through `verbatim_uia::served_by`, local
calls), and `read::focused_control_in` reads no focused element in another
process's window (`read::control_window`). `GetFocusedElementBuildCache`
reads the keyboard focus again itself and can return an element of the
application the focus has just moved to
(`experiments/getfocusedelement-after-the-focus-moves/`), so its request
caches the element's process (`Request::with_process`), and an element
whose cached process is not the outpost's application, the console host
for a console, or that has none cached, is read no further: no
enrichment, walk, subscription or report, `None` answered, and a debug
line naming both processes. Each read is logged at debug with what asked
for it (`read::FocusRead`: a focus-now query, the gap's read, or a cut
walk, below). The one connection the call makes into
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
(`check_served_by` in `supervisor/owner.rs`; a debug build fails). The
crate re-exports `provider_process` and `console_host_of` so Core routes a
console client's focus-now query to its host.

A focus whose top-level window belongs to another process, as the Settings
page inside `ApplicationFrameHost`'s frame, is read by the listener up to
the window below the boundary, and the outpost completes the chain with
that window's parent or owner, and every window above it to the desktop,
read from Windows' own state with no call into the other process
(`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
fallback" and "The foreground wait"; the decision of 2026-10-29). A plain
window (`read::plain_window`) is read from Windows' own state alone: role
window, named by `InternalGetWindowText`, unavailable when its style
disables it, never focused and never invisible, kept under its window
object's address. A window `read::plain_windows_from` reads, above the
process boundary or past a walk's bound or failure, that is not a
ghost's or a hung application's is read the same way but described as
NVDA's MSAA client for it (`read::client_pane`): role pane, marked
`top_level_client` when its style has `WS_SYSMENU`, so Core speaks the
Settings frame, or a top-level window published at its bound, by its
title, and takes a child window between as structure; a ghost's or a
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
MSAA `accParent` that fails or names nothing, or a UIA step that fails, the
parent window of the outermost object's window; after a window read
through the other API, that window; above the boundary, the parent or
owner. Each window is read by its own object as NVDA chooses it, its UIA
element when the outpost judges it UIA, its MSAA client otherwise, and
walked up by that backend; a window another process serves and every
window above it, a ghost's or a hung application's window, and a window
whose object cannot be acquired is a plain window, the chain going on from
the parent window of the last two. Every UIA walk, the listener's
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
walk instead. A walk cut by its time
(its bound, for the continuation of a listener's read that read's bound
from the event's observation, `UiaFocusRead::bound_us`; for an MSAA
focus's walk and the gap's read, `read::walk_left`; `ENRICHMENT_BUDGET`
for a focus-now answer) or its depth limit of 64 is completed by plain
windows from its outermost element's nearest window, included. Whether a
window can be read is the system's judgement (`window_unreadable`), which a
test that cannot wait five seconds for Windows to judge an application
hung replaces (`Outpost::set_window_readability_reader` and
`FocusReader::set_window_readability_reader`, each a
`WindowReadabilityReader`).

### Starting an outpost and asking for the focus

A fact for an application with no outpost starts one
(`policy::may_start_outpost`) unless it is a selection or a
notification other than the shell's window-snap results. Node identity
never crosses a process: the outpost mints the `NodeId` from the forwarded
runtime id. Nothing polls: at startup, and after an outpost or the listener
is replaced, Core asks for the focus with one focus-now query.

An application usually gets a spare (`docs/design/focus-pipeline.md`,
section 19): the supervisor keeps two outposts started with `--spare`, with
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
walk-up races (`docs/design/listener-without-connections.md`, section 2),
so no lock is needed around them. No node is searched for by runtime id:
a UIA node whose kept element is gone, or that has none, and whose focus
route's walk finishes without it is answered gone
(`read::resolve_uia_element`), while a walk that fails, a window lookup
timed out or refused or a classic walk past its budget among them, fails
the query (`focus_path::find`), since a slow application has not shown
the element gone. The limit: a slow classic walk that truly finds nothing,
but whose last call returned after `PATH_BUDGET`, fails the query rather
than answering the node gone. A
listener's selection is reported with what its event's cache holds
(`verbatim_uia::ListenerSelection`), since a search of a subtree enters
any child window another process serves (the study's instance 3). The supervisor's `App` table holds each application's
process open while it has a record, so its pid is never reused while an
outpost or reading thread serves it. Registrations scope windows' subtrees
by their elements (`Scope::Subtrees`), with no connection by handle.

## Public API

### The protocol

The messages themselves, one enum per direction and role, their kinds,
their merging and the types they carry, are `verbatim_model::protocol`'s
([verbatim-model](verbatim-model.md), "The wire protocol"); this crate's
`protocol` module re-exports none of them. It holds what needs the
Windows crates: the framing below; `now_us`, the run's shared clock;
`fact_of_win_event`, the fact an MSAA `WinEvent` makes, whichever hook
took it; and `peek`, which reads a frame's kind from its first bytes
(`message_kind`) as a `Peeked`, the variants the relay handles itself by
name, so `outpost_frame` and `spare_frame` match it exhaustively and a test
pins every variant's name.

Each message is stamped as its write begins, by whichever thread writes
it: `listener_written_at_us` on a listener's fact or menu end,
`written_at_us` on an outpost's `Event`, `Reply`, or `Pong`; the listener
adds `listener_handled_at_us`. verbatim.exe stamps its relay times on a
routed fact or query as it writes it, and a ping or a hand-over notice to
the listener with `sent_at_us`; the listener logs each hop at trace under
`verbatim::stage`.

Framing. Each message is one frame of `verbatim_process::transport`, a
`Content-Length` header and compact JSON, through `write_message` and
`read_message`, for any type that implements `Message` (`put_frame` and
`from_parts`): the four direction enums, the outpost's with the raw
section below.
`write_message` writes the whole frame in one `WriteFile`,
`encode_message` gives the bytes, `put_message` appends them to a sender's
buffer, and `decode_message` parses a frame's `Parts`. A message that
carries a terminal's text (an event with a `TerminalSnapshot`, or a reply
with a `TerminalSnapshot`, `TerminalHistory`, or `TerminalFound`) has a raw
section (the private `protocol::raw` module;
`docs/design/communication.md`, section 4.2): each text is UTF-8 there and
a range in the JSON, `"raw":[start,length]` in a `TerminalText` and
`"text":[start,length]` for a history's or match's text, and colour runs
are 16-byte records of four little-endian `u32`s (column, length,
foreground, background), the JSON giving
`"colours":{"raw":[start,length],"rows":[...]}` with each row's run count.
The module converts while encoding, borrowing from the message, and back
while decoding, validating text with `simdutf8`; the model's types know
nothing of it. A range outside the section, invalid UTF-8, records that do
not fill their range, a section longer or shorter than its ranges, or a
section on any other message is a protocol error. For example, a history
whose text is `abc` has a header with `Raw-Length: 0000000003`, a body whose
outcome is
`{"Done":{"Text":{"TerminalHistory":{"text":[0,3],"above":0,"read":0}}}}`,
and then the bytes `abc`. `message_kind(body)` reads a frame's variant name
from its first bytes without parsing the rest; the relay reads it on every
outpost frame.

### The arbitrator

`Arbitrator` is NVDA's per-window backend decision (`_isUIAWindowHelper`),
each outpost's one verdict per window. It holds the configuration: the
class lists, the `SetBackendOverride` and the clocks. The verdicts probes
found are the reducer's state, `Verdicts` (`outpost/reducer/verdicts.rs`),
which callers give it. `verdict(&verdicts, hwnd, &WindowClasses)` answers
from the good class list, the Windows 11 shell rule, the bad class list,
the override, or a kept probe, and `None` when only the probe can decide;
a program then probes the window with the raw probe
(`verbatim_uia::raw_probe`) through `arbitration::uses_uia`, and records
its answer through its `arbitration::Judgement`, the request's context
(`Arbitrator::record_probe` makes the same record for a test that probes
by hand). No connection follows a probe
(`docs/design/listener-without-connections.md`, decision 2). The listener
makes no verdict; it skips a read only by `arbitration::class_verdict`,
the class lists alone, from the same tables and code.

- `WindowClasses::of(hwnd)` reads the raw class, the class normalized as
  NVDA does (`normalize_class_name`), and the root ancestor's class.
- `post_probe_check` names NVDA's check before using a provider: a list
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
  Each probe is committed as it returns, alone in its commit, and applied
  to the program's copy, which it judges by for the rest of its walk. The
  shell passes a commit's verdicts to the reducer even when it applies
  nothing else of it, and the reducer records a probe only from the worker
  generation in charge, but a timed-out probe and a destroyed window's
  forgetting from any generation, so a window is not probed again more
  often than NVDA probes it. A probe committed before its worker is
  abandoned stays recorded. When the record changes the reducer publishes
  the new snapshot to the mailbox's push-time copy (`Effect::Verdicts`),
  and after it, for a probe that found a provider, nudges the event thread
  to decide its hook again (`Effect::Narrow`).
- `lasting_verdict` answers only a verdict that lasts the window's
  lifetime.
- `Clock`, `Arbitrator::set_clock`, and `Arbitrator::now` time the classic
  walk budgets (`PATH_BUDGET`, `ENRICHMENT_BUDGET`, through
  `arbitration::time_limit`); `Outpost::set_clock` and
  `FocusReader::set_clock` set it for tests that count calls. The keeps'
  ends and an entry with no event are stamped by the `ObservationClock`
  (`Arbitrator::set_observation_clock`), the run's shared clock, which
  `Outpost::set_observation_clock` sets, with the stamps of the outpost's
  own events, for a test that moves that time on.
- Both class lists are lifted from NVDA and pinned by unit tests naming
  their sources (`badUIAWindowClassNames` and `goodUIAWindowClassNames` in
  `nvda/source/UIAHandler/__init__.py`, and `isGoodUIAWindow` in
  `nvda/source/appModules/explorer.py`), so an NVDA sync is a diff.

### The outpost

`Outpost`, `run_pipe`, and `run_attach` are the per-application outpost.

- `run_pipe(pipe_in, pipe_out, target_pid, options)` runs one on Core's
  inherited overlapped pipes, watching `target_pid`, or for `None` as a
  spare. `Outpost::assign` sets the application once: the clock, the hook on
  the `WinEvent` thread (`EventThread::hook`), and `Ready`. When the hook
  fails it reports a `Fault`, sends no `Ready`, and exits with code 1
  (`docs/design/focus-pipeline.md`, section 19.8). It serves until
  `Shutdown` or the end of the command pipe, then shuts down cleanly, and a
  thread waiting on the target sends `TargetExited`.
- `run_attach` watches a pid, asks for its focus, and prints each outbound
  message as the model's JSON on stdout: the development mode.
- `OutpostOptions::remote_operations` says whether a UIA focus's ancestry
  may be read with a remote operation (on; `--classic-uia` turns it off).
- For tests in their own process: `Outpost::new` and its siblings watch
  from construction and panic when the hook fails, with a sender thread in
  place of the I/O thread; `with_options`; `with_focused_element_reader` (a
  `FocusedElementReader` in place of `Uia::focused`);
  `set_foreground_reader` (a `ForegroundReader` in place of
  `GetForegroundWindow`, for acceptance and the window a focus entered);
  `set_focus_window_reader` (a `FocusWindowReader` for focus-now and the
  gap's read); `set_observation_clock` (an `arbitration::ObservationClock`
  for the outpost's own events' stamps and its verdicts' keeps); and
  `set_gap_timer` (a `GapTimer` for the gap record's bound, which
  `gap_bound_reacher` reaches); and `without_listener`, for an outpost
  no focus listener forwards to, whose hook's copies are never held.
- `Outpost::settle` waits until everything queued is handled, follow-ups
  included, the subscription has made every move asked, every published
  message is written, and the event thread has taken every nudge. It covers
  only what has reached the outpost.
- `Outpost::failed_reads` gives the worker's and subscription callbacks'
  failed cached reads (`verbatim_uia::calls`), also logged at shutdown.
- `Outpost::observe_heard` hands a `HeardObserver` each event the outpost's
  own handlers and hook take in, as a `Heard`, once queued.

Inside an outpost (`docs/design/communication.md`, section 6.4;
`docs/design/ownership-outpost-reducer.md`):

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
- The reducer (`outpost::reducer`), a pure state machine on a thread of
  its own, run by its shell (`outpost::shell`): it holds the intake's
  order (`reducer::intake`), the gap record, the reports Core may hold
  (`reducer::held`), the follow state (`reducer::follow`), the hand-over's
  hold, the arbitrator's verdicts (`reducer::verdicts`), each node's text
  facts (`reducer::text`), the worker generation in charge with its
  deadline, the moved-on grace and the walk's bound, and the queries not
  yet answered. Inputs
  come in mailbox order; effects go out: dispatch a program, send a
  message, release, move the subscription, abandon a worker, set a timer.
  It names no Windows type and holds no application object; its local
  reads (a hung window, whether the user moved on, a publication at a
  walk's bound) reach it through `reducer::Locals`. `cargo xtask ci`
  reads every file of its directory by the pure rule
  (`xtask/src/ownership.rs`). The shell waits on the mailbox's alarm
  (`outpost/alarm.rs`): an event set by an append while it waits, and a
  high-resolution timer set to the reducer's next timer, so the watchdog
  thread and the gap record's threadpool timer are gone, and each timer
  costs one wake. Every lock its thread takes is a short hold with no
  call: the mailbox's, the backlog's with the store's inside it for a
  release, the store's for a commit's writes, and a worker's charge.
- Programs (`outpost::worker`): the worker generation in charge takes one
  program at a time from its inbox, an entry's handling, the owed work, a
  settle, a release's drop, the console poll, or the finish of a caret
  read cut at its value change check ("Text and caret"), with copies of
  what it
  reads
  (`reducer::Facts`: the reports, the follow state, the marks, the
  verdicts), and posts
  each commit (`commit::Effects`) to the mailbox, applying its pure part
  to its own copies (`reducer::apply`), so what it reads after is what the
  reducer will hold. The shell applies a commit only while its request is
  live, its generation in charge and its walk not published, but for its
  verdicts ("The arbitrator"): first the
  writes only it makes, to the store and its table of the elements the
  subscription follows (`commit::Writes`), the program's text writes
  among them, then the reducer's change. A
  program receives too the caret events waiting when it was dispatched
  (`reducer::WaitingCaret`), which a caret key's check asks after. The inbox
  (`outpost::inbox`) is a channel of jobs with the worker's own alarm as
  its doorbell: the shell sends a job and then sets the alarm's event,
  and lets go of an abandoned generation's sender before a last ring, so
  the worker sees the inbox closed; the worker takes jobs without waiting
  and waits on the alarm when none is there, so the console poll waits for
  its next read and its next job in one high-resolution wait.
- The focus-following subscription (`docs/design/focus-pipeline.md`, section
  17): one registration with NVDA's two groups. The base group, given every
  UIA focus whose element the outpost has, holds name, value, and state
  changes and `Text_TextSelectionChanged`; the extended group, for a focus
  with text read through UIA, adds `Text_TextChanged` and the active text
  position changed event. The console host's text area takes the group
  without its text events (`Group::ExtendedWithoutTextEvents`), and the
  console's caret events are its caret events
  (`docs/design/wake-ups.md`, item 15). Each is on the element of every
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
- The queue (`docs/design/focus-pipeline.md`, section 5; the rules are in
  `docs/architecture.md`, "Outposts"): one entry per object and kind, and for
  an MSAA event other than a menu opening per raising thread (a menu opening
  per address; the console host's events per window and kind), a newer
  replacing it at the back. A batch is what accumulated during the previous
  one, in arrival order, but a focused terminal's caret event ahead of its
  text change goes just after the change, whose read reads the caret
  (`caret_after_update`). Per batch the newest 4 MSAA focus events and 10
  other MSAA events per thread are kept (the focus's own events beyond the
  limit, a foreground change by either); events from a hung window are
  dropped.
- Runs: each run of MSAA focus and foreground events is tried newest first,
  each try a program under its own deadline, until one is reported, at the
  run's newest place, as NVDA's `pumpAll` does (`ends_a_run`); the reducer
  dispatches the next try when a try's program ends unreported, and the
  replacement of a worker abandoned in a try that was not published tries
  the next. Three
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
  (`outpost::acceptance`; `docs/parity/focus-events.md`, "Event
  acceptance"): `accepts_event`, a pure function of the event, its window,
  `WindowQueries` (local calls in `SystemWindows`), and `RequestedEvents`;
  its `foreground_relation` goes in the window facts
  (`verbatim_core::focus_rules::ForegroundRelation`). It takes the backend
  crates' per-thread call counts into each published `EventTiming`; a
  program that publishes nothing logs its calls (`docs/performance.md`,
  "Cancelled traces").
- Deadlines and abandonment are the reducer's: an event, a focus, or a
  focus-now 10 s, NVDA's watchdog timeout; a navigation or activation
  400 ms; an ancestor walk, a tree dump or a release 5 s; a text request
  2 s. After half a second on a window it also abandons a worker once the
  user has moved to another UI thread of the same provider process,
  judged when the grace ends and at each foreground fact
  (`window::front_is_another_thread_of_its_application`). It publishes an
  armed walk first, marks the generation dead in the worker's charge
  (`worker::Charge`), answers the query the program was running
  `Abandoned`, and starts a replacement. A returning abandoned worker has
  its commits refused, posts that it returned, and exits; the count of
  those that have not returned is published to an atomic the I/O thread
  answers pings with.
- The bound met when a call is stuck (`outpost::bound`;
  `docs/design/focus-pipeline-rework.md`, "A window's name, and the one
  fallback"). An MSAA focus that has passed NVDA's checks, its
  focused-state test among them, and been read, and the gap's read once
  its focused element is read, arm their walk in the worker's charge
  (`Charge::arm`, a `bound::FocusProgress`) and tell the reducer its bound,
  by the design's Terms (`read::walk_left`): `MOVED_ON_GRACE` when the
  focus's top-level window is not that of the focus the outpost expects
  Core to hold (`Held::expected`), `ENRICHMENT_BUDGET` otherwise, measured
  from the event's observation for an MSAA focus and from the read's start
  for the gap's read. The walk tells what it reads as it reads it
  (`read::WalkStep`), each step taken into the charge under its lock: the
  MSAA walk each ancestor and its chain, the classic UIA walk each
  ancestor it keeps, a remote program nothing until it returns. A walk
  already past its bound when it begins is not armed. Should the walk
  still be under way at the bound, the reducer publishes the focus from
  that progress (`Locals::publication`, `worker::publication`), its chain
  as `bound::chain_at_bound` makes it, by local calls on the reducer's
  thread, with no selected child, dialog text or text of its own. A walk
  that returns past its bound is marked so in the charge, and the reducer
  publishes it from the same progress, so the report does not depend on
  which comes first; one that returns before is disarmed there, and its
  program reports it. The publication records the report, sends it,
  makes the focus the followed one with the work it owes, and claims the
  walk's request: nothing more of its program is applied. The reducer
  publishes an armed walk before it abandons its worker, in the same
  transition. The owed work, a terminal's first read or a
  `SensitiveSlider`'s lookup for a thumb published at its bound, is a
  program the reducer dispatches before anything else, once per worker
  generation, under a deadline of its own with no moved-on window.
- Requests and their tags: each program is a request with a mint tag of
  its own (`store::Tag`, the generation and a request number the reducer
  gives), which its `RequestContext` carries with the store's handle; the
  program in flight's request is live until it ends, its worker is
  abandoned, or its walk's publication claims it. A program's text writes
  go to the store only with a commit the reducer accepts, as every other
  write of the shell's does ("Window destruction and held objects", text
  on copies). A terminal read works on a
  copy of its entry, put in place only by its commit.
- Queries: each query the outpost takes has a reply token
  (`verbatim_model::protocol::Responder`) in the shell, keyed by request
  id, consumed by the one answer the reducer sends: the reply, `Abandoned`
  at the deadline, `Failed` for a program that ended without answering,
  `NotStarted` for a `Cancel` before dispatch, `WatchEnded` for a caret
  key's watch. A token dropped unanswered answers `Failed`. In debug builds
  the shell asserts after each transition that its tokens are the queries
  the reducer records.
- Time limits in an entry (`docs/design/focus-pipeline.md`, section 6): UIA
  calls end at `verbatim_uia::CONNECTION_TIMEOUT` (2 s) or
  `TRANSACTION_TIMEOUT` (5 s); the listener's read's bound bounds the
  continuation of its chain, the walk's bound an MSAA focus's or the gap's
  read's ancestor walk between calls, and `ENRICHMENT_BUDGET` (2 s) a
  focus-now answer's; a UIA dialog's text runs on a
  `verbatim_uia::BoundedClient` within `DIALOG_TEXT_WAIT` (1 s); tree view,
  list view, and edit messages use `SendMessageTimeoutW`; an `IAccessible`
  call is bounded by the entry's deadline.
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
  (`docs/crates/verbatim-model.md`, "The text protocol"), public so mockapp
  drives it. `perform(source, anchors, op)` answers a `TextOp` over a
  `TextSource`, a backend's view of one node's text in UTF-16:
  `uia::UiaText` over a text pattern (positions are `UiaPos`; `new` takes
  the element for remote operations; `remote`, `fetches`, `support`, and
  `known_support` say whether caret reads try a remote operation, which
  formatting they read, and the `TextSupport` known before and after), or
  `edit::EditText` over an edit control's messages (offsets; a word is the
  line segmented by `verbatim-text`'s rules, as NVDA finds it).
  `caret_report`, `caret_report_since` (with `LastSelection` and
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
  names, which a copy must hold. `check_caret` checks a caret key's
  watch with one read and answers `Watched::Answered` or
  `Watched::Watching`; `CaretSignal` is what the check knows besides the
  text, with a clock for tests.
- `dialog_text(&dialog)` gathers a dialog's own text by NVDA's rules
  (`docs/nvda/object-model.md`, "A dialog's own text") over a
  `DialogParent` whose children are `DialogObject`s: MSAA through
  `verbatim_ia2::dialog::DialogObject`, UIA through
  `UiaDialog::new(uia, request, element)` and `UiaObject`, each child read as
  a `verbatim_uia::DialogText` with its description whatever the theme
  (`docs/design/uia-shapes.md`, decision 5). `is_dialog(role)` says which
  roles have such text; more than `MAX_OBJECTS` objects gives no text.

### Ignored processes

`supervisor::IgnoredProcesses` (`IGNORE_PIDS_ENV`, `VERBATIM_IGNORE_PIDS`;
`from_env`, `hold`, `contains`, `parse_pids`) are processes Verbatim
ignores, named by the end-to-end harness: its own Windows Terminal. Each is
held open; no outpost starts for one (`ensure_spawned` answers
`NotWatched`), none of its facts is routed, and the listener, given
`--ignore-pids`, drops them before queuing. `Supervisor::is_ignored` lets
the app pass them over.

### The focus listener

`run_listener` is the listener runtime (`docs/design/communication.md`,
section 6.3).

- Its main thread is its I/O thread (completion port,
  `THREAD_PRIORITY_HIGHEST`); every thread that produces a fact writes it
  itself, encoded into the sender's one buffer.
- It installs the global MSAA hooks (`LISTENER_SUBSCRIPTIONS`: focus,
  foreground, menu-popup, menu and switcher end, alert, and tooltip shown,
  forwarded as `DeliveredFact::Show`) and, through one `Registration`, the
  focus handler (`Subscription::Focus`) then element selected, menu opened,
  and notifications as one group on the desktop root, and says `Ready` only
  once all are in, so a menu opened right after start is seen. Range-value changes and live regions are not subscribed:
  values are spoken only for the focus, and live regions are milestone
  M7's.
- A UIA menu opening takes the focus's route, judged as NVDA's automation
  event handler judges one, and is read as a focus on the menu. One that
  arrives while a focus of its application waits or is read is dropped
  (`Readers::focus_pending`). A newer focus or menu opening of an element
  replaces its waiting one.
- Other events become `FocusFact`s (`ListenerFact`: pid and
  `DeliveredFact`) from cached and local reads. A foreground event is
  forwarded without checking the foreground, since a starting window raises
  it early; the outpost checks. `focus_read::FocusReader` is public for
  tests.
- While a write is in flight, waiting facts (`listener::Waiting`) merge per
  element and kind (`DeliveredFact::key`), a `WinEvent` fact but a menu
  opening also per raising thread, a menu opening per address alone
  (`docs/parity/backends.md`, the limiter's entry); notifications and a
  focus's marker, read, and drop never merge; pongs and `Ready` go first.
  The backlog judges under the sender's lock whether a fact was raised after
  its outpost's hook went in, so none follows `ForwardingEnded`.
- A menu's or the Alt+Tab switcher's end goes to Core (`MenuOrSwitchEnded`)
  once its 50 ms grace passes with no later end, from a threadpool timer
  that a later end sets again; a callback that runs early sends nothing.
- For every foreground event, whatever its process, the listener sends
  `ForegroundWindow` (window, the event's thread, whether a console host
  window, integrity level by `integrity::process_integrity`;
  `integrity::own_integrity` is verbatim.exe's), only the newest waiting
  (`FactKey::ForegroundWindow`), and once its hooks are in, the window then
  in front (`NUDGE_FOREGROUND_START`). The console's events stay unhooked
  (`verbatim_ia2::suspend_kinds`) unless it starts with a console window in
  front, until one of that window's events names its thread
  (`foreground::ForegroundWatch`). The app receives it as
  `OutpostMessage::ForegroundWindow` (a `ForegroundReport`).
- It handles `Ping`, `HookInstalled`, `HookRemoved` (which also ends the
  application's reading thread and its override), `SetBackendOverride`
  with a `pid` (an override forcing UIA, after which that application's
  readers skip nothing by class), `IdleQuery`, and `Shutdown`, and ignores
  the rest. A replacement gets `--hooks-in`
  (`listener::parse_hooks_in`) and does without each thread's last focus,
  at the cost of one fuller ancestor read per application. It makes no
  remote operation, so takes no `--classic-uia`.
- `listener::capture` is what the notification callback captures, and
  `listener::capture_selection` what the element-selected callback
  captures, public so mockapp makes a selection's fact as the listener does
  (`crates/mockapp/tests/combo_box.rs`).

### The supervisor

`Supervisor`:

- `new(events)` starts the relay, which starts the listener at once;
  `with_executable` uses a given executable and shutdown limit, for tests.
  Both take the `OutpostOptions` every outpost gets.
- `shutdown()` shuts every child down and returns a `ShutdownSummary`: how
  many children ended cleanly, had exited, exited with an error, were killed
  at their time limit, or were killed because ending them failed.
- `ensure_spawned(pid)` starts an outpost, from a spare when one waits.
- `send_to_outpost(outpost, command)` queues a command without waiting,
  failing at once with a `QueueError` when the generation ended or its
  queue is full.
- `note_views(attention, holding)`, `leave_nodes_held` (left in the
  writer's slot for the next command), `send_nodes_held` (written behind
  what waits, in place of a list left), `query_idle(round)`, `is_ignored`,
  and `frames`.
- `SPARES` is two; Verbatim's readiness waits for them.

The app hears through `OutpostMessage`: `ListenerReady`, `SpareReady`,
`Regenerated`, `IdleQueried`, `NotWatched`, `ForegroundWindow`, and per
generation `Started`, its messages as unparsed `Frame`s, and `Ended` with a
reason (exited, killed, retired, target exited) and whether a next
generation follows (`follows`), the supervisor's decision as it ended it.
Every removal of a record, at a retirement or a target exit, one waiting
for a failure's backoff among them, sends `NotWatched` after the `Ended` of
every generation the app heard started, so nothing waits for an outpost
that will not come. The lifecycle notices (`Started`, `Ended`,
`Regenerated`, `NotWatched`, `SpareReady`, `ListenerReady`,
`IdleQueried`) carry a `Sealed` that only the supervisor makes, so no other
code announces a generation; the `lifecycle-for-tests` feature adds
`Sealed::for_tests` for the app's tests. `Frames::arrived` (from
`Supervisor::frames()`, on the reducer's thread) turns a frame into
`Event(pid, outpost_id, position, message)`, the payload boxed: it parses
each frame, stamps node ids with the outpost's id, records when the relay
read it, and numbers the messages that carry node ids
(`docs/design/communication.md`, section 4.8). A frame that is not a message
ends its outpost: `Frames` drops that outpost's later frames at once,
handing on only its end, and the relay drops every frame from an outpost
it has already ended. Messages go through the `EventSink` given to `new` (a
channel converts into one; `EventSink::new` takes the app's posting
function). `Undelivered` says why one was not: `Gone`, or `Refused`, when
the relay keeps it and everything after it and delivers them again in order
(`crates/verbatim-outpost/tests/target_gone.rs`).
`EventSink::watched(progress, stalled)` reports a `ReducerStall` once the
reducer thread has taken nothing for three heartbeats while inputs waited;
nothing is ended for it (`docs/design/communication.md`, section 6.8).

How it works:

- The owner and relay (`owner`, `relay`) is one thread, `verbatim-relay`,
  that makes every lifecycle decision and owns the records
  (`docs/design/communication.md`, section 6.2). It waits only on one
  completion port, timing out at the next heartbeat (3 s) or sweep (30 s).
  It reads every child's pipe (`transport::PortReader`) and handles each
  frame in the pass that read it: a listener's frame decodes as a
  `ListenerToCore`, matched exhaustively, and its fact is routed and
  written to its outpost there and then; an outpost's frame is read only
  for its kind (`protocol::peek`), a pong and `TargetExited` handled,
  `Ready` handled and passed on, anything else handed on whole
  (`transport::FrameBody::Owned` or a copy). Helpers report through `relay::Mailbox`. It never blocks:
  launches, retirements, and hang checks run on threads of their own.
- Each application's record keeps what belongs to the application (its
  held facts, held idle round, attention time and backoff) and its
  generation's state (`generation::Generation`): `Launching`,
  `SpareGiven`, `Starting`, `Ready`, or `Waiting` for a failure's backoff
  with no generation, the one that failed before its `Ready` kept to be
  replaced. Each transition (`given`, `launched`, `assigned`, `ready`,
  `failed`, `removed`) consumes the state and returns the next with its
  notices in order, which the owner applies (`Owner::apply`): the
  listener's hand-over messages, a spare's `Assign`, a launch, and the
  app's messages. Leaving `Ready` always removes the listener's hook in the
  same step, and the listener's "forwarding ended" is relayed only to the
  `Ready` generation it names. The listener and the spares waiting, with
  no generations, are `Run::Launching` or `Run::Running`.
- Each child's commands go through a `transport::Sender` (`writer`), of
  the commands its kind of child takes (`WriterHandle<M>` over
  `ToChild`, whose routed fact for the listener is uninhabited, so the
  listener's writer cannot be given one): the
  sending thread writes when nothing is in flight or waiting, and otherwise
  the command waits in a queue of 64, written by the relay as writes
  complete (`relay::CommandPipes`). When the queue is full a ping, the
  hand-over's messages, and a UIA focus's parts still get through, never
  merged (`Outgoing::delivered`); a routed fact replaces a waiting one for
  the same object and kind (and thread, for an MSAA event but a menu
  opening) or the oldest fact makes room; anything else fails at once.
- Facts for a starting outpost are held, merged by the same rule, and
  released on `Ready` (`HeldFacts`); then the listener gets `HookInstalled`,
  and later `HookRemoved`, through `Owner::tell_listener`.
- Each application is identified by a process held open (`process::Target`)
  while it has a record. Nothing is started for an application that exited
  or cannot be opened; the app hears `NotWatched`.
- An outpost whose pipe closes is ended once everything it wrote is handed
  on: with its target if the application exited, otherwise by the failure
  path (`crates/verbatim-outpost/tests/spares.rs`). Every child is pinged
  every three seconds, and nine seconds without a pong ends it; eight
  abandoned workers end an outpost unless its application's windows are
  reported hung (`wedge_decision`). An outpost whose application has not
  held attention for two minutes, is not Core's own, and holds no nodes of
  the reducer's is retired by the sweep (`retirement_decision`). An
  outpost says `TargetExited` when its application exits, and the heartbeat
  also checks every held application.
- Every ending is a clean shutdown (`retire`, under "Shutting down"); an
  outpost ended for any reason but its pipe's end has `Ended` sent at once,
  and the app drops what it wrote after.
- The listener is supervised the same way; a replacement's `Ready` makes
  Core ask the attention application for its focus.
- `process` holds the roles' command lines and log names and two local
  queries; launching is [verbatim-process](verbatim-process.md)'s.

## Implementation notes

### Spawning

Each child is created suspended with two unnamed pipes
(`verbatim_process::overlapped::pipe`, 64 KiB each way, verbatim.exe's ends
overlapped), inheriting only its pipe ends and log handle
(`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), placed in a kill-on-close job with a
200 MB memory cap, and resumed (`docs/architecture.md`, section 1). Command
lines carry `--target-pid`, `--spare <launch>`, or `--listener`, and
`--classic-uia` when `uia.remote_operations` is off in `settings.toml`. Each
child's standard error and output go to an append-mode file in this launch's
log directory: `logs\<Verbatim's pid>\outpost-<target image>-<target
pid>.log`, `outpost-spare-<launch>.log`, or `listener.log`, beside the
outpost executable. `verbatim_process::prepare_launch_logs`, called before
any child starts, empties a directory an earlier process with the same pid
left and keeps the newest ten. A failed log open leaves the child
unredirected, never unspawned.

### Foreground changes and focus-now

- A batch with a foreground fact is handed out at once. A foreground fact
  whose window is not in front when handled is dropped, as NVDA's
  `processForegroundWinEvent` drops it, since Windows raises its own event
  once the change is made (`docs/parity/focus-events.md`, "Stale focus
  events"). It counts among its thread's other events and among focus events
  in the batch limits, so later focus events do not crowd it out.
- A foreground fact in front is reported, whatever the focus: NVDA's two
  tests against its last queued focus are Core's, which refuses a
  foreground report of the window its focus is in. In a focus group, a
  reported change whose window holds a report's line lets the older entry
  be tried, and every report of the group carries its number and rank
  (`verbatim_model::FocusGroup`), so Core takes the older entry only when
  it refused the change; a reported change of any other window ends the
  group, as a reported focus does.
- It is reported at once as a focus on the window, stamped when
  confirmed in front. The window's own object is read (`window_element`, or
  `OBJID_CLIENT`), so a focus on its client area is the same node; a popup
  menu window (`#32768`) reads as a menu (`read::window_snapshot`). An
  unreadable window is reported from local data, and an empty name takes
  the window text (`InternalGetWindowText`). A report whose window has left
  the front by sending time is dropped.
- A foreground event for a UIA window is dropped, as NVDA drops every MSAA
  event for one; the window is announced from the focus's ancestry
  (`docs/design/focus-pipeline.md`, section 16.4). A console window's title
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
  (`verbatim_ia2::acquire::focused_snapshot`). There are no retries.
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
  `docs/design/wake-ups.md`, "Narrowing the outpost's hook", which states
  what can be lost): while the focus window and foreground window are both
  UIA, the value, state, name, description, selection, and text selection
  events are unhooked (`narrowing::NARROWED`), hooked again once the focus,
  a menu opening, or the foreground is in an MSAA or undecided window.
  Those windows come from the hook's own events, or, until it delivers one,
  from nudges for routed facts (`event_thread::Nudger`); the shell nudges
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
  coordinates (`gates::node_gates`).

### Reading the focus

- A UIA focus is reported from the listener's read (`Worker::uia_focus`):
  tested against the foreground with its nearest window (or 0, which passes,
  as NVDA's `shouldAcceptEvent`; `docs/design/focus-pipeline.md`, section
  16.2), its backend decided. Its runtime id keeps its node; when the
  listener read that the earlier element under it lost the focus or is
  gone, the report carries that evidence (`FocusChanged::earlier`) and its
  commit evicts the element kept for the node, if still kept
  (`commit::Stale`, `NodeIdRegistry::evict_element_if`), not moving the
  subscription to it; for one gone, the node's text is dropped from the
  store before the caret read, while the entry's request is live
  (`Store::forget_text`), and its terminal entry goes in the commit. The subscription's target is
  compared by node, group and element, so a node whose element was
  replaced is followed again. Its
  ancestors are the listener's chain, filtered, spliced at the previous
  chain, and completed above as the listener's walk ended
  (`read::uia_chain_from_read`). Every UIA
  focus is handled, oldest first; Core culls expired speech.
- The outpost then finds its own copy by the chain's runtime ids
  (`docs/design/focus-pipeline.md`, section 16.5), after the report for a
  focus without text and before it for one that may have text: one
  `ElementFromHandle` and one remote operation
  (`verbatim_uia_rops::element_by_path`, or the classic walk within a
  second). It keeps the element under the focus's node and follows it. The
  chain is kept (`outpost::focus_path`) for the controller relation, a
  dialog's text, and queries. A focus whose element is in no tree, as
  Windows Terminal's tab focuses, is reported all the same.
- A text focus with no window of its own whose walk up the listener cut,
  so that the chain holds only the focus (`FocusPath::is_cut_at`), has no
  route; the outpost reads its application's focused element instead
  (`read::focused_element_in`, trigger `read::FocusRead::CutWalk`), with
  the focus read's checks and process-id check, and keeps it when its
  runtime id is the focus's (`Worker::element_after_cut_walk`;
  `docs/design/focus-pipeline-rework.md`, the audit's difference 5). It
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
  redirected one is dropped (`Marks::focus_redirected`).
- The reports (`reducer::held`;
  `docs/design/ownership-one-judge.md`, "The reports, from the store"):
  every focus or foreground report whose focus node the outpost has not
  released, added at a program's commit and removed by a release, one
  per node and line. Core's focus line is always among them. With each
  the outpost keeps its MSAA address, whether a simple child, the window in
  front as it was entered (inherited from the newest report in its
  top-level window, set by a foreground change of it), and the text each
  dialog of its line was given. The newest is `expected()`. A report of
  the newest report's node whose own evidence says the same element still
  has the focus leaves the states last read as they were. The followed
  focus (`Tracking::focus`, `focus_role`, `window`, `chain`) is the newest
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
  `read::msaa_enrichment`) stop at the previous chain and splice it, within
  `ENRICHMENT_BUDGET`. A walk that finds no parent, or whose step fails,
  below the root (or below a top-level window, for the remote program and
  `Uia::ancestors`) goes on from the parent window of its outermost
  element's nearest window (`Ancestry::failed_hop`, `AncestorWalk::Failed`,
  `verbatim_ia2::acquire::Walked::Onward`), and one cut by its time or depth
  limit is completed by plain windows (`AncestorWalk::Cut`, `Walked::Cut`),
  for a UIA walk from the parent window of its outermost element's window
  when that element is the window's own, so the window is not in the
  chain twice (`verbatim_uia::plain_windows_start`);
  a walk that fails before it reads anything, a remote program by UIA's
  transaction timeout among them, or an MSAA node no longer reachable,
  is completed by plain windows from the parent window of the focus's own
  window, or from the window it is in (`read::plain_windows_above`). The
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
  foreground report's (`Tracking`'s `dialog`) serves the focus that
  follows.

### Menus

Within a batch, focus and foreground events are handled first and the menu
opening last, as in NVDA's MSAA handler: ignored if a focus already landed on
a menu or menu item, or if its object is not a popup menu, and otherwise
reported as a focus on the popup menu. The end of a menu or of the Alt+Tab
switcher is the listener's (`MenuOrSwitchEnded`); Core then asks the
foreground application for its focus unless a newer focus was applied.

### Window destruction and held objects

- Window destruction: `EVENT_OBJECT_DESTROY` for a window drops its verdict,
  the gap record's answer kept for it (through its program's commit), its
  classic mark, and its MSAA nodes.
- Held objects: the object store (`outpost::store`;
  `docs/design/ownership-outpost-reducer.md`, "The store") owns both
  registries, each node's text patterns and anchors, the terminals' kept
  ranges, and every node's and anchor's mint tag, the requests that issued
  or looked it up. Its API returns owned clones and owned removed objects
  (`store::Removed`), never a guard, and takes no closure. Walks issue and
  look up nodes through the request's `store::Nodes` handle
  (`Store::nodes_for(tag)`, `RequestContext::nodes`), which implements the
  interning traits of `verbatim-uia` and `verbatim-ia2` and tags each node
  with the request's tag in the same hold of the store's lock, keeping an
  element a lookup replaced for its holder to drop (`take_removed`, which
  a publication hands to the next program's worker); a plain
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
(`docs/design/uia-shapes.md`), one function per shape and use (`gates.rs`):
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
(`docs/crates/verbatim-uia-rops.md`, "The caret read"). `IsHidden` is never
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
  whether they moved, the caret's line and the watch's unit with offsets,
  and the formatting to be spoken: for the report after a focus and a caret
  key's answer (not a paragraph or page), never a typed character's. It
  travels as `FormatRun`s: bold at weight 700 or more, underline and
  strikethrough as `LineStyle`, size as "11.0 pt", colors by NVDA's named
  hues (`text/color.rs`, ported from NVDA's `colors.py`), `BulletStyle`,
  and links.

A caret key's watch (`AwaitCaret`) looks for the evidence NVDA's caret
scripts wait for, without waiting:

- The worker ends the watch it replaces, then checks the new one with one
  read (`text_reads::check_watch`, `text::check_caret`): on evidence it
  answers at once with the caret's line, the unit, and the selection's
  changes; otherwise the watch stays open (`reducer::OpenWatch`, at most
  one, in the follow state) and the program ends. Each later caret event, text
  change, or text selection change of the node checks it again, unless
  observed before the caret was last read; the answering event's own report
  is not sent. A read that covers a waiting caret event observed after the
  key's press counts it (`text_reads::Signal`), as the program's copy of
  the waiting caret events says. While a check reads, the watch is taken
  out of the follow state, and an abandonment answers its query.
- A watch ends with `TextReply::WatchEnded`, which Core does not speak,
  when the next key's watch replaces it, a focus on another node is
  reported, or at `CARET_WATCH_BOUND` (10 s, `Item::Wake`, for a watch
  that does not poll). So a key that
  moves nothing is silent (`docs/parity/text.md`, "TextInfo-equivalent
  layer").
- The console host's caret poll (`reducer::Poll`, a documented
  workaround; `docs/design/key-holding.md`, "The console host's caret
  poll"): the console host raises caret events only on its cursor's blink
  timer, so in its focused text area a watch the first check leaves open
  re-reads the caret every 5 ms until a check finds evidence or 50 ms after
  the key's `pressed_at_ms`, and ends `WatchEnded` when that last re-read
  finds nothing. A watch without `pressed_at_ms` is bounded 50 ms after it
  opened. The poll is one program (`Work::Poll`, `Worker::poll`), which
  the reducer dispatches into a slot of its own when nothing else is ready
  and no other program is in flight, under `HANDLING_DEADLINE`, its query
  the watch's. It waits for each re-read on its worker's inbox
  (`outpost::inbox`), whose high-resolution timer it sets to the next
  read, because a wait at the system's timer resolution, 15.6 ms by
  default, would miss most reads; so each re-read wakes only the worker.
  It commits once: the answer, the watch removed by its request, the caret
  read and the caret sent, then the terminal's read; or the watch ended at
  its last read; or a yield. Its reads' text changes are carried in the
  program to that commit, never committed read by read. Before each read and at each wake it looks in
  its inbox, and a program waiting there other than a release's drop makes
  it yield, committing its next and last reads, its calls so far and its
  last caret read into the watch; the reducer dispatches it again once no
  program is in flight and nothing else is ready, while its watch is open.
  A drop waits for the poll's end, at most its bound. While the poll is in
  flight the reducer dispatches the next program as usual, which rings
  the inbox's doorbell, so the poll yields at once and the program, a
  console caret event's check included, reads at once; that program's
  deadline starts when the poll ends, and the worker applies the pure part
  of the poll's commits, its text changes included, to its copies, and
  gives it the UIA ranges the poll held, since the reducer may have
  dispatched it before taking them in. If the poll passes its deadline,
  its worker is abandoned, its watch ended and answered `Abandoned`, and
  the program waiting behind it is dispatched again to the replacement,
  which an abandoned worker never runs. A poll that panics ends its watch.
  A console caret event for the watched node does not stop the poll when
  its check finds no evidence, because a blink tick left over from an
  earlier cursor set, such as `PSReadLine`'s set that moves nothing, can
  come after the key and before its move. The poll's changes name its
  watch by the request, not the node, so a poll of a replaced watch leaves
  the newer one alone.
- Evidence is the caret away from where it was known to be; the characters
  either side of it changed (only those, since a line can rewrap without a
  key); the text at the caret changed after Delete; the caret's line
  changed after a deleting key (the watch's `deletes`, for a run of one
  character such as a password mask); the selection changed; or the
  selection of the report the caret was known from differs from now, since
  a caret event's report can replace the kept selection before the watch
  arrives (compared only when that report was not already compared, by its
  number). A caret event alone is evidence only when nothing knew the caret.
- The baseline is the newest caret the outpost reported from a read that
  finished before the key's `pressed_at_ms` (the keyboard hook's stamp, in
  milliseconds on the clock of `observed_at_us`), else Core's; never a
  read at or after the key, which may already show its effect. The last eight reports per node
  are remembered, forgotten when the outpost moves the caret itself.
- Before answering, the caret is read again (`CaretRequest::recheck`;
  `CaretRead::moved_while_read` at the end of the same program), and a
  caret that moved meanwhile answers nothing, since its line may be from
  after a write; the following event checks again. Selection changes come
  from comparing endpoints (`CaretRead::changes`).
- Evidence by application: the Windows Forms text box raises a caret
  location change, then `EVENT_OBJECT_TEXTSELECTIONCHANGED` (and a value
  change for Delete); Windows 11 Notepad raises
  `EVENT_OBJECT_TEXTSELECTIONCHANGED` and UIA's text selection changed;
  the console host raises caret events on its blink timer; Windows Terminal
  raises only UIA's text selection changed. A Common Controls 6 edit
  control raises `EVENT_OBJECT_TEXTSELECTIONCHANGED` on every caret move;
  the classic edit control raises nothing for an unfocused one.
- The caret before a held key (`TextOp::ReadCaret`,
  `docs/design/key-holding.md`) is read as a caret event's report, without
  formatting or touching the kept selection (`LastSelection::Ignore`),
  answered as a `CaretReply` with `moved` false, and joins the remembered
  reports as the held key's baseline.

Selections no key made (`docs/parity/text.md`, "TextInfo-equivalent
layer"):

- Each node keeps the selection of its last caret report, but a
  terminal's, which keeps none. The reducer holds it among the node's text
  facts (`reducer::NodeAnchors`, in `reducer::TextFacts`): its anchors and
  the chunks' text, the positions reported, the last eight carets reported
  and the count of reports, the kept selection with its text, and the note
  that the text changed. A position is an edit control's offset, or the
  key under which the store keeps a UIA range, so the facts hold no
  application object and an edit control's offsets cross between programs
  as they are. They change only by a program's commits
  (`reducer::TextChange`: a text call's part put back, but for an anchor
  it copied that a release freed meanwhile; a selection forgotten; a value
  change noted; a node's text forgotten), and the reducer has the store
  let go of each UIA range no fact names any more
  (`Effect::ForgetRanges`). A caret event's report of a focus
  that is not a terminal (`text_reads::CaretFor::Event`) compares with it
  (`LastSelection::Compare`) and sends `selection_changes`; through UIA in
  the same round trip, comparing nothing when both are empty
  (`CaretRequest::previous_collapsed`). It is forgotten at a new focus, and
  replaced by what the outpost made when it selects or moves the caret for
  Core (`NodeText::keep_selection`, `RangeRead::selection_after`). Each
  change is compared once, by whichever read sees it first.
- A report's selection starts where Core can read before it: in the caret's
  line, or an earlier line sent as `CaretReport::selection_line`; through
  UIA, where the caret can be the selection's other end, the caret read
  finds the start (`CaretRead::selection_start`).
- A comparison is in general terms (`text::content_changed`,
  `text::general_changes`: ends only, `content_changed`, and the new
  selection's text in `new_selection`) when the node's note is set (NVDA's
  `hasContentChangedSinceLastSelection`, set by a classic edit control's
  value change, `text_reads::note_value_change`); when the kept selection's
  text no longer matches what its ends hold (through UIA,
  `CaretRequest::read_selection_text`, `KeptSelection::text`,
  `CaretRequest::read_previous_text`); or when the control's value change
  waits once the caret is read, for the Windows Forms text box. That check
  is the reducer's, so the read is cut in two programs there
  (`text::Checked::Cut`, `text::caret_report_checking`,
  `text::check_caret_cut`), only for an edit control's client area with a
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
  [its design](../design/ownership-outpost-reducer.md), "The value change
  check".
- Two waiting caret reports merge with both reports' changes in order
  (`Merge for OutpostToCore` in `verbatim_model::protocol`); one with
  changes never merges across a reply or with a report in general terms.

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
  (`Intake::absorb_console_updates`) while no watch is open, the mark
  cleared before a terminal request, a caret key, or a new focus.
- A focus-now answer's control has its caret read with it, is recorded
  among the reports (`reducer::held`), as NVDA's fake focus is, and then
  becomes the followed focus (`Worker::follow_answer`, as `emit_focus`
  does): another node's watch ends, the intake is told through the
  follow change it commits, and the tracking takes it, unless it is
  already followed under the same node, which is followed again, not
  adopted.

### Queries

- `ShellItems` (`outpost/shell_items.rs`) reads the notification area
  (`TrayNotifyWnd`, with the overflow flyout) or the taskbar
  (`Shell_TrayWnd` without it) in the control view as `ShellButton`s, and
  answers every named, on-screen button as a `ShellItem` (name and runtime
  id). `ShellItemPosition { kind, item }` walks the same way and answers
  `ShellCandidates` with each matching button's `Place` and whether every
  button was read; Core picks (`verbatim-gui`'s `refind`). Both read only
  their application's windows, look the shell windows up without the proxy
  rule (`any_window_element`), stop at `WALK_BUDGET` (2 s) with what they
  read, and read a failed step as no more items, logged as a warning.
- `DumpTree { locations }` walks the foreground window (or the first
  visible top-level window): UIA with `verbatim_uia_rops::tree_dump`, MSAA
  with `verbatim_ia2::acquire::walk_tree`, capped at depth 64 and 4096
  nodes, saying whether incomplete. Locations are read only with
  `locations`, since a title bar's are asked of the UI thread
  (`WM_GETTITLEBARINFOEX`).
- `Ancestors`, `Navigate`, and `Activate` go to the issuing registry: UIA's
  raw-view walker with the base cache, or MSAA's `accParent`,
  `accNavigate`, and `accDoDefaultAction`. An unknown or unreachable node
  answers `Gone`.

### Selection and notification events

Selections emit `NormalizedEvent::SelectionChanged` and UIA notifications
`NormalizedEvent::Notification`. A UIA selection inside an element the
focus names in its ControllerFor relation, or inside none of them (with the
first controlled element's current selection as its node), emits
`NormalizedEvent::ControlledSelection` with the focus's id, as NVDA reports
a search suggestion, unless it names the focus itself; the relation and the
search are one remote operation
(`verbatim_uia_rops::controlled_selection`). A UIA selection or
notification goes through its application's reading thread behind a marker
(`DeliveredFact::UiaMarked`), which finds a non-window sender's nearest
window; one with none is dropped, but for the shell's window-snap
notifications. A waiting notification is replaced by a newer one from the
same element with the same kind, processing, and activity id, as NVDA's
UIA limiter keys it.

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
- An outpost queues it (`Item::IdleQuery`), dividing what waits and ending
  its batch; a noted caret goes just ahead (`Intake::push_ahead`), carrying
  its event's timing, so its read is skipped only when a caret read since
  that event covers it. The reducer reaches the query with no program in
  flight, so nothing before it is unhandled, and answers it
  (`Reducer::answer_idle`) as it is reached and after every input, once the
  hook's copies are no longer held at the outpost's start, no
  caret key's watch is open or held by the program in flight, no console
  poll is in flight, and the gap record is closed and not being read by the
  program in flight. Entries after the query are not waited for, nor is a
  program handling one. An outpost that ends answers by its `Ended`.

## Shutting down

Every child ends by a clean shutdown, the kill being only the fallback
(`docs/architecture.md`, section 1, gives the reason): at Verbatim's exit
(`Supervisor::shutdown`, run while the exit sound plays, after the keyboard
hook is removed; no child starts afterwards), when a child is replaced, and
when an outpost is retired or its application exits.

The supervisor's side (`supervisor::retire`): it sends `Shutdown`, which
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
4. The reducer's thread ends once the program in flight has ended, with no
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
   its `RemoveAllEventHandlers` (`docs/crates/verbatim-uia.md`,
   `Registration`); `Outpost::shutdown` does not, because the thread is
   the process's, shared by every outpost in it, as in a test process.

The outpost logs each step's time (`the outpost shut down`). The listener
removes its hooks and handlers, writes what waits, and leaves COM. Both
then end with `TerminateProcess` and exit code 0 (`docs/architecture.md`,
section 1). The limit outlasts any UIA call (`TRANSACTION_TIMEOUT`, five
seconds, `CONNECTION_TIMEOUT`, two, for classic calls and `Execute` alike)
and a search of several; only an MSAA call into a hung application can
outlast it. A clean shutdown takes milliseconds: over one end-to-end run, 4
to 443 ms from the message to the exit, median 15.

Tests: `crates/mockapp/tests/shutdown.rs` (a shutdown during a held read,
remote and classic: the answer sent, the four UIA registrations removed,
the pipe closed, mockapp still answering; and a pong while the worker is
held), `crates/verbatim-outpost/tests/shutdown_fallback.rs` (a stand-in
that never exits is reported killed at its limit), and
`crates/verbatim-outpost/tests/target_exit.rs` (`TargetExited`, then a
clean exit). The end-to-end suite checks every child exited with code 0.

## Terminals

The `terminal` module (public, so mockapp drives it) reads a focused
terminal and sends each read to Core, whose terminal model aligns it and
applies the reading rules (`docs/design/terminal-model.md`, "Where it runs";
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
  length (one pass, `terminal::unpadded_utf16`, into buffers a written read
  returns, `terminal::recycle`, each kept to `SPARE_KEPT_BYTES`, 64 KiB),
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
  Core's parse of that frame, in a release build (`docs/performance.md`,
  "A terminal's reads in Core"); mockapp's `terminal` and
  `terminal_reading` tests drive a real outpost.
