# verbatim-listener-logic

The focus listener's pure logic (decision D13; ADR 0003, "Readers tell,
Core asks"): what its check threads, its reading threads and its
foreground watch decide, with no Windows type, no call and no clock, so
every ordering of inputs is a replay test. It is a logic crate: in the
GPL tier, with no Windows dependency, so it builds and is tested on any
host. It depends on `verbatim-model`, on
[verbatim-reader-rules](verbatim-reader-rules.md) for the class verdict
and the limiter's count per thread, and on `tracing`, which the schedule
logs through. The listener ([verbatim-listener](verbatim-listener.md))
runs it on its threads and makes every call; that guide's "Tells, checks
and Core's requests" describes the threads around it.

Public API:

- `schedule` — the reading threads' schedule (design
  `docs/design/readers-tell-core-asks-build1.md`, Table 9), as methods on
  `Schedule<J, W>`, each returning the `Effects` the shell applies in
  order under its lock: threads to start, threads to wake, records let
  go, jobs ended unrun (`Ended`, with its `Ending`), requests abandoned,
  idle waiters to run, and the alarm. One record per application process
  generation (`AppKey`): `running`, one per process id, which a job looks
  in alone, and `exited`, by `AppKey`, for records with a thread still in
  charge or abandoned. A record's reader is one value: `Idle` with at
  least one abandoned thread out, `InCharge` with its `Charge` (its thread
  number, its abandoned count, the job running, when it was last idle and
  the jobs waiting), or `Starved` at `ABANDONED_LIMIT` (two) with the jobs
  waiting. A job (`JobKind`) is Core's request, a classic or UI Automation
  bar's change, or a release of kept elements; requests wait in a lane
  ahead of the rest. `push` queues a job, starting a thread when none is in
  charge and fewer than two are abandoned; `next` hands the thread its
  next job, or has it wait for the idle time the check threads use
  (`IDLE_US`, 30 seconds), then lets its record go, at once for an exited
  application; `returned` says whether a job's outcome is sent, a job
  abandoned while it ran discarded, and the thread number it goes on as,
  an abandoned thread's return at the limit reused for what waits;
  `cancel` ends a queued request cancelled for any reason, abandons a
  running one for the foreground with a fresh thread for what waits, and
  leaves it alone for any other reason; `tick` abandons a bar read past its
  deadline (`BAR_READ_DEADLINE`, 1 s), counted apart per process
  (`BAR_READS_ABANDONED_LIMIT`, 2, at which the application's bar changes
  are dropped), never towards `ABANDONED_LIMIT`; `application_exited`
  moves the record to `exited`, a starved record's jobs ended "not
  there"; `spawn_failed`, `when_idle` and `stop` end the rest. A bar's
  change replaces a waiting one of its object, a classic one is limited to
  the newest `EVENTS_PER_THREAD` of its raising thread and
  `BARS_PER_APPLICATION` (64) in all, the focused object's always kept, and
  a focus read drops its application's waiting classic changes. An idle
  waiter runs once every job queued before it is no longer waiting or
  running; abandoned threads do not count. The alarm is set only while a
  bar read runs.
- `focus` — the progress path's vocabulary (`classic_value_change`,
  `BarObject`, `BarVerdict`, `ClassicBarRead`, `BarRaiser`,
  `WindowAnswer`, `BAR_VERDICTS_KEPT`, the bar limits) and
  `ABANDONED_LIMIT`.
- `foreground` — what the listener learns of each window that came to
  the foreground, for Core: `ForegroundWatch` takes the window in front at
  the start (`start`), each foreground event with whether its window was
  in front as it was handled (`foreground`), any other event while a window
  not in front waits (`event_seen`, with `has_pending`), and each console
  event (`console_event`), and answers a `Step`: the window to report, with
  when it was observed, and whether to hook or unhook the console's
  events. A window not in front at its event is kept, replacing any kept
  before, until a later event finds it in front or a newer foreground
  event replaces it; none sent replaces another.

- `check` — the calls a UI Automation event's check makes at delivery on
  its application's check thread, NVDA's handler for its kind in its
  order, as one pure step function: `Check::next` names the next `Call`
  (the walk to the nearest window, the window's classes, the probe,
  each kept element's keyboard focus, the window's acceptance facts, the
  previous sibling, the parent's automation id, the element's keyboard
  focus) and the thread records each answer. `Handler::of` maps a
  kind to its handler: a focus or menu opening makes every call, a walk
  that finds no window ending it; an
  element selected, a window opened and a property change walk and probe,
  then read the acceptance facts, where NVDA's handler judges acceptance;
  a notification walks alone, a failed walk telling no window. The probe
  the tell carries, and the record keeps, is the raw probe's answer with
  NVDA's checks after it (`listener_provider`, judged with the classes
  `Check::classed` recorded): a list view's provider counts only from
  Windows Forms. A window on one of NVDA's lists is not probed, nor one whose probe in the thread's
  `ProbeRecord` began within `PROBE_RECORD_US` (half a second), the record
  keeping whether a provider answered: the record only spares a probe
  call, since Core recorded the same probe from the earlier tell. A window
  read through MSAA, by a list, the probe or the record, does not end the
  check: Core's backend cache is the one owner of a window's verdict and
  judges NVDA's native-element test, so the keyboard-focus reads are made
  whatever was found. A nameless
  `NetUITWMenuItem` menu item has its previous sibling read, and with
  none, or none read, its keyboard focus is not, as NVDA's placeholder
  allows its focus. A focus whose cache names a list item of
  `SWITCHER_ITEM_CLASS` (`ListViewItem`), the task switcher's item, has
  its raw-view parent's automation id read with one call
  (`Check::reading_parent`), before its keyboard focus. `Check::finish`
  answers what the walk found (`Walked`), the `Checked` the tell carries,
  and the parent's automation id, which the tell carries among its cached
  properties (`PropertyId::ParentAutomationId`).
- `check_threads` — the check threads' records (`CheckThreads`), one per
  application, keyed by its generation, under the listener's one lock:
  `push` queues an event, starting a thread when there is no record or
  waking an idle one; `take` hands the thread its next event, or has it
  wait, or exit by removing its record once idle for `IDLE_US`
  (30 seconds); `checked` says whether its tell is placed, only while the
  record is its own; `ended` drops an ended application's queue untold;
  `stop` drops every queue; `push_to_running` queues a release on a
  thread whose record lives, starting none, and hands it back otherwise;
  and `when_idle` holds the idle barrier's waiter until every event pushed
  before it is told or dropped.
- `kept` — one application's kept elements (`KeptElements`, Table 6 of
  the build design): told, with whether its event was a focus or a menu
  opening, in read for a request, a focus read or not, read, and
  released, each kept until Core's held view (`View`) leaves it out.
  `with_runtime_id` lists those a check reads the keyboard focus of, the
  ones that can be the readers' last focus: told by a focus or a menu
  opening, in a focus read, and read. `view` lets go of every told or
  read element a view rules out, one in read left for its read's end;
  `read_ended` keeps a focus read's element as read unless the last view
  rules it out; `read_abandoned` hands back the kept reference of the
  element a request cancelled for the foreground was reading; `ended`
  lets go of every kept reference, a read in flight releasing its own as
  it ends. `Reported` is the writer's numbering of the messages carrying
  tokens, in pipe order, and where each token's tell was written
  (`queued`, `written`, `withdrawn`, `forget`), the outposts' record of
  reported nodes in the same shape; `too_early` says a view cannot judge
  a token whose tell waits to be written or was written after the
  position it acknowledges. `ListenerKept` is the listener's whole
  judgement: every application's `KeptElements` and Core's last view,
  each call given the writer's `Reported`, whose reports of released
  tokens it forgets; a read whose answer was not sent is released at its
  end. `focus_kind` is the one rule of which kinds a check reads a told
  element for, a focus or a menu opening. Every element let go is handed
  back for release on a thread that may block.
- `tell` — what the listener decides of a `WinEvent` before it tells
  Core of it: `tell_window`, NVDA's mapping of a window of 0 to the
  desktop window, and of an invalid one to it for the end of a menu, a
  popup menu or the task switcher, dropping any other event of an invalid
  window, and a window's destruction told only for the window's own
  object; `dropped_by_class`, Windows Live Messenger's hidden window and a
  show of neither a client nor a custom object, and a name change of the
  mouse pointer (`OBJID_CURSOR`), whose name is its shape; `hang_tested`,
  whether a hung or ghosted window drops an event of a kind, true only
  for the kinds NVDA's `winEventToNVDAEvent` tests, so a window's
  destruction, the end of a menu, a popup menu or the task switcher, the
  system caret and the desktop switch are told from a hung window and
  Core's shell refuses its request there; and
  `ForegroundHold`, which keeps every MSAA item placed behind a foreground
  event whose window `GetForegroundWindow` does not name until it does or
  `FOREGROUND_HOLD_US` (20 ms) pass, a newer foreground awaiting its own
  window and counting its time again, a dropped foreground's window still
  awaited, and answers what to send, in order, and how its one timer
  changes; a UI Automation tell passes it (`ForegroundHold::passing`),
  sent at once, never held.

`cargo xtask ci` reads `schedule.rs` by the pure rule
(`xtask/src/ownership.rs`).

Tests: each module's unit tests; the schedule's, one per rule and per
cell of Table 9, a replay of calls with exact effects
(`crates/verbatim-listener-logic/src/schedule/tests.rs`). The foreground
watch is tested by every sequence of up to six steps against its rules
(`crates/verbatim-listener-logic/src/foreground.rs`): nothing before the
start, an event in front reported at once, one not in front reported
only as the newest, once, with the observation of the event that found
it, and a console's thread named only while the consoles' events are
hooked. Its exact cases are hand-written examples.
