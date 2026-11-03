# A redraw in a key's window is spoken as its answer

The evidence for the known limit in `docs/roadmap.md`, M6.4, and `docs/flaky-tests.md`, `conhost_footer_overflow`: one failed local run of that scenario on 2026-11-03, at commit 000f36f3, on Dickson's x64 machine (Windows 11, build 26200). Two runs before it passed.

What was heard: after "n" starts the script's 12,000-line flood, "flood line 2" and "flood line 3", then "flood line 20", then "flood line 4", where 4 to 15 and the skip were expected before anything later.

The probable cause, inferred from these inputs and the effect counts, not yet replayed: the script reads "n" without an echo and redraws its footer every 20 lines; the redraw at line 20 came about 90 ms after the key, and every read within the 300 ms key window counts as the keys' (`KEY_WINDOW_MS`, `owner`, `crates/verbatim-text/src/terminal_reading.rs`), so "Rows the keys own" (`docs/design/terminal-model.md`) spoke the redrawn row at once as the key's answer, ahead of the queued flood.

## Files

- `flight-recorder.jsonl`: every input Core received in the run, in order, with the count of effects each produced. The key "n" is `key_seq` 17; the terminal snapshots that follow it, from "flood line 1" alone to lines 1 to 20 with "status: busy", are lines 93 to 104, and the key's own answer, query 19, is line 105. Line 104's one effect is "flood line 20".
- `timeline.txt`: the scenario's timeline, each step with what was said, queued, played and cancelled, in milliseconds from Verbatim's start.

## Replaying it

Feed the snapshots from line 93 on, with the key at its recorded time, through the terminal reading rules (`crates/verbatim-text`) as Core does, and check which rule turns line 104's read into an answer.
