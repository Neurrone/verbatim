# verbatim-listener-logic

The focus listener's pure logic (decision D13): what its reading threads,
its read scheduler and its foreground watch decide, with no Windows type,
no call and no clock, so every ordering of inputs is a replay test. It is
a logic crate: in the GPL tier, with no Windows dependency, so it builds
and is tested on any host. It depends on `verbatim-model`, on
[verbatim-reader-rules](verbatim-reader-rules.md) for the UIA focus wait
and the work in flight,
and on `tracing`, which the scheduler logs through. The listener
([verbatim-listener](verbatim-listener.md)) runs it on its threads and
makes every call; that guide's "How a UIA focus is judged and read"
describes the scheduler with the threads, the watchdog and the reads
around it.

Public API:

- `checks` — what a UIA focus event goes through before its read, NVDA's
  focus handler's checks, or for a menu opening its automation event
  handler's, in their order, as one pure step function: `checks::next`
  takes a `Gathered`, what the callback knew of the event (its kind, its
  sender's own window, the window of the current focus a menu opening is
  compared with, and whether a test forces UIA) and each call's answer
  once made (the comparison, the nearest window's lookup as a `Nearest`,
  the window's classes, the acceptance test against the foreground, the
  sender's own window's class, the hidden frame, and a redirected menu
  opening's keyboard focus), and answers a `Step`: `Call(Check)`, the one
  call to make next, `Drop(reason)`, or `Read { window, require_focus }`.
  The listener's reading thread loops, making the call named and recording
  its answer. A failed comparison drops a menu opening, as NVDA catches no
  error from it; a nearest window's lookup that timed out or found none
  drops the event; a window the class verdict calls MSAA is skipped; and a
  redirected menu opening's keyboard focus read that fails lets it
  through, as NVDA's COM error rule does. The hung-window test is the
  callback's, made as the event arrives, and not among these. A
  `Gathered` lives for one read; the tests are a table of exact cases, one
  per check and drop reason.
- `focus` — the vocabulary a focus event is judged and scheduled in: its
  kind (`FocusEvent`, among them `Release`, a job releasing kept elements,
  which like a bar's change starts and holds no outpost), what the
  callback knew of it (`Meta`) and where it
  was queued (`Arrival`, `arrival_of`), and what became of it (`Judged`,
  with `into_fact`). `classic_value_change`, `BarObject`, `BarVerdict` and
  `ClassicBarRead` are the listener's hook's table for a classic value
  change, `BAR_VERDICTS_KEPT` its memo's cap (the memo is the listener's
  event thread's), and `WindowAnswer` what a job found a window to be. A
  classic value change comes as `ScheduleInput::PushBar`, with its raising
  thread and whether it is the focused object (`BarRaiser`): at arrival
  its application's waiting value changes keep the newest
  `EVENTS_PER_THREAD` of each thread and at most `BARS_PER_APPLICATION`
  (64) in all, the focused object's always kept, and a focus push drops
  them. Its read has its own deadline (`BAR_READ_DEADLINE`, 1 s) and an
  abandoned one does not count towards `ABANDONED_LIMIT`: the schedule
  keeps a separate count per process (`BAR_READS_ABANDONED_LIMIT`, 2), at
  which the application's value changes are dropped with no thread
  started, clearing as each abandoned call returns. A bar's record is let
  go when idle. Every reason an event is dropped is a constant here
  (`NOT_IN_TIME`, `MOVED_ON`, `WAITED_TOO_LONG`, `REPLACED`, `NO_WINDOW`,
  `NOT_FOCUSED`, `MSAA_CLASS`, `NEAREST_TIMED_OUT` and the rest). A
  nearest window's lookup that fails with UIA's timeout
  is dropped for `NEAREST_TIMED_OUT`, however long it took.
  `ABANDONED_LIMIT` is how many of an
  application's threads may be abandoned in their calls before it gets no
  new one.
- `schedule` — the read scheduler: `Schedule::step` takes one
  `ScheduleInput` and the time, in microseconds on the shell's monotonic
  clock, and returns the `ScheduleEffect`s in the order the shell applies
  them under its lock: which thread reads which application's event, when a
  read is abandoned, when an event waiting behind
  abandoned reads is dropped, which idle waiters may run, and whether a
  menu opening's thread may compare it with its own last read
  (`QueuePlace`). Each application's read under way is its work in
  flight (`InFlight`, re-exported `Request`): the schedule starts it at a
  thread's `Next`, steps it with what the read learns (`Learned`), the
  watchdog's `Tick` and the answer to its ask (`Judged`), and ends it at
  the thread's `Returned`. Its ask is an effect (`Judge`), at most one per
  application in a step, which the shell answers once the step's other
  effects are applied. `Judged` carries the judgement's verdict,
  `Reason::MovedOn` when the user has left the read's window, or none; a
  read abandoned so is dropped for `MOVED_ON`, and one past its deadline
  for `NOT_IN_TIME`. A read is never abandoned for its window's hang: it
  waits for the application. Records are
  keyed by application generation (`AppKey`, a process id and which run of
  it): `Schedule` holds `running`, one record per process id, and `exited`,
  by `AppKey`. An event (`Push`) looks in `running` alone, so it cannot
  reach an exited record, and one no record names starts a new generation
  with no abandoned readers. Each record's reader is one value: `Reader`
  (`Idle` with at least one reader out, `InCharge` with its `Charge`, or
  `Starved` at `ABANDONED_LIMIT` with its queue), inside `Life` (`Live`, or
  `Ending` once `OutpostEnded` asked that it be let go, which any event
  turns back to `Live`); an exited record holds `Draining` (`Idle` or
  `InCharge`), since nothing can wait for it. `ApplicationExited` moves the
  record from `running` to `exited`, its queue dropped at once when no
  thread is in charge; a read returning for an exited generation is
  dropped for `PROCESS_EXITED`, as a new process may have its id. The generation in charge is the `Charge`'s work in
  flight alone; every start of a thread, new or reused, takes a new
  generation (`InFlight::succeed`), and `App::counters` carries the numbers
  from one `Charge` to the next. An abandonment has the shell stop watching
  the read (`Unwatch`). A read's top-level window comes by `Placed`, once
  per read, into its record; a successful return keeps it as the record's
  last. A record with no thread in charge, no reader out and nothing queued
  is dropped at the step that reaches that: `Forget(key)`, after every
  other effect naming it, so the shell drops its record
  (`Schedule::let_go`, the one place a record is removed; `Schedule::held`
  lists those kept). A state the rules exclude is `LogicError`, for the
  shell to log. The generated test compares every step with a reference
  model of processes and threads. The schedule owns the watchdog's alarm: after every
  step it computes the soonest of each read's next wake and each
  application's end of its wait at the abandoned limit, and emits
  `Alarm(Set(due))` only when that changed, or after a `Tick`, which
  spends the one-shot timer; `Alarm(Now)` is the
  shutdown's. A foreground event only moves applications away for menu
  currentness. `GraceState::step` is the listener's one timer, which the
  listener's threadpool timer and its callers drive: a menu end's grace,
  the desktop's return 200 ms after its switch and the desktop name's due
  time 250 ms after its event, each sent once its time is over, soonest
  first, the timer set for the soonest still waiting, and the idle
  barrier's answers held while anything waits. `FocusEvent::DesktopName`
  is the kind of the desktop name's read, which, like a live re-read, has
  no marker, moves away from no application and is no pending focus.
- `foreground` — what the listener learns of each window that came to
  the foreground, for Core: `ForegroundWatch` takes the window in front at
  the start (`start`), each foreground event with whether its window was
  in front as it was handled (`foreground`), a focus event while a window
  not in front waits (`event_seen`, with `has_pending`), and each console
  event (`console_event`), and answers a `Step`: the window to report, with
  when it was observed, and whether to hook or unhook the console's
  events. A window not in front at its event is kept, replacing any kept
  before, until a later event finds it in front or a newer foreground
  event replaces it; none sent replaces another.

- `check` — the calls a UI Automation event's check makes at delivery on
  its application's check thread, NVDA's handler for its kind in its
  order, as one pure step function: `Check::next` names the next `Call`
  (the walk to the nearest window, the window's class verdict, the probe,
  each kept element's keyboard focus, the previous sibling, the element's
  keyboard focus) and the thread records each answer. `Handler::of` maps a
  kind to its handler: a focus or menu opening makes every call, a walk
  that finds no window ending it; an element selected, a window opened
  and a property change walk and probe; a notification walks alone, a
  failed walk telling no window. A window on one of NVDA's lists is not
  probed, nor one whose probe in the thread's `ProbeRecord` began within
  `PROBE_RECORD_US` (half a second). A nameless `NetUITWMenuItem` menu
  item has its previous sibling read, and with none, or none read, its
  keyboard focus is not, as NVDA's placeholder allows its focus.
  `Check::finish` answers what the walk found (`Walked`) and the
  `Checked` the tell carries. Every read but the walk is made whatever the
  probe answered, since Core holds the verdict.
- `check_threads` — the check threads' records (`CheckThreads`), one per
  application, keyed by its generation, under the listener's one lock:
  `push` queues an event, starting a thread when there is no record or
  waking an idle one; `take` hands the thread its next event, or has it
  wait, or exit by removing its record once idle for `IDLE_US`
  (30 seconds); `checked` says whether its tell is placed, only while the
  record is its own; `ended` drops an ended application's queue untold;
  `stop` drops every queue; and `when_idle` holds the idle barrier's
  waiter until every event pushed before it is told or dropped.
- `kept` — one application's kept elements (`KeptElements`, Table 6 of
  the build design): told and not yet asked about, in read, read and not
  yet named, and named as the readers' last focus by a focus request
  (`focus_request`); `with_runtime_id` lists those the check thread reads
  the keyboard focus of; `sweep` lets go of told elements past
  `KEPT_FOR_US` (60 seconds) or beyond the newest `KEPT_PER_APPLICATION`
  (64); `ended` lets go of every one but a read in flight. Every element
  let go is handed back for release on its application's reading thread.
- `tell` — what the listener decides of a `WinEvent` before it tells
  Core of it: `tell_window`, NVDA's mapping of a window of 0 to the
  desktop window, and of an invalid one to it for the end of a menu, a
  popup menu or the task switcher, dropping any other event of an invalid
  window, and a window's destruction told only for the window's own
  object; `dropped_by_class`, Windows Live Messenger's hidden window and a
  show of neither a client nor a custom object; `WindowPart`, acceptance's
  window facts over the `WindowReads` trait, the window's own part read
  as the event arrives and the foreground's part
  (`WindowPart::with_foreground`) as the tell is placed, a console
  window's process the console host the event's thread names; and
  `ForegroundHold`, which keeps every item placed behind a foreground
  event whose window `GetForegroundWindow` does not name until it does or
  `FOREGROUND_HOLD_US` (20 ms) pass, a newer foreground awaiting its own
  window and counting its time again, a dropped foreground's window still
  awaited, and answers what to send, in order, and how its one timer
  changes.

`cargo xtask ci` reads `schedule.rs` by the pure rule
(`xtask/src/ownership.rs`).

Tests: each module's unit tests, and the scheduler's a replay of inputs
with exact effects. The
scheduler has a generated test against a reference model
(`crates/verbatim-listener-logic/src/schedule/generated.rs`): lists of
actions weighted toward the lifecycle, run through proptest's
`TestRunner::run` under a fixed seed and 2,000 cases (about 2.4 s in a
debug build), each made valid when applied by resolving its target
against the model (an application's thread waiting or reading, its
oldest abandoned thread, its read under way or the request before it).
The actions are events of every kind from three applications, keyed or
not, with a window known, unknown or never judged; the threads' asks and
returns; abandoned threads' returns; windows learned, stale among them;
time; foreground events with each read's top-level window; MSAA focuses;
outposts' ends; idle waiters; and the stop. Each carries the answers to
what it may cause: whether the user moved on, and whether a new thread
fails to start. They are
given as the listener's shell gives them: effects in order, then each
failed start, then each ask's answer, each a step of its own. The model
is lists and sets: each application's waiting markers, read under way,
thread and generation, whether a thread is in charge, abandoned
threads, whether its outpost ended and requests given, and
globally each marker's place and outcomes, the idle waiters, stopping
and the alarm as last set; it predicts no place, moving away, wait's
bound or alarm time, and takes the work in flight's decisions as given.
After every step it
checks each effect (every marker's one outcome, by the reason its input
allows; jobs in push order to the thread in charge; each spawn's generation above
every earlier one in the application's life, with nothing waiting
unthreaded below the abandoned limit; no new thread while two are
abandoned; at most one ask per application, each of its read under way;
a deadline abandonment at or past its deadline, and a moved-on one after
that verdict; each read ended exactly once, its outcome sent once, by its
return or its abandonment, so every marker has exactly one answer, read
or dropped; one forget-last per abandoned thread's return; an
alarm set only when it changes; and after the stop no spawn, job, ask or
alarm and every push dropped), and after every action compares the
schedule's open markers, queues, reads under way, applications held,
each application's thread in charge, abandoned threads and ending, and
stopping with the model's, checks that no abandoned thread is in
charge, that each idle waiter ran in the step its last event got its
outcome, that no read survives a tick past its deadline, and that the
alarm is never later than a read's next wake and is later than each
tick. Which place and moved-away place a job carries, the wait at the
abandoned limit and `focus_pending` are the unit tests' examples. Each case ends by
draining, then every marker has had one outcome and every waiter run
once, or, with the listener stopped, at most once; it runs twice with the
same effects, and the gate fails if any listed state is never reached.
The cases the generated test found are kept as fixed cases, pasted as
the failure prints them (`books/dev/src/reference/testing.md`, "Exploration tests"), and a
failing sequence of either enumeration below is kept in its own found
list. The menu
end's grace and the foreground watch are each tested by every sequence of
up to six steps against their rules (in the generated test's file, and in
`crates/verbatim-listener-logic/src/foreground.rs`): for the grace,
against a reference model that keeps each thing waiting on the timer, the
end, the desktop's return and the name's due time, at its own time, each
sent once, only once its time is over, the newest end alone, those one
firing sends in the order of their times and none due left unsent, the
timer set for the soonest waiting, no round answered while anything waits,
twice, or held with nothing waiting, and once drained every round answered
and the newest end sent unless closed; for the foreground
watch, nothing before the start, an event in front reported at once, one
not in front reported only as the newest, once, with the observation of
the event that found it, and a console's thread named only while the
consoles' events are hooked. Their exact cases are hand-written
examples.
