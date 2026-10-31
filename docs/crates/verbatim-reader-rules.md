# verbatim-reader-rules

What both of Verbatim's readers, an outpost and the focus listener, apply
before reading a window: NVDA's class verdict and the class name
normalization it is judged on, NVDA's acceptance test, and the waits both
readers judge by. A rule both apply is defined here once, because the two
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
- `acceptance` — NVDA's test for accepting an event before anything is
  read of it (`shouldAcceptEvent`; `docs/parity/focus-events.md`, "Event
  acceptance"). `accepts_event(event, window, &windows, &requested)` is a
  function of the event's name (`EventName`), its window, the window
  queries (`WindowQueries`, which `verbatim_win32::window::SystemWindows`
  answers with local calls and the tests answer from a made-up desk) and
  the events application support asked for (`RequestedEvents`).
  `foreground_relation(window, class, &windows)` is how the window stands
  to the foreground window, the rules the test ends with, which an outpost
  also reports in an event's window facts.
- `CONSOLE_WINDOW_CLASS` — the console host's window class.
- `MOVED_ON_GRACE` — how long a slow call into an application is waited
  for before it is abandoned because the user moved on to a window of the
  same application on another UI thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`,
  500 ms. An outpost's reducer judges its programs by it, and the
  listener's reading threads their reads.
- `UIA_FOCUS_WAIT` — how long an outpost waits for a UIA focus after a
  focus `WinEvent`, a second: the bound of the outpost reducer's gap
  record (`gap_record::BOUND_US`), and, by definition, the bound under
  which the listener counts a nearest window's lookup that timed out as
  failing fast (`focus_read::NEAREST_FAILED_FAST_BOUND`).

Tests: the class normalization, the class lists pinned to their NVDA
sources so an NVDA sync is a diff, and the acceptance test rule by rule
against a made-up desk.
