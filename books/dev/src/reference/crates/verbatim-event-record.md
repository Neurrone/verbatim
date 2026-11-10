# verbatim-event-record

An executable beside `verbatim.exe` that records what an application raises, through MSAA and UI Automation, as JSON Lines, so that a design can cite what a real application raises beside NVDA's capture. Most recordings are made without Verbatim running; running beside Verbatim works too (Dickson, 2026-11-10). It changes nothing in Verbatim's processes. A tool crate (`xtask/src/tiers.rs`), Windows-only.

## Command line

`verbatim-event-record` takes exactly one target, `--process <name or id>` or `--window <handle>` (decimal or `0x` hexadecimal). `--process <name>` matching several processes refuses, listing each id with its window title. A `--window` target that is not a top-level window, or that is not found under the desktop's root, is refused, as is a stop key Windows refuses, before anything is written.

- `--seconds <n>`: the time limit. Otherwise recording runs until the stop key, the target's exit, or Ctrl+C.
- `--stop-key <keys>`: the key that ends the recording, in the end-to-end agent's key vocabulary, by default `control+alt+shift+f12`, registered with `RegisterHotKey`, so that nothing runs per key.
- `--output <file>`: the file written, replaced if it exists, by default `verbatim-events-<UTC time>.jsonl`.
- `--steps <file>` with `--agent <host:port>`, by default `127.0.0.1:44001`: runs the file through the end-to-end agent while recording. A step per line: `press <keys>`, `type <text>`, `pause <ms>`. A run with steps ends two seconds after the last step, or sooner at `--seconds` or the stop key. With `--window` the agent brings the target forward through `SetForeground` before any registration starts, so that its minimize and restore are not recorded; with `--process` the person running it brings the target forward.
- `verbatim-event-record print <file>`: a recording as sentences. It reads only the file and refuses another format version.

## The file

JSON Lines, in arrival order. The first line is the header: `format_version` (1), `started_utc`, `windows_build`, `target`. An event line has every field, null when it has no value: `t_us` (microseconds from the start at arrival), `kind`, `in_target`, `pid`, `window`, `runtime_id`, `name`, `control_type`, `class`, `automation_id`, `framework`, `property`, `value`, `is_focus`, and for an MSAA line the raw `event`, `object` and `child`. A UI Automation line's `kind` is `uia_focus`, `uia_property`, `uia_structure` or `uia_event`, whose `property` holds the event's name; an MSAA line's is `msaa_focus`, `msaa_foreground` and so on for the events the table names, `msaa_other` for the rest. An MSAA line holds local facts only: its name, control type, automation id, framework, runtime id and `is_focus` are null.

Two other lines:

- `behind` (`t_us`, `source`, `late_ms`): an MSAA event arrived over 100 ms after its Windows timestamp (`source` `msaa`), or a UI Automation focus arrived over 100 ms after its MSAA focus (`source` `uia_focus`).
- `registered` (`t_us`, `window`): the registration of a window of the target has completed, so a gap before it is visible.

## How it records

- MSAA: a hook of the recorder's own, one `WINEVENTPROC` writing raw events, because Verbatim's hook code filters and normalizes. One thread hooks every event of the target's process; another hooks focus for every process. In `--window` mode an event whose root window is not the target is dropped, except the target's own destruction.
- UI Automation: two registrations. Desktop-wide: the focus handler, written for every process, and menu opened, window opened and tool tip opened, kept only for the target's, so that a new window's first event is not missed. On the target's windows' subtrees: property changes (with the new value), structure changes, the remaining automation events, notifications, and active text position. Every element arrives with `RecordedEvent`'s cache, so a callback makes no cross-process call, the runtime id included.
- A window's element comes from a descent of its owner chain from the root, not `ElementFromHandle`: the raw view puts an owned window under its visible owner, and a new connection by handle while another process's remote operation waits can answer UI Automation's window proxy. A window of a `--process` target not found is reported on standard error and has no `registered` line.
- The window watcher, with `--process` only, follows the target's top-level windows from its shown and destroyed `WinEvent`s and moves the target's registration, which each `registered` line then reports. A settle past UI Automation's connection timeout is said on standard error.
- Writing: each callback stamps its line and sends it under one lock, so the file's order is the time order; one thread serializes and writes, flushing whenever the channel is empty, so a slow disk delays no handler and nothing is dropped.
- The Steps thread (with `--steps`) holds the agent connection and ends two seconds after the last step, at the first failure, or when main ends it.

## Pairing for `behind` lines

The writer thread pairs each process's MSAA and UI Automation focus events, by the MSAA event's window against the UI Automation element's native window, in one `Waiting` value per process: a queue of one side only, so that both sides waiting cannot be written. An element with no window never pairs, and an MSAA event whose window has no provider writes no entry. The rules:

| State | MSAA focus for window W | UI Automation focus for window W | Any arrival |
| --- | --- | --- | --- |
| Nothing waits | Becomes MSAA waiting | Becomes UI Automation waiting | Nothing |
| MSAA waiting | Queued, unless it repeats the newest entry's window, object and child, which is dropped | Takes the oldest entry for W, dropping the entries older than it; over 100 ms between the arrivals writes `behind`. With no entry for W, becomes UI Automation waiting holding only this arrival | First drops entries older than 10 seconds |
| UI Automation waiting | Takes the oldest entry for W, dropping the entries older than it; no line. With no entry for W, becomes MSAA waiting holding only this arrival | Queued | First drops entries older than 10 seconds |

## Ending

Main waits on the stop key, the target's process handle, an early-end event and the time limit. The early-end event carries one reason: a write error, a failed step, a `--window` target's window destroyed, the steps' two seconds elapsing, or a console control event, whose handler sets it so that Ctrl+C never reaches the detach fault. Main ends the Steps thread first, sends the writer its end and joins it, prints the file's path and the reason, then calls `end_without_detach`: no UI Automation handler is removed and no other thread is joined, so exit never waits on a hung application. The file is complete first. The exit code is 0, or 1 for a failure.

## Cost and limits

The recorder adds no process, hook, pipe or message to Verbatim, and no work per key. It sets no budget (Dickson, 2026-11-10); the desktop-wide cost is measured and reported.

- While recording, a second process holds a desktop-wide UI Automation focus handler, so Verbatim's own focus can arrive late ([UIA defects](../uia-bugs.md), "Desktop-wide focus handlers in a second process deliver late"), and a failed new connection can make an outpost read a new window through MSAA, so speech during a recording can be wrong and not only late (Dickson, 2026-11-10). Without Verbatim running there is neither effect.
- For a window UI Automation proxies, some events appear only as MSAA lines. `ignore_win_events` stays as every client sets it; the recorder's added events keep UI Automation's mapping, so that the file shows what a client of such a window receives.
- Events a new window raises before its registration completes are lost, but for menu, window and tool tip openings. A `behind` line says delivery may be late, not which event was. An owned window is registered beside its owner, so a provider that delivers through the owner's subtree can write its events twice. Without UIAccess, elevated targets cannot be read.
