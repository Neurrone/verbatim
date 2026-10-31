# verbatim-listener

The focus listener process, `verbatim-listener.exe` (decision D13): one
permanent process holding the desktop-global UIA focus registration and
the global MSAA hooks, judging and reading each UIA focus from the event's
own element on a thread per application, and forwarding each focus fact to
Core, which routes it to the application's own outpost
([verbatim-outpost](verbatim-outpost.md)). Core's supervisor
([verbatim-supervisor](verbatim-supervisor.md)) starts it with
`--listener` and its arguments (`verbatim_process::arguments::ListenerArguments`),
and replaces it when it ends. What it decides without a call, the read
scheduler, the menu end's grace, the foreground watch and a focus event's
vocabulary, is [verbatim-listener-logic](verbatim-listener-logic.md); the
class verdict and acceptance test it applies before a read are
[verbatim-reader-rules](verbatim-reader-rules.md)'s. The reasons for this
shape are in `docs/architecture.md`, section 1 ("The focus listener
(D13)").

## How a UIA focus is judged and read

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
  every outpost's arbitrator gives (`verbatim_reader_rules::class::class_verdict`), unless a
  backend override forcing UIA holds for its application
  (`SetBackendOverride` with its `pid`, `FocusReader::forces_uia`); the
  listener makes no other backend verdict, no probe and no console check,
  which are its outpost's. A shell window that only stages a switch is read
  and Core drops it. The listener makes no duplicate test and keeps no
  judgement of Core's (`docs/design/ownership.md`): a focus that passes the other checks is read in full, and
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
  against the foreground window (`verbatim_reader_rules::acceptance`,
  shared with the outposts) on the element's own or nearest window
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
  (`verbatim_win32::window::front_is_another_thread_of_its_application`,
  by provider process, `verbatim_win32::provider::provider_process`), as NVDA's watchdog
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
  The watchdog sleeps on an alarm (`verbatim_process::alarm::Alarm`, as an
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
(`verbatim_win32::window::top_level_of`), stamp, and the bound's due time, on one clock.

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

## Public API

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
  window, integrity level by `verbatim_win32::integrity::process_integrity`;
  `verbatim_win32::integrity::own_integrity` is verbatim.exe's), only the newest waiting
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
  (`verbatim_process::arguments::parse_hooks_in`) and does without each thread's last focus,
  at the cost of one fuller ancestor read per application. It makes no
  remote operation, so takes no `--classic-uia`.
- `listener::capture` is what the notification callback captures, and
  `listener::capture_selection` what the element-selected callback
  captures, public so mockapp makes a selection's fact as the listener does
  (`crates/mockapp/tests/combo_box.rs`).
