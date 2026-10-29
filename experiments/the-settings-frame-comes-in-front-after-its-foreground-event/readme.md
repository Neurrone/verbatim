# Experiments: the Settings frame comes in front after its foreground event

These measurements answer how long after its `EVENT_SYSTEM_FOREGROUND` the Settings app's frame (`ApplicationFrameHost.exe`, class `ApplicationFrameWindow`) becomes `GetForegroundWindow`, which the review of "The foreground wait" in `docs/design/focus-pipeline-rework.md` asked for before that design's bound of two 10 ms waits is accepted. `settings_system_page` fails because the frame's outpost drops the frame's foreground change when, about 4.6 ms after the event, `GetForegroundWindow` still names Program Manager. NVDA's own wait is two core cycles, about 35 ms in practice. They ran on 2026-10-30 on Dickson's x64 machine (Windows 11, build 26200), with no screen reader running.

## The probe

`main.rs` (manifest `Cargo.toml.txt`) installs an out-of-context `WinEvent` hook for `EVENT_SYSTEM_FOREGROUND` on a thread of its own and stamps each event with `QueryPerformanceCounter` as the callback starts, then reads `GetForegroundWindow` in the callback. A poller thread at the highest thread priority reads `GetForegroundWindow` every 0.5 ms from a spin loop and records each change, with the time of the read before it, and every read that started or took more than 2 ms late. An event's lag is the time from the event to the first read naming its window, negative when the window was already named before the event arrived; "never" means no read named it within 1 s.

The probe drives the desktop itself, as the end-to-end agent does:

- Settings from the desktop, 20 times: the taskbar's Minimize All and Program Manager given the foreground (the agent's `minimize_all`), then `explorer.exe ms-settings:display` launched with `SW_SHOWMINNOACTIVE` and `AllowSetForegroundWindow` for its process (the agent's `launch` with `minimized`, as `Scenario::open_settings_page` asks), the shown, uncloaked Settings frame waited for and brought forward as the agent's `set_foreground` does, and closed with `WM_CLOSE` 1.5 s later. The twentieth launch is the one that opens Settings for the Alt+Tab switches.
- Alt+Tab, 10 times each way: with Settings and then Notepad open, Alt, Tab, Tab up, Alt up in one `SendInput`, from Notepad to Settings, and back.
- Notepad from the desktop, 10 times, as the control: launched minimized, restored and set as the foreground as above, and closed.

The first attempt waited on a high-resolution waitable timer between reads, and its reads were up to 61 ms apart; that pilot's lags are not used (see Files).

## Results

From `runs/full/summary.txt`; percentiles by nearest rank. For the launches, Settings' and Notepad's, no read near an event started late and the read before each change was 0.5 ms before it, so each lag is exact to 0.5 ms. During the Alt+Tab switches the poller's reads started up to 31 ms late (`slow-reads.tsv`), so a switch's window came in front somewhere between the read before the change and the first read naming it; both were before the event in every switch, so the window was in front before each event, though the figures below say only by at least how much.

- Settings from the desktop, the frame: one foreground event per launch, 20 in 20, and `GetForegroundWindow` named Program Manager in the callback in all 20. The frame then came in front at min 23.7, median 50.9, P90 72.8 and max 77.7 ms after the event, never more than 1 s. The lags fall in two groups: 8 launches at 23.7 to 30.1 ms, and 12 at 43.9 to 77.7 ms. Program Manager stayed in front until the frame, with no other window, and no empty foreground, between them.
- Alt+Tab from Notepad to Settings, the frame: already in front at every event, at min -4.2, median -1.4, P90 -0.4 and max -0.4 ms.
- Alt+Tab from Settings to Notepad, Notepad: already in front at every event, at min -22.1, median -2.6, P90 -1.2 and max -0.7 ms.
- Notepad from the desktop: at min -0.8, median 0.0, P90 0.3 and max 0.3 ms, so in front within one read of the event in all 10.
- The Alt+Tab switcher raises two foreground events for its staging windows (class `ForegroundStaging`, explorer.exe) before each switch's own, 40 in 20 switches; 9 of them never came in front in a read, and the others were in front from 9.3 ms before to 0.6 ms after their events.

So the frame's lag fits within 20 ms in none of 20 launches, within 35 ms but not 20 ms in 8 of 20 (40 percent), and exceeds 35 ms in 12 of 20 (60 percent), by up to 43 ms. Only the launch is late: switching to the open frame with Alt+Tab is not.

The frame ignores the show state: in all 20 launches it opened restored and took the foreground by itself, since the launch's show state reaches `explorer.exe`, which hands the URI to the Settings app, and not the frame. The bring-forward's `SetForegroundWindow` then found it already in front.

In the pilot's first launch, a frame left from an earlier Settings session (Settings had been closed and was running suspended before the probe started) came in front 230 ms after the launch with no foreground event at all (`runs/pilot/run.log`, the frame `0x22309a6`). Every later launch, 26 of them in the pilot, in two short trial runs not kept and in the full run, made a new frame with one event. It was seen once and not made again.

## Files

- `main.rs`, `Cargo.toml.txt`: the probe.
- `runs/full/`: the full run. `summary.txt`, the distributions and each trial's events; `events.tsv`, each foreground event (trial, kind, time in ms, window, class, title, image, object and child ids, the foreground window's class read in the callback, the lag, and the read before the change relative to the event); `foreground-changes.tsv`, each change the poller saw; `steps.tsv`, the driver's steps; `slow-reads.tsv`, the reads that started or took more than 2 ms late; `run.log`, the probe's log.
- `runs/pilot/`: the pilot run, one launch of each kind with the timer-driven poller, kept for the frame that came in front with no event; its lags are not valid.

## Rerunning

Copy `main.rs` to `src/main.rs` and `Cargo.toml.txt` to `Cargo.toml` in a scratch folder, build with `cargo build --release`, and run `foreground-lag-probe <out folder> 19 10 10` (Settings launches before the Alt+Tab switches, Alt+Tab rounds, Notepad launches) on an unlocked desktop with no Settings or Notepad window open and no screen reader running. It takes over the desktop for about two and a half minutes, spins one core throughout, and ends with every window minimized and the desktop in front.
