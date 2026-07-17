# Braille

NVDA's braille subsystem (`source/braille/`, recently split from one
module into a package) renders the current context to a refreshable
braille display, independently of speech. The moving parts: regions,
the buffer, tethering, translation via liblouis, display drivers,
and braille input.

## Rendering model: regions in a buffer

- A **Region** (`braille/regions/base.py` and siblings) renders one
  source to cells: `NVDAObjectRegion` (an object's braille
  presentation: name, role abbreviation, states — the wording rules
  live in `braille/labels.py` and `braille/properties.py`),
  `TextInfoRegion` (a range of text with its caret/selection dots
  and control-field markers; `regions/textInfo.py`), plus special
  regions (input composition, speech-in-braille when "show speech"
  is on — `_showSpeechInBraille`).
- The **buffer** (`braille/buffers.py`) concatenates the regions for
  the current context (focus ancestors' labels, then the focus
  region — mirroring speech's context-then-control order), maps
  buffer cells to display-sized *windows*, and supports scrolling
  (`scrollForward`/`Back`), with config for word wrap and how much
  ancestor context shows ("focus context presentation").
- **Cursor routing**: each cell remembers the text position it came
  from (`regions/_routing.py`); routing keys route to click/caret
  placement at that position.

## Tethering

`BrailleHandler.setTether` / `getTether`
(`braille/brailleHandler.py`): the display follows *focus* or
*review* ([Review modes](review-modes.md)), with auto-tether (default) switching
to whichever the user last moved. Event integration: the handler
subscribes to focus, caret, and property-change NVDA events
(`handleGainFocus`, `handleCaretMove`, `handleUpdate` — called from
event handling and the core pump; [Main loop and watchdog](main-loop-and-watchdog.md)
lists `braille.pumpAll` in the cycle) and re-renders the affected
region rather than the world.

## Translation

Text-to-cells goes through liblouis (`source/louisHelper.py`,
bundled `louis` module) with the user's table
(`source/brailleTables.py` — contracted and uncontracted, many
languages); input (typing on a braille keyboard) goes the reverse
direction (`source/brailleInput.py`), including contracted-braille
back-translation with correct handling of partial words (cells held
until a word terminator). Unicode braille and per-table quirks are
absorbed here.

## Displays

Drivers (`source/brailleDisplayDrivers/`, base class in
`braille/display/driver.py`) implement: probe/connect (USB, serial,
Bluetooth, HID — with *automatic detection* via `source/bdDetect.py`
device matching), `display(cells)`, key events in (mapped through
the gesture system as `BrailleDisplayGesture` —
`braille/display/gesture.py` — so display keys bind like keyboard
gestures; [Keyboard input](input.md) step 4 in script resolution), and display-size
reporting (including multi-line displays via `displayDimensions`).
The standard HID braille protocol is supported
(`hidBrailleStandard`; design notes in
`nvda/projectDocs/design/hidBrailleTechnicalNotes.md`). Drivers can
come from add-ons; a `noBraille` driver is the null case. Secure
desktop transitions hand the display between NVDA instances
(`_onSecureDesktopStateChanged`; [Secure mode](secure-mode.md)).

## Points of coupling to remember

Braille is a *second renderer of the same state*, not a transform of
speech: it consumes objects, TextInfos, and events directly, with
its own formatting rules and its own update triggers. Any
architecture that plans braille later should preserve access to (a)
the focus/review position as a TextInfo, (b) object property
changes at event granularity, and (c) the ancestor context chain —
those are the inputs NVDA's braille actually uses.
