# Experiments: the console host raises its caret events on its blink timer

These measurements answer whether the console host (`conhost.exe`, classic console windows) gives any evidence of a caret move sooner than its console caret event, so that a held caret key's watch could be answered when the caret moves rather than at that event (`docs/design/key-holding.md`, "The outpost"; `docs/performance.md`, "Live, against the build before", where 43 to 45 percent of the console host's caret keys wait 80 to 220 ms). They ran on 2026-10-26 on Dickson's x64 machine (Windows 11, build 26200), with no screen reader running and the cursor blink rate at its default of 530 ms.

## The probe

`main.rs` (manifest `Cargo.toml.txt`) opens a classic console window with `conhost.exe cmd.exe` or `conhost.exe powershell.exe -NoLogo`, clicks it to the front, and types a 70-character `echo` line with virtual-key presses (PSReadLine drops `KEYEVENTF_UNICODE` input). It then presses Left 20 times and Right 20 times, three times over, 120 presses in all, each press and release in one `SendInput`, at a fixed interval. It records, with `QueryPerformanceCounter` times:

- every WinEvent on the console window, every console WinEvent (`0x4001` to `0x40FF`) and every event on a caret object (`OBJID_CARET`) from any window, through an out-of-context hook over all events;
- UIA's text selection changed, text changed, text-edit text changed and conversion target changed, changes, notification, active text position changed, live region changed and system alert events, and changes to bounding rectangle, name, keyboard focus, value, and the legacy name, value, state and description properties, all over the console window's subtree from a multithreaded client, and UIA's focus changes;
- the console cursor read through `GetConsoleScreenBufferInfo` (the probe attaches to the shell's console) before each press, then read repeatedly for up to 100 ms after it, giving when the cursor moved, then again before the next press.

`analyse.py` takes each signal's first arrival between one press and the next. Bridged WinEvents that carry a cookie as their object id (the `0x01xx` range and `EVENT_OBJECT_TEXTSELECTIONCHANGED`) are counted together.

## Results

Runs at 600 ms between presses, so that every press is followed by at least one tick of the blink timer (`runs/cmd-600-summary.txt`, `runs/powershell-600-summary.txt`); every one of the 120 presses moved the caret in each shell.

- The caret's signals are `EVENT_CONSOLE_CARET`, `EVENT_OBJECT_LOCATIONCHANGE` on `OBJID_CARET`, `EVENT_OBJECT_TEXTSELECTIONCHANGED`, UIA's text selection changed, and four bridged WinEvents (`0x10a`, `0x10c`, `0x11b`, `0x121`). They arrive together, within about 1 ms of each other, once per press: in cmd at P50 278, P95 505, maximum 540 and minimum 1 ms after the key; in PowerShell at P50 257, P95 501, maximum 535 and minimum 3 ms.
- Consecutive console caret events are 522 or 538 ms apart, or twice that when a tick had no move: the console host raises them from its 530 ms cursor blink timer, on the first tick after the cursor moved.
- No other signal follows a caret move: no UIA text change, console update, property change or focus change arrived after any press in either shell. Nothing arrived in 3 seconds idle before the presses.
- The cursor itself had moved, by the repeated reads, at P50 0.65, P95 33, maximum 54 ms after the key in cmd, and P50 0.95, P95 5.1, maximum 51 ms in PowerShell.

Runs at 150 ms between presses, about the end-to-end measure's rate (`runs/cmd-150-summary.txt`, `runs/powershell-150-summary.txt`): the same signals came after 37 of 120 presses in cmd and 34 of 120 in PowerShell, one tick covering the presses since the last, at P50 82 and P95 143 to 145 ms after the press they followed. So the caret's events do not accompany every caret move: one follows each move within one blink period, for whatever moves that period held. The cursor had moved at P50 3.1, P95 20, maximum 46 ms in cmd, and P50 0.84, P95 1.2, maximum 2.7 ms in PowerShell. The PowerShell run at 150 ms also recorded another console's start and focus, from a process outside the probe, after one press; they are not the probe's console.

In an earlier PowerShell run whose typing was lost, so that every Left and Right met an empty line and moved nothing, each press was still followed at the next tick by `EVENT_CONSOLE_CARET` and the caret location change, but not by UIA's text selection changed or the bridged events: PSReadLine sets the cursor even when it does not move it. That run's output was overwritten by the runs above and is not kept.

Established, on this machine: the console host gives no evidence of a caret move before its blink timer's next tick, 0 to about 540 ms after the move, with nothing in its UIA or WinEvents for a caret move between ticks; the move itself is visible to a read within about 1 ms at the median.

## Files

- `main.rs`, `Cargo.toml.txt`: the probe.
- `analyse.py`: the summary (`python -I analyse.py runs/<run>.tsv`).
- `runs/<shell>-<interval>.tsv`: each run's presses (index, time in microseconds, key, cursor before, cursor after, ms until the move was read or -1) and records (time in microseconds, source and fields); `runs/<shell>-<interval>-summary.txt`: its summary.

## Rerunning

Copy `main.rs` to `src/main.rs` and `Cargo.toml.txt` to `Cargo.toml` in a scratch folder, build with `cargo build --release`, and run `conhost-caret-probe <cmd|powershell> <out.tsv> 3 20 <interval ms>` on an unlocked desktop with no other console window in front; it takes over the keyboard for about 80 seconds at 600 ms and closes its console when done.
