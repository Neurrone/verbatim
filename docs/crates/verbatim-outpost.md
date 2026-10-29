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

An event the listener receives before its desktop-wide UIA subscriptions
are all installed is dropped (`Outgoing::uia_event_process` in
`crates/verbatim-outpost/src/listener.rs`), as NVDA drops events before its
global handlers are registered; Verbatim's start asks the foreground
application's outpost for its focus instead (`ShellCommand::FocusNow`).

### The hand-over of an application's MSAA events

An outpost's own process-scoped hook takes in all its application's MSAA
events, focus, foreground, menu-popup, alert, and tooltip included, so they
reach it by one route in the order Windows raised them
(`docs/design/focus-pipeline.md`, section 12.4). The listener forwards
those events only until the outpost's hook is in, so that an application's
first events start its outpost:

- An outpost reads the `WinEvent` clock (`verbatim_ia2::event_clock`)
  before it installs its hook, and says `Ready` once it is in.
- On `Ready` the supervisor sends the listener `HookInstalled` (application
  and incarnation). The listener reads the clock, forwards none of that
  application's facts raised after that tick, still forwards those raised
  at or before it, and queues `ForwardingEnded`, naming the tick, behind
  every fact queued; the supervisor relays it to that incarnation, or drops
  it if the incarnation has gone.
- Every `WinEvent` fact carries its raising thread and time
  (`WinEventStamp`). The outpost matches the two hooks' copies by counting
  (`outpost::handover`): a copy whose kind, address, thread, and time the
  other source has an unmatched copy of is dropped, any other is handled
  and counted, so nothing from one source alone is dropped. Copies are
  queued under the hand-over's lock, in the order judged. Only copies
  raised between the two clock readings are counted after
  `ForwardingEnded`, and only for `handover::TWIN_BOUND` (15 seconds).
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
  dropped before any call; a shell window that only stages a switch is read
  and Core drops it. Repeats are judged against the focus Core holds
  (`docs/architecture.md`, "Where the rules run (D14)";
  `verbatim_core::focus_rules::repeats_focus`): Core tells the listener of
  every judgement and focus change (`CoreFocus`), the readers share the
  elements of the focuses they accepted (`focus_read::HeldElements`), and an
  event whose sender `CompareElements` equates with Core's focus while that
  element's `HasKeyboardFocus` reads true is dropped; a failed comparison
  lets it through. Nothing is compared while an accepted focus awaits
  Core's judgement, after Core refused it, or once Core holds a focus no
  listener read gave it. Then the acceptance test against the foreground
  window (`outpost::acceptance`, shared with the outposts) on the element's
  own or nearest window (`verbatim_uia::nearest_window_handle`); the
  outpost does not test a UIA focus again.
- The read (`verbatim_uia_rops::focus_ancestry_classic`) makes every call
  on an element the listener holds, with no remote operation and no
  connection: whether the element with the same runtime id as the previous
  focus still has the keyboard focus (`UiaFocusRead::same_as_last`); value,
  states, and details afresh; a list's or tab control's selected child, or
  for a combo box without `Value` or a looping selector the first selected
  item as its value (`UiaFocusRead::value_from_selection`), left out when
  its read times out or finds it gone; the raw-view
  ancestors with their window handles, up to the first the thread knew or
  the desktop root, never across the process boundary
  (`verbatim_uia::parent_across_process_boundary`); and a menu item's
  legacy MSAA state. A read that fails reports the focus from its event,
  ancestors unknown. Each read is logged at debug with the sender's runtime
  id, class, framework (read only with debug logging on), and window.
- The read (`DeliveredFact::UiaFocusRead`, whose `focus` says focus or menu
  opening) or the drop (`DeliveredFact::UiaFocusDropped`, whose `focus`
  says the same) follows the marker through verbatim.exe, never coalesced
  or dropped. A focus dropped on the listener's own verdict on its nearest
  window (`Judged::DroppedOnVerdict`) carries `on_verdict`, a
  `VerdictDrop`: the window and `DroppedVerdict::NotUia`, or
  `DroppedVerdict::Unknown` when its raw probe did not answer in time, of
  which the reading thread keeps nothing. A focus whose nearest window's
  lookup timed out (`focus_read::NEAREST_TIMED_OUT`) is dropped with no
  other call.
- Budget. A read has one budget, `READ_BUDGET` (2 seconds), asked before
  each call after the first (`Budget`), not counting the raw probe's wait
  of up to 3 seconds. Past it the read makes no calls but still makes
  every check that needs none, taking the keyboard focus from the cached
  `HasKeyboardFocus` (also when a live read fails by UIA's transaction
  timeout); the repeat check is skipped, a deliberate divergence from NVDA.
  A call over a second is logged at info (`FocusQuery::before_call`).
- Watchdog. A read past `READ_DEADLINE`, 11 seconds (the budget, the
  probe's 3, the 5-second transaction timeout, and a second), is abandoned;
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
  abandoned read is marked (`Reading::abandoned`, `HeldElements::abandoned`)
  so a late return keeps nothing, the event is reported dropped, and a new
  thread takes later events. With `ABANDONED_LIMIT` (two) threads still in
  calls the application gets no new thread and its events wait;
  `READ_DEADLINE` after that (`App::limit_reached`) every waiting and later
  event is dropped until a thread returns.
- When the listener is replaced, the supervisor tells every outpost its
  reads have ended (`DeliveredFact::UiaFocusReadsEnded`).

The gap record (`gap_record`; `docs/design/focus-pipeline-rework.md`, "Who
owns each duplicated fact", the gap's evidence;
`docs/design/listener-without-connections.md`, decision 11). UI Automation
sometimes delivers an application's focus late or never while its
`WinEvent` focus comes, so the listener keeps, in the reading threads'
shared state, one record per application whose outpost has its hook in
(`Readers::hook_installed`, ended by `Readers::outpost_ended`): the oldest
focus `WinEvent` copy no UIA focus has answered, with its window,
top-level window (`window::top_level_of`) and microsecond stamp, on one
clock, with no call and no verdict of its own.

- Opening. The `WinEvent` callback gives every focus and foreground event
  to the records (`Readers::observed`, `gap_record::Observed`). A focus of
  a hooked application with no record open opens one, unless a UIA focus
  observed up to 100 ms before it (`gap_record::ANSWERED_BEFORE_US`), in
  its top-level window, has already answered it.
- Answers. A UIA focus event observed within 100 ms before the copy or
  after it answers it once its outcome is reported, under the readers'
  lock (`focus_read::gap_answer`, `gap_record::Answer`): a read, a drop on
  the listener's verdict, or a verdict of NVDA's checks
  (`focus_read::VERDICTS`: hung, not related to the foreground, the
  console window's own focus, the hidden frame, a repeat) answers a copy
  in the top-level window of the window the read found, the cached
  window or, for a windowless sender, its nearest window, found outside
  the lock; the listener giving up on the application
  (`focus_read::GAVE_UP`, the timed-out nearest-window lookup among them)
  answers a copy in any window; and a drop for no window above the sender
  (`focus_read::NO_WINDOW`) answers it to be read at once, focus-missing
  sent behind the drop, as is a later copy that drop answers. A focus or
  foreground `WinEvent` observed in another top-level window of the
  application or in another application answers it too. A drop for no
  keyboard focus (`focus_read::NOT_FOCUSED`), one a newer event of its
  element replaced, and the other drops that could not establish the
  focus leave it open, since they come in fast navigation; so does
  anything but a focus.
- The bound (`gap_record::BOUND`, one second from the copy's
  observation) is armed by the callback when the application has no UIA
  event waiting or being read, waking the watchdog, and otherwise by the
  reading thread when the application's reads finish with the record
  open (`Readers::push` disarms it); the readers' watchdog times it, so no
  reading thread waits on it. Unanswered at the bound, the watchdog sends
  `DeliveredFact::FocusMissing` with the copy's window, stamped with its
  observation (`Reading::focus_missing`), relayed in order and never
  dropped (`DeliveredFact::is_focus_read`).
- The idle barrier's round waits, in the listener, for every record
  opened before it (`Readers::when_idle`), so a read for focus-missing is
  queued ahead of the outpost's query: at most about 12.2 seconds for one
  focus marker on an application that answers (the bound, the read
  deadline, the relay, and the read), within the harness's 15-second idle
  timeout.
- The outpost (`Worker::focus_missing`) reads its application's focus
  only when its own verdict for that window is UIA, with the focus-now
  read (`read::focused_control_in`, trigger `read::FocusRead::FocusMissing`),
  and reports it as a UIA focus, logged at info with whether a listener
  UIA focus had reached it before the copy (`uia_focus_before`) and
  whether the copy's window was gone (`sender_gone`), stamped with the
  copy's observation so Core's stale rule drops it when a later focus
  elsewhere was applied first; a late UIA focus of the same element is
  reported under the read's node, which Core takes as a duplicate. In a
  window it judges MSAA, or that its application no longer serves, it
  does nothing, having handled its MSAA copy.

The verdicts. The listener and the outpost judge a window's backend with
the same code (`arbitration`) and send no verdict to each other; the
outpost's decides which backend's focus counts in each of its windows
(`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated fact",
the verdicts). A listener's read in a window the outpost judges MSAA, by
the read's nearest window, is dropped (`Worker::uia_focus`). A drop with
`on_verdict` takes its marker's place in the intake, as a read does, and
the worker judges its window (`Worker::focus_dropped_on_verdict`): in a
window it judges UIA it reads the application's focus at once with the
focus-now read (`read::focused_control_in`), stamped with the dropped
event's observation; in one it judges MSAA it does nothing. Such a drop
answers the listener's gap record, so that read is made once.

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
for it (`read::FocusRead`: a focus-now query, focus-missing, the read
after a listener's drop on its verdict, or a cut walk, below). The one connection the call makes into
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
the window below the boundary, and the outpost continues the chain from
that window's parent or owner through MSAA (`read::uia_chain_onward`), as
NVDA switches API at a window boundary, even for a parent with a UIA
provider (`docs/design/listener-without-connections.md`, sections 4.2 to
4.4, decisions 7 and 13). The outpost connects to, probes, and walks up
into only its application's windows (`read::serves`): its UIA walk
(`read::uia_ancestors`) stops below the boundary, a step that reaches
another process's window anyway switches to MSAA there
(`read::own_window_uses_uia`), the MSAA walk does not probe such a window,
and a focus route through one is not followed (`Context::focus_route`). The
accepted exception is object navigation's raw-view step (`Uia::neighbor`),
which reads such a neighbor through UIA before `read::corrected_backend`
switches it to MSAA.

### Starting an outpost and asking for the focus

A fact for an application with no outpost starts one
(`DeliveredFact::may_start_outpost`) unless it is a selection or a
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
`Ready` the app hears `Reincarnated`, then the failed one's `Ended` if it
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

`protocol` is the wire vocabulary the supervisor and each child speak.

- `SupervisorToOutpost`: `SetBackendOverride` (one backend for every
  window, or normal arbitration); `DeliverFact` (a routed fact: a UIA
  focus's marker, read, or drop, focus-missing, an MSAA focus or
  menu-popup address, a foreground window, with the listener's trace id, its `EventTiming`, whose
  `observed_at_us` is the observation the fact is ordered by, and for a
  `WinEvent` its `WinEventStamp`); `HookInstalled`
  and `HookRemoved` (to the listener) and `ForwardingEnded` (to an
  outpost); `CoreFocus`; `Query`; `Cancel` (withdraws a query not
  started); `Ping`; `IdleQuery`; `Shutdown`; `Assign`; `NodesHeld`; and
  `Fetches`.
- `CoreFocus` is sent to the listener, to the outpost whose focus event
  Core just judged, and to the outposts whose focus Core's focus was or now
  is, whenever Core judges a focus event or its focus changes: `judged`
  (the trace judged), `focus` (a `verbatim_core::focus_rules::HeldFocus`:
  trace, focus and ancestors, top-level window, alive; to the listener
  without ancestors, to an outpost only when the focus is its own, with its
  own node ids, `HeldFocus::for_outpost`).
- `Query` is a request id and a `Query`: `FocusNow`, `Navigate` (a model
  `QueryKind`), `Activate`, `Ancestors`, `DumpTree`, `Text` (a node and a
  model `TextOp`), `ShellItems`, or `ShellItemPosition`.
- `OutpostToSupervisor`: `Ready`; `SpareReady`; `Event` (trace id,
  observation time in microseconds from one clock read, for every event
  kind, backend, `WindowFacts`, normalized event); `Reply` (exactly
  one per accepted query, with its `EventTiming` and a `QueryOutcome`:
  `Done` with a `QueryResult`, `Gone`, `Failed`, `NotStarted`, or
  `Abandoned`, after which side effects may have happened); `Pong` (the
  ping's sequence number and the count of abandoned workers not returned);
  `Fault`; `FocusFact`, `ForwardingEnded`, `MenuOrSwitchEnded`, and
  `ForegroundWindow`, from the listener only; `TargetExited`, once, ahead of
  ordinary messages; and `IdleAnswer` (`is_idle_answer`).
- A `FocusNow` answer carries the foreground window and its facts when the
  application holds the foreground, its focused control with ancestors,
  selected child, window facts, and text read (`FocusedControl::text`), and
  when reading began, which orders it among focus events.
- A `ListenerFact` strips to a pid-less `DeliveredFact` once routed. Node
  ids from Core carry the outpost's stamp, cleared for lookup
  (`NodeId::unstamped`); `OutpostToSupervisor::assign_outpost` applies it.
  `WALK_DEADLINE` (5 seconds) is public
  so Core's deadline for a shell walk can outlast it.

Timing (`docs/design/communication.md`, section 10.1). `EventTiming`
records, in microseconds from `now_us`, when an event was observed, relayed,
taken from the outpost's queue, and published, how long before observation
Windows raised a `WinEvent`, and the worker's cross-process calls (`calls`,
a `CallCounts`); a caret key's late answer adds `awaited_at_us` and
`awaited_calls`. verbatim.exe adds `core_received_at_us`, `core_relay_us`,
`core_held_us`, `core_sent_at_us`, and `core_read_at_us`
(`OutpostToSupervisor::core_read`); a `Query` carries the first three to the
outpost, which copies them into its reply. Each message is stamped as its
write begins, by whichever thread writes it
(`OutpostToSupervisor::written`): `listener_written_at_us` on a
`FocusFact`, `written_at_us` on an `Event`, `Reply`, or `Pong`; the listener
adds `listener_handled_at_us`. Commands to the listener carry `sent_at_us`
(`SupervisorToOutpost::stamp_sent`), and the listener logs each hop at trace
under `verbatim::stage`. `hop_us` is the time between two stamps.

Framing. Each message is one frame of `verbatim_process::transport`, a
`Content-Length` header and compact JSON, through `write_message` and
`read_message`, for any type that implements `Message` (`put_frame` and
`from_parts`): `SupervisorToOutpost` and `OutpostToSupervisor`.
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

`Arbitrator` is NVDA's per-window backend decision (`_isUIAWindowHelper`).
`verdict(hwnd, &WindowClasses)` answers from the good class list, the
Windows 11 shell rule, the bad class list, a `SetBackendOverride`, or a kept
probe, and `None` when only the probe can decide; the window is then
probed with the raw probe (`verbatim_uia::raw_probe`), by the worker
through `arbitration::uses_uia_keeping` and by the listener through
`arbitration::window_verdict`, and its answer recorded (`record_probe`).
Each process keeps its own verdicts; neither copies the other's. No
connection follows a probe (`docs/design/listener-without-connections.md`,
decision 2).

- `WindowClasses::of(hwnd)` reads the raw class, the class normalized as
  NVDA does (`normalize_class_name`), and the root ancestor's class.
- `post_probe_check` names NVDA's check before using a provider: a list
  view's Windows Forms origin, or a console's formatted text (read by an
  outpost from the window and by the listener from the sender). A provider
  that fails is recorded with `record_excluded`, MSAA for the window's
  lifetime.
- A "yes" is kept for the window's lifetime and dropped by `forget` on
  destruction (decision D15). A "no" lasts `NEGATIVE_VERDICT_LIFETIME`
  (500 ms, NVDA's cache period), since a busy application answers late. A
  timed-out probe is an unknown verdict (`arbitration::Verdict::Unknown`):
  the listener keeps nothing of it, and an outpost's worker reads the
  window through MSAA and keeps it as "no" for the rest of the entry that
  made it, on that thread, and for every thread for
  `NEGATIVE_VERDICT_LIFETIME` from the probe's end, never renewed with the
  entry (`record_timed_out`, `begin_entry`); a verdict kept for the
  window's lifetime meanwhile stays.
  `lasting_verdict` answers only a verdict that lasts the window's
  lifetime.
- `Clock`, `Arbitrator::set_clock`, and `Arbitrator::now` time the verdicts
  and the classic walk budgets (`PATH_BUDGET`, `ENRICHMENT_BUDGET`, through
  `arbitration::time_limit`); `Outpost::set_clock` and
  `FocusReader::set_clock` set it for tests that count calls.
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
  `GetForegroundWindow`, for acceptance and the window a focus entered); and
  `set_focus_window_reader` (a `FocusWindowReader` for focus-now).
- `Outpost::settle` waits until everything queued is handled, follow-ups
  included, the subscription has made every move asked, every published
  message is written, and the event thread has taken every nudge. It covers
  only what has reached the outpost.
- `Outpost::failed_reads` gives the worker's and subscription callbacks'
  failed cached reads (`verbatim_uia::calls`), also logged at shutdown.
- `Outpost::observe_heard` hands a `HeardObserver` each event the outpost's
  own handlers and hook take in, as a `Heard`, once queued.

Inside an outpost (`docs/design/communication.md`, section 6.4):

- The intake (`outpost::intake`): the hook callbacks (`APP_SUBSCRIPTIONS`),
  the subscription's callbacks, routed facts (each matched first by
  `outpost::handover`), and queries only add an entry and return. A UIA
  callback keeps the cached parts, window handle, and an agile reference.
- The focus-following subscription (`docs/design/focus-pipeline.md`, section
  17): one registration with NVDA's two groups. The base group, given every
  UIA focus whose element the outpost has, holds name, value, and state
  changes and `Text_TextSelectionChanged`; the extended group, for a focus
  with text read through UIA, adds `Text_TextChanged` and the active text
  position changed event. The console host's text area takes the group
  without its text events (`Group::ExtendedWithoutTextEvents`), and the
  console's caret events are its caret events
  (`docs/design/wake-ups.md`, item 15). Both are on the focused element
  only (`docs/parity/backends.md`, "UIA event registration"). Elements
  arrive as `Cached<PropertyEvent>` or `Cached<TextEvent>`, carried as
  `intake::EventElement` and resolved as `Kept`. The worker moves the
  subscription with each focus it reports (`follow_focus`): add the group to
  the new focus, then `RemoveEventHandlerGroup` from the old;
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
  each under its own deadline, until one is reported, at the run's newest
  place, as NVDA's `pumpAll` does (`ends_a_run`, `Planned::handle`); three
  focus candidates per backend are kept (`Planned::Focus`). A foreground
  change removes a waiting focus on its client object from the same thread
  not raised after it. A run whose events all carry raise times is tried in
  that order (`in_raised_order`), ties by the hand-over's `Arrival`. UIA
  focus events are handled at their own places, the newest menu opening
  per backend last.
- Markers: nothing that arrives after a UIA focus's marker is planned until
  the read, its drop, or the end of reads resolves it, as NVDA's UIA events
  pass through one flusher thread. When the read's ancestors include the
  focus last reported, that old focus's own state changes queued after the
  marker are planned ahead of the read (`old_focus_events_first`), since a
  provider moving the focus into a child raises the old focus's change
  first but it comes by another route. The check compares runtime ids.
- The worker (`outpost::worker`), one thread, the only one calling into the
  application, handles entries in order, so events and replies leave in
  planned order but for menu openings, a run's older focuses, and a caret
  key's answer. Before any call it applies NVDA's acceptance test
  (`outpost::acceptance`; `docs/parity/focus-events.md`, "Event
  acceptance"): `accepts_event`, a pure function of the event, its window,
  `WindowQueries` (local calls in `SystemWindows`), and `RequestedEvents`;
  its `foreground_relation` goes in the window facts
  (`verbatim_core::focus_rules::ForegroundRelation`). It takes the backend
  crates' per-thread call counts into each published `EventTiming`; an
  entry that publishes nothing logs its calls (`docs/performance.md`,
  "Cancelled traces").
- The worker waits on the intake's alarm (`outpost/alarm.rs`): an event set
  by a push while it waits, and a high-resolution timer set to its
  requested wake (`Intake::wake_at`). Only the worker sets a wake.
- The watchdog waits on an alarm the worker sets per entry and cancels as it
  publishes, waking only when a deadline or grace passes
  (`docs/design/wake-ups.md`, item 3). Deadlines: an event, a focus, or a
  focus-now 10 s, NVDA's watchdog timeout; a navigation or activation
  400 ms; an ancestor walk or tree dump 5 s. After half a second it also
  abandons a worker once the user has moved to another UI thread of the
  same provider process (`abandon_reason`, `next_check`,
  `is_another_thread_of_its_application`). It answers the query
  `Abandoned` and starts a replacement worker. A returning abandoned worker
  publishes nothing, since publishing checks under the watchdog's lock that
  it is in charge; nothing is sent while that lock is held.
- Time limits in an entry (`docs/design/focus-pipeline.md`, section 6): UIA
  calls end at `verbatim_uia::CONNECTION_TIMEOUT` (2 s) or
  `TRANSACTION_TIMEOUT` (5 s); `ENRICHMENT_BUDGET` (2 s) bounds a focus's
  ancestor walk between calls; a UIA dialog's text runs on a
  `verbatim_uia::BoundedClient` within `DIALOG_TEXT_WAIT` (1 s); tree view,
  list view, and edit messages use `SendMessageTimeoutW`; an `IAccessible`
  call is bounded by the entry's deadline.
- The I/O thread, the main thread (`verbatim_process::transport::ChildIo`,
  at `THREAD_PRIORITY_HIGHEST`), waits only on its completion port, hands
  each command to `Outpost::handle_command` (which answers pings itself and
  withdraws an unstarted cancelled query with `NotStarted`), and finishes
  pending writes.

The worker, its reads, the intake, the writer, the protocol, and the
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
  `CaretMoved`. `Anchors` keeps a backend's anchors by node and never
  forgets those in `HeldAnchors` (`Anchors::sharing`), with
  `note_content_changed` and `forget_selection`; `NodeText` is one node's
  (`position_at`, `forget_selection`). `check_caret` checks a caret key's
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
  application's reading thread), `CoreFocus`, `IdleQuery`, and `Shutdown`,
  and ignores the rest. A replacement gets `--hooks-in`
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
- `send_to_outpost(outpost_id, command)` queues a command without waiting,
  failing at once with a `QueueError` when the incarnation ended or its
  queue is full.
- `note_views(attention, holding)`, `core_focus(judged, focus, outposts)`,
  `leave_nodes_held`, `query_idle(round)`, `is_ignored`, and `frames`.
- `SPARES` is two; Verbatim's readiness waits for them.

The app hears through `OutpostMessage`: `ListenerReady`, `SpareReady`,
`Reincarnated`, `IdleQueried`, `NotWatched`, `ForegroundWindow`, and per
incarnation `Started`, its messages as unparsed `Frame`s, and `Ended` with a
reason (exited, killed, retired, target exited). `Frames::arrived` (from
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
  frame in the pass that read it: a listener's fact is routed and written to
  its outpost there and then; an outpost's frame is read only for its kind
  (`message_kind`), a pong and `TargetExited` handled, `Ready` handled and
  passed on, anything else handed on whole (`transport::FrameBody::Owned`
  or a copy). Helpers report through `relay::Mailbox`. It never blocks:
  launches, retirements, and hang checks run on threads of their own.
- Each child's commands go through a `transport::Sender` (`writer`): the
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
- A foreground fact in front is also dropped, uncounted, when the last
  focus or foreground change reported (`Tracking`'s queued focus) is in its
  window or a child (`window::is_descendant_window`), or came from its own
  address as the client object, and no focus Core queued from another
  application has replaced it: NVDA's two tests
  (`worker::foreground_repeats_focus`).
- Otherwise it is reported at once as a focus on the window, stamped when
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
- Focus-now answers the foreground window, when the application holds the
  foreground (`GetForegroundWindow`, never `hwndFocus`), and the focused
  control (`hwndFocus`, through its window's backend) with its ancestors or
  them unknown (`ancestors_unknown` in Core), selected child, and caret
  (`read_focus_text`; a console's text area reads as a nameless terminal,
  `read::console_text_area`). There are no retries.
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
  `verbatim_ia2::WindowCache`), asking the arbitrator only until
  `lasting_verdict`; an entry goes on destruction or when the handle is
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
  from nudges for routed facts (`event_thread::Nudger`); the worker nudges
  when it keeps a UIA verdict.
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
  16.2), its backend decided, its runtime id given a new node when the
  listener read that the earlier element under it lost focus or is gone. Its
  ancestors are the listener's chain, filtered, spliced at the previous
  chain, and continued through MSAA (`read::uia_chain_from_read`). Every UIA
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
  `accFocus` redirect (`focus_candidate`). It is dropped as a repeat
  (`focus_rules::repeats_focus`) when Core's focus and every unjudged focus
  of this outpost is the object it names, counting as reported, and
  accepted only when it or an ancestor has the focused state, read live.
  A child's own focus event from the same `SetFocus` as its control's
  redirected one is dropped (`Intake::take_redirected_focus`).
- Focuses events are judged against (`outpost::held`;
  `docs/architecture.md`, "Where the rules run (D14)"): Core's focus when it
  is this outpost's, and every focus reported that Core has not judged. A
  change is read when it can be spoken for any; a focus is a repeat only of
  all. With each the outpost keeps its MSAA address, whether a simple child,
  and the window in front as it was entered. The followed focus
  (`Tracking::focus`, `focus_role`, `window`, `chain`) is what the outpost
  reads text from; no rule judges by it.
- MSAA name, value, and state changes are spoken by the reducer's rule
  (`focus_rules::change_target`): name, description, and value for a focus,
  state for a focus or ancestor. The object is identified
  (`EventObject::which_of`, against `focus_rules::change_candidates`)
  before any read, and only the named property is read. A newly expanded
  `SysTreeView32` item carries its child count; a progress bar's value
  change reads role, states, value, and location (`EventObject::progress`).
  A selection on a focus or ancestor is a state change
  (`focus_rules::selection_target`); any other is read only inside a
  focused selection container (`focus_rules::is_selection_container`).
- Ancestors (`read::uia_remote_enrichment`, `read::uia_enrichment`,
  `read::msaa_enrichment`) stop at the previous chain and splice it, within
  `ENRICHMENT_BUDGET`. A walk that fails or finds no parent below the root
  (or below a top-level window, for the remote program and
  `Uia::ancestors`) reports the ancestors unknown, never empty
  (`Ancestry::ancestors_unknown`, `AncestorWalk::Unknown`). The listener
  does not stop its next read at such a focus. Object navigation's ancestor
  query reads the whole chain.
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
  description of every newly entered dialog in the chain
  (`focus_rules::already_entered`), and of a dialog focus, without one,
  while descriptions are read, so Core speaks it where NVDA does. The
  splice carries it while the focus stays inside, and a foreground report's
  (`Tracking`'s `dialog`) serves the focus that follows.

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
- Held objects: both registries keep every node reported with its live
  element or object. Each message that carries node ids
  (`OutpostToSupervisor::carries_nodes`) takes the next position as it is
  written, and the nodes issued or looked up since the last publish are
  recorded at it, so positions are those Core counts. The app sends
  `NodesHeld` (the reducer's nodes and text anchors in that outpost and the
  last position handled) when any changes, never on its own: it waits in the
  writer's slot (`Supervisor::leave_nodes_held`) for the next command, a
  request or a ping within three seconds, behind every command queued
  before it. The worker in charge then releases every unheld node reported
  at or before that position, under the watch lock and the sender's lock,
  dropping the objects after, so a replacement worker never reports a
  vanishing node. A released node answers `Gone`. Only the newest list is
  kept.

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
learned from its answers and kept per node (`text_support`); spelling and
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
  changes; otherwise the watch stays open (`text_reads::OpenWatch`, at most
  one) and the worker returns to its queue. Each later caret event, text
  change, or text selection change of the node checks it again, unless
  observed before the caret was last read; the answering event's own report
  is not sent. A read that covers a waiting caret event observed after the
  key's press counts it (`text_reads::Signal`). While a check reads, the
  watchdog treats the request as running.
- A watch ends with `TextReply::WatchEnded`, which Core does not speak,
  when the next key's watch replaces it, a focus on another node is
  reported, or at `CARET_WATCH_BOUND` (10 s, `Item::Wake`). So a key that
  moves nothing is silent (`docs/parity/text.md`, "TextInfo-equivalent
  layer").
- The console host's caret poll (`outpost/caret_poll.rs`, a documented
  workaround; `docs/design/key-holding.md`, "The console host's caret
  poll"): the console host raises caret events only on its cursor's blink
  timer, so in its focused text area a watch the first check leaves open
  re-reads the caret every 5 ms until a check finds evidence or 50 ms after
  the key's `pressed_at_ms`, and ends `WatchEnded` when that last re-read
  finds nothing. A watch without `pressed_at_ms` is bounded 50 ms after it
  opened. The worker makes each re-read itself, woken by its intake's wait
  on a high-resolution timer, because a wait at the system's timer
  resolution, 15.6 ms by default, would miss most reads, and because a wake
  time only the worker sets cannot be overwritten by another thread's. A
  console caret event for the watched node does not stop the poll when its
  check finds no evidence, because a blink tick left over from an earlier
  cursor set, such as `PSReadLine`'s set that moves nothing, can come after
  the key and before its move. A re-read is matched to its watch by the
  request, not the node, so a worker the watchdog abandoned during a
  re-read leaves its replacement's watch alone.
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

- Each node keeps the selection of its last caret report (`NodeAnchors`),
  but a terminal's, which keeps none. A caret event's report of a focus
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
  waits once the caret is read (`text_reads::value_change_raised`,
  `CaretSignal::text_change_waiting`), for the Windows Forms text box. That
  check first syncs with the `WinEvent` thread (`Nudger::sync`), which
  raises a `verbatim_ia2::SyncMarker` event, since a posted message can
  overtake a `WinEvent`. So "x" typed over a selection in Notepad says
  "selection removed" where NVDA says "x unselected".
- Two waiting caret reports merge with both reports' changes in order
  (`outbound::merged`); one with changes never merges across a reply
  (`Pending::waiting_with`) or with a report in general terms.

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
- A focus-now answer's control has its caret read with it, is recorded as
  an unjudged focus (`outpost::held`), as NVDA's fake focus is, and then
  becomes the followed focus (`adopt_focus`, as `emit_focus` does): another
  node's watch ends, the intake is told (`note_focused`), and the tracking
  takes it, unless it is already followed under the same node (File
  Explorer's reused runtime ids get a new one, `docs/uia-bugs.md`). It is
  followed (`follow_text`) but not counted as reported
  (`Tracking::reported`).

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
Nothing that sends ever waits, so the watchdog can always abandon a worker.

- Pongs, `Ready`, and `TargetExited` go ahead of everything waiting.
- Waiting events merge by node and kind (the same property, or the same kind
  of focus), the newer at the back, but not past a caret report with
  selection changes that has a reply after it, and never a terminal's read.
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
  queued before it is waiting or being read and every gap record opened
  before it is answered or sent as focus-missing, behind a menu end still
  in its grace and every earlier fact.
- The relay then asks every outpost in the same pass, telling the app which
  first (`IdleQueried`); outposts being launched are asked once every launch
  has reported, a starting outpost behind its held facts. The query gets
  through a full queue.
- An outpost queues it (`Item::IdleQuery`), dividing what waits and ending
  its batch; a noted caret goes just ahead (`Intake::push_ahead`). It is
  answered once nothing before it is handled and no caret watch or
  terminal read owed to Core remains, checked after every entry; terminals'
  owed requests have their own lock, never held across a call. Entries after
  the query are not waited for. An outpost that ends answers by its `Ended`.

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

1. The intake closes and drops what waits.
2. The `WinEvent` hook and its thread go.
3. The subscription's groups are removed (`RemoveEventHandlerGroup`), each
   waiting for callbacks in progress.
4. The watchdog ends, then the worker finishes its entry and every
   abandoned worker and bounded-client thread returns; no call is cut off,
   since UIA's timeouts end it.
5. Every node leaves both registries, with its text patterns, anchors, and
   a terminal's kept start.
6. What waits is written, every write in flight completes
   (`ChildIo::finish`), and the pipe closes, since an exit would cancel a
   write (`docs/design/communication.md`, section 4.6).
7. The thread leaves COM's apartment and the process's hold is given back
   (`verbatim_uia::release_mta_usage`), ending the registering thread with
   its `RemoveAllEventHandlers` (`docs/crates/verbatim-uia.md`,
   `Registration`).

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
