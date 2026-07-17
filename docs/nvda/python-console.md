# The Python console

NVDA's interactive console (`source/pythonConsole.py`, NVDA+Ctrl+Z)
is the live-inspection REPL inside the running screen reader —
reference material for roadmap M10's extension console.

## Shape

A wx text-control REPL wrapping `code.InteractiveConsole`, executing
on a dedicated console thread with results marshaled back — but with
full access to NVDA's modules and live state. Tab completion
(`rlcompleter`-based `Completer`) and a persistent input history are
built in; `help(...)` and `exit()` are patched to behave sensibly in
a GUI console (`HelpCommand`, `ExitConsoleCommand`).

## Snapshot variables

The load-bearing feature: opening the console (or pressing the
"snapshot" gesture) captures the interesting state *at that moment*
into short variables, because by the time the console has focus, the
focus is the console:

- `focus` (the focus object when snapshotted), `focusAnc` (its
  ancestor chain, copied), `fdl` (focus difference level), `fg`
  (foreground object), `nav` (navigator object), `caretObj`/
  `caretPos`, `review`, `mouseObj`, and `brlRegions`.

The console also imports the everyday modules (`api`, `speech`,
`braille`, `config`, …) automatically. The workflow this enables —
focus the thing, snapshot, then interrogate `focus.role`,
`focus.IA2Attributes`, `nav.treeInterceptor` interactively — is the
primary way NVDA developers and power users diagnose app behavior,
and the capability bar an extension console should be measured
against.

## Boundaries

The console is blocked in secure mode ([Secure mode](secure-mode.md))
since it is arbitrary code execution by design. A remote variant
(`source/remotePythonConsole.py`) exposes the same REPL over a local
socket for development; it is disabled by default and gated the same
way. Anything typed there runs with NVDA's full privileges on the
user's session — the exact risk profile Verbatim's D6 sandboxing
exists to avoid for *extensions*, while a *developer* console
deliberately keeps it.
