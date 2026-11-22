# verbatim-reader-rules

What both of Verbatim's readers, an outpost and the focus listener, apply
before reading a window: NVDA's class verdict,
NVDA's limit on a thread's events, and a window's handle that is never 0.
NVDA's acceptance test is Core's (`crates/verbatim-core/src/focus/acceptance.rs`),
since Core judges every tell. A rule both readers apply is defined here once, because the two
readers' crates may not depend on each other and no test could tie two
copies (decision D14, "Where the rules run"). It is a logic crate: pure,
in the GPL tier, with no Windows dependency, so it builds and is tested on
Linux. It depends on `verbatim-model`, whose `class` module holds
NVDA's class lists and the class name normalization the verdict is
judged on, on `verbatim-text` for the text algorithms' character
boundaries, and on `tracing`.

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
- `dialog` — a dialog's own text, gathered as NVDA gathers it
  (`docs/nvda/object-model.md`, "A dialog's own text"): `dialog_text(&dialog)`
  over a `DialogParent` whose children are `DialogObject`s, which each
  backend implements beside its reads (`verbatim_ia2::dialog::DialogObject`
  for MSAA, `verbatim_uia_rops::dialog::UiaDialog` and `UiaObject` for UI
  Automation, each child read as a `verbatim_uia::DialogText` with its
  description whatever the theme,
  `books/dev/src/explanation/design/uia-shapes.md`, decision 5).
  `is_dialog(role)` says which roles have such text. The walk counts every
  child it fetches against `MAX_OBJECTS` (65,536) and stops at an object
  it visited before (`Visited`, by runtime id for UIA and by window,
  object id and child id for an MSAA window's client area, the one object
  of the walk with an address of its own; Claude, 2026-11-21), giving the
  text read up to either. The traits' methods may call into the
  application; the rule itself makes no call.
- `numbered` — a reader's numbering of the messages it writes to Core
  that carry what it keeps for Core: `Numbered<K>` (`queued`, `written`,
  `withdrawn`, `report`, `forget`, `position`) gives each key's `Report`,
  the position of the last message carrying it and how many still wait,
  and `too_early` says a held view cannot judge a key no message carried
  yet, one a message waits to carry, or one carried after the position
  it acknowledges. The listener numbers its tokens with it and an outpost
  its nodes, anchors and caret tells' elements.
- `Window` — a window's handle that is never 0. The protocol and the
  system's calls carry a handle as an `isize`, 0 for none; the logic
  crates parse one where it enters them, `Window::new(handle)` giving
  `None` for 0, and give it back for a call with `get`, so no rule judges
  a 0 as none.
- `CONSOLE_WINDOW_CLASS` — the console host's window class.
- `text` — a reader's side of the text protocol
  (`books/dev/src/reference/crates/verbatim-model.md`, "The text
  protocol"), here so that both executors run one implementation of it
  (Claude, 2026-11-22). `TextSource` is a backend's view of one node's
  text in UTF-16, in its own positions: the outpost's `UiaText` over a
  text pattern, and an edit control's window messages. The algorithms
  over it: `by_calls` runs one text step call by call (`CallStep`);
  `ops` a `Caret` and a `CompareSelection`; `check` a wait's check by
  parts; `sentence` a move by sentence over a source that splits by
  paragraph, its clock read through the `now_us` its caller passes;
  `held` a focus's caret read (`held::read_focus`), a text summary
  (`held::summary`) and a terminal key's starting point
  (`held::starting_from`), each taking a read UI Automation's program
  made in one go when there is one; `caret_report_from` reports a caret
  and line another program read. `facts` holds one node's text facts as
  pure data (`NodeAnchors`, `Anchor`, `LastRead`, `KeptSelection`,
  `ReadSelection`), which the outpost's reducer stores. `Anchors` keeps a
  backend's anchors by node for a reader that holds them for the whole
  read, a test or mockapp, until `forget_anchors`; `NodeText` is one
  node's, borrowed from it or from a `NodeTextCopy`, the copy a
  program's text call works on (`position_at`, `reported_point`). Both
  hold `NodeAnchors` with each position a `Slot`: the source's position
  and the key it was kept under, so a call's result names a position it
  was given by the same key and only one it made is given a new one
  (`NodeTextCopy::finish`). The module allows indexing and plain
  arithmetic, unlike the rest of the crate, since it moved whole from
  the outpost (Claude, 2026-11-22).

Tests: the dialog text rule over scripted objects, the text algorithms over an
in-memory source, the class normalization, the class lists pinned to their NVDA
sources so an NVDA sync is a diff, the class verdict `classify_with` gives
with NVDA's lists (the good, bad and shell rules), the numbering, the
limit per thread, and a window's handle parsed.
