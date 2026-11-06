# verbatim-reader-rules

What both of Verbatim's readers, an outpost and the focus listener, apply
before reading a window: NVDA's class verdict and the class name
normalization it is judged on, NVDA's acceptance test, the waits both
readers judge by, and the work in flight both keep. A rule both apply is defined here once, because the two
readers' crates may not depend on each other and no test could tie two
copies (decision D14, "Where the rules run"). It is a logic crate: pure,
in the GPL tier, with no Windows dependency, so it builds and is tested on
Linux. It depends on `verbatim-core`, for `ForegroundRelation`.

Public API:

- `class` — window classes as NVDA judges them.
  `normalize_class_name(raw)` is a class name as NVDA normalizes it
  before matching any class-based rule: a Windows Forms name cut down to
  the control class it wraps, an `ATL:` prefix dropped, and the result
  looked up in NVDA's class map. `WindowClasses::new(raw, root)` holds a
  window's raw class, the normalized one and its root ancestor's; the
  caller reads the names (`verbatim_win32::window::window_classes`).
  `class_verdict(&classes)` is the verdict NVDA's class lists give
  (`ClassVerdict::Uia`, `NonUia`, or `Unknown` when only the probe can
  decide): the good list and the Windows 11 shell rule first, then the
  bad list. The focus listener skips a read by it; every outpost's
  arbitrator gives the same verdict through `classify_with`, which takes
  the arbitrator's own lists, built from `GOOD_UIA_CLASSES` and
  `BAD_UIA_CLASSES` with its configured additions. `SHELL_ROOT_CLASSES`
  are the shell's top-level windows.
- `Window` — a window's handle that is never 0. The protocol and the
  system's calls carry a handle as an `isize`, 0 for none; the logic
  crates parse one where it enters them, `Window::new(handle)` giving
  `None` for 0, and give it back for a call with `get`, so no rule judges
  a 0 as none.
- `acceptance` — NVDA's test for accepting an event before anything is
  read of it (`shouldAcceptEvent`; `docs/parity/focus-events.md`, "Event
  acceptance"). `accepts_event(event, window, &windows, &requested)` is a
  function of the event's name (`EventName`), its window, `None` for an
  event with no window, which is accepted, the window queries
  (`WindowQueries`, which `verbatim_win32::window::SystemWindows` answers
  with local calls, parsing each handle the system gives into an
  `Option<Window>`, and the tests answer from a made-up desk) and
  the events application support asked for (`RequestedEvents`).
  `foreground_relation(window, class, &windows)` is how the window stands
  to the foreground window, the rules the test ends with, which an outpost
  also reports in an event's window facts.
- `in_flight` — the work in flight: a reader's record of the one slow-call
  item a worker is running. `InFlight<T>` holds the worker
  generation in charge and the last request number given, and the item
  running: its `Request` (generation and number), its start and deadline,
  its `Walk` (`Unarmed`, `Armed(bound)` or `Published`, so a walk armed
  after its publication cannot be represented), its `Moved` rule
  (`Never`, `Unknown` until the reader learns the window,
  `Known { window, next }`, or `Named { next }` for a window the reader's
  worker names, which its shell reads at each judgement) and the reader's
  own item `T`. The outpost's
  reducer embeds one, and the listener's schedule one per application.
  Times are microseconds on the reader's monotonic clock, passed in.
  - Transitions: `start(item, deadline, window, now)`, the window a
    `RuleWindow`: `Never` for an item never judged, `Unknown` for one still
    to learn its window, `Named` for one whose worker names it, judged on
    the grace's schedule from its start, or `Known(window)`, which
    `RuleWindow::from_cached` parses from a cache's handle; `hand_on()`, a new request for the running item
    keeping its times, walk and rule (the outpost's finish of a cut caret
    read); `learned(request, window, now)`; `armed(request, bound)`;
    `walk_returned(request, past)`; `tick(now)`; the answers
    `judged(request, verdict)`, the answer to `Ask::Judge` (`None` when
    the item stays, else `Reason::Hung` or `Reason::MovedOn`, its window
    found hung, tested first, or the user moved on; an armed walk is
    published first) and `published(request, got, then_abandon,
    now)`, with `published_judging_nothing` for a reader that has stopped,
    which makes only the abandonment the ask carried; `end(request)`, which
    gives the item once with whether it was published; `succeed()`, a new
    generation without an abandonment, for the listener's start of a
    thread; and `next_request()`, a request for work the record does not
    keep, the outpost's last drop as it stops.
  - Each transition returns a `Decision`: `Nothing`, one `Ask`
    (`MovedOn { request, window }` or `Publish { request, then_abandon }`)
    for the reader's shell to answer by the matching transition, or
    `Abandon(Abandoned { dead, replacement, reason, item, published })`,
    `reason` being `Reason::Deadline` or `Reason::MovedOn`, which each
    reader puts in its own words.
  - A tick, and the judgment a publication's answer makes again, apply
    the first of these that is due: the deadline (publishing an armed walk
    first, by a `Publish` carrying the abandonment, or abandoning at once);
    the walk's bound (`Publish`, the walk unarmed until the answer); and
    the moved-on rule, judged at `MOVED_ON_GRACE` after the start and
    every `MOVED_ON_GRACE` after that, published or not, as NVDA's
    watchdog judges. A window learned past the grace is judged at once.
    A true moved-on answer publishes an armed walk first and abandons any
    other at once. An answer whose request is no longer running is
    dropped, with the abandonment it carried.
  - Queries: `accepts(request)` (running and not published),
    `generation()`, `running()`, `deadline()`, `item_mut()` and
    `next_wake()`, the soonest of the deadline, the bound while armed and
    the next judgment while the window is known.
- `CONSOLE_WINDOW_CLASS` — the console host's window class.
- `MOVED_ON_GRACE` — how long a slow call into an application is waited
  for before it is abandoned because the user moved on to a window of the
  same application on another UI thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`,
  500 ms. The work in flight judges by it, at the grace's end and every
  grace after.

Tests: the class normalization, the class lists pinned to their NVDA
sources so an NVDA sync is a diff, the acceptance test rule by rule
against a made-up desk, and the work in flight: a replay test per rule,
and a generated test under a fixed seed and case count that drives lists of
actions weighted toward the lifecycle: starts, learned windows, armed walks
(after a publication too), ticks, returned walks, hand-ons, ends,
successions, requests the record does not keep, answers to a dead request
of every kind, and a reader that stops, after which it ticks no more and a
publication judges nothing. An action is made valid when applied: a start
or a succession ends the item running, and an answer to a dead request
names the newest request no longer running. Each ask is answered at once
with a value drawn by the time it is asked at. It compares each item's fate
(running, ended, or abandoned with its reason, each with whether it was
published) with the one a scan of its timeline names, and the running
item, its deadline and `item_mut` with the model's, and checks its
invariants after every step: each item leaves at most once, the generation
rises by one per abandonment or succession and an abandonment names the
item's generation as dead, a published walk followed an armed one, a wake
that is due is acted on, every request is of the generation in charge and
no request number recurs, and an answer to a dead request decides nothing
and changes nothing. Each case ends by draining, the item ticked past its
deadline or, once stopped, ended, and then every item has left exactly
once. Each case runs twice and the two runs' decisions are compared, and
the test fails if a listed lifecycle state is never reached. A failure
shrinks to its smallest case through proptest's `TestRunner::run`. Beside
it, `explore_in_flight` runs the same cases under the seed and case count
in `VERBATIM_EXPLORE_SEED` and `VERBATIM_EXPLORE_CASES`; it is ignored, and
only the nightly workflow (`.github/workflows/explore.yml`) runs it. The
crate's one development dependency is `proptest`.
