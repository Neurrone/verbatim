# NVDA parity ledger

This ledger tracks, per behavior, what NVDA does (linking the
`docs/nvda/` file that documents it) and where Verbatim stands. It is
the working checklist for parity reviews and the definition of done
for parity work: an "unverified" entry is a claim someone still has to
check against a live NVDA or against the cited NVDA source.

Statuses:

- **matched (verified)** — implemented and confirmed by a live E2E
  scenario or a cross-process test that asserts the NVDA-equivalent
  behavior.
- **matched (unverified)** — implemented to match the cited NVDA
  behavior, but no test or live comparison pins it yet. These are the
  entries most likely to hide inconsistency bugs.
- **different (Dn)** — deliberately divergent, recorded in
  [Architecture](../architecture.md) decision Dn or noted inline in
  [the crate guides](../crates/readme.md).
- **not yet (Mn)** — not implemented; scheduled milestone in
  parentheses.

Keep entries short; move discussion into the linked docs. When a
review confirms an entry, upgrade its status and note how it was
verified.

## Citing an entry

Each entry is a `###` heading in one of the files below. Cite an entry
as `docs/parity/<file>.md`, "<entry heading>", quoting the heading
exactly, so a search for the heading finds the entry and every place
that relies on it. Cite this file, `docs/parity/readme.md`, only for
the ledger as a whole or for its status legend.

## The files

- [Focus announcement](focus-announcement.md): what a focus announcement contains and in what order: names, roles, states, values, multi-line fields, and the reading of an edit field on focus.
- [Focus context](focus-context.md): the ancestry spoken with a focus: entered containers, top-level windows, the window announced on switching applications, a File Explorer folder window, and the desktop.
- [Focus events](focus-events.md): how focus events are accepted, deduplicated, and ordered: duplicate and stale focus events, event acceptance, recovery after an outpost is replaced, held objects, and when speech is cut off.
- [Changes and selection](changes-and-selection.md): what is spoken when something changes on or near the focus: names, menus, toggles and negated states, state changes, tree item counts, selection, controlled lists, suggestions, and values.
- [Alerts](alerts.md): speech that arrives without a focus change: UIA notifications, toasts, other alerts, live regions, and the items of the MSAA audit not yet matched.
- [Windows and processes](windows-and-processes.md): windows whose parts live in different processes or behave unlike ordinary controls: a window in two processes, console windows, a control's own focus, and the shell's staging windows.
- [Object navigation and review](review.md): object navigation, the navigator and review cursor, and the commands that report and review objects.
- [Backends](backends.md): how the MSAA, UIA, and other accessibility backends are read and their events taken in.
- [Speech and audio](speech-audio.md): speech priorities, say-all, synthesizer settings, sounds, verbosity, and speech processing.
- [Input](input.md): the Verbatim modifier, repeated presses, gestures, typed-character echo, IME, and the mouse.
- [Text and documents](text.md): the text layer, caret and selection reporting, segmentation, and documents.
- [Terminals](terminals.md): how new output in a terminal is read.
- [System integration](system.md): starting and stopping, the settings dialog, other Windows integration, and features not built yet.

## Architecture-level stances

### Hang isolation

NVDA: one main thread plus watchdog cancellation
([Main loop and watchdog](../nvda/main-loop-and-watchdog.md)). Verbatim: **different (D9,
D13)** — per-app outpost processes; no output-producing thread may
block cross-process. The evidence that NVDA's freezes come from
synchronous cross-process calls is collected in that NVDA doc.

### Event intake

NVDA: single in-process winevent/UIA registration
with foreground gating ([Event handling](../nvda/events.md), [MSAA and winevent handling](../nvda/msaa.md)).
Verbatim: **different (D13)** — a dedicated no-cross-process-call
focus-listener process plus per-app outposts.

### Extensions

NVDA: unrestricted Python add-ons
([App modules mechanism](../nvda/app-modules-mechanism.md)). Verbatim: **not yet (M5)**,
and **different (D6)** by design — sandboxed Wasm components.

### Injection

NVDA: universal lazy DLL injection
([Process injection](../nvda/process-injection.md)). Verbatim: **not yet (M6)**, and
**different (D2)**: injection only ever accelerates, never required
for correctness.

### Display model

NVDA: GDI hooks power screen review
([The display model](../nvda/display-model.md)). Verbatim: **different (D11)** —
screen review will be a spatial projection of the tree (M6); GDI
model deliberately last (M14, gated).

### Spoken vocabulary and key layouts

NVDA: the spoken role and state
names, messages, synth setting labels, and the desktop and laptop
gesture layouts ([Speech](../nvda/speech.md), [Keyboard input](../nvda/input.md)).
Verbatim: **matched (verified)** by decision, not by accident: users'
ears and hands already know them, so Verbatim speaks the same English
words and binds the same keys wherever it implements the same command
(E2E scenarios assert the wording; the gesture tables live in
[verbatim-input](../crates/verbatim-input.md)). Only the English catalogue
is shared vocabulary; Verbatim's other-language resources are written
fresh, never taken from NVDA's translations. **Different, deliberately**
(Dickson, 2026-10-03): Caps Lock is a Verbatim key by default, with the
two Insert keys, where NVDA's default modifiers are the Insert keys
alone; and Verbatim's menu is on Verbatim+V, not NVDA+N. The laptop
layout has the numpad bindings too, as NVDA binds them for every layout
(since 2026-10-03; they had been desktop only).

### NVDA's global commands Verbatim has not built

**not yet**. Among them
are the settings ring (NVDA+Control+arrows), speech modes, the title
(NVDA+T), the status bar (NVDA+End), read window
(NVDA+B), sleep mode, quit (NVDA+Q), pass the next key through
(NVDA+F2), and the desktop layout's current line (NVDA+UpArrow). As in
NVDA, a Verbatim key combination with no command reaches the
application as the bare key, so until each is built, pressing it types
its letter or performs the key's own action.
