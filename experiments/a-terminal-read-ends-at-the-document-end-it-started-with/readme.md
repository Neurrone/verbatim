# Experiments: a terminal's read ends at the document end it started with

These measurements answer when the console host's document text, as Verbatim's read takes it, leaves out the rows below the caret's line: in GitHub run 40317394858, `conhost_footer_flood` failed after reads 29 and 30 had document text ending above the footer, "status: busy", while their screen text showed it (`docs/design/terminal-model.md`, "The read"). They ran on 2026-10-26 on Dickson's x64 machine (Windows 11, build 26200), with no screen reader running.

The answer: neither terminal leaves rows out of its document. A text range's endpoints are places in the buffer, fixed when the range is made, so the document range's end is the document's end at the moment of the `DocumentRange` call. When output moves the document's end lower between that call and the snapshot's `GetText`, the `GetText`, atomic in itself, reads the buffer as it now is but only down to the old end. Under a scroll region the footer moves down with the view, so the old end's row holds a flood line by then, and the footer lies below the text. Windows Terminal does the same.

## The probe

`main.rs` (manifest `Cargo.toml.txt`) opens `conhost.exe powershell.exe`, or `wt.exe -w new powershell.exe`, running a script it writes: the footer "status: busy" on the screen's bottom row and the caret on the row above it, as `crates/verbatim-e2e`'s `footer.ps1` draws them. When the probe writes a go file, the script floods until a stop file, redrawing the footer every 20 lines, then writes "status: done". There are three modes:

- `region`: the flood is written as `footer.ps1` writes it, a line feed and "flood line N" at a time, inside a scroll region over the rows above the footer;
- `region-slow`: the same, sleeping 30 ms after each 20 lines and the footer's redraw;
- `plain`: no scroll region; each line is written at its own row by moving the cursor there, cycling through the rows above the footer, so nothing scrolls and the caret is above rows with text, the footer always among them.

Each read takes the calls of Verbatim's read program (`crates/verbatim-uia-rops`, `terminal_snapshot_classic`) one by one, in its order: the document range, the first visible range and its text, a start two screens above the document's end, the snapshot's one `GetText` from there to the document range's end, and the caret's rows to the document's end. Then, for the measurement, it takes a second `DocumentRange` at once and compares its end with the first's; when the end moved, it reads the snapshot again to the new end and compares a third `DocumentRange` with the second. The console cursor and window come from `GetConsoleScreenBufferInfo` before and after each read, and the console host's update WinEvents (`0x4002` to `0x4004`) on the window are timed through an out-of-context hook. Each run takes 200 reads before the flood, 200 during it and 200 after "status: done" is written, back to back.

A read is cut when the snapshot's last row with text is not the footer. `analyse.py` counts, for each phase, the cut reads, the reads whose document end moved, the retries, the screen texts whose last row is the footer, and the update events during each read.

## Results

The console host, `region` (`runs/conhost-region-summary.txt`): 186 of 200 reads during the flood were cut, and the document's end had moved in every one of them, by 2 to 8 rows; it moved in 6 more without cutting the text. Every cut read had a console update event during it. The screen text's last row was the footer in only 60 of the 200, since the visible range is fixed at the same moment and read before the snapshot. Reading again to the moved end was still cut in 166 of 192, the end having moved again in 185. Reads took P50 4.9 ms during the flood, 1.0 ms idle. No read before the flood or after "status: done" was cut, and every read after it ended with "status: done".

The console host, `region-slow` (`runs/conhost-region-slow-summary.txt`): 9 of 200 cut, each with the end moved and an update event during the read; the 191 others were taken between writes.

The console host, `plain` (`runs/conhost-plain-summary.txt`): 0 of 200 cut during the flood, with the caret above other rows with text on every read. The document's end never moved, since nothing scrolled.

Windows Terminal, `region` (`runs/wt-region-summary.txt`): 130 of 200 cut, the end moved in all 130, by 16 to 21 rows; a second read to the moved end was still cut in 124 of 130. None before or after the flood. Windows Terminal raises no console WinEvents, so its timing columns are empty. `plain` (`runs/wt-plain-summary.txt`): 0 of 200.

So, in both terminals: a read is cut only when output moves the document's end during the read, which a scroll region's flood does under nearly every read and a cursor-positioned footer over unscrolled rows never does; the caret's place does not matter; a document range taken after the text tells every cut read (325 of 325 across the runs); reading again does not catch up during a flood; and the first read after the output stops sees the whole document. The document's end, when taken, is where `docs/crates/verbatim-term-sim.md` says it is.

## Files

- `main.rs`, `Cargo.toml.txt`: the probe. With the `DUMP` environment variable set, it also writes the first four reads of each phase whole to `<out.tsv>.dump`.
- `analyse.py`: the summary (`python -I analyse.py runs/<run>.tsv`).
- `runs/<terminal>-<mode>.tsv`: each read: phase, index, time and duration in ms, ms since the last console update event and the events during the read, the newest flood line on the screen, the screen's and snapshot's rows, the caret's rows to the end, how far the end moved in characters, whether the read and its retry were cut, whether the screen ends with the footer, how far the console window moved, the cursor's row and the window's height, and the snapshot's last row, the caret's line and the screen's last row with text. `runs/<terminal>-<mode>-summary.txt`: its summary.

## Rerunning

Copy `main.rs` to `src/main.rs` and `Cargo.toml.txt` to `Cargo.toml` in a scratch folder, build with `cargo build --release`, and run `footer-omission-probe <conhost|wt> <region|region-slow|plain> <out.tsv> 200` on an unlocked desktop. It takes no input and closes its terminal when done, in about two seconds.
