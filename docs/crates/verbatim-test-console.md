# verbatim-test-console

A console the tests start, for the tests of a console window's provider
process in `verbatim-uia` and `verbatim-outpost`
(`docs/design/listener-without-connections.md`, section 4.3). It is a
dev-dependency only: kept out of the crates it serves, so that their
tests and their builds compile them once, with the same features.

Public API:

- `Console::start()` starts `conhost.exe` running PowerShell in a hidden
  console window, in a job that ends both when the `Console` is dropped,
  and returns once the evidence that the client runs in it arrives: the
  window's title changed by the client to one unique to the test process,
  raised as a `WinEvent` from the console host. It panics when the console
  cannot be started or the title has not changed within ten seconds.
  PowerShell is the client because `cmd.exe`'s `title` command prefixes
  "Administrator: " when elevated, as GitHub's runner is.
- `Console::host`, the console host's process id, and `Console::window`,
  the console window, whose owner by `GetWindowThreadProcessId` is the
  client, PowerShell, while the host serves it.
- `Console::start_with_client_window()` starts the console as `start`
  does, its client first making a hidden Windows Forms window of its own
  on PowerShell's pipeline thread, another thread than the one Windows
  names as the console window's, and finds it by a title unique to the
  test process; it panics as `start` does, or when that window is not
  found. `Console::client_window` is that window, 0 for a console started
  with `start`. The moved-on rule's test in `verbatim-outpost` uses it.

## verbatim-stream-test

The crate's binary, `verbatim-stream-test` (`src/bin/verbatim-stream-test`),
is a program of Verbatim's own that streams text into the terminal it runs
in, each way programs stream it, at up to 60 frames a second, beside a
redrawn status row with Pi's spinner and a timer, and a prompt that takes
typing (`docs/design/terminal-model.md`, section 12, item 13). It lives
here because it is a console program for the tests that needs the Windows
bindings this crate already has, and no other crate's tests depend on it.

Each phase clears the screen, prints 26 lines of earlier output (the
screen's height less four), streams its text in tokens of 2 to 12 bytes
from a fixed seed, one token a frame, says it is done on its status row,
and then leaves its state as programs do (the scroll region removed, the
alternate screen left). Its frames depend only on their number and what
has been typed, so a phase writes the same bytes at any pace. A phase is
named `<way>:<placement>`:

- The ways: `append` (each token where the output ends, split anywhere,
  inside a character's bytes too, the character's first bytes written
  alone), `rewrite` (each sentence written again whole for each token,
  after a carriage return or after erasing the row), `ink` (the whole
  region erased upward and drawn again, as Ink does, the paragraph inside
  growing and wrapping), `wrap` (the program wrapping its own text, a
  partly written word that no longer fits erased and written on the next
  row), `scroll-region` (tokens appended into a scroll region above fixed
  status rows), `insert` (the program's own rows, each new one made by
  deleting the output's first row and inserting one above the status
  rows), `repaint` (the screen erased and painted whole each frame, and
  `repaint:<placement>:alternate` on the alternate screen), and `markdown`
  (a heading, a list, a table and a paragraph rendered as they stream: a
  line restyled once its markup closes, a table's columns widened as its
  rows come, the rows from the first that changed drawn again).
- The placements: `footer` (below the output, the prompt and then the
  status row), `header` (the status row on the first row above a scroll
  region that starts below the top, whose scrolled-out lines never reach
  the history, and the prompt on the last row), and `middle` (the status
  row between the output and the prompt, as Pi draws it). `scroll-region`
  has no header phase: every way draws its header above a scroll region.

`verbatim-stream-test [--run <phase>]... [--pipe <name>] [--fps <n>]
[--write-log <path>]` runs the `--run` phases in order at its own rate,
60 frames a second unless `--fps` says otherwise, on a high-resolution
waitable timer. With `--pipe` it then takes commands, one a line, from
the pipe `\.\pipe\<name>-commands`, which its driver creates, and
answers on `\.\pipe\<name>-replies` (two pipes, since a synchronous pipe
handle read on one thread blocks writes on another): `run <phase>` runs a
phase and answers `done <phase>` once its last frame is written; `paced
<phase>` starts one whose frames come one for each `frame` command, each
answered `frame <number>`, and `done <phase>` after the last; `busy` while
a phase runs; `quit` ends the program. It writes to `CONOUT$` and reads
keys from `CONIN$` as they are typed, unechoed; typing shows at the
prompt in the next frame while a phase runs and at once between phases,
Backspace and Enter edit it, and Control+C ends the program. Its output
mode is the conformance player's (a line feed does not return the
carriage), so its bytes mean the same there. `--write-log` writes a line
for each write: the milliseconds since the program started and the bytes
in hexadecimal, which `cargo xtask record-terminal-streams` turns into a
stream of the conformance corpus (`docs/crates/verbatim-term-sim.md`).
