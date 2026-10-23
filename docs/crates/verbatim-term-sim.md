# verbatim-term-sim

A simulated terminal for Verbatim's tests (`docs/design/terminal-model.md`, section 9): a VT emulator whose rows carry identities, answering the questions Verbatim's read program asks of a terminal's UI Automation provider as the console host or Windows Terminal answers them. Its answers are checked against the real terminals by the conformance expectations in its `conformance` folder. It is used by tests only, and it is platform-neutral: it depends on no Windows bindings.

## Public API

- `Terminal::new(flavor, width, height, history)` makes an empty terminal of a `Flavor` (`ConsoleHost` or `WindowsTerminal`), `width` cells by `height` rows, its history holding up to `history` rows above the screen. `Terminal::write(bytes)` writes to it as a program writes to its output.
- `Terminal::snapshot(&Query)` answers one read as the read program takes it (`verbatim_uia_rops::terminal_snapshot`), returning a `Snapshot`: the text from the read's start to the document's end, the screen's text, whether the start was used or refused, whether the read and the screen start at the document's start, the document's first row, the caret's row, its column, how many lines the caret moves to the document's end, and where the next read starts. A `Query` gives the start the last read named (`Range`), the screen's height as last found, and whether to read the whole text. A call the real provider would fail returns `Refused`.
- `Terminal::colours()` gives each screen row's stretches of one foreground and background as shown (`Run`), from the screen's top to the document's last row. `Terminal::text(range)` gives a range's text.
- `Terminal::grid()` exposes the state: the main and alternate `Buffer`s, their `Row`s with their `RowId`s, cells and styles, and the cursor. Row identities follow rows through scrolls, rotation and clears, which no real terminal exposes; the oracle's checks rest on them.
- `screen_rows(screen)` counts the rows of a screen's text as Verbatim counts them.
- `conformance`: the corpus (`streams()`, each a `Stream` of chunks), the expectations (`Expectation`, `Answer`, `expectation_path`), `simulate` and `replay`, which reads the simulated terminal after each chunk as the runner reads a real one and lists where it differs from an expectation, and `history(flavor)`, the history each terminal keeps by default.

## How it works

The grid (`grid.rs`) is a buffer of rows with fixed coordinates. The main buffer holds up to its capacity, the screen at its bottom; a line feed on the screen's last row moves the screen down the buffer until the buffer is full, then discards the first row so that every row's content moves up one coordinate, as the real buffers rotate. The alternate buffer is one screen tall. A scroll region whose top is the screen's top scrolls into the history as the whole screen does, the rows below it keeping their places and the new blank row coming in at the region's bottom; any other region scrolls among its own rows. Erasing the screen on the main buffer pushes its rows with text into the history; erasing the history drops the rows above the screen.

The VT parser is the `vte` crate; `vt.rs` maps what it parses onto the grid: printing with deferred wrapping, carriage return, line feed, backspace and tab; cursor moves, absolute and relative; saving and restoring the cursor; erasing in the line and the display; inserting and deleting rows and characters; scrolling; scroll regions; the alternate screen (modes 47, 1047 and 1049); the cursor's visibility (mode 25); and colours (the sixteen-colour palette as Campbell's, 256 colours, true colour, and reverse video). Anything else is ignored.

The provider (`provider.rs`) answers as the real providers do, each rule measured by the conformance expectations:

- Positions are a row's coordinate and a column; a position anywhere in the allocated buffer is accepted, unwritten rows reading as blank, and one past it is refused, so a range kept on the main screen reads the alternate screen while its row is within the alternate screen's height.
- The document ends at the end of the lower of the last row with text and the cursor's row.
- Rows are padded to the width and joined by line breaks, none between a row and the row its line wraps onto.
- The console host's ranges end before a row's line break, so an expanded line, the screen and the document end without one. Windows Terminal's end after it, except on the buffer's last row, which has no row after it for the break: there the range ends one cell short of the width. That is the alternate screen's last row, and the main screen's once the history is full.
- Moving by lines stops at each row's start and at the document's end, so the last step forward goes from the last row's start to its end.
- Expanding a position past the document's last row gives that row.

## Conformance

`cargo xtask regenerate-terminal-conformance-expectations [--stream <name>]... [--flavor console-host|windows-terminal]...` (`xtask/src/conformance.rs`) writes each stream of `conformance/streams` into the real console host and the end-to-end suite's pinned Windows Terminal, 120 by 30. It reads each after every chunk with `verbatim_uia_rops::terminal_snapshot`, the caret classically, and every screen cell's colours, and writes `conformance/expected/<stream>.<flavor>.json`. It opens terminal windows, so it runs on demand on a desktop, never in continuous integration: when a stream is added, when the simulator's behaviour is changed, and when Windows or Windows Terminal updates. The changed expectation files are reviewed in their commit.

The terminal runs the same executable's `conformance-player`, which connects back over a local socket, writes each chunk it is sent to its console, and then asks for the cursor's position; the terminal answers only once it has handled everything written before, so the answer is the evidence that the chunk is on the screen before it is read. The console host is started through `cmd /c start`, since given any standard handles it serves them as a pseudoconsole instead of opening a window; the player opens `CONOUT$` and `CONIN$` itself for the same reason, and sets the console mode programs run with (processed output, wrapping at the end of a line, VT processing).

Colours are read cell by cell: over a stretch of two colours both providers answer 0 rather than "mixed", and their format unit does not stop at every change of colour.

## Tests

- `src/tests.rs`: the emulator's own behaviour, one test per operation: padding and the document's end, wrapping, rotation under fixed coordinates, row identity through a scroll, a kept range on the alternate screen, a kept start, reverse video, and Windows Terminal's line breaks.
- `tests/conformance.rs`: every stream of the corpus replayed and compared field by field with both terminals' expectations, failing with each differing field's first difference.
