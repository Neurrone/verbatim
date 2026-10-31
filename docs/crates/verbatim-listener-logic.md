# verbatim-listener-logic

The focus listener's pure logic (decision D13): what its reading threads,
its read scheduler and its foreground watch decide, with no Windows type,
no call and no clock, so every ordering of inputs is a replay test. It is
a logic crate: in the GPL tier, with no Windows dependency, so it builds
and is tested on any host. It depends on `verbatim-model`, on
[verbatim-reader-rules](verbatim-reader-rules.md) for the UIA focus wait,
and on `tracing`, which the scheduler logs through. The listener
([verbatim-listener](verbatim-listener.md)) runs it on its threads and
makes every call; that guide's "How a UIA focus is judged and read"
describes the scheduler with the threads, the watchdog and the reads
around it.

Public API:

- `focus` — the vocabulary a focus event is judged and scheduled in: its
  kind (`FocusEvent`), what the callback knew of it (`Meta`) and where it
  was queued (`Arrival`, `arrival_of`), and what became of it (`Judged`,
  with `into_fact`). Every reason an event is dropped is a constant here
  (`NOT_IN_TIME`, `MOVED_ON`, `WAITED_TOO_LONG`, `REPLACED`, `NO_WINDOW`,
  `NOT_FOCUSED`, `MSAA_CLASS`, `NEAREST_TIMED_OUT`, `NEAREST_FAILED_FAST`
  and the rest), and `gap_answer` is what each outcome says of its
  application's gap record, by the lists `VERDICTS` and `GAVE_UP`.
  `NEAREST_FAILED_FAST_BOUND` is the UIA focus wait
  (`verbatim_reader_rules::UIA_FOCUS_WAIT`), so a nearest window's lookup
  that took as long as the outpost's gap record waits counts as timed out
  (`nearest_timeout_reason`). `ABANDONED_LIMIT` is how many of an
  application's threads may be abandoned in their calls before it gets no
  new one.
- `schedule` — the read scheduler: `Schedule::step` takes one
  `ScheduleInput` and the time, in microseconds on the shell's monotonic
  clock, and returns the `ScheduleEffect`s in the order the shell applies
  them under its lock: which thread reads which application's event, when a
  read is abandoned or published at its bound, when an event waiting behind
  abandoned reads is dropped, which idle waiters may run, and whether a
  menu opening's thread may compare it with its own last read
  (`QueuePlace`). `GraceState::step` is the menu end's grace, which the
  listener's threadpool timer and its callers drive.
- `foreground` — what the listener learns of the window in the foreground
  for the keyboard hook: `ForegroundWatch` takes the window in front at
  the start (`start`), each foreground event (`foreground`) and each
  console event (`console_event`), and answers a `Step`: what to report,
  and whether to hook or unhook the console's events.

`cargo xtask ci` reads `schedule.rs` by the pure rule
(`xtask/src/ownership.rs`).

Tests: each module's unit tests, the scheduler's a replay of inputs with
exact effects, and `gap_answer`'s every drop reason.
