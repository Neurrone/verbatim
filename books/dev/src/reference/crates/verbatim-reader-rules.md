# verbatim-reader-rules

What both of Verbatim's readers, an outpost and the focus listener, apply
before reading a window: NVDA's class verdict and the class name
normalization it is judged on, the kept verdicts on a window's provider,
NVDA's limit on a thread's events, and a window's handle that is never 0.
NVDA's acceptance test is Core's (`crates/verbatim-core/src/focus/acceptance.rs`),
since Core judges every tell. A rule both readers apply is defined here once, because the two
readers' crates may not depend on each other and no test could tie two
copies (decision D14, "Where the rules run"). It is a logic crate: pure,
in the GPL tier, with no Windows dependency, so it builds and is tested on
Linux. It depends on nothing of Verbatim's.

Public API:

- `class` — window classes as NVDA judges them.
  `normalize_class_name(raw)` is a class name as NVDA normalizes it
  before matching any class-based rule: a Windows Forms name cut down to
  the control class it wraps, an `ATL:` prefix dropped, and the result
  looked up in NVDA's class map. `is_progress_bar_class(normalized)` is
  whether a normalised name is one NVDA recognises as a progress bar.
  `WindowClasses::new(raw, root)` holds a
  window's raw class, the normalized one and its root ancestor's; the
  caller reads the names (`verbatim_win32::window::window_classes`).
  `class_verdict(&classes)` is the verdict NVDA's class lists give
  (`ClassVerdict::Uia`, `NonUia`, or `Unknown` when only the probe can
  decide): the good list and the Windows 11 shell rule first, then the
  bad list. The focus listener skips a read by it; every outpost's
  arbitrator gives the same verdict through `classify_with`, which takes
  the arbitrator's own lists, built from `GOOD_UIA_CLASSES` and
  `BAD_UIA_CLASSES` with its configured additions. `SHELL_ROOT_CLASSES`
  are the shell's top-level windows. `post_probe_check`, `PostProbeCheck`,
  `provider_usable` and `has_windows_forms_class` are NVDA's checks of a
  window with a provider before it is used, the outposts' arbitrator's
  whole test. `listener_provider(classes, raw_provider)` is the focus
  listener's answer from them, the one its check threads' probes and its
  value changes are judged by: a list view's provider counts only from
  Windows Forms, and a console's with no check, a limit its guide states.
- `verdicts` — the kept verdicts on a window's provider: `Verdicts`,
  `Probe`, `Probed`, `VerdictChange`. An outpost's reducer holds them as an
  immutable snapshot, and the listener's event thread keeps a record per
  application of them. `kept_at` answers for an event's observation, the
  verdict before the newest for one observed before the newest's probe
  began, `newest` the latest whether or not its keep has ended, and
  `len` and `evict_oldest` keep a record to a cap.
- `NewestPerThread`, `EVENTS_PER_THREAD` — NVDA's limit of 10 events per
  raising thread, walked from the newest: both the outposts' batch and the
  listener's waiting classic value changes use it.
- `Window` — a window's handle that is never 0. The protocol and the
  system's calls carry a handle as an `isize`, 0 for none; the logic
  crates parse one where it enters them, `Window::new(handle)` giving
  `None` for 0, and give it back for a call with `get`, so no rule judges
  a 0 as none.
- `CONSOLE_WINDOW_CLASS` — the console host's window class.

Tests: the class normalization, the class lists pinned to their NVDA
sources so an NVDA sync is a diff, the limit per thread, and a window's
handle parsed. The verdicts' keeping is tested through the outposts'
arbitrator and the listener.
