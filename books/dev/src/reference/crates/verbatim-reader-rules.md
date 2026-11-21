# verbatim-reader-rules

What both of Verbatim's readers, an outpost and the focus listener, apply
before reading a window: NVDA's class verdict,
NVDA's limit on a thread's events, and a window's handle that is never 0.
NVDA's acceptance test is Core's (`crates/verbatim-core/src/focus/acceptance.rs`),
since Core judges every tell. A rule both readers apply is defined here once, because the two
readers' crates may not depend on each other and no test could tie two
copies (decision D14, "Where the rules run"). It is a logic crate: pure,
in the GPL tier, with no Windows dependency, so it builds and is tested on
Linux. It depends on `verbatim-model` alone, whose `class` module holds
NVDA's class lists and the class name normalization the verdict is
judged on.

Public API:

- `class` — window classes as NVDA judges them.
  `WindowClasses::new(raw, root)` holds a
  window's raw class, the one `verbatim_model::class::normalize` gives
  and its root ancestor's; the
  caller reads the names (`verbatim_win32::window::window_classes`).
  `class_verdict(&classes)` is the verdict NVDA's class lists give
  (`ClassVerdict::Uia`, `NonUia`, or `Unknown` when only the probe can
  decide): the good list and the Windows 11 shell rule first, then the
  bad list. The focus listener skips a read by it; every outpost's
  arbitrator gives the same verdict through `classify_with`, which takes
  the arbitrator's own lists, built from the model's `GOOD` and `BAD`
  with its configured additions. `post_probe_check`, `PostProbeCheck`,
  `provider_usable` and `has_windows_forms_class` are NVDA's checks of a
  window with a provider before it is used, the outposts' arbitrator's
  whole test. `listener_provider(classes, raw_provider)` is the focus
  listener's answer from them, the one its check threads' probes and its
  value changes are judged by: a list view's provider counts only from
  Windows Forms, and a console's with no check, a limit its guide states.
- `Window` — a window's handle that is never 0. The protocol and the
  system's calls carry a handle as an `isize`, 0 for none; the logic
  crates parse one where it enters them, `Window::new(handle)` giving
  `None` for 0, and give it back for a call with `get`, so no rule judges
  a 0 as none.
- `CONSOLE_WINDOW_CLASS` — the console host's window class.

Tests: the class normalization, the class lists pinned to their NVDA
sources so an NVDA sync is a diff, the class verdict `classify_with` gives
with NVDA's lists (the good, bad and shell rules), the limit per thread,
and a window's handle parsed.
