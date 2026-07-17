# Logging

NVDA's logging (`source/logHandler.py`, viewer in
`source/gui/logViewer.py`) is Python's `logging` with screen-reader
specifics layered on. Reference for roadmap M9.

## Levels and categories

The custom `Logger` adds levels between the standard ones: `IO` (12 —
every keypress, gesture, and speech sequence at this level or below)
and `DEBUGWARNING` (15 — the workhorse "something odd but survivable"
level), plus `OFF` (100). The user-facing choice (General settings)
is effectively: disabled, info, debug warning, input/output, debug.
Separately, *debug logging categories* (the `[debugLog]` config
section) gate the chatty per-subsystem streams — `speechManager`,
`MSAA`, `UIA`, `events`, and so on — so debug-level logging of one
subsystem doesn't drown the log ([Speech](speech.md) and
[MSAA and winevent handling](msaa.md) both check their category
before formatting).

Practical behaviors worth copying:

- **Code-path attribution**: `getCodePath` prefixes every record with
  the class and function it came from, computed from the stack frame —
  what makes NVDA logs greppable by subsystem without discipline from
  call sites.
- **Error sounds**: errors play a sound in test/dev builds
  (`shouldPlayErrorSound`) so a blind developer notices exceptions
  without watching a console.
- **Secure-mode redaction**: in secure mode logging is forced off and
  a `redactSecrets` path scrubs known-sensitive values on the ordinary
  levels ([Secure mode](secure-mode.md)).
- **Crash context**: unhandled exceptions and the watchdog's freeze
  dumps route through the same log
  ([Main loop and watchdog](main-loop-and-watchdog.md)), with
  base-path stripping so tracebacks are readable
  (`stripBasePathFromTracebackText`).
- Files: `nvda.log` plus the previous session's `nvda-old.log` in the
  user config directory; the log is line-oriented UTF-8 designed to be
  read *in* NVDA.

## The viewer

The log viewer (NVDA+F1) is a plain read-only text window over the
live log with refresh — deliberately simple; filtering happens with
browse-mode find rather than viewer features. NVDA+F1 also dumps *dev
info for the navigator object* (a formatted property dump appended to
the log then shown) — the everyday inspection tool, the role
`verbatim-inspect` plays for Verbatim.
