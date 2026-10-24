# verbatim-outpost

The per-application outpost process, the focus listener, the Core-side
supervisor, and the private protocol between them (architecture sections 1
and 4, decisions D9 and D13, implemented in full: one outpost per
application, many at once, with a dedicated listener that detects focus
and the outposts announcing it).

An outpost's target application is set once and never retargeted: the pid
arrives on its command line (`--target-pid`), or, for a spare started
before it had an application, in the supervisor's `Assign`; its
`WinEvent` hooks install once, when it has its application, its UIA
registration once during construction, and there is no rebind path.
Outposts stay alive when their application loses foreground.

## How focus reaches an outpost

Focus detection is a separate process (decision D13). The focus listener —
the same `verbatim-outpost.exe` run with `--listener` and no target pid —
holds the one desktop-global UIA focus registration and the global MSAA
hooks for focus, foreground, and menu-popup opens (process id zero). Its
callbacks never make a cross-process call: they read only what each event
carries (a UIA element's cached properties, an MSAA event's raw address)
plus the hang-safe `GetWindowThreadProcessId`, and forward each captured
fact to Core. A UIA focus is the
exception: the listener judges and reads it from the event's own element,
on a thread for the element's application (below). The supervisor routes
every fact to the target application's own outpost, which acquires,
arbitrates, enriches, and announces it exactly as it does for the events
it still hooks itself.

### The hand-over of an application's MSAA events

Following `docs/design/focus-pipeline.md` (section
12.4, and decision 2 of its section 15), an outpost's own process-scoped
hook takes in its application's MSAA focus, foreground, menu-popup,
alert, and tooltip events too, so every MSAA event of the application
reaches it by one route, in the order Windows raised them. The listener
forwards those events only for an application whose outpost's hook is not
yet in, so that an application's first events start its outpost. The
hand-over, step by step:

- An outpost reads the `WinEvent` clock (`verbatim_ia2::event_clock`)
  before it installs its hook, and says `Ready` once the hook is in.
- On `Ready` the supervisor sends the listener
  `SupervisorToOutpost::HookInstalled`, with the application and the
  outpost incarnation. The listener reads the clock, forwards none of that
  application's `WinEvent` facts raised after that tick, still forwards
  those raised at or before it however late they reach its hook, and
  queues `OutpostToSupervisor::ForwardingEnded`, naming the tick, behind
  every fact it has queued. The supervisor relays it to that outpost
  incarnation, in order behind the facts, as
  `SupervisorToOutpost::ForwardingEnded`; one for an incarnation that has
  gone is dropped.
- Every `WinEvent` fact carries the thread that raised it and when
  (`WinEventStamp`, from `idEventThread` and `dwmsEventTime`), whichever
  hook took it in. The outpost matches the two hooks' copies of an event by
  counting (`outpost::handover`): a copy whose identity, its kind, address,
  thread, and time, the other source has an unmatched copy of is dropped,
  and any other is handled and counted. Two identical events raised by one
  thread in one tick are two copies from each source, matched pair by
  pair, so both are handled; nothing that came from one source alone is
  dropped. Each copy is queued under the hand-over's lock, so the intake
  receives the copies in the order they were judged. A listener copy
  raised before the outpost's first clock reading has no twin and is not
  counted; after `ForwardingEnded`, only copies raised between the two
  readings are counted, a handful at most, and only for
  `handover::TWIN_BOUND` (15 seconds) after it arrived, by the outpost's
  monotonic clock, after which nothing is counted again.
- When an application's ready outpost ends, for any reason, the
  supervisor sends the listener `SupervisorToOutpost::HookRemoved`, and the
  listener forwards that application's events again. A listener that
  replaces another is told on its command line (`--hooks-in`) which
  applications have outposts with their hooks in; it forwards none of
  their events and first sends each such outpost a `ForwardingEnded`, which
  ends a hand-over the listener it replaces left unfinished. Messages for
  the listener while its launch is at work are held, in order, and written
  once it has a writer.
- A console window's events reach the console host's outpost by its own
  hook, which Windows scopes by the event's thread, the console host's,
  as the listener routes them (below).

### A UIA focus, judged and read by the listener

A UIA focus (`docs/design/focus-pipeline.md`,
section 12, option (b), and decisions 1, 3, and 9 of its section 15) is
judged and read by the listener, since a UIA element cannot be handed to
another process and the listener is the one process that receives the
event's own element:

- The listener's focus registration and its desktop-wide event handler
  group are shaped as `verbatim_uia::ListenerEvent` (`Part::shaped`): NVDA's
  base cache request, with `IsDialog` and
  `IsTogglePatternAvailable` added (`docs/design/uia-shapes.md`, decision
  6), so the events' elements arrive with their process, window, runtime
  id, control type, name, keyboard focus, NVDA's other base properties,
  and what refines a role, and every callback receives a
  `Cached<ListenerEvent>`. The role is read through `RoleCore`, refined
  for dialogs and toggle buttons as every other read refines it, which
  departs from NVDA's base request at the cost of two properties per
  desktop-wide event; `ListenerEvent::parts` gives the role, the name,
  and the focused state, nothing else. The reading thread takes the
  value, states, and details from its read of the sender, and an outpost reads a selected item it was sent from its
  focus's subtree (`Worker::fresh_selection`); a menu opening and a
  notification are reported from the role and name.
- The listener's focus callback makes no call. It sends the element's
  application a marker (`DeliveredFact::UiaFocusMarker`, numbered by the
  listener) and queues the element, as an `Agile<ListenerEvent>`, for that
  application's reading thread (`focus_read::Readers`): one thread per
  application, created on its first focus event, and ended once the
  events waiting for it are read when the supervisor's `HookRemoved` tells
  the listener the application's outpost has ended
  (`Readers::outpost_ended`), or at the listener's shutdown, so it keeps
  its last focus for as long as the application has an outpost
  (`docs/design/focus-pipeline.md`, section 16.6). A later event of an
  application still running starts a new thread.
- The thread judges and reads each event in arrival order
  (`focus_read::FocusReader::read`, which takes the sender as the
  `Cached<ListenerEvent>` it resolves, as `FocusReader::check_fact` does
  for a selection or notification, and reads the window, class name and
  framework of its judgment and log from that cache). An event from a hung window, a shell
  window that only stages a switch, or the console window's own element
  is dropped before any call. Then the classic read
  (`verbatim_uia_rops::focus_ancestry_classic`, with `refresh`, and with
  the nearest window the checks found as `nearest_window`), every call on
  an element the listener holds, with no remote operation and no
  connection to any window (`docs/design/listener-without-connections.md`,
  sections 2 and 3, step 8): when the element has the runtime id of the
  focus the thread read before, whether that earlier element still has
  the keyboard focus (NVDA's duplicate evidence, sent as
  `UiaFocusRead::same_as_last`); the element's value, states, and details
  afresh, its name and role staying the event's; a list's or tab control's
  selected child, and a combo box's without the `Value` pattern, whose
  first selected item, like a looping selector's (a time picker's list),
  is sent as its value and not as a selected child, with
  `UiaFocusRead::value_from_selection` set, as NVDA reads such a combo box's
  value (`ComboBoxWithoutValuePattern`); and its raw-view ancestors, one tree walker step each,
  each with its own window handle through the client side's window proxy,
  the console window's title included, up to the first one the thread
  knew, the rest reused from the read before, or to the desktop root, as
  NVDA's raw-view walk does. The element's keyboard focus and nearest
  window, read by the checks just before, are not asked again. The walk
  never steps across the process boundary: an element whose window's
  parent window, or visible owner for a top-level window, another process
  serves (`verbatim_uia::parent_across_process_boundary`), as the Settings
  page's content inside `ApplicationFrameHost`'s frame, is the last
  ancestor it reads, and a step that reaches another process's window, as
  an owner shown between the check and the step makes it, is not kept
  (section 3, step 11). A menu item's legacy MSAA state is one live read
  after the walk. A read that fails
  reports the focus from its event, its ancestors unknown, whatever the
  event's cache says of its keyboard focus, which the checks before the
  read have already judged, as NVDA lets a focus through whose read
  fails.
- The readers share the last focus any of them accepted
  (`focus_read::LastAccepted`), as NVDA keeps one last focus for the whole
  program, and judge a repeat by it before any read, as NVDA does
  (section 16.3 of the focus-pipeline design): an event whose sender
  `CompareElements` equates with that focus, while that element's
  `HasKeyboardFocus` reads true, is dropped, its marker resolved as a drop;
  a comparison or a read that fails lets it through. Then NVDA's
  acceptance test against the foreground window, with the outpost's port
  of it (`outpost::acceptance`), on the element's own window or, one call
  (`verbatim_uia::nearest_window_handle`), its nearest ancestor's: a focus
  that fails is dropped before it is read and is never recorded as the
  last accepted, which only a focus that passed and was read becomes. The
  outpost does not test a UIA focus itself. Each read is also
  logged at debug level: the sender's runtime id, class name, framework,
  and own window handle, and the comparison's result; the framework is a
  call made only while debug logging is on. Each thread's own focus before
  still serves the ancestor read's stop and `same_as_last`, the evidence
  that a runtime id still names the element it named, by which the
  outpost gives a reused runtime id a new node and keeps the element it
  found for a node.
- The read (`DeliveredFact::UiaFocusRead`, a `UiaFocusRead`) or the drop
  (`DeliveredFact::UiaFocusDropped`) follows the marker through
  verbatim.exe, never coalesced and never dropped on the way: the
  listener queues them unkeyed, and the supervisor's writer lets them
  through a full queue in order. A drop says, in `evidence`
  (`DropEvidence`, set by `focus_read::drop_evidence` from the drop's
  reason and the event's kind), what it tells an outpost waiting for a
  UIA focus: `Focus`, a drop that ends the wait, either a verdict of
  NVDA's checks on the event (`focus_read::VERDICTS`: its window hung, not
  related to the foreground window, the console window's own, or a shell
  window that only stages a switch, or a repeat of the focus that still
  has the keyboard focus, or the comparison of a menu opening with the
  focus failed) or the listener giving up on the application
  (`focus_read::GAVE_UP`: the read did not return in time, waited too
  long behind its application's abandoned reads, ran out of time before
  its window was looked up or judged, or its event said it lacked the
  keyboard focus when the read's time was up); `Inconclusive`, any other
  drop of a focus, which could not establish where the focus is (its
  element no longer with the keyboard focus, its window not using UIA,
  which is another window than the copy's, the read abandoned when the
  user moved on, no thread to read it, the listener shutting down, its
  element not kept for the read or not resolved, its reading thread
  without a UIA client, or a newer event of its element replacing it);
  `NoWindow`, its sender with no window above it
  (`focus_read::NO_WINDOW`), as a sender gone before its read has; or
  `NotFocus`, not a focus at all. Two of these are judgement calls of
  their own (Dickson, 2026-10-21): the drop whose event said it lacked
  the keyboard focus when the read's time was up ends the wait, and the
  one a newer event replaced is inconclusive. A read says, in `focus`,
  whether it was a focus's rather than a menu opening's. Both go on the pipe as JSON,
  the field `"evidence"` on each drop and `"focus"` on each read; the
  listener, the supervisor and the outposts build from one `protocol.rs`,
  so neither needs a version.
- A `WinEvent` focus copy whose window uses UIA is dropped by the worker
  for its UIA focus to report, and the worker waits for that UIA focus
  (`outpost/gap.rs`, `docs/design/listener-without-connections.md`,
  decision 11): it opens a gap, unless a UIA focus was observed no earlier
  than 100 ms before the copy, which a focus's UIA read from the listener
  closes, as do a focus marker's drop that ends the wait (`Focus`), the
  end of the listener's reads, a focus (not a foreground change) the
  outpost reports observed after the copy, and another application's
  focus Core queued after it. That last one reaches an outpost only when
  it reported the focus Core had queued just before: `FocusQueued` goes
  to the listener and to that outpost alone, and the worker reads it only
  when it wakes for the gap, so it saves a read, never a wake-up. A
  selection's, a notification's, or a menu opening's marker neither
  closes a gap nor reads it, and nor does an `Inconclusive` drop, which
  leaves the gap to its bound; one that is resolved after the bound has
  passed lets the gap be read then, as any marker's resolution does, the
  gap then being due. A read answers a copy only in the copy's top-level
  window (`window::top_level_of`, a local call made for each copy and each
  read), so a UIA focus in another window of the application, such as the
  folder window a new File Explorer window was opened from, answers
  nothing of it; a drop carries no window and is matched by its
  observation alone, a limit (Dickson, 2026-10-21). A gap a drop or the
  end of the reads closes is logged at debug, one line per close. The
  gap is the worker's alone, without a lock, and a replacement worker
  starts with none. While a marker waits,
  the gap is not judged, whatever the worker wakes for. The intake hands
  the worker, with each entry, the news of the focus markers it resolved
  without a read (`MarkerNews`), under the lock `Intake::next_with_gap`
  takes anyway, so a copy such a drop already answers opens no gap; and
  `next_with_gap` gives the worker `Item::Wake` at the gap's bound,
  `UIA_FOCUS_BOUND`, a second from the copy's observation by the
  outpost's clock, or at once for news while a gap is open. Due, or at
  once when a focus marker of it is dropped for `NoWindow`, the worker reads
  the focus with the focus-now query's read (`read::focused_control_in`)
  and reports it as a UIA focus, once per gap, logging it at info ("no
  UIA focus followed a WinEvent focus in a UIA window"). That read makes
  no call into the application when the focus window is no longer a
  window, has a ghost window standing in for it, or is hung
  (`window::event_window_unreadable`, local calls); and it reads again,
  through the window's proxy, an ancestor that is a window's own element
  read inside the provider without its handle, as the console host's
  window is (`docs/uia-bugs.md`, "Inside a remote operation, conhost's
  window has no handle or title"), one call, so the read reports the
  window the listener's read would. The idle barrier's answer waits while
  a gap is open, as it waits for a caret key's watch: until the bound, a
  second after the copy's observation; while any marker waits for its
  resolution then, a focus's, a selection's, a notification's or a menu
  opening's alike, until it is resolved, which can take as long as the
  listener's read, up to `focus_read::READ_DEADLINE` (11 seconds), a
  marker queued behind its application's abandoned reads waiting up to
  that deadline again, and each marker that follows while the gap is open
  adding its own time; then the read, if the gap comes to one. For one
  focus marker on an application that answers, that is about 12.2
  seconds at most (the bound, the deadline, the relay's 146 ms at most
  measured, and the read's milliseconds), within the harness's 15-second
  idle timeout; a slower application can exceed it, and the harness's
  idle timeout then fails the step, which is the right outcome. A settle
  request (`Outpost::settle`) reads a gap that is due before it is
  answered.
- A read, its checks included, has one time budget, `READ_BUDGET`, 2
  seconds, asked before each of its calls after the first, so at most the
  call under way runs past it. The raw probe's wait, up to 3 seconds, is
  not counted (`Budget`): the budget's end moves on by the time the
  window's verdict took, so for a sender with a window of its own the
  budget starts once the probe has answered. Once it has passed the read
  makes no more calls, but still makes every one of NVDA's checks that
  needs none: the native check takes the window's verdict when it is
  known without a call, from the class lists or a kept verdict, and drops
  the event only when the probe, and so perhaps the console check, would
  be needed; the acceptance test and the refusals are local; and the
  sender's keyboard focus is taken from the event's cached
  `HasKeyboardFocus`, a sender whose cache does not hold `true` being
  dropped. A live keyboard focus read that fails by UIA's transaction
  timeout is judged the same way, from the cache; any other failure lets
  the focus through. The
  repeat check, which needs the last focus's keyboard focus read live, is
  skipped, so a repeat out of time is reported, a deliberate divergence
  from NVDA. A focus so judged is reported with what was read, the
  event's cache giving the rest, its ancestors unknown.
- Each call of a read is timed by the system's clock, one reading at the
  start of each call, which ends the call before it
  (`FocusQuery::before_call` names the classic walk's), and a call that
  took longer than a second is logged at info with its name and its
  elapsed milliseconds, so the slowest calls behind `READ_DEADLINE` can be
  seen.
- A watchdog abandons a thread whose read runs past `READ_DEADLINE`, 11
  seconds: the budget, the raw probe's three seconds it does not count,
  UIA's five-second transaction timeout for the call that can be under
  way when the budget passes, and a second to spare.
 It bounds how long the application's
  later events wait behind the read's marker, but not the call under way:
  the transaction timeout bounds each provider call, not a UIA operation
  (`docs/uia-bugs.md`, "A timeout bounds each provider call, not an
  operation"), so an operation of several provider calls, such as the
  nearest-window lookup, can run past it in an application that still
  answers each of them, and its read is abandoned. The watchdog also
  abandons a read, as NVDA's watchdog cancels a slow call
  (`_shouldRecoverAfterMinTimeout`), once it has taken `MOVED_ON_GRACE`,
  half a second, and the window in front belongs to the read's
  application on another UI thread than the window being read, the
  application being each window's provider process
  (`verbatim_uia::provider_process`), so a console window is its console
  host's, never its client's
  (`outpost::window::front_is_another_thread_of_its_application`, local
  calls, which an outpost's worker shares): File Explorer's folder
  windows, the taskbar and the Alt+Tab switcher are one process on
  several threads, and a focus on one must not wait behind a slow read
  of another. Console host's Find, Properties and Defaults windows are on
  the console window's real thread, which `GetWindowThreadProcessId`
  reports as the client's for the console window, so a slow read of the
  console window with one of them in front is abandoned after the half
  second, as NVDA's watchdog abandons it; a console window whose host
  cannot be read has no provider process and is never judged moved on
  from or to. Only a focus's or a menu opening's read is judged so, and
  only once the window being read (`focus_read::WatchedRead`, the read
  as the watchdog watches it) is known: the event's cached window
  handle, or for a sender that is not a window the nearest window once
  the read's lookup has returned (`WatchedRead::learn`); a selection's
  or a notification's check is never judged so, as NVDA makes those
  checks on its callback threads, whose calls its watchdog never
  cancels. The user is judged at the end of the grace only with
  evidence that the user may have left the window: a foreground event
  the listener's own hook delivers (`Readers::foreground_changed`,
  called after the event is forwarded, so its observation time is taken
  first) since the read's event was queued, or the window in front
  already on another thread of the application when the read's window
  became known, judged once then with the same local calls
  (`WatchedRead::judge_front`: by the watchdog as the read starts for a
  cached window handle, by the read's `learn` for a nearest window,
  which wakes the watchdog only when the front is elsewhere). Past the
  grace it is judged on each foreground event, until the deadline. A
  read with no such evidence costs the watchdog no wake-up besides its
  start's. The event is dropped with the reason "the user moved on from
  a slow window" and the abandonment logged at warn. When the watchdog
  abandons a read, either way, it marks the read abandoned, so that a
  read that returns later makes no focus the last one accepted, and a
  focus the read accepted just before is no longer the current focus or
  a repeat's evidence (`Reading::abandoned`,
  `LastAccepted::abandoned`); the event is reported dropped, a new
  thread takes the application's later events, and the abandoned
  thread, once its call returns, lowers the application's count of
  abandoned threads, says nothing and ends. With two of an
  application's threads abandoned and still in their calls
  (`ABANDONED_LIMIT`), it gets no new thread: its focus events wait, a
  newer event of an element replacing its waiting one, and the first
  abandoned thread whose call returns reads them. The bound is one per
  application (`App::limit_reached`): `READ_DEADLINE` after its second
  thread was abandoned, the watchdog drops every event waiting with the
  reason "it waited too long behind its application's abandoned reads",
  which resolves their markers, and each later event is dropped with it
  as it is pushed, until an abandoned thread's call returns, which ends
  the bound; so its outpost runs at most `READ_DEADLINE` behind the hung
  reads, once. A focus waiting so counts for `Readers::focus_pending`
  only when the menu opening asked about is its own application's (the
  callback passes the opening's cached process id, no call).
- When the listener is replaced, the supervisor tells every outpost,
  ready or starting, that its reads have ended
  (`DeliveredFact::UiaFocusReadsEnded`), so none waits on a marker the old
  listener cannot resolve.

The outpost's side is under the intake and the worker below: the intake
holds everything behind a marker until its read takes the marker's place,
and the worker reports the read and only then finds its own copy of the
element (`outpost::focus_path`).

### Console windows and frames of another process

A console window is one application in one outpost, its console host's,
as NVDA gives it and its text area the client's app module
(`docs/design/focus-pipeline.md`, section 5.4). Windows names the console's
client (such as `powershell.exe`) as a console window's owner, while its
UIA facts, the text area's and the window element's, come from the
console host's provider and so go to the console host. The listener sends
each `WinEvent` for a `ConsoleWindowClass` window to the process of the
event's own thread, the console host, read with `OpenThread` and
`GetProcessIdOfThread`, local calls (`listener::routed_process`, as NVDA's
`consoleWindowsToThreadIDs` learns the real thread); when the thread
cannot be opened, the event is dropped, logged at debug once per window,
rather than sent to the client, whose outpost would take the console
host's window as its application's. The console host's
outpost marks each event in a console window with the client as its
`application` (`OutpostToSupervisor::Event`), which Core takes as the
event's source, and answers a focus-now query for its console window.
An outpost's window filters (`window::served_by`, which
`focus_window_of`, `foreground_window_of`, `main_window_of` and
`top_level_windows` decide by) take a window as the application's when
the application serves its UIA (`verbatim_uia::served_by`, local calls),
so a console window is its console host's and never its client's, even
when the host cannot be read from the client, and `read::focused_control_in`
reads no focused element in a window another process serves
(`read::control_window`; `docs/design/listener-without-connections.md`,
section 4.3). Where the supervisor starts an outpost for a process on
account of a fact in a window, it checks that the process serves that
window (`check_served_by` in `supervisor/owner.rs`): a debug build fails,
a release build logs an error. The crate
re-exports `provider_process` and `console_host_of`, so Core routes a
console client's focus-now query to its console host without depending
on `verbatim-uia`. A focus whose top-level window belongs to
another process, as the Settings app's page inside `ApplicationFrameHost`'s
frame, is read by the listener up to the window below the process
boundary, and the outpost continues the chain from that window's parent
or owner through MSAA, from its client area up, whether that window has
a UIA provider or not (`read::uia_chain_onward`, for the listener's read
and a focus-now read alike), as NVDA switches API at a window boundary;
a parent with a provider, such as a WinUI host of WebView2 content, is
read through MSAA too, named and roled as MSAA gives it, where NVDA
reads it through UIA (sections 4.3 and 4.4 of that design). UIA's
client side still sends such a parent the message a connection begins
with, and connects to one with a provider, whenever the outpost's read
returns the child window's element: an accepted limit, shared with the
listener and with NVDA (decision 13 of that design).
The outpost connects to, probes and walks up into only the windows its
application serves (`read::serves`, by the window's provider process),
with one accepted exception: object navigation's raw-view step
(`Uia::neighbor`, from `read.rs`'s classic navigation) can land in
another process's window, such as the Settings frame above the page's
content, and reads that neighbor through UIA before
`read::corrected_backend` switches it to MSAA; it is user-initiated and
off the per-event path, and a boundary stop before it is a follow-up
(section 4.3 of that design). Otherwise:
its own UIA walk (`read::uia_ancestors`, for the ancestors query and a
focus-now read's classic read) stops below the boundary and is
continued the same way, and a step that reaches another process's
window after all, as an owner shown between the boundary check and the
step makes it, switches to MSAA at that window without probing it
(`read::own_window_uses_uia`, which the continuation of the listener's
chain, `read::uia_chain_from_read`, also asks); the MSAA walk (`read::msaa_ancestors`) answers
another process's window as read through MSAA without probing it; a
navigation neighbor that is another process's window is kept as read
(`read::corrected_backend`); and a focus route through another
process's window is not followed (`Context::focus_route`).

### Starting an outpost and asking for the focus

A fact for an application with no outpost starts one
(`DeliveredFact::may_start_outpost`) unless it is a selection or a
notification other than the shell's window-snap results: Core speaks no
other notification from an application that cannot hold the focus. Node
identity never crosses a process: the listener forwards a UIA runtime id,
and the receiving outpost mints the `NodeId` from it. There is no announce
poll: at startup, and after an outpost or the listener is replaced, Core
asks the application for its current focus with a single focus-now query.

The outpost an application gets is usually a spare
(`docs/design/focus-pipeline.md`, section 19): the supervisor keeps two
outposts started with no application (`--spare`), each with its pipes,
tracing, COM apartment, UIA client and focus-following registration
(scoped to nothing), worker with its UIA client already made, watchdog,
and `WinEvent` thread with no hook, which say `SpareReady` and answer
pings. A start for an application takes the oldest, ready or not, and
sends it `Assign`; the spare sets its application, reads the hand-over's
clock, installs its process-scoped hook on its running `WinEvent` thread,
starts waiting for the application's exit, logs "watching its
application" with the application's image and pid, and says `Ready`. The
supervisor holds the application's facts meanwhile and releases them on
`Ready`, as for an outpost started for the application.

The supervisor starts the two spares once Core's own outpost, which is
started for Verbatim's own process at startup, is ready, or has ended
without becoming ready. It reads a spare's pipe as the spare's, by its
launch, until `Assign`, and as the application's after. A given spare
never returns to the list, and its replacement starts at once, as it is
given. Spares are pinged as outposts are, are never asked in idle rounds,
sent end-of-reads markers, or retired, and are shut down with the rest.

Every outpost failure takes one path (`Owner::outpost_failed`), whatever
its cause: the process exits or crashes, its hook fails (it reports the
fault and exits), its launch fails, it misses its pings for three
intervals, or it piles up abandoned workers. The supervisor ends it and
logs the failure with its reason, the last fault it sent included: as an
error, whose sound a user hears, for a spare's failure and for one before
its `Ready`, and as a warning for a ready outpost that crashed or was
stuck, as before. If its application has exited, it ends as the application's
exit. Otherwise the application keeps its record, with its held facts and
any idle query held for it, and is given the next spare, or an outpost
started for it when none waits, whether or not it holds attention; facts
that arrive meanwhile are held for it. Nothing is ever given up: the
application's first failure is replaced at once, and each further one
after a backoff (`policy::Backoff`) of 1, 2, 4, 8 and 16 seconds, then 30
seconds for every one after; the count starts again when the outpost that
failed had been up for a minute (`STAYED_UP`), and when the application
comes to the foreground, which also ends a backoff that runs. One that was
ready is ended for the app at once (`Ended`, exited, or killed when the
supervisor ended it), and its hook goes with it (`HookRemoved`), so the
listener forwards the application's MSAA events again until the next is
ready. One that failed before its `Ready` lost nothing: when the next is
given, the app hears `Reincarnated`, naming the failed incarnation and the
next, then the failed one's `Ended` if it heard it started, then the next
one's `Started`; in an idle round that asked the failed one, the next
answers in its place, and the app asks the application for no focus at
that `Ended`. A spare that fails while it waits (it exits, misses its
pings, or fails to launch) is replaced, at once for the first failure and
after the same backoff, counted for the spares, for each further one. An
outpost that cannot watch the application it is
given, its hook failing or its having one already, reports why as a
fault and exits with code 1 without `Ready`.

### Who calls into an application

Only an application's own outpost sends it a remote operation or makes a
new UIA connection to one of its windows
(`docs/design/listener-without-connections.md`, section 2): the listener
reads every focus from elements it holds, with no connection, and an
outpost connects to, probes and walks up into only the windows its
application serves, but for object navigation's step, an accepted
exception described above. Two processes' new connections and a remote operation
into one application cannot then come from Verbatim's own calls, so no
lock is needed around them. The supervisor's table of applications
(`App`) holds each application's process open from its first record until
the process exits, through any retirement and every outpost failure, so
its process id is never reused while an outpost or a reading thread
serves it. The registrations make no connection by handle: a scope of
windows' subtrees is given as their elements (`Scope::Subtrees`).

## Public API

### The protocol

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
  `DeliverFact` and `Query`, and the outpost's `Event` and `Reply` carry
  it, and Core's latency ledger reads it. verbatim.exe adds its own stamps
  (`docs/archive/relay.md`, section 4.5): `core_received_at_us`, when it
  took in a listener's fact or queued a request; `core_relay_us`, its own
  time from then to its relay thread beginning the write, measured on its
  monotonic clock alone; `core_held_us`, the part of that a fact was held
  for its starting outpost; `core_sent_at_us`, the first plus the second;
  and `core_read_at_us`, when its reader read an outpost's `Event` or
  `Reply` (`OutpostToSupervisor::core_read`). A `Query` carries
  `core_received_at_us`, `core_relay_us`, and `core_sent_at_us` to the
  outpost, which copies them into its reply. Each message is stamped as
  its write begins, just before it is encoded, by whichever thread writes
  it, the one that sent it or the I/O thread finishing what waited behind
  a write in flight (`OutpostToSupervisor::written`, called from the
  backlog's `encode`): `listener_written_at_us` on a listener's
  `FocusFact`, and `written_at_us` on an outpost's `Event` or `Reply`.
  The listener stamps `listener_handled_at_us` when it has handled a fact:
  a reading thread as it finishes with a UIA focus, menu opening,
  selection or notification, and the sending side, as it sends it, for
  any other fact (a `WinEvent`'s, a marker, a drop decided without a
  read). These
  split the hops to verbatim.exe into the sender's own time and the
  pipe's. `MenuOrSwitchEnded` carries an `EventTiming` with the same
  listener stamps, and `Pong` a `written_at_us` stamped the same way. The
  commands to the listener (`Ping`, `HookInstalled`, `HookRemoved`,
  `FocusQueued`, which an outpost also receives) carry `sent_at_us`, which verbatim.exe's writer stamps
  as the write begins (`SupervisorToOutpost::stamp_sent`); the listener
  stamps its read of each and logs the hop at trace level under
  `verbatim::stage`, and verbatim.exe logs each pong's trip back and a
  menu end's stages the same way. `hop_us` is the time between two such
  stamps, `None` where either is missing.
  `SupervisorToOutpost`: `SetBackendOverride` (forces one backend for every
  window of the target, or restores normal arbitration), `DeliverFact` (a
  focus fact the listener captured, routed to this outpost — a UIA focus's
  marker, its read or its drop, an MSAA focus or menu-popup address, or a
  foreground window — carrying the listener's own trace id and observation
  timestamp so the latency timeline starts at the OS event, its
  `EventTiming`, and for a `WinEvent`'s fact its `WinEventStamp`),
  `HookInstalled` and `HookRemoved` (to the listener, the hand-over above),
  `FocusQueued` (to the listener: the trace and observation time of each
  focus Core queued, after which the last focus the listener accepted is
  no repeat's evidence unless it is that focus or newer, as NVDA's last
  queued focus is replaced by every focus it queues; and, when the focus
  is another application's, to the outpost that reported the focus
  before, whose MSAA duplicate test then no longer compares with its last
  focus unless that was observed later),
  `ForwardingEnded` (to an outpost, the hand-over's end), `Query` (a
  request id and a `Query`: `FocusNow`, `Navigate` with a model `QueryKind`,
  `Activate`, `Ancestors`, `DumpTree`, `Text`, a node and a model
  `TextOp`, milestone M4's text protocol, or `ShellItems`, the shell's
  tray icons or taskbar buttons for Core's list dialog), `Cancel` (withdraws a query that
  has not started), `Ping`, `IdleQuery` (the harness's idle barrier's
  round, under "The idle barrier" below), `Shutdown` (shut down
  cleanly and exit, under "Shutting down" below), and `Assign` (to a
  spare: the application it watches from now on). `OutpostToSupervisor`:
  `Ready` (the outpost has its application and its hook is in: the first
  message of an outpost started for an application, a spare's answer to
  `Assign`), `SpareReady` (a spare's first message: it waits for
  `Assign`), `Event`
  (trace id, observation timestamp, backend, the event window's
  `WindowFacts`, normalized event), `Reply` (exactly one per accepted query,
  echoing its request id, with its `EventTiming`, default for a query
  withdrawn or abandoned, and a `QueryOutcome`: `Done` with a
  `QueryResult`, `Gone` when the node is no longer reachable, `Failed` with
  a reason, `NotStarted` when it was withdrawn before it ran, or `Abandoned`
  when it started and passed its deadline, so side effects such as an
  activation may already have happened), `Pong` (echoes the ping's sequence
  number and reports how many abandoned workers have not yet returned, which
  the supervisor watches), `Fault`, `FocusFact` and `ForwardingEnded` (sent
  only by the listener), `TargetExited` (the outpost's application has exited,
  sent once, ahead of ordinary messages, so the supervisor shuts the
  outpost down), and `IdleAnswer` (the answer to `IdleQuery`, behind
  every message sent before it; `is_idle_answer` names it, since nothing
  is merged across it). A `FocusNow` answer carries the application's foreground
  window and its facts, when it holds the system foreground, and its focused
  control with ancestors, selected child, window facts, and what its text
  read found (`FocusedControl::text`, as `FocusChanged`'s `text`), and the time
  the outpost began reading it, which orders it among the focus events. A
  `ListenerFact` strips to a pid-less `DeliveredFact` once the supervisor
  has routed it. Node ids arrive from Core stamped with this outpost's id;
  the outpost looks them up with the stamp cleared (`NodeId::unstamped`).
  `OutpostToSupervisor::assign_outpost` is the stamp Core applies to every
  node id in a message. Each message is one frame of
  `verbatim_process::transport`, a `Content-Length` header and the message
  as compact JSON, through `write_message` and `read_message`;
  `write_message` hands its writer the whole frame in one write, so a
  message is one `WriteFile` on a bare pipe, `encode_message` gives those
  bytes without writing them, `put_message` appends them to a buffer a
  sender writes from, and `decode_message` parses a frame's body.
  `message_kind(body)` reads a frame's kind, the name of the message's
  variant, from the body's first bytes without parsing the rest, as
  serde's external tagging writes it (`{"Pong":...}`, `"TargetExited"`);
  the relay reads it on every outpost frame.

### The arbitrator

- `Arbitrator` — NVDA's per-window backend decision (`_isUIAWindowHelper`):
  `verdict(hwnd, &WindowClasses)` answers from the good class list, the
  Windows 11 shell rule (the root ancestor's class is a shell top-level
  window, and the window is not the Start button), the bad class list, a
  forced override from `SetBackendOverride`, or a kept probe result, and
  `None` when only the probe can decide; the worker then probes, with
  the raw probe (`verbatim_uia::raw_probe`) in an outpost and in the
  focus listener alike (`arbitration::uses_uia`), and calls
  `record_probe`. No connection to the window follows the probe, in an
  outpost or the listener: when a new File Explorer window's first UIA
  focus events arrive in a burst after their senders are gone, the focus
  the burst ends on is recovered by the outpost's focus read
  (`outpost::gap`; the listener design's risk 13). `WindowClasses::of(hwnd)`
  reads the window's raw class, its class normalized as NVDA normalizes it
  (`normalize_class_name`: NVDA's class map, and the Windows Forms and
  `ATL:` wrappers removed), and its root ancestor's class. When the probe
  finds a provider, `post_probe_check` names the check NVDA makes before
  using it (a console's formatted text, a list view's Windows Forms
  origin, which an outpost and the focus listener alike read as its raw
  class's `WindowsForms10.` prefix, a local call; and a
  console's formatted text, which an outpost reads by the window's handle
  and the listener from the event's sender, the text area or the console
  window's element, through the console check `uses_uia` is given); a provider
  that fails it is recorded with `record_excluded` and
  is MSAA for the window's lifetime. `probe_found_provider(hwnd,
  &WindowClasses)` answers whether a kept probe found a UIA provider of
  the window's own on a window its class lists do not decide; the
  runtime-id search applies the proxy rule only to such a window, because
  a window UIA by class may be served only by UIA's proxies, and the
  focus listener keeps a probe for every UIA focus it reports. A probe that finds a UIA
  provider is kept for the window's lifetime and dropped by `forget` when
  the window is destroyed, as decision D15 specifies. A probe that finds
  none is trusted for only `NEGATIVE_VERDICT_LIFETIME` (500 ms, NVDA's
  cache period) and then repeated: a busy or starting application answers
  the probe late or not at all, and the probe then reports no provider for
  a window that has one. The renewal is by time because no event says a
  window has begun to answer the probe. The time is the arbitrator's
  `Clock`, the system's unless `Arbitrator::set_clock` sets another, and
  `Arbitrator::now` reads it, for the worker's renewal of the verdicts an
  entry probed. The same clock measures the time budgets of the outpost's
  classic walks (`PATH_BUDGET`, `ENRICHMENT_BUDGET`), each checked
  between calls through `arbitration::time_limit`, and of a focus
  reader's whole read (`READ_BUDGET`), checked between calls through
  `focus_read`'s `Budget`, which leaves out the raw probe's wait.
  `Outpost::set_clock` sets an outpost's, and
  `FocusReader::set_clock` a focus reader's, for a test that counts calls
  and decides when a probe is made again and when a walk's time is up,
  however long its calls take. Both class lists are
  lifted from NVDA and pinned by unit tests naming their exact NVDA source
  locations, so a future NVDA sync is a diff of two lists: the bad list is
  `badUIAWindowClassNames` in `nvda/source/UIAHandler/__init__.py`, and the
  good list concatenates `goodUIAWindowClassNames` from the same file with the
  Windows 11 shell tuple from the Explorer app module's `isGoodUIAWindow`
  (`nvda/source/appModules/explorer.py`).

### The outpost

- `Outpost`, `run_pipe`, `run_attach` — the per-application outpost (the
  `outpost` module; outpost redesign, "Inside an outpost").
  `run_pipe(pipe_in, pipe_out, target_pid, options)` runs one on Core's
  pipes, watching `target_pid`, or, for `None`, as a spare that sends
  `SpareReady` and watches the application `Assign` names. Either way the
  application is set once, by `Outpost::assign`: the hand-over's clock is
  read, the process-scoped `WinEvent` hook installed on the outpost's
  `WinEvent` thread, which started with no hook (`EventThread::hook`), and
  `Ready` sent. When the hook cannot be installed the outpost reports a
  `Fault` saying why, sends no `Ready`, shuts down, and exits with code
  1, since the listener stops forwarding an application's events once its
  outpost is ready (`docs/design/focus-pipeline.md`, section 19.8, item
  5); the supervisor handles it as an outpost that ended before `Ready`.
  `Outpost::new` and its siblings, for tests in their own process, watch
  their application from construction and panic when the hook cannot be
  installed. Its parts:
  - Intake (`outpost::intake`): the MSAA hook callbacks (`APP_SUBSCRIPTIONS`:
    focus, foreground, menu-popup, alert, and tooltip events, which become
    the same facts the listener forwards, value, state, name, selection,
    and destroy, for its application's pid),
    the focus-following UIA callbacks, and the reader's routed facts
    (the UIA focus's marker and read, the listener's desktop-wide selections, menus, and
    notifications, and the MSAA facts it forwards until the hand-over ends,
    each matched with the hook's copy first, `outpost::handover`) and
    queries only add an entry to the queue and return. A UIA callback captures the element's cached parts, its cached
    window handle, and an agile reference; it never calls into the
    application.
  - The focus-following UIA subscription, following NVDA's selective
    registration on Windows 11 (`UIAHandler/__init__.py` 642 to 756,
    `NVDAObjects/UIA/__init__.py` 1623 to 1630;
    `docs/design/focus-pipeline.md`, section 17): one registration, on one
    thread and client, with NVDA's two local groups built once. The base
    group, given every UIA focus whose element the outpost has, holds the
    name, value, and state changes and `Text_TextSelectionChanged`; the
    extended group, given a focus with text read through UIA, also holds
    `Text_TextChanged` and the active text position changed event, except
    for the console host's text area, which takes it without
    `Text_TextChanged`, since its console updates tell the outpost of its
    output (`docs/design/wake-ups.md`, item 15). Both
    are on the focused element only, since the reducer acts on no other
    element's changes (NVDA's scope also names the focus's ancestors, but
    UIA delivers no ancestor's event to it; `docs/parity/backends.md`, "UIA event
    registration"). The parts are shaped: a property change's element
    arrives as a `verbatim_uia::Cached<PropertyEvent>`, everything a name,
    value, or state set is read from and no detail; a caret, text, or
    active text position change's as a `Cached<TextEvent>`. The worker's
    event carries it as `Agile<PropertyEvent>` or `Agile<TextEvent>`
    (`intake::EventElement`), resolved on the worker as the `Kept` shape
    both cover, which is what the node registry keeps and all any reader
    of the event's element reads.
    The worker moves it each time it reports a focus, without waiting
    (`follow_focus`): the matching group is added to the new focus, then
    removed from the previous one alone (`RemoveEventHandlerGroup`);
    `RemoveAllEventHandlers` is called only at shutdown, as the
    registering thread ends. A move to the
    node and group already followed is not asked of the subscription, and
    one to the same element under another node makes no call, as the
    subscription compares the elements. A focus whose element the outpost
    does not have, or one read through MSAA, is followed by nothing. Each
    move is logged at debug level, by the worker and by the subscription's
    thread with every call it makes.
  - The queue applies NVDA's limiter rules to MSAA events only
    (`docs/design/focus-pipeline.md` section 5). One entry waits per
    object and kind, and for an MSAA event other than a menu opening, per
    thread that raised it, as NVDA's limiter keys them (a menu opening
    waits per address alone, and the console host's events merge per
    window and kind); a newer one replaces it and moves to the back, which
    is all that limits UIA events. A batch is everything that accumulated
    while the worker handled the previous one, planned in the order it
    arrived, but for a focused terminal's caret event waiting ahead of a
    text change of the terminal (its console update or UIA text change)
    with only events between them, which is planned just after the newest
    such change, since that change's read reads the caret too
    (`caret_after_update`). Per batch the newest 4 MSAA focus events and
    the newest 10 other MSAA events per thread that raised them are kept
    (each MSAA event carries its own thread, from the outpost's hook or
    the listener's), the focused object's other events beyond the limit
    though they count, and a foreground change by either limit, since it
    counts among both; UIA events and menu openings are not counted;
    events from a window the system reports hung (`IsHungAppWindow`) are
    dropped before any read. Each consecutive run of MSAA focus events and
    foreground changes, every foreground change of the batch among them,
    is tried newest first until one is reported, at the place of the run's
    newest, as NVDA's `pumpAll` does (only an MSAA event NVDA's pump
    receives ends a run, and every other entry is handled at its own
    place, `ends_a_run`; `Planned::handle` dispatches each planned item).
    Of two foreground changes of the same window from the same thread
    (`Key::Foreground` names both), the later to arrive replaces the one
    waiting. A foreground change removes a focus event waiting on its
    window's client object from the same thread when the focus was not
    raised after it (in one tick, unless the focus is the hook's copy and
    the change is known to have been raised before the hook went in), as
    NVDA's limiter removes a waiting focus when it adds the change. A run
    whose events all carry their raise time is tried in that order
    (`in_raised_order`), events of one clock tick ordered by the
    hand-over's `Arrival` (`outpost::handover`): a listener copy known to
    have been raised before the outpost's hook went in, and every listener
    copy of the batch that arrived before it, go ahead of the rest, which
    keep their arrival order. Every UIA focus event is handled at its own
    place, and the newest menu opening from each backend is handled last.
    A UIA
    focus's marker holds its place: nothing that arrives after it is
    planned until the listener's read takes its place, or its drop, or the
    end of the listener's reads, removes it, so an event raised after a
    focus is never handled before it, as NVDA's UIA events pass in order
    through its one flusher thread; a read whose marker the outpost never
    held, as when the outpost started after it, is taken where it
    arrives. When the read takes its marker's place and the focus last
    reported is among the read's ancestors, the old focus's own UIA
    state changes that the outpost's subscriptions queued after the
    marker, up to the next marker or idle query, are planned ahead of the
    read, in their order (`old_focus_events_first`): a provider moving the
    focus into a child of the old focus, as a combo box opening into its
    list, raises the old focus's state change first, but the change comes
    by the outpost's own route and the focus by the listener's, so the
    change can be queued behind the marker. A state change raised after
    the focus is moved too, and spoken, as NVDA speaks a state change of
    the focus or of any focus ancestor in either order. The old focus's
    name and value changes, events of other elements, the listener's
    facts, and every event when the new focus is not inside the old keep
    their places (`docs/design/focus-pipeline.md`, section 12.4). The
    check compares runtime ids the read carries and makes no call.
  - The worker (`outpost::worker`): one thread takes entries in order and
    finishes each before the next. Before any call into the application
    for an entry, it tests the event by NVDA's acceptance test against the
    foreground window (`outpost::acceptance`;
    `docs/parity/focus-events.md`, "Event acceptance"): `accepts_event`,
    a pure function of the event's name, its window, the window queries
    (`WindowQueries`, local calls in `SystemWindows`), and the events
    application support asked for (`RequestedEvents`, empty), and drops
    an event from a window unrelated to the foreground window. A caret key's evidence is watched for
    between entries (under "Text" below), not waited on. It is the only
    thread that calls into the application, so events and replies leave in
    the order their entries were planned: the order they joined the queue,
    but for a batch's menu openings, the older focus events of a run of
    MSAA focus events, and a caret key's answer,
    which leaves when its evidence comes. Being the
    only such thread, it is where the calls are counted: the backend crates
    count each call on the thread that makes it, and the worker takes both
    counts when it publishes an event or a reply, which carries them in its
    `EventTiming`. Calls an entry makes without publishing, for an event it
    drops, are taken when the entry ends, logged, and belong to no trace
    (`docs/performance.md`, "Cancelled traces").
  - The watchdog waits on an alarm (`outpost/alarm.rs`), a high-resolution
    waitable timer the worker sets as it starts an entry and cancels as it
    publishes, and an event set for a foreground fact and the shutdown;
    setting and cancelling wake no thread, so the watchdog wakes only when
    a deadline or grace passes (`docs/design/wake-ups.md`, item 3).
  - The watchdog abandons a worker whose call passes its deadline (an
    event, a focus, or a focus-now query 10 s, NVDA's normal watchdog
    timeout, since an application that is starting up can take seconds to
    answer a read that then succeeds; a navigation or activation 400 ms; an
    ancestor walk or tree dump 5 s), or, after half a second, once the user
    has moved to a window served by the same process, by its provider
    process, on another UI thread (the decision is `abandon_reason` and
    `next_check`, the window test `is_another_thread_of_its_application`,
    each unit-tested), answers the stuck query
    `Abandoned`, and starts a replacement that continues with the queue. An
    abandoned worker that returns publishes nothing, since publishing checks
    under the watchdog's lock that the worker is still in charge, lowers the
    abandoned count, and exits. Nothing is sent while that lock is held:
    publishing claims the entry's result under the lock and queues the
    message after releasing it, and the watchdog releases it before it
    queues an `Abandoned` answer, so the watchdog never waits on the
    writer.
  - Time limits inside an entry (`docs/design/focus-pipeline.md`, section
    6). Each UIA call ends after `verbatim_uia::CONNECTION_TIMEOUT`, two
    seconds, when it connects, or `verbatim_uia::TRANSACTION_TIMEOUT`,
    five seconds, on a held element, both set once for the process.
    A focus's ancestor walk checks its two-second budget
    (`ENRICHMENT_BUDGET`, by the outpost's clock) between calls, so it
    ends within the budget and one call. The one operation made of many provider calls, a UIA
    dialog's own text (one second, `DIALOG_TEXT_WAIT`), runs on the
    context's `verbatim_uia::BoundedClient` and is given up at its
    deadline, its thread abandoned; a dialog's text given up is no text. Tree
    view messages are sent with `SendMessageTimeoutW`, half a second,
    aborting at once for a hung window, as list view and edit messages
    were already. An `IAccessible` call has no bound of its own: the
    entry's deadline, enforced by abandonment, is its bound.
  - The I/O thread, the process's main thread in `run_pipe`, on the
    transport's child side (`verbatim_process::transport::ChildIo`;
    `docs/design/communication.md`, sections 4.6 and 6.4): it waits only
    on its completion port, reads Core's commands and hands each to
    `Outpost::handle_command`, which answers pings itself, the pong
    written at once unless a write is in flight, withdraws a cancelled
    query that has not started with a `NotStarted` reply, and queues
    everything else; and it finishes the writes to Core that pended. It
    runs at `THREAD_PRIORITY_HIGHEST`.
  - The outbound queue (`outpost::outbound`), under "Messages to Core"
    below: every thread that sends a message writes it itself.
  `run_pipe` is the production mode over inherited overlapped pipe
  handles, which reads Core's commands until Core asks it to shut down,
  or the command pipe ends, and then shuts the outpost down cleanly
  (`Outpost::shutdown`, under "Shutting down" below), waiting for every
  write in flight; it also tells Core when the target application exits,
  from a thread that waits on the application's process. An outpost made
  with `Outpost::new` and its siblings, as tests make one in their own
  process, takes the overlapped end it writes to, and a thread of its
  sender's own finishes its writes that pend in place of the I/O thread.
  `run_attach` watches a pid directly, asks for its focus, and prints each
  outbound message's JSON as a line on stdout, from a thread that reads
  the outpost's pipe, the standalone dev mode. Both take an
  `OutpostOptions`, fixed for the outpost's life, whose one field,
  `remote_operations`, says whether a UIA focus's ancestry may be read
  with a remote operation (`Outpost::with_options`; `Outpost::new` uses
  the default, on). `Outpost::with_focused_element_reader` takes a
  `FocusedElementReader` too, the function the worker reads a UIA focus's
  element with, given the request it fills the element by and answering a
  `verbatim_uia::Cached<Node>`, which is otherwise the system's keyboard
  focus (`Uia::focused`): mockapp's tests hand a real outpost a UIA
  focus in a window that does not have the keyboard focus, and measure
  everything it does with it. `Outpost::set_foreground_reader` gives the
  outpost a `ForegroundReader`, the function it reads the foreground
  window with when it tests an event for acceptance and when it records
  the window a focus entered, which is otherwise `GetForegroundWindow`:
  an event from a window not related to it is dropped, and an MSAA state
  change on that window's
  own object, when the window was the foreground as the focus entered it,
  is not spoken as an ancestor's, as NVDA's foreground object takes its
  place (`docs/parity/focus-announcement.md`, "A top-level window's state change"), and
  mockapp's windows, on desktops where none takes the foreground, are made
  the foreground this way. `Outpost::set_focus_window_reader` gives it a
  `FocusWindowReader`, the function a focus-now query reads the window
  with the keyboard focus with, which is otherwise `GetGUIThreadInfo`'s:
  the focused element is read only when the application serves that
  window, so mockapp's tests name mockapp's window. `Outpost::settle` waits until the worker has
  handled everything queued, follow-ups included, the focus-following
  subscriptions have made every move asked of them, and every message
  published has been written to the pipe: the evidence those tests wait on
  before they read what the application answered, and before they assert
  the outpost said nothing more. It then waits for the event thread to
  have taken every nudge sent before it, so any change of the hook that
  handling asked for has been made. `settle` covers only what has reached the
  outpost: an application's event reaches a client after the call that
  raised it has returned, on UI Automation's threads or the hook thread.
  `Outpost::failed_reads` gives the failed cached reads the outpost has
  made so far, its worker's and its focus-following subscription's
  callbacks' (`verbatim_uia::calls`), which a test reads once the outpost
  has settled; the outpost logs them when it shuts down, and the listener
  logs its own, its reading threads' and its registration's.
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

### Text and dialog text

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
  its read finished; `caret_report_since` also says whether the caret left
  a given position and, as its `LastSelection` asks, how the selection
  changed from the node's last report (`ReportedCaret`), with whether the
  report's selection is kept for the next to compare with, and with the
  change in general terms when the node's text changed since
  (`ReportedCaret::content_changed` and `new_selection`);
  `caret_report_checking` is `caret_report_since` that also asks a given
  check, once the caret is read and only for a comparison with a
  selection in either report, whether an unnoted change of the text
  waits, and then makes the comparison in general terms;
  `Anchors::note_content_changed` notes such a change, and
  `NodeText::forget_selection` and `Anchors::forget_selection` make the
  next report carry no change. `Anchors` keeps one backend's anchors, by
  node, numbered from a counter both of an outpost's backends share, and never forgets
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
  `DialogParent`, what the text is gathered under, whose children are
  each a `DialogObject`, the traits each backend implements: MSAA through
  `verbatim_ia2::dialog::DialogObject`, both at once, and UIA through
  `UiaDialog::new(uia, request, element)`, the dialog, of which nothing
  is read but its children (the worker resolves the dialog's kept
  `Agile<Kept>` on the bounded client's thread and hands over its `Live`
  element), and `UiaObject`, each child, whose children
  come with their properties cached, one call per container, each read as
  a `verbatim_uia::DialogText`, with its description whatever the theme,
  as NVDA's dialog text includes it (decision 5 of
  `docs/design/uia-shapes.md`).
  `is_dialog(role)` says which nodes have such text: a dialog, an alert,
  or a property page. A gathering looks at no more than `MAX_OBJECTS`
  objects; a dialog with more says nothing rather than part of its text.

### Ignored processes

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
  listener's `a_fact_from_an_ignored_process_is_never_sent`).

### The focus listener

- `run_listener` — the focus-listener runtime (decisions D13 and D14;
  outpost redesign, "The focus listener"), on the transport's child side
  (`docs/design/communication.md`, sections 4.6 and 6.3): its main thread
  is its I/O thread, which waits only on a completion port, reads Core's
  commands, and finishes the writes to Core that pended, at
  `THREAD_PRIORITY_HIGHEST`; every thread that produces a fact (UIA's
  callback threads, the reading threads, the `WinEvent` thread) writes it
  itself, at once when no write is in flight, with its write stamp taken
  as the write begins and its frame encoded into the sender's one write
  buffer, kept from frame to frame. It installs the global
  MSAA hooks (`LISTENER_SUBSCRIPTIONS`, pid zero: focus, foreground,
  menu-popup, menu and switcher end, alert, and a tooltip window shown,
  forwarded as `DeliveredFact::Show`, whose help balloon the worker
  reports as an alert), and desktop-wide UIA
  subscriptions for the events NVDA registers globally on Windows 11, all
  through one `Registration` and so one client, as NVDA's are: the focus
  handler (`Subscription::Focus`) on the client first, then an element
  selected, a menu opened, and notifications, registered together as one
  event handler group on the desktop's root element. A UIA menu opening
  takes the focus's route: its marker, then its application's reading
  thread, which judges it as NVDA's automation event handler judges a
  menu opening (the focus handler's checks but the duplicate test, with a
  sender UI Automation counts as the current focus judged as that focus,
  with its window and without the native check) and reads it as a focus
  on the menu, with its ancestors; one that arrives
  while a focus event waits for or is in its read is dropped, as NVDA
  drops a menu opening while a focus is pending. A newer focus event, or
  menu opening, of an element replaces its waiting one, as NVDA's UIA
  limiter keeps one. And only then
  announces `Ready`, so focus and menus are seen from the moment it does
  (announcing first lost a menu opened right after start, found
  2026-10-06). The event thread (`EventThread::spawn`) likewise returns
  only once its hooks are installed. Each event becomes a `FocusFact` (a
  `ListenerFact`: the owning pid and a `DeliveredFact`) built entirely from
  cached and hang-safe local reads, but for a UIA focus, whose marker is
  sent at once and whose read is made on its application's reading thread
  (`focus_read`, above; `focus_read::FocusReader` is public so a test
  stands in for the listener with the listener's own reader). A
  foreground event is forwarded without
  checking the foreground: a starting application's window raises it before
  it actually becomes the foreground window, so the check waits for the
  outpost's worker, as NVDA's waits for its main-thread pump. Outgoing facts are
  coalesced with NVDA's UIA limiter rule, one waiting fact per element and
  kind (`DeliveredFact::key`, which takes the event's stamp, so that a
  `WinEvent`'s fact other than a menu opening (a foreground change, an MSAA
  focus, an alert, or a tooltip shown) is keyed by the thread that raised
  it too, as NVDA's `WinEvent` limiter keys it (for the alert and tooltip,
  Dickson, 2026-10-21); a menu opening is
  keyed by its address alone, since NVDA's limiter keeps only its most
  recent menu event, whatever its address or thread, and the outpost's
  intake handles only a batch's newest, which follows NVDA per
  application, for popup starts (the difference is recorded in
  `docs/parity/backends.md`, the limiter's entry); notifications and a
  focus's marker, read, and drop are never merged), with pongs
  and `Ready` first, while a write is in flight; this backlog of the
  sender's (`listener::Waiting`) also judges, under the sender's lock,
  whether a `WinEvent`'s fact was raised after its outpost's hook went in,
  so none is ever sent behind "forwarding ended". The end of a menu or of
  the Alt+Tab switcher is told to Core once its grace, 50 milliseconds, has
  passed with no later end: a threadpool timer, set again by a later end,
  whose callback sends it through the same sender, behind every fact
  waiting, so no thread of the listener's waits for the grace. The end
  carries when its grace is over, and a callback that runs before then (one
  of an earlier end's grace that took the lock only once a later end was
  noted, or a timer firing a little early) sends nothing and sets the
  timer again for the rest. It answers
  `Ping` with a `Pong`, takes the hand-over's
  `HookInstalled` and `HookRemoved` (above), `HookRemoved` also ending the
  application's reading thread, and ignores everything else.
  Its per-application state is which applications' outposts have their
  hooks in, which a replacement is given on its command line
  (`--hooks-in`, `listener::parse_hooks_in`), and each reading thread's
  last focus, which a replacement does without at the cost of one fuller
  ancestor read per application, so a crash respawns into full
  capability at once. It makes no remote operation, so it is not given
  `--classic-uia`. Range-value changes and live regions, which NVDA
  also registers globally, are not subscribed yet: the reducer speaks values
  only for the focus, and live regions are milestone M7's.
- `listener::capture` — what the listener's UIA callbacks capture from an
  event's element: its cached window handle and snapshot parts, nothing
  read live. Public so that mockapp's tests make a selection's fact from
  mockapp's own event as the listener does (`tests/combo_box.rs`).

### The supervisor

- `Supervisor` (the `supervisor` module; outpost redesign, "The
  supervisor") — `new(events)` starts the lifecycle owner thread, the
  relay, which starts the focus listener at once (`with_executable` launches every
  child from a given executable with a given shutdown time limit, for the
  test that launches a stand-in); `shutdown()` shuts every outpost and the
  listener down for Verbatim's exit, returning once every one has ended
  with a `ShutdownSummary`, how many of the children that ended during
  Verbatim's life, spares included, shut down cleanly, had already exited
  or exited with an
  error, were killed because their time limit passed, or were killed
  because ending them cleanly failed (the wait for their exit failed, or
  there was no thread to end them, or it panicked); `ensure_spawned(pid)` starts an outpost
  without asking it to report anything, giving it a spare when one waits
  (used at Core startup for Core's own pid, so its outpost is warm before
  the first gesture, and for a focus-now query of an application with
  none);
  `send_to_outpost(outpost_id, command)` queues a command for one outpost
  incarnation without waiting, failing at once with a `QueueError` when that
  incarnation has ended or its queue is full; `note_views(attention,
  holding)` passes the views the app derives from the reducer state;
  `focus_queued(trace_id, observed_at_ms, superseded)` sends
  `FocusQueued` to the listener, and to the outpost `superseded` when the
  app names one, the outpost that reported the focus Core queued before,
  when that was another application's; and
  `query_idle(round)` runs the idle barrier's round through the children
  (under "The idle barrier" below), the app hearing which outposts it
  asked through `OutpostMessage::IdleQueried`. The
  app hears that the listener is ready, and whether it replaced one that
  ended, through `OutpostMessage::ListenerReady`; that a spare is ready,
  with how many are ready and waiting, through `SpareReady` (Verbatim's
  readiness waits for `SPARES`, two, so every end-to-end scenario's
  applications take a spare); that an outpost failed before its
  `Ready` and which incarnation holds its application's facts, through
  `Reincarnated`; and of each incarnation
  through
  `OutpostMessage`: `Started` before
  any of its messages, then its messages, and `Ended` with a reason
  (exited, killed, retired, or target exited). Its messages are delivered
  as the relay read them, `Frame`s, whole and unparsed; the app passes
  every `OutpostMessage` through the `Frames` that `Supervisor::frames()`
  makes, on the thread that takes them, and `Frames::arrived(message,
  out)` appends what to handle now: the message itself, or for a frame
  the `Event(pid, outpost_id, position, message)` it carries. `Frames`
  parses each frame, stamps every node id with the outpost's id, records
  when the relay read it, and numbers the messages that carry node ids
  in pipe order, as the outposts' reader threads did; a frame larger
  than `LARGE_FRAME_BYTES` (256 KiB) goes to a helper thread,
  `verbatim-parse`, started on first use, whose result comes back
  through the sink as `Parsed`, and that outpost's later frames and its
  `Ended` wait behind it, so each outpost's order holds while the reducer
  goes on with its other inputs (`docs/design/communication.md`, section
  4.8). `Frames::parsing()` says whether a frame is with the helper; the
  idle barrier waits for it. A frame that is not a message has its
  outpost ended, as a pipe that fails does, and its later frames are
  dropped, only its `Ended` handed on. Word that an outpost that failed
  before its `Ready` is replaced (`Reincarnated`) waits behind that
  outpost's frame with the helper as its `Ended` does. While an outpost's
  `Ended` waits
  behind its frame with the helper, everything about the same application
  that arrives after it waits behind it too (a replacement's `Started`,
  frames and `Ended`, and `NotWatched`), so the app, which keys its
  outpost state by application, always hears an outpost's end before its
  replacement's start. The relay drops every frame from
  an outpost it has already ended itself (stuck, retired, its target
  gone, or unreadable), though it reads that pipe until it ends. Every message is delivered
  through the `EventSink` given to `new`: a channel of `OutpostMessage`s
  converts into one (the tests use that), and `EventSink::new` takes a
  function, which the app uses to post the messages to the reducer
  thread's port, in line with its keys and commands. The function says
  why a message was not delivered (`Undelivered`): `Gone`, the app having
  gone, which stops the helper parsing large frames; or `Refused`, the
  message given back because the port refused its packet, in which case
  the helper delivers its result again every 50 ms until it is taken,
  since that outpost's later messages and the idle barrier wait for it,
  and the relay keeps a message refused, with every message it sends
  after it, behind it, and delivers them again, in order, at its next
  send and at every heartbeat, so nothing it sends is lost or overtaken
  (an `Ended` still comes before its replacement's `Started`;
  `crates/verbatim-outpost/tests/target_gone.rs`).
  `EventSink::watched(progress, stalled)` lets the relay watch the thread
  taking them: at each heartbeat it reads `progress`, the counts of a
  `verbatim_process::port_queue` (`Progress`), and once that thread has
  taken nothing for three heartbeats in a row while inputs waited it
  reports a `ReducerStall` (how long, and how many waited) to `stalled`,
  once per stall. Nothing is ended for it: the relay reads the outposts'
  pongs itself, so a stalled reducer never makes an outpost look wedged
  (section 6.8).
  - The lifecycle owner (`owner`) is one thread, `verbatim-relay`, that
    makes every lifecycle decision and owns the per-application records
    and the listener record. It is also the relay
    (`docs/design/communication.md`, section 6.2; the `relay` module): it
    waits only in `GetQueuedCompletionStatusEx` on one completion port,
    with a timeout that is the next heartbeat (3 seconds) or sweep (30
    seconds), and is woken by a read of a child's pipe completing, a
    write to a child that pended completing, or its mailbox. It reads the
    listener's pipe and every outpost's, each through a
    `transport::PortReader`, one read in flight per pipe, and handles each
    frame in the pass that read it: a listener's fact is parsed, routed,
    and written to its outpost there and then, so a fact crosses
    verbatim.exe with this one thread's wake-up; an outpost's frame is
    read for its kind only (`message_kind`): a pong and `TargetExited` are
    handled there, `Ready` is handled and handed on, and every other frame
    is handed to the app whole as `OutpostMessage::Frame`, with when it
    was read: the reader's own buffer when it gave it up
    (`transport::FrameBody::Owned`, a frame read over several reads), a
    copy of the body otherwise. The pipe's read is armed again at once, so every pipe
    drains however busy the reducer is (section 4.4). The pipe's end is
    handled after every frame before it. Launch helpers, retirements,
    hang checks and the app report to it through the mailbox
    (`relay::Mailbox`, a channel whose sends post one wake to the port
    unless one is already waiting; a wake the port refuses is not counted
    as waiting, so the next send posts again, and the relay also takes the
    mailbox at every heartbeat, so nothing sent waits longer than a ping
    interval); there is no shared map lock. It never
    blocks: launching blocks, so the owner records the application as
    starting and hands the launch to a helper thread, which makes the
    child's writer and reports back; the relay then sends `Started`, in
    order with its other messages to the app, and reads the child's pipe
    from then on, a second fact meanwhile being held,
    never a second launch; a retirement runs on a thread of its own
    and reports how the child ended; and whether an application with
    eight abandoned workers is hung, which walks its windows, is checked
    on a thread of its own, the outpost being judged on its answer. At
    Verbatim's exit the relay keeps serving its pipes until every
    retirement has reported and every pipe has finished, since a child's
    shutdown message must be written.
  - Each child's commands go through a `transport::Sender` on verbatim.exe's
    overlapped end of its command pipe (`writer`), on the relay's port:
    the thread that sends a command, the relay or the reducer, writes it
    itself when no write is in flight and nothing waits, and otherwise it
    waits in a bounded queue (capacity 64), written in order by the relay
    once the write that pended completes; the relay finds the writer by
    the completion's key (`relay::CommandPipes`, to which a writer is
    added as it is made). So the relay never waits on a pipe while
    deciding and the reducer thread never waits on one, and a child that
    stops reading holds up only its own queue. When the queue is full a
    ping still gets through, and so does a message of the MSAA hand-over,
    or any part of a UIA focus's read, which keeps its place behind
    everything queued before it and is never merged or dropped
    (`Outgoing::delivered`); a routed fact replaces any waiting fact for
    the same object and kind, and for an MSAA event other than a menu
    opening the same thread that raised it (or the oldest waiting fact makes room), and
    anything else, a query above all, fails at once. The supervisor's
    thread count is the relay, plus the helper parsing large frames once
    one has come.
  - Facts that arrive while an outpost is starting are held in arrival
    order, a newer fact for the same object and kind, and for an MSAA event
    other than a menu opening the same thread that raised it, replacing
    the older one and moving to the back (NVDA's limiter rule), and
    released in that order
    on `Ready` (the pure `HeldFacts`); then the listener is told the
    outpost's hook is in (`HookInstalled`), and when a ready outpost's
    record goes, that it is not (`HookRemoved`), through
    `Owner::tell_listener`, which holds them while the listener's launch is
    at work.
  - Targets: the owner identifies each application by a process it holds
    open (`process::Target`), opened when the application's first outpost
    is started and kept while the application has a record: an outpost, or
    a failed outpost's replacement waiting for its backoff. Windows never gives a process's id to another process while a
    handle to it is open, so the pid the owner knows, and passes to the
    outpost, names that application and no other for as long as it is
    held. No outpost is started or replaced for an application whose
    process has exited, or that cannot be opened to be held; the app hears
    `OutpostMessage::NotWatched`, so nothing waits for one. Until an
    application is held, the pid in a routed fact, or in the app's request,
    is trusted to name the process that raised the fact or holds the
    foreground, which was observed moments before it is opened.
  - Failure: when an outpost's pipe closes, the owner ends it after the
    relay has handed on everything it wrote. If its application has
    exited, it ended with its target (`Ended`, target exited), is not
    replaced, and the application's process is let go. Otherwise it
    failed, and takes the one failure path described above, under "Spare
    outposts": its application is given the next outpost, at once or after
    the backoff, and is never given up
    (`crates/verbatim-outpost/tests/spares.rs`). The sweep ends the record
    of an application that has exited while its replacement waited.
  - Stuck: every child is pinged every three seconds; nine seconds without
    a pong ends it (`Ended`, killed). An outpost whose worker waits on a
    slow application still answers pings from its I/O thread, so it is not
    stuck. Eight abandoned workers end an outpost too, except while its
    application's windows are reported hung (`IsHungAppWindow`), when a
    replacement would hang the same way (the pure `wedge_decision`): each
    abandoned worker is a thread blocked in the application, freed only
    when the process exits. Either is followed by the failure path, an
    application that has exited included. The pongs are read by the relay
    itself, never through the reducer, so a reducer that has stalled ends
    no outpost; it is reported as its own stall
    (`EventSink::watched`).
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

## Implementation notes

### Spawning

- Spawning (`Supervisor`, through `verbatim-process`): the outpost is
  created suspended with two pipes with no name
  (`verbatim_process::overlapped::pipe`, 64 KiB each way), verbatim.exe's
  ends overlapped and the child's synchronous, whose child ends are the
  only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost. A per-application outpost's command
  line carries `--target-pid`, fixing the watched application for its whole
  life, and a spare's `--spare <launch>`, the supervisor's number for the
  launch, its application coming later in `Assign`; both carry
  `--classic-uia` when `uia.remote_operations` is off in
  `settings.toml` (`Supervisor::new` takes the `OutpostOptions` every
  outpost is launched with, which `verbatim-app` reads from the setting at
  startup); the listener's carries `--listener` and no pid.
  Each child is spawned with `STARTF_USESTDHANDLES` and an inheritable,
  append-mode file handle as its standard error (and output), so the outpost's
  and listener's own `tracing` output — which otherwise had no subscriber and
  went nowhere — lands in a per-role log file in this Verbatim launch's own
  log directory (`logs\<Verbatim's pid>\outpost-<target image>-<target
  pid>.log`, `outpost-spare-<launch>.log` for a spare, whose application
  is not known when its log opens, and `listener.log`, next to the outpost
  executable; the
  synthesizer host's `synth-<id>.log` lands there too). The app calls
  `verbatim_process::prepare_launch_logs` at startup, before it starts
  any child, which empties
  that directory if an earlier process with the same pid left it and
  keeps only the newest ten launch directories. The outpost binary
  installs a stderr `tracing` subscriber at startup for exactly this; the E2E
  harness fetches these logs alongside the timeline and stderr, so a silent
  outpost is readable after the fact instead of theorized. Best-effort: a
  failed log open leaves the child unredirected, never unspawned.

### Foreground changes and focus-now

- Foreground changes (the worker): a batch that holds a foreground fact
  is handed out at once. A foreground fact whose window is not the
  system's foreground window when the worker handles it is dropped, as
  NVDA's `processForegroundWinEvent` drops it: measured live on
  2026-10-09, Windows raises the foreground event once the change is
  made, and the events that come before are stale or raised early by the
  application itself, such as File Explorer's as it creates its window,
  which the system's own event follows (`docs/parity/focus-events.md`, "Stale focus
  events"). The intake held such a batch for up to 250 ms, checking every
  10 ms, until then. The batch limits keep a foreground fact as one of
  its thread's other events as well as one of the focus events, as NVDA's
  limiter does, so the focus events an application raises after it do
  not crowd the system's event out (`docs/design/phase6-design.md`, "A foreground
  event before its window is in front"). A foreground fact in front is
  dropped too, before anything is read and without counting as reported,
  when the last focus or foreground change the outpost reported
  (`Tracking`'s queued focus) is in its window or a child window of it
  (`window::is_descendant_window`), or came from the fact's own event
  address, taken as the client object (`OBJID_CLIENT`, `CHILDID_SELF`),
  as the hook takes a foreground event's window object, and no focus Core
  queued from another application has replaced it: NVDA's two tests in
  `processForegroundWinEvent` (`worker::foreground_repeats_focus`).
  Otherwise a foreground fact is reported at once, whether or not its window has been shown yet,
  stamped with the time its window was confirmed as the foreground, after
  its name is read, rather than the time Windows raised the event,
  named or not, as a focus on the window, since the foreground change is what
  moves the reducer's attention; the reducer does not speak a nameless
  foreground window, and nothing announces it later. The window's own
  accessible object is read: UIA via `window_element`, MSAA via the
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
  A foreground event whose window is not yet in front when it is handled
  is dropped and never kept, and so is one for a window read through UI
  Automation, even in front, as NVDA drops every MSAA event for such a
  window. Either window is announced as the focus enters it, from the
  focus's ancestry, where the reducer speaks the top-level window like any
  other entered ancestor (`docs/design/focus-pipeline.md`, section 16.4);
  a console window's foreground event comes before the window is in
  front, and its title comes from the console window element, the text
  area's parent, which the listener's read keeps among the ancestors.
  Whether a window is the foreground window is read by the reader the
  outpost was given (`Outpost::set_foreground_reader`, the system's
  `GetForegroundWindow` unless a test gives another), for the foreground
  fact as for the acceptance test.
- A window's client area read through MSAA is marked
  `NodeDetails::top_level_client` when it is a client object of a window
  with the `WS_SYSMENU` style (`verbatim_ia2`'s snapshot read, one local
  style read for a client object only): it reads as a pane, and the
  reducer presents it, by its title, as the window it stands for, as
  NVDA's `isPresentableFocusAncestor` presents it.
- Focus-now (the worker): the answer to Core's focus-now query is the
  target's foreground window, when it holds the system foreground
  (`GetForegroundWindow`, never `GetGUIThreadInfo`'s `hwndFocus`, which can
  name a child window such as Windows 11 Notepad's text area), and its
  focused control (`hwndFocus`, through that window's backend) with its
  ancestors, or with them unknown (`FocusedControl::ancestors` is then
  `None`, and Core receives the focus with `ancestors_unknown`),
  selected child, and, for a control that may have text, its caret read
  by the reported focus's own read (`read_focus_text`), so a text field
  focused at startup or after a menu closes or Alt+Tab speaks its line
  and Core has its caret; a console's text area reads as a nameless
  terminal there too, as on the event path (`read::console_text_area`).
  There are no retries: a nameless window is
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
  query (`focused_control_in` treats the hidden frame as "nothing focused").
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
  (`docs/parity/focus-announcement.md`, "Focus announcement content and property order").
- `verbatim-gui`'s `force_foreground` (see that crate's section) injects a
  bare `VK_CONTROL` tap before attempting `SetForegroundWindow`: a gesture
  that arrived via the control plane (no physical input, as every E2E test
  and any future remote session sends) fails Windows' foreground-lock
  heuristic and falls back to the slow `AttachThreadInput` path, observed
  at roughly two seconds; the tap satisfies the heuristic directly, cutting
  that to roughly 150 to 450 ms. `VK_MENU` was tried and rejected — a lone
  Alt press activates menu bars and bounces foreground straight back.

### Event flow

- Event flow (the worker): the event thread hosts the WinEvent hooks,
  installed once for the fixed target pid before the message loop starts
  (never rebound — a second live `WINEVENT_OUTOFCONTEXT` hook set on a
  thread that already has one has been observed to permanently kill
  WinEvent delivery on that thread for the rest of the process, which
  decision D9's one-pid-per-outpost-for-life design sidesteps). Every event
  is arbitrated per window so exactly one backend reports it: an MSAA event
  for a UIA window is dropped, and a UIA event for an MSAA window. The
  event thread drops an MSAA event whose window is already known to be
  read through UIA before it is queued, asking its own record of the
  application's windows (`arbitration::EventWindows`, kept by window
  handle in a `verbatim_ia2::WindowCache`): a window's classes are read
  once, and the arbitrator is asked only until its backend is decided for
  the window's lifetime (`Arbitrator::lasting_verdict`), so a flood from
  one window costs one lookup per event. A window's entry goes when its
  destroy event arrives, or when its handle is found owned by another
  thread; every backend is decided anew after Core changes the backend
  override. The event thread also narrows the hook (`outpost/narrowing.rs`,
  `docs/design/wake-ups.md`, "Narrowing the outpost's hook"): while the
  application's focus window and foreground window are both known to be
  read through UIA, it unhooks the value, state, name, description,
  selection and text selection events
  (`narrowing::NARROWED`, with `verbatim_ia2::suspend_kinds`), which it
  would drop for those windows anyway, nearly all of them UI Automation's
  own copies of its events, and it hooks them again as soon as the focus,
  a menu opening, or the foreground is in a window read through MSAA or
  not yet decided. Focus, foreground, menu opening, alert, tooltip, window
  shown, the system caret's location changes, window destroyed, and the
  console's updates and caret stay hooked for the outpost's life, since
  the worker uses them for UIA windows too.
  The focus and foreground windows are those of the hook's own focus,
  menu-opening and foreground events, decided again before each is
  queued; until the hook delivers one, those of the facts the listener
  routed, which the reader passes on as nudges (`event_thread::Nudger`, a
  thread message the event thread hands to its owner's nudge handler). The
  worker nudges it to decide again when it has just kept a UIA verdict for
  a window, and the reader when Core changes the backend override. What can
  be lost is an event of a narrowed kind raised in a window read through
  MSAA after the focus or foreground entered it but before the event
  thread took that event, and one from a window read through MSAA that is
  neither the focus window nor the foreground window, which the worker's
  acceptance takes only from a topmost window such as a menu or a
  drop-down list. An MSAA
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

### Reading the focus

- A UIA focus is reported from the listener's read (`Worker::uia_focus`):
  the listener judged it by its own element's
  keyboard focus, read live, as NVDA's `shouldAllowUIAFocusEvent` does,
  and read its states, value, details, ancestors, and selected child from
  that element. The outpost tests it against the foreground window
  (NVDA's acceptance test, with the element's nearest window: its own,
  else the nearest the listener's read found, else the nearest ancestor's
  in the read; an element with no window in its chain is tested with 0,
  which passes, as NVDA's `shouldAcceptEvent` lets through an event with
  no window; `docs/design/focus-pipeline.md`, section 16.2), decides its
  window's backend, and gives its runtime id a new node first when the
  listener read that the element it read before under that id has lost
  the keyboard focus or is gone. It reports the focus at once, with
  nodes minted from runtime ids; its ancestors are the listener's chain,
  filtered as a walk filters them, stopped at a container of the previous
  focus's chain and spliced, and continued through MSAA from a window MSAA
  owns (`read::uia_chain_from_read`). It finds its own copy of the
  element by the chain's runtime ids, off the way to speech for a focus
  without text, and before the report for one whose role may have text
  (edit field, document, terminal), whose caret is read with it, from the
  nearest element of the chain with a window of its own: one
  `ElementFromHandle` and one remote operation
  (`verbatim_uia_rops::element_by_path`; the classic walk within a second
  where a program cannot import the window's element), and keeps it
  under the focus's node and follows it. While the focus is one whose
  value is its selected item (`UiaFocusRead::value_from_selection`), a
  selection the listener passes on of one of the focus's children, found
  by one `FindFirstBuildCache` over its children whose cache holds the
  item's name and keyboard focus (`verbatim_uia::SelectedItem`), is
  reported as `ValueChanged` on the focus with the item's name as found
  (`Worker::selection_as_value`), unless the item has the keyboard focus,
  as NVDA's list item raises a value change on such a focused parent when
  its state changes and it does not have the focus; the focus-now answer
  reads such a value the same way. NVDA's same rule for a
  `Windows.UI.Core` combo box with the `Value` pattern, which no Settings
  combo box has (`docs/parity/changes-and-selection.md`), and for the
  classic .NET Framework Windows Forms combo box, is not built. A repeated focus whose element
  the outpost already holds, and which the listener read as still
  focused, is not looked for again. The chain is kept
  (`outpost::focus_path`) and serves whenever the outpost needs an element
  of it it does not hold: the focus's controller relation for a
  selection, a newly entered dialog's own text, and a query on a node of
  the chain, before any search. A focus whose element its path does not
  reach, as Windows Terminal's tab focus elements, raised from elements
  that are in no tree, do not, is reported from the read all the same,
  with its own ancestors, and nothing is followed until a later focus
  brings an element, as NVDA hears nothing local before its group is added
  to a focus (`docs/design/focus-pipeline.md`, section 17). Removed with
  the step: the outpost's read of
  the focused element for a focus event, the moved-on rules (a focus
  reported as its event said, the gone element, the container holding
  the new focus, and 8be2223's rule for an item that lost its selection),
  the name-and-position search, the selection event's read of the focused
  element, and the outpost's own console window rule, now the listener's.
  Every UIA focus is handled in turn, oldest first, as NVDA's UIA handler
  queues each one, and Core culls the speech of those that have expired.
  A UIA selection fact
  carries, in `focus_window`, the nearest window the listener's reading
  thread found for an element with no window of its own, the window its
  acceptance is tested with;
  a menu opening uses the application's focus window when it is handled,
  as before. The UIA focus, selection, menu opening, and the outpost's
  own UIA property events pass NVDA's acceptance test after the window's
  UIA test; a notification does not, as NVDA tests none, and neither do a
  text focus's caret, text, and active text position. An MSAA focus fact is
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
  that is not the focus is not read. For the focus or an ancestor only
  the property the event names is read, as NVDA reads it, with the rules
  that depend on the object's role taken from the role the outpost last
  read it with (the tracking state's `focus_role` and `chain`). A state
  change that newly expands the focus, by the states the outpost last
  read for it, carries a `SysTreeView32` item's child count. A progress
  bar's value change reads its role, states, value, and location alone
  (`EventObject::progress`). A selection event on the focus or an
  ancestor is a change of its state, as NVDA handles it: its states alone
  are read and reported as one. Any other selection is spoken only inside
  a focused selection container of this outpost, so it is read in full
  only while the focus the outpost last reported is one
  (`verbatim_core::is_selection_container`, Core's own rule), and is
  otherwise dropped after its acquisition.
- Ancestors (`read::uia_remote_enrichment`, `read::uia_enrichment`,
  `read::msaa_enrichment`): the walk stops at the first ancestor in the
  previous focus's chain (the tracking state's `chain`) and splices the
  rest of that chain in, as NVDA does; the remainder is read within
  `ENRICHMENT_BUDGET`, two seconds (UIA calls inside it wait no longer
  than that), after which the ancestors are reported unknown. A walk
  that fails, the remote program by UIA's transaction timeout or the
  classic walk by a hop that timed out, also reports them unknown, never
  as an empty chain (`crates/mockapp/tests/remote_ops.rs`,
  `a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown`),
  and so does a classic focus walk (`focus_ancestry_classic`, the focus
  listener's read and the program's fallback) whose step from an element
  below the desktop root fails or finds no parent
  (`Ancestry::ancestors_unknown`;
  `a_focus_whose_parent_step_fails_is_reported_with_its_containers_unknown`).
  The outpost's own walks do the same: the remote program's walk that
  ends at a null parent anywhere but at a top-level window or a window
  below the process boundary, and
  `read::uia_enrichment`'s per-hop walk (`Uia::ancestors`) at such a step
  (`AncestorWalk::Unknown`), report the ancestors unknown: the focus-now
  answer's `FocusedControl::ancestors` is `None`, which Core receives as
  `ancestors_unknown`, as it does from the focus the worker reads after
  a `WinEvent` focus no UIA focus followed
  (`a_focus_now_whose_program_finds_no_parent_below_the_window_has_its_ancestors_unknown`,
  `a_focus_now_whose_classic_walk_finds_no_parent_below_the_root_has_its_ancestors_unknown`,
  `a_focus_read_after_a_winevent_focus_whose_walk_finds_no_parent_has_its_ancestors_unknown`).
  The focus listener keeps a last focus whose ancestors are unknown out of
  where its next read stops, so the next focus, even one inside it, is
  read to its window as a first focus is
  (`a_focus_inside_one_whose_parent_step_failed_is_read_with_its_whole_chain`).
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
- A focus-now query's UIA ancestry, by default, is one remote operation
  (`verbatim_uia_rops::focus_ancestry`, called by
  `read::uia_remote_enrichment` right after the focused element is read):
  one `Execute` checks the element's `HasKeyboardFocus` live, reads a list's
  or tab control's selected child, walks the raw-view parents with the
  base cache filled inside the provider, and finds the element's nearest
  window. The parents become the chain through `Uia::ancestors_from`,
  with the same presentable filter, the same stop at a window read through
  MSAA and the same continuation through MSAA from there. With remote
  operations off (`--classic-uia`),
  or for a window whose element could not be imported into a program (a
  client-side proxy, marked in the context for the window's lifetime and
  forgotten when it is destroyed), the per-hop walk (`read::uia_enrichment`)
  is used. A program that fails otherwise is answered by the classic walk
  for that call and logged with the failing instruction's source line.
  The program's walk ends at a null parent, as it reaches the top of a
  program's tree; one that ends there anywhere but at a top-level window
  or a window below the process boundary ended below it, and reports the
  ancestors unknown, as the classic walk does. A
  UIA focus event without text costs the outpost no UIA call before it is
  reported: the listener's read is its classic walk
  (`focus_ancestry_classic`), and the outpost's walk to its own copy of the
  element after the report is one `ElementFromHandle` and one `Execute`
  (`docs/performance.md`); for a focus with text the walk and the caret's
  read come before the report. A
  focused menu item's legacy MSAA checked state, which a menu item that no
  pattern makes checkable may have, costs the listener's read one call
  more, read live for it alone, as are the focus-now answer
  (`verbatim_uia::map::with_legacy_checked_state`) and a navigation step's
  neighbor.
- A UIA object-navigation step (`read::uia_remote_step`) is one remote
  operation (`verbatim_uia_rops::navigation_step`): from the element the
  registry keeps, read as `Kept` for its own window handle, without
  refreshing its cache first, the program finds
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

### Menus

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

### Window destruction and held objects

- Window destruction: an `EVENT_OBJECT_DESTROY` for a window drops its kept
  arbitration verdict, its mark as read without remote operations, and its
  MSAA nodes, so a reused handle is probed afresh and never inherits them.
- Held objects (outpost redesign, "Held objects"): both registries keep
  every node the outpost reports, with the live UIA element or MSAA object
  behind it where it has one.
  Each message that carries node ids (an event, or a reply with a result:
  `OutpostToSupervisor::carries_nodes`) takes the next position as it is
  written, and every node issued or looked up since the last publish is
  recorded as reported at that position then; until it is written, a node
  a waiting message may report has no position and is kept. So the
  positions are those Core counts, in the order Core reads the messages
  (under "Messages to Core" below). Core's reader counts
  the same messages and hands the app each message's position. The app
  sends `SupervisorToOutpost::NodesHeld` with the node numbers the reducer
  holds in that outpost, the text anchors it holds there (milestone M4),
  and the position of the last message it has
  handled, whenever any of the three changes, so an outpost whose held set
  stays the same still releases what it reported meanwhile. The list is
  never written on its own (`docs/design/communication.md`, step 11): the
  app leaves it in the writer's slot (`Supervisor::leave_nodes_held`), and
  the next command written to that outpost, a request or the relay's ping
  within three seconds, carries it after its own frame in the same
  write; while commands queued before it was left still wait, it goes
  with the last of them, so every request the app issued before it is
  handled first. The worker then releases every node not held that was
  reported at or before the acknowledged position; a node reported later,
  or not yet reported, is kept, since Core may not have seen it. Only the
  worker in charge releases, and the released nodes leave both registries
  under the watch lock and the outbound sender's lock, and their objects are
  dropped after both are released, so a worker that replaces an abandoned
  one can never report a node that is about to vanish. A query
  for a released node answers `Gone`. A newer list replaces one still in
  the writer's slot; the intake queue likewise keeps only the newest and
  never limits it.

### What to read

- What to read (milestone M4, `docs/design/phase6-design.md`, "Themes: one model for
  verbosity, speech, and sounds"). `SupervisorToOutpost::Fetches` carries
  the details the active theme wants read for each node
  (`SrState::fetches`); the app sends it to each outpost as it starts and
  again whenever the theme in use changes, and the reader applies it at
  once. Until it arrives, everything is read. A detail whose indication is
  off is not read: UIA reads in the worker build their requests, and the
  remote programs their fills, from the gates of the `Node` and
  `Ancestor` shapes (`docs/design/uia-shapes.md`), which the outpost maps
  from `Fetches`, one function per gated shape (`gates.rs`, a unit test
  per gated field), so a request leaves out the description's,
  shortcut's, position's, and level's properties when those are off. An
  ancestor's own description is fetched whatever the theme, since the
  layout judgment of whether an unnamed grouping is focus context reads
  it whatever NVDA's setting (decision 4), though the description it is
  reported with still follows the theme; the theme does not reach a
  dialog's text either (decision 5). MSAA
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

### Text and caret

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
    lock of its own, `text::HeldAnchors`, so the reader never waits for
    the anchor stores' locks, which the worker holds while it reads
    text) are kept; any other is
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
    scripts wait for, without waiting. When the request arrives the worker ends the watch it
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
    own report is not sent, the answer carrying the same caret. A check
    whose read covers a caret event of the node that is still waiting in
    the intake, observed at or after the key's press, counts that event as
    the event's own check would have (`text_reads::Signal`): a watch with
    no caret known before the key, for which a caret event is the
    evidence, is then answered by the read that covers it, as a terminal's
    read does for a caret event the intake planned after its text change,
    rather than left open when the covered event checks nothing. While a
    check reads, the watchdog treats the watch's request as the one running,
    so an abandonment answers it. A watch ends without evidence, answered
    `TextReply::WatchEnded`, for which Core says nothing, when the next
    caret key's watch replaces it, when a focus on another node is
    reported, or when it is `CARET_WATCH_BOUND` (10 seconds) old, which
    only frees it: the intake wakes the idle worker then (`Item::Wake`,
    `Intake::wake_at`), and nothing is spoken. So a key that moves nothing
    is silent (`docs/parity/text.md`, "TextInfo-equivalent layer"), and a key
    whose application reports nothing when it moves the caret is silent
    too. Through UIA each check is the whole caret read above, one round
    trip remotely, so the check that finds the evidence is the answer, with
    nothing more to read. The evidence is the caret no longer where it was
    known to be, the characters either side of the caret changed from what
    was known, the text at the caret changed after a Delete, the caret's
    line changed, after a key that deletes, from the line of the caret
    this outpost last reported before the key (the watch's `deletes`: in
    a run of one character, such as a password field's mask, deleting one
    leaves the caret's neighbors as they were, and a provider's positions
    follow the edit; a position Core gives is not compared so, since it
    may be in any chunk, such as the paragraph say-all last read), the
    selection changed, or the selection in the report the caret was known
    from differs from the selection now: something selected in one and
    not the other, or ends that are not the same positions. The last
    counts because a caret event's report can compare the selection, and
    replace the one kept, before the watch arrives: a provider's positions
    follow edits, so a Backspace over a selection in Windows 11 Notepad
    can leave the caret's neighbors as they were, and a selection
    extended by Shift and a click after a key that changed nothing leaves
    a classic edit control's caret, the selection's start, where it was.
    The ends are compared only when that report's selection is not the
    one the read compared with already (each report is numbered, and the
    kept selection carries its report's number), so through UIA the
    comparison's calls are made only after such a report. Where it was known to be is the newest caret this
    outpost reported for the node from a read that finished before the key
    was pressed (the watch's `pressed_at_ms`, stamped by the keyboard hook
    on the same Unix millisecond clock as `observed_at_ms`), which can be
    newer than Core's (an earlier key's late caret event, or a paste's),
    else Core's. A caret read at or after the key's time is never the
    baseline: the application's caret event for this very key can reach
    the outpost, and be reported, before Core's request does, and judging
    against it would find no evidence. The outpost remembers its last eight
    reports per node for this, and forgets them when it moves the caret
    itself (`TextOp::MoveCaret`, as say-all does), since Core takes such a
    move as its caret. The characters matter because a provider's
    positions follow edits (a deleted character takes the known position
    with it) and the application may have handled the key before the
    request arrived; only the caret's neighbors count, since a line can
    wrap anew with no key at all, except after a key that deletes, which
    changes its line. A caret event alone is evidence only when
    nothing knew the caret: it can be the application's late report of
    something earlier. Before a check answers, the caret is read again
    (`CaretRequest::recheck`; at the end of the same program through UIA,
    `CaretRead::moved_while_read`, and with one more read and comparison
    for an edit control), and when it moved while it was read the check
    answers nothing: the caret may be from before a write and the line
    from after it (a Backspace's character message in an edit control,
    the console host writing a typed character between two calls of the
    one program), a state the text never had; the event that follows the
    move (the control's caret event, the terminal's text change) checks
    again. The selection's changes are worked out by comparing
    endpoints, as the contract says; through UIA the caret read works them
    out and reads their text in the same round trip when the selection
    moved (`CaretRead::changes`), and the edit controls work them out call
    by call.
  - The caret before a held key (`TextOp::ReadCaret`,
    `docs/design/key-holding.md`) is read as a caret event's
    report reads it, with no formatting and without comparing or
    replacing the selection kept for the node (`LastSelection::Ignore`),
    one round trip through UIA where the provider runs remote operations,
    and is answered as a `CaretReply` with `moved` false and its read
    time. It joins the node's remembered reports, so the held key's watch,
    which Core sends with a press time just after the read, takes it as
    its baseline. It is not noted as a read of the caret that covers a
    caret event observed before it.
  - A selection no key made, such as the mouse's or the application's, is
    reported with the caret event that follows it, as NVDA's auto-select
    detection reports any selection change of a UIA or classic edit focus
    (`docs/parity/text.md`, "TextInfo-equivalent layer"). Each node keeps
    the selection of the outpost's last caret report for it, its ends and
    whether it held nothing (`NodeAnchors`), set by every report, a caret
    key's answer included, but a terminal's after its focus. A caret
    event's report of a focus that is not a terminal
    (`text_reads::CaretFor::Event`) asks the caret read to compare with it
    (`caret_report_since`'s `LastSelection::Compare`), and its changes
    travel in `CaretMoved`'s `selection_changes`, by the caret watch's
    rule. Through UIA the comparison and the change's text are part of
    the caret read's one round trip; when the last selection held nothing
    (`CaretRequest::previous_collapsed`) and nothing is selected now, the
    program compares nothing, so a typed character's report costs what it
    did. The edit controls compare offsets, with no message, and read a
    change's text with the window's text. The selection compared with is
    forgotten at a new focus, so the next report carries no change and
    sets it anew (the report after it, whose selection Core already speaks
    as the focus's). When the outpost selects or moves the caret for Core
    (`Select`, `MoveCaret`), whose own reply speaks what it did, it keeps
    what it made instead (`NodeText::keep_selection`): the selection as
    read after the select, with its text up to the cap for compared text,
    in the select's one round trip through UIA (`RangeRead::selection_after`,
    one `GetText` more), or the caret where it moved, collapsed; so the
    application's report of it carries no change and the next key compares
    with it. The kept selection is the only old selection any key compares
    with, a selecting key's too: its answer compares with it exactly as a
    caret event's report does and replaces it, so each change is compared
    once, by whichever reads it first, and Core sends no selection of its
    own. When the key's caret event is reported before its watch arrives,
    the report carries the change and the answer has none left; Core says
    the report's change after the key's own words
    (`docs/crates/verbatim-core.md`). A terminal's reports carry no changes: NVDA's
    console class replaces the detection, which is not yet traced, so a
    terminal's reports keep no selection. A text change never forgets the
    selection.

    A report's selection starts at a position Core can read before: in
    the caret's line chunk when the selection starts there, or in the
    line it starts in, sent as `CaretReport::selection_line`, when that
    is an earlier line, so a Backspace over the selection says what comes
    before its start, as NVDA's caret, the start, reads it. Through UIA,
    where `TextPattern2`'s caret can be the selection's other end
    (Windows 11 Notepad's after Shift with Right Arrow or End), the caret
    read finds the start in the same program (`CaretRead::selection_start`):
    its offset in the caret's line, or the earlier line it starts in and
    its offset there, and, when it starts at its line's start, whether
    that is the text's start, which marks the line `first`. Read by parts,
    the start lies in the caret's line where it is the caret, as in a
    classic edit control.

    Whether a comparison is made in general terms is one decision, the
    same for a caret event's report and every key's answer
    (`text::content_changed`): the text changed since the kept selection
    was reported when the node's note is set, or, with a selection in the
    kept one or now, when the kept selection's text as it was reported no
    longer matches what its ends hold now, or when the control's value
    change waits once the caret is read. The note is NVDA's
    `hasContentChangedSinceLastSelection` (`docs/parity/text.md`,
    "TextInfo-equivalent layer"): it is set when a classic edit control's
    value change is handled (`text_reads::note_value_change`), and cleared
    by the next report, a caret key's answer included, that has a
    selection to compare with, and by a focus. The waiting value change
    (`text_reads::value_change_raised`, through `caret_report_checking` and
    `CaretSignal::text_change_waiting`) covers the Windows Forms text box,
    which raises its caret's move before its value change and may raise
    the value change while it answers the read, so the check first waits
    for the `WinEvent` thread to have taken every event raised before it
    (`Nudger::sync`): a thread message posted to that thread can be
    handled before a `WinEvent` raised before the post has reached it, so
    the sync, besides its thread message, raises a sync marker, a
    `WinEvent` of the outpost's own process that the thread's one hook set
    also hears (`verbatim_ia2::SyncMarker`), and returns once both have
    arrived, the marker arriving behind every event raised before it; "x"
    typed over a selection, the text box's
    Control+X, and Backspace or Delete over a selection are said so. The
    remembered text is Verbatim's own, Dickson's decision of 2026-10-22:
    through UIA the caret read reads the whole selection's text in the
    same round trip and keeps it with the selection
    (`CaretRequest::read_selection_text`, `KeptSelection::text`), and the
    next comparison reads what the old selection's range holds now
    (`CaretRequest::read_previous_text`), which a provider moves as the
    text changes; when the two differ, the comparison is in general
    terms, so "x" typed over a selection in Windows 11 Notepad says
    "selection removed" where NVDA, which notes no UIA text change but for
    Word and the terminals, says "x unselected". Both are read in full,
    with no limit, and compared length first. An edit control's offsets do
    not follow edits, and its note covers its changes, so it reads
    neither. Such a comparison (`text::general_changes`)
    compares the ends only, and sends each change with no text, whether
    something was selected or unselected, with `content_changed` and the
    whole new selection's text in `new_selection`, the one text it reads,
    when anything changed and something is selected. Two caret reports merged while they wait to be
    written (`outbound::merged`) carry both reports' changes in order, a
    waiting report that carries changes is never merged across a reply
    (`Pending::waiting_with`), which would have it reach Core after the
    reply to a key pressed before Core saw it, nor with a report in
    general terms, or into one, whose words come from its own selection.
  - Which applications raise the evidence, as checked live on
    2026-10-08 (`docs/design/phase6-design.md`, "Live caret event
    checks"): the focused Windows Forms text box raises, for every caret
    move, `EVENT_OBJECT_LOCATIONCHANGE` on `OBJID_CARET` between the
    caret's hide and show, then `EVENT_OBJECT_TEXTSELECTIONCHANGED` on its
    client, and for Delete, which moves no caret, a value change and the
    caret's hide and show; Windows 11 Notepad recreates its caret on every
    move (destroy, create, location change, show) and raises
    `EVENT_OBJECT_TEXTSELECTIONCHANGED` and UIA's text selection changed;
    the console host raises its caret events together, 10 to about 520 ms
    after the key; and Windows Terminal raises no caret `WinEvent`, only
    UIA's text selection changed. The hook reports the caret's location
    change and show as caret events. Measured the same day with mockapp's
    real edit control, a Common Controls version 6 edit control raises
    `EVENT_OBJECT_TEXTSELECTIONCHANGED` whenever its caret moves, focused
    or not, and the classic edit control raises nothing for an unfocused
    caret move.
  - Reads move first when asked: from the start of the unit containing the
    point, by whole units, never past the text's ends, saying how far they
    went; a document movement goes to the start or the end. A unit the
    source lacks is `UnsupportedUnit`; UIA has no sentence, and an edit
    control's sentence read is its paragraph, which is its line, for Core
    to split. `ReadRange` reads up to 1 MB for a copy, `Select` and
    `MoveCaret` select through the backend, `Select` reading the selection
    before and after it and answering how it changed, by the caret
    watch's rule, and `Location` gives the
    screen position of the character at a point. `ReadAhead`, say-all's
    read, reads up to its count of units in one request, each the unit
    after the one before, stopping once 32 K UTF-16 code units are read,
    and marks the last chunk as the text's last when no unit follows it.
  - Through UIA, every request but the caret watch is also one round
    trip where the window's provider runs remote operations (`Select`'s
    read before, selection, and read after included, in one program), with
    the same choice, fallback, logging, and marking as the caret read: a point
    named as the protocol names it (`PointFrom`: the caret, a selection's
    end, an end of the text, a kept position, or a position some text after
    a kept one, which is found inside the provider by matching that text)
    is found, moved from, and read in one program
    (`TextSource::read_units`, `range`, and `point_location`, which
    `UiaText` implements with `verbatim_uia_rops::text_units`,
    `text_range`, and `text_location`). A point found by matching text is
    remembered as one the outpost reported, so naming it again costs
    nothing; the points just past a range's two characters, which the
    review copy sends with its ends (`TextThrough`), are not, since each
    press sends them once. The edit controls keep their message-by-message
    reads.
- Caret and text events (the worker). A focus that may have text (through
  UIA an edit field, a document, or a terminal; through MSAA an edit
  control's client area), and any other focus whose role is edit field,
  document, or terminal, has its caret read while the worker handles the
  focus, before it is reported (`Worker::read_focus_text`), and the read
  comes in the focus's own message (`FocusChanged`'s `text`), so the line
  at the caret arrives with the name, role and states and nothing from any
  source comes between them, as NVDA's object announcement reads an
  object's text within it (`docs/nvda/speech.md`, "What an object with text
  says"). When something is selected the selected text is read too, which
  Core speaks in place of the line, a protected (password) field's too,
  whose text is read masked as the control gives it. A focus with no
  text to read (an MSAA
  object that is not an edit control, a UIA element with no text pattern)
  carries `FocusText::NoText`, and Core speaks its value in place of its
  line, as NVDA speaks the value of an object without navigable text. One
  whose text could not be read now (its element was not found, the
  provider did not answer, or the caret read failed) carries
  `FocusText::NotRead`, and Core speaks neither the line nor the value,
  which for a document is all of its text. A caret read that hangs holds
  the whole focus report until the handling deadline (Dickson,
  2026-10-21). A focused terminal's caret is read as any other's, so a new
  terminal window or tab says "blank" for its empty line, in Windows
  Terminal as in the console host (`docs/parity/terminals.md`, "New terminal output"). The worker then
  follows its caret:
  the focus-following UIA subscription, given the focus with its extended
  group when the focus has text, its caret, text, and active text
  position events' elements arriving with
  only `verbatim_uia::TextEvent` cached (nothing is spoken
  from them), delivers `Text_TextSelectionChanged`,
  reported as `CaretMoved`, and `Text_TextChanged`, reported as
  `TextChanged`, and, in the same event handler group, the active text
  position changed event, reported as `ActiveTextPositionChanged` with the
  start of its range kept as a position in the focus's text (an anchor
  minted for it, no call), and dropped when its element is not a node the
  outpost knows or it carries no range, as NVDA's handler drops it; the
  intake keeps the newest one per element. For an edit control, the hooks' caret
  (`EVENT_OBJECT_LOCATIONCHANGE` or `EVENT_OBJECT_SHOW` on `OBJID_CARET`)
  and text selection events are reported as `CaretMoved`, and its value
  change as `TextChanged` without reading the control's whole text as its
  MSAA value. For a UIA focus, the hooks' caret event from any window
  NVDA's acceptance test takes a location change from
  (`acceptance::EventName::LocationChange`) is the focus's caret event,
  handled as its UIA caret event is (`Worker::system_caret_of_uia_focus`),
  as NVDA handles the system caret's events on its last queued focus,
  since some UIA documents, such as Visual Studio's, raise no UIA caret
  event. Once the focus has raised a UIA caret event of its own, and in a
  focused console host terminal from the start, whose console caret event
  is its caret event, the worker marks the focus
  (`Intake::drop_system_caret`) and the intake drops the system caret's
  events as they arrive, waking nobody, until the next focus is
  reported: Windows 11 Notepad raises both for every move, the system
  caret's first, and the console host raises both at the same blink
  tick, so each would read the caret a second time. NVDA's exceptions
  are not built: an IAccessible2 focus, not read before milestone M7,
  Word's UIA document, Word's own rule, and Scintilla's refusal of the
  caret's shows and hides, Notepad++'s, both for the app-module track
  (`docs/parity/text.md`).
  The intake keeps one waiting caret report per node, and a caret event
  observed before the worker last read that node's caret (a caret key's
  check reads it after the event) is dropped. A caret event of a focused
  terminal whose reading is held while its text has changed unread is
  noted instead of read (`Item::NotedCaret`): the intake hands it out
  only when nothing else is ready (`Intake::when_idle`), and it then reads
  the caret unless a read since, such as the one Core asks for, saw it.
  While such a terminal is the console host's and no caret key's watch is
  open, the worker marks its window in the intake
  (`Intake::absorb_console_updates`), which then drops the window's
  console updates as they are pushed, without waking the worker, which
  would do nothing for them; the worker clears the mark before it handles
  a terminal request or cancel, a caret key, or a new focus, so the read
  that ends the hold covers every update absorbed. A caret event, a text
  change, and a text selection change of the node a caret key's watch is
  open on check the watch first. A focus-now answer's control has its
  caret read with the answer, by the reported focus's own read, since
  Core takes it as its focus. Once the answer is sent it becomes the
  outpost's focus as a reported focus does, by the same function
  (`adopt_focus`, which `emit_focus` calls too): a caret key's watch on
  another node ends, the intake is told (`note_focused`), and the
  tracking takes its node, role, states, window and chain; a control that
  is already the focus this outpost last reported, under the same node,
  is not adopted again, and one whose element it has under another node,
  as File Explorer's reused runtime ids give (`docs/uia-bugs.md`), is.
  It is then followed as a reported focus is (`follow_text`): the
  focus-following subscription moves to it, so its caret events no key
  caused are reported and a caret key's watch on it is answered by them,
  and a terminal has its first read, so what its text changes say is
  reported and Core's on-demand reads of it are answered. It
  gets no property subscription. Adopted or not, it becomes the last
  focus the foreground tests compare with (`Tracking::queued`, by
  `queue_answer`), as NVDA's fake focus, queued as a focus event, becomes
  its last queued focus (`eventHandler.py`, `_trackFocusObject`), so a
  late foreground change of its window is dropped (Dickson,
  2026-10-22): an answer for the node already queued refreshes only its
  observation time, keeping the window and the MSAA event address its
  report recorded, which the second foreground test compares with;
  another node replaces it with no event address; and an answer with no
  window leaves it as it was. It is not counted among the focuses
  reported (`Tracking::reported`), and leaves the MSAA duplicate rule's
  qualifiers (`Tracking::foreground_since_focus`,
  `Tracking::focus_was_simple_child`) as the last reported focus left
  them, since they describe that focus's event. Whether the focus is a
  terminal is decided by the focus's own role (`Tracking::focus_role`),
  which a foreground report, naming a window, leaves as it was, so a
  terminal stays followed through a foreground report that no focus
  event follows. The watch is
  checked before a system caret event is taken as the caret event of a
  UIA focus this outpost last reported, at the cost of the watch's lock
  for each such event, and the registry's while a watch is open; while
  that UIA focus has the intake dropping the system caret's events
  (`Intake::drop_system_caret`), the edit control's are dropped with
  them, so only its text selection changes answer its caret keys until
  the next focus is reported. A classic edit control that raises none
  while focused would have its caret keys unanswered until then; whether
  it raises them is not known, since the measurements above cover only
  its unfocused caret moves.

### Queries

- Queries (the worker): `ShellItems` (`outpost/shell_items.rs`) reads the
  notification area (`TrayNotifyWnd` under `Shell_TrayWnd`, with the
  overflow flyout while visible) or the taskbar (`Shell_TrayWnd` without
  the notification area's subtree) in the control view, each element as
  the `ShellButton` shape, and answers every named, on-screen button with
  a rectangle as a `ShellItem`; it reads no window its application does
  not serve, looks `Shell_TrayWnd` and `TrayNotifyWnd` up without the
  proxy rule (`any_window_element`) and the overflow flyout's window
  under it (`window_element`), and asks its time budget (`WALK_BUDGET`,
  2 seconds) before each call after the first, answering the items read
  by then once it has passed, within the worker's deadline for the query
  (`protocol::WALK_DEADLINE`, 5 seconds, public so that Core's deadline
  for the shell items can outlast it).
  `DumpTree` walks the target's foreground window (or
  its first visible top-level window) through its backend — UIA via
  `Uia::tree`, every node read as the `TreeDump` shape, MSAA via
  `verbatim_ia2::acquire::walk_tree` from `OBJID_CLIENT` — capped at depth 64
  and 4096 nodes, reporting whether a cap cut it short. `Ancestors`,
  `Navigate`, and `Activate` dispatch to whichever registry issued the node:
  UIA hops through the raw-view tree walker with the base cache request, MSAA
  through `accParent`, `accNavigate`, and `accDoDefaultAction` on the object
  the registry kept for the node. A node no registry knows, or one that can
  no longer be reached, answers `Gone`.

### Selection and notification events

- Selection and notification events: both backends' selection events emit
  `NormalizedEvent::SelectionChanged` (the selected node's full snapshot)
  and UIA notifications emit `NormalizedEvent::Notification`. A UIA
  selection inside an element the focused element names in its
  ControllerFor relation emits `NormalizedEvent::ControlledSelection`
  instead, carrying the focus's id, as NVDA reports a search suggestion,
  and so does one inside none of them, with the first controlled
  element's current selection as its node, logged at debug ("the first
  one's current selection is reported"), unless the selection names the
  focus itself (compared by runtime id, with no call), which then goes on
  to the other selection handling, as NVDA's base selection handler falls
  through to its state change handler; one found in none with no current
  selection read is logged at debug ("a selection in none of the elements
  the focus controls, and no current selection read from the first"):
  the relation is read live for each selection, as NVDA reads it, and it
  and the search for the selected element under what it names are one
  remote operation where the focus's window allows one
  (`verbatim_uia_rops::controlled_selection`). The reducer
  announces both ([verbatim-core](verbatim-core.md)).
- `OutpostMessage::Event`, which `Frames` makes from a frame, carries the
  target pid, the outpost id, the message's position, and the boxed
  `OutpostToSupervisor` payload: the replies grew the message enum well
  past the lifecycle notices, and boxing keeps every post small. `Supervisor::leave_nodes_held` leaves a
  `NodesHeld` list for one incarnation, which the next command written to
  it carries.

## Messages to Core

The thread that sends Core a message writes it itself
(`outpost::outbound`, through the transport's `Sender`;
`docs/design/communication.md`, section 4.3): the worker its events and
replies, the I/O thread its pongs, the thread watching the target
application `TargetExited`. When no write is in flight the message is
written at once, with one `WriteFile`, and no other thread wakes. When
one is in flight, the write pending until Core has read enough of the
pipe, the message waits in one queue, the sender's backlog, and the I/O
thread writes what waits once the write in flight completes. Nothing that
sends a message ever waits: a watchdog that waited behind a full queue,
as it did when queuing waited for room under the watch lock, could not
abandon a hung worker.

- Pongs, `Ready`, and `TargetExited` go ahead of everything else that
  waits, behind only the write in flight, which is never cut, so a busy
  outpost is never mistaken for a dead one.
- While messages wait, they are merged by object and kind, as the intake
  and the supervisor merge what reaches the outpost and as NVDA's limiters
  do. An event replaces a waiting event of the same kind for the same node
  (for a property change, the same property; for a focus, the same kind of
  focus) and goes to the back, unless the waiting one is a caret report
  carrying selection changes with a reply after it, when both are kept in
  order. What a terminal says follows what it said still waiting, in one
  message, in order; Core's flood policy then treats it as any other.
- Answers to Core's requests are never merged and keep their order, and
  so do notifications, each of which says something of its own, and
  faults. A cancelled query's `NotStarted` answer takes its place among
  them too.
- Nothing is merged across a flush (`Outpost::settle`'s), and a flush is
  answered only once every write before it has completed, so everything
  published before it is in the pipe when it is answered.
- What waits is bounded by the nodes and kinds there are, by Core's
  requests (the supervisor's queue to the outpost holds at most 64), and
  by the intake's limits, without a bound of its own.
- The messages that carry node ids are numbered as Core counts them as
  each is written, and the nodes each may have reported are recorded at
  its number then (`Pending`, the backlog, sees every message, written at
  once or after waiting).
- What a terminal says too large for one message is split as it is
  written (`terminal::split`), each part a frame of its own, numbered as
  Core counts it.
- A terminal read that reports the caret and its speech both sends them
  together (`Outbound::publish_all`, the transport's `Sender::send_all`):
  two frames, each with its own header, in one write when none is in
  flight, so Core's relay wakes once for both; either alone is sent alone.
- Every message is stamped as its write begins, by whichever thread
  writes it (`written_at_us`, under `EventTiming` above).

The supervisor's side already never waits: each child's writer queue
fails or replaces a command when it is full (the `Supervisor` entry above),
the relay hands every frame on through the app's sink, which never
waits, and the
maps of writers are locked only to look one up, never across a push or a
write. The focus listener writes the same way, each fact by the thread
that produced it, and its backlog merges its facts by element and kind,
and for an MSAA event other than a menu opening by the thread that raised
it too, and never waits
either.

## The idle barrier

The harness's `AwaitIdle` (`verbatim-app`'s `idle` module;
`docs/design/communication.md`, section 6.7 and step 6a of section 11)
is answered only after a round in which every child said nothing that
came before the question was still in progress. The round is in band, so
it covers whatever is in pipes and backlogs without counting it.

- `Supervisor::query_idle(round)` posts the round to the relay. The relay
  writes `IdleQuery` to the listener, behind every command sent to it
  before; while the listener is launching, the query waits with the
  hand-over's news and is written once it is launched. A listener that is
  missing, fails to launch, or ends or is replaced during the round
  counts as having answered.
- The listener hands the query to its `WinEvent` thread as a thread
  message (`Nudger::nudge`), behind every event delivered to that thread,
  and the thread hands it to the reading threads (`Readers::when_idle`),
  which run the answer under their lock once no event queued before it is
  waiting or being read, so it goes behind every such read's or drop's
  outcome. Each queued event takes its place from one count across every
  application and the answer keeps the count when it came, so events
  queued after it are not waited for.
  A menu or switcher end still in its grace goes first, and the answer
  behind it. The answer goes through the listener's sender behind every
  fact sent before it, and no fact is coalesced with one before it across
  it.
- On reading the listener's answer, the relay asks every outpost in the
  same pass, so each query is behind every fact routed to that outpost
  before it, and first tells the app which it asks
  (`OutpostMessage::IdleQueried`), so that message reaches the app ahead
  of every answer. While an outpost is still being launched, the app may
  not have heard it started, so the relay asks once every launch has
  reported; an outpost not yet ready is asked behind its held facts when
  it is. In the relay's writer the query gets through a full queue and no
  fact queued after it replaces one queued before it.
- An outpost queues the query in its intake (`Item::IdleQuery`), where
  it divides what waits: no entry that arrives after it replaces one
  waiting before it, and it ends the batch it is planned in, so that
  batch's menu opening goes before it. The worker reaches it once
  everything that arrived before it has been handled; a noted caret goes
  first, the query going directly behind it, ahead of what arrived after
  the query (`Intake::push_ahead`), and waiting for nothing more when it is
  reached again. It is then answered as soon as no
  caret key's watch is open and no terminal owes Core a read: checked after
  every entry the worker in charge handles. The requests terminals owe are
  kept under a lock of their own, never held across a call into the
  application, so a worker abandoned in a terminal's read cannot keep its
  replacement waiting there. Entries that arrive after the
  query are not waited for, nor are the follow-ups they queue or the
  carets they note, so a flood cannot hold the answer back. The
  answer goes through the ordinary queue, never merged, and no event is
  merged across it (`Pending::waiting_with` stops at it).
- An outpost's answer reaches the app as an `Event` like any other
  message; an outpost that ends during the round answers by its `Ended`,
  which follows everything it sent.

## Shutting down

Outposts and the listener are UI Automation clients of every application
Verbatim reads, and killing a client in the middle of a call or a remote
operation into an application is the leading suspect in a Windows Terminal
crash inside `UIAutomationCore.dll` (`docs/design/phase6-design.md`, "Windows Terminal
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
in the summary as `killed`; so is an exit with another code. A wait that
fails, rather than running out, also ends in a kill, logged as an error
and counted apart, as `killed_on_failure`. The end-to-end suite
checks after every scenario that each outpost and the listener exited
with code 0 before Verbatim did.

The outpost's side (`Outpost::shutdown`), in order:

1. The intake closes: nothing new is taken, and what was waiting is
   dropped.
2. The `WinEvent` hooks are removed, with the thread that pumped them.
3. The focus-following UIA subscription is closed: the process's
   registering thread removes each of its groups one by one
   (`RemoveEventHandlerGroup`), each removal waiting for callbacks in
   progress; `RemoveAllEventHandlers` comes at step 7, as the
   registering thread ends.
4. The worker finishes the entry in hand and every abandoned worker
   returns from its call, and so does every thread of the client for
   operations with a deadline of their own. The watchdog ends first, so
   nothing more is abandoned. No call is cut off: a call into an application that does
   not answer is ended by UIA's own timeouts.
5. Every object held is released: every node leaves both registries, with
   its text patterns, anchors, and terminal memory.
6. What waits is written, the answer to the call that was in progress
   included, every write in flight completes, and the pipe is closed. The
   outpost never exits with a write in flight, since its exit would cancel
   the write and cut the frame (`docs/design/communication.md`, section
   4.6): its I/O thread serves its port until the sender has finished
   (`ChildIo::finish`).
7. The thread leaves COM's multithreaded apartment, and the process's
   hold on it is given back (`verbatim_uia::release_mta_usage`), so COM
   ends the apartment once every thread has left it; each worker leaves it
   as it ends, and so does the process's registering thread, which that
   call ends first (`docs/crates/verbatim-uia.md`, `Registration`).

The outpost logs how long each step took (`the outpost shut down`). The
listener's shutdown removes its hooks, its desktop-wide focus handler, and
its other desktop-wide handlers, writes what waits, waits for every write
in flight as the outpost does, and gives back the apartment; it makes no call into an application, so nothing else can
be in progress. Both then end the process with `TerminateProcess` and
exit code 0 rather than returning from `main`, so no DLL's process-detach
code runs (`docs/architecture.md`, section 1, "Process and thread model").

The time limit, `SHUTDOWN_LIMIT`, is 21 seconds. Over the end-to-end
suite on the development machine (2026-10-08, 436 shutdowns), a clean
shutdown took from 4 to 443 milliseconds from the shutdown message to the
process's exit, as the supervisor logs it (`shut down cleanly`,
`elapsed_ms`): a median of 15 and a 99th percentile of 31, the slowest
the focus listener's once. The only thing that can make it take longer is a call in
progress, which the outpost lets finish. UIA ends a call to a provider
that never answers by its transaction timeout, and one to a provider that
cannot be reached by its connection timeout, `TRANSACTION_TIMEOUT`, five
seconds, and `CONNECTION_TIMEOUT`, two, set once per process; this bounds a classic call and a remote
operation's `Execute` alike (mockapp's `remote_ops` and `time_limits`
tests pin it). A search makes several calls, each waiting up to that
long, so the limit was left at 21 seconds when the timeouts were
shortened from UIA's default of 20 (2026-10-11); waiting longer could
only wait on an MSAA call into a hung application, which nothing times
out, and that child is killed. The kill only ever happens while an application is not
answering, and Verbatim's exit then waits up to the limit, after the exit
sound.

Tests: `crates/mockapp/tests/shutdown.rs` tells a real outpost to shut
down while its read into mockapp is held at its first provider call, once
for a remote operation and once for classic reads, and checks that the
read's answer is sent, the outpost's four UIA event registrations on
mockapp's window are removed (counted by mockapp's fragment root through
`IRawElementProviderAdviseEvents`), the pipe is closed, and mockapp still
answers another client; and that a ping handed to the outpost while its
worker is held in a call is answered with a pong at once.
`crates/verbatim-outpost/tests/shutdown_fallback.rs`
has a real supervisor launch a stand-in listener that never exits, and
checks that the shutdown's summary reports it killed because its time
limit passed, not because the wait for its exit failed. It does not time
the kill: the wait runs on the system timer tick and can return a few
milliseconds before the limit.
`crates/verbatim-outpost/tests/target_exit.rs` runs the real outpost
binary against a stand-in application, which it ends, and checks that the
outpost says `TargetExited` and, sent `Shutdown`, closes its pipe and
exits with code 0. The end-to-end suite checks every run's outposts and
listener as above.

## Terminals

The `terminal` module (public, so mockapp's tests drive it as the worker
does) reads a focused terminal and gives each read to the terminal's
reading rules (`docs/design/terminal-model.md`, sections 3 to 5;
`verbatim_text::terminal_reading`), which say what is new and what the
keys did. Terminal behavior is generic, keyed by the control, never the
window's title: a UIA element of class `TermControl` (Windows Terminal) or
`WPFTermControl` (the terminal embedded in Visual Studio), or a focus in a
`ConsoleWindowClass` window (the console host), is `Role::Terminal`. The
console host's text area is read through UIA, as NVDA reads it on current
Windows (its provider reports text formatting, which arbitration checks).
Its focus comes from the host's own process, while
`GetWindowThreadProcessId` names the console's client as the window's
owner, and inside a remote operation the host's provider gives no native
window handle for the window; the classic walk to the nearest window finds
it.

- The worker keeps, per terminal node, a `Terminal`: the reading rules
  (`TerminalReader`, whose memory holds the terminal's rows, lingering rows,
  known history, review position and command marks), where the next read
  starts (`Snapshot::next_start`), the screen's height and width as the
  last read found them, whether the next read takes the colours, its
  on-demand reading state (`terminal::reading`), when the quiet-period
  timer is due, the places its review answers minted, and the caret last
  sent to Core. It is kept while the node is, across focus changes
  (section 5.10): the focus arriving tells the rules (`Terminal::focus`),
  whose next read speaks nothing, and the focus leaving does too
  (`Terminal::blur`, from `Worker::blur_terminal_elsewhere`), which drops
  what waited for the quiet period.
- Every read is one snapshot (`verbatim_uia_rops::terminal_snapshot`, with
  the classic fallback behind the same entry point): the text from where
  the last read said to start, as many rows above it as the memory asks
  (`TerminalMemory::above`), to the document's end, in one `GetText`, with
  the screen, the caret and its line, and, in the first read after a key,
  the screen's colours, all in one remote operation (`terminal::read`). It
  becomes the rules' `Read` (`terminal::read_of`: the text as UTF-8, the
  caret's row and its column in characters from the caret read with it,
  stamped with when the read began on the run's shared clock), the
  colours converted at the boundary from `verbatim_uia_rops::ColourRun` to
  the rules' own. When the rules ask for the whole document or for a read
  from further above to find how far a full history rotated, that read is
  taken at once, up to `MAX_SNAPSHOTS` (12) snapshots for one read. What
  the rules say becomes Core's `TerminalSpeech` (`terminal::speech_of`):
  output (`TerminalItem::Output` for a new line, what a growing line
  gained, a redrawn row's change and a held line as last read, and
  `TerminalItem::Skipped` for a read's own flood summary) and the answers
  to keys (`Echo`, `Caret` in its unit, `Gained`, `Lost`, `Answer`,
  `Suggestion`), and whether the review cursor followed the caret. It is
  sent as `NormalizedEvent::TerminalSpeech` when it says anything, after
  the caret read with it (`text_reads::terminal_read`, `CaretMoved`), since
  a terminal raises no caret event for every character typed; a caret at
  the same place, with the same line read the same way
  (`text::same_caret`), is not sent again, unless the read answers Core's
  request. The read is stamped with when it began, so output read before a
  key's cut is never spoken after it.
- Keys. Core tells the outpost of every key that reaches a focused
  terminal (`TextOp::TerminalKey`, answered `Done`): a typed character, a
  caret key Verbatim knows with its unit, a key that forgets typing not yet
  shown, or any other, with when it reached the program. The rules open the
  key's window, 300 milliseconds in which reads can carry its effects
  (section 5.5), and the next read takes the colours. A typed character
  the terminal does not show within 300 milliseconds is dropped at the next
  read. A held caret key's read before it is passed on (`TextOp::ReadCaret`)
  is, in a focused terminal, a snapshot given to the rules
  (`Worker::terminal_before_key`), whose caret answers the request and
  joins the node's caret reports, so the key's watch takes it as the caret
  before the key; the watch's evidence, a caret event or a text change,
  then reads the terminal, and the rules' answer to the key is the key's
  speech. The watch's own answer keeps Core's caret current and says
  nothing in a terminal. The time a key's report reaches the outpost and
  the time the program shows its effect race for a key that is not held:
  a read that began after a typed character reached the program but was
  handled before the character's report arrived speaks the echo as the
  line's growth instead, the same text.
- The quiet-period timer (section 5.14): the rules say when what waits for
  the quiet period is due (a volatile row to settle, a growing line's
  unfinished word, rows a key changed waiting for the keys to pause); the
  worker keeps the focused terminal's due time apart from the terminals'
  lock (`Context::terminal_due`) and asks the intake for one wake-up then
  (`Intake::terminal_timer_at`, `Item::TerminalTimer`), whose speech it
  sends. The idle barrier's answer waits while the timer is armed, so a
  scenario hears everything the rules hold before it is asked to be quiet.
- When to read. In the console host the console's own `WinEvents`
  (`EVENT_CONSOLE_UPDATE_REGION`, `_SIMPLE`, `_SCROLL` and
  `EVENT_CONSOLE_LAYOUT`, merged into one per window while they wait) are
  its terminal's text changes, and UIA's text changes are not subscribed
  for it; in Windows Terminal UIA's text changes are, and its caret
  events read the terminal too, since it sometimes moves its caret with
  no text change (section 5.1); the read takes the caret, so it is not
  read again on its own. The console host's caret event
  (`EVENT_CONSOLE_CARET`), raised on its cursor's blink timer when the
  cursor has moved, has the caret read and reported, and reads the
  terminal when it is a caret key's evidence. A change observed before
  the last read began is covered by it and reads nothing
  (`Terminal::covers`). Reads made on Windows Terminal's events never wait
  for a frame; the read before a held key, prompted by no event, can cost
  that key one software-rendered frame (section 2.1). Windows Terminal
  raises no text change for writes made only of blank lines, so such a
  scroll is read at the next event, its caret's move among them.
- Taking the focus. Once a terminal's focus is reported with its caret,
  the focus-following subscription is moved to it and the worker waits
  for the move (`Registration::settle`) before the first read, so output
  written after that read began raises a change that reads it (section
  5.10). That first read speaks nothing.
- On-demand reading (`terminal::reading`, section 5.1), a pure transition
  function the worker drives. Live, the default, a text change of the
  focused terminal is read at once and what it says sent. Core's text
  requests change it: `TextOp::TerminalHold` (Core's queue is full: changes
  are only noted, answered `Done`), `TextOp::TerminalRead { hold }`
  (answered with what the read says, read only if the text changed since
  the last read, the read taken as one that waited for Core's queue, then
  holding or live again), and `TextOp::TerminalCancel` (speech was cut
  off: live again, and, when the text changed while held, read and
  answered, Core speaking only its answers to keys; otherwise answered
  empty without reading). A read is one snapshot, which the terminal
  answers from one locked state of its buffer, so no read is disturbed by
  the terminal writing to it. While reading is held with the text changed,
  a caret event is noted, not read, and its caret is read once the outpost
  has nothing else waiting; and while such a terminal is the console
  host's and no caret key's watch is open, the intake drops the window's
  console updates without waking the worker (`docs/design/wake-ups.md`).
- The review cursor (`terminal::review`, section 5.6). A review command's
  request in a focused terminal (a line read at a point after a movement,
  rows read ahead for say-all, a range for a copy) is answered from the
  memory, never from a live provider range: the worker first takes a read
  (`Worker::terminal_review`), so the memory's alignment has moved every
  place onto its text as it is now, and the request is then answered
  within that read. A line is a row, without its padding (section 5.12).
  Each row it answers with gets a text anchor, from the counter the anchor
  stores share, whose place the memory keeps
  (`TerminalMemory::add_place`): it moves with its row as output scrolls
  it, into the history included, is kept aside with the memory while the
  alternate screen is up, and is dropped when its row cannot be followed;
  the oldest places beyond 64 whose anchors Core does not hold are
  forgotten. A read lands the memory's review position where it lands.
  Rows above those the memory keeps are read from the provider in a
  snapshot of their own, from as many rows above where the next read
  starts, or the whole document for its start (`review::History`), split
  into rows as the memory splits a read (`verbatim_text::terminal_rows::rows_of`);
  a place there is checked against its line's hash, and found again by it
  within 64 rows when the history moved between the two reads. A request
  naming an anchor the terminal did not mint, such as a caret report's, is
  answered as any text's; a terminal's text is not selected, nor its caret
  moved, by Verbatim, and a place's screen position is not read
  (`Unsupported`).
- What the terminal says too large for one message is split as it is
  written (`terminal::split`), and what it says while an earlier message
  of the same terminal still waits follows it in one message.
- Each snapshot logs its cost at debug (`terminal snapshot timing`: the
  path, microseconds, calls, whether it read the whole document, the rows
  above its start, its text's length, and its flags), as does the
  worker's handling of each read (`terminal read timing`, with its wait in
  the queue and what the rules said).
- The rules' settings ("report new output", "review follows caret", and
  the flood policy's limits) come from Core,
  `SupervisorToOutpost::TerminalSettings`, sent when an outpost starts and
  whenever they change, and given to each terminal's rules at its next
  input.
- Every UIA notification from a terminal control or the console host's
  text area is ignored by the focus listener
  (`verbatim_uia::map::ignores_notifications`, from the class name and
  automation id of the sender's `ListenerEvent` read), whatever its
  activity id, as NVDA's terminal overlays ignore them, or output would
  be spoken twice.
- A UIA selection or notification goes, like a focus event, through its
  application's reading thread behind a marker
  (`DeliveredFact::UiaMarked` takes the marker's place in the intake):
  for a sender that is not a window, the thread walks up to its nearest
  window, one call, as NVDA does. A selection whose sender has none is
  dropped, and the window found is the one the outpost tests the
  selection's window and acceptance with; a notification whose sender
  has none is dropped, as NVDA's app module default drops it, but for the
  shell's window snap results.
- A UIA notification waiting in the listener or the intake is replaced
  by a newer one from the same element with the same kind, processing,
  and activity id, whatever its text, as NVDA's UIA limiter keys one.
- The unit tests: `terminal/reading.rs` holds every state and event of
  on-demand reading; `terminal/tests.rs` drives a `Terminal` over the
  simulated terminal (`verbatim-term-sim`) as the worker does: the rules'
  speech as Core takes it, a message too large split in order, Core's key
  as the rules' key, new output line by line, the review position moving
  with its row as output scrolls it into the history, the history read up
  to the document's start, a row ending at its text and a range joining
  lines, and the review reading at the caret. mockapp's `terminal` test
  reads its UIA terminal provider through the module (new output said line
  by line, one call remotely; typing echoed as the terminal shows it and
  Backspace saying what it deleted, through Core), and `terminal_reading`
  drives every transition through a real outpost's worker.
