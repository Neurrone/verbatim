# Focus for a newly opened or shown window is not held

## Fact

During a hold by a hung window named in a focus `WinEvent`, UIA raises focus for a newly opened or shown window, such as a menu's popup, on a thread-pool thread (`ClientEventManager::OpenShowWinEventCallbackThreadProc`, then `EventManager::AssociateWithRecord` and `RaiseLocalEvent`), and that focus is not held: in the same hold that kept another application's notification from the listener for 3.8 seconds, Notepad's File menu focus reached it 26 to 112 ms after Alt+F (`both`). Focus moving within an existing window, name changes and notifications are held (`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events", "Symptoms").

## What Verbatim relies on

Nothing is built on it: it explains why a menu's focus is announced during a hold while focus within an existing window is not, as in NVDA (`books/dev/src/explanation/design/focus-pipeline.md`, section 12.3).

## Method

On the unlocked desktop, run only when named: the hung-window scene of support's `uia_facts_support::hung` on the user's desktop, P1's T a plain Win32 window and P2's B a UIA window, and a third provider program P3 with a hidden plain window O; Verbatim's stand-in, a listening process with a desktop focus handler, Verbatim's proxy factory mapping and B's name changes. T's UI thread is held until released; a focus `WinEvent` names T; B is renamed 200 ms later; 0.6 s after that P3 brings O to the front and opens a popup menu of its own over it (`TrackPopupMenuEx`, items `Menu first item` and `Menu second item`), a menu opened during the hold as Alt+F opens one. The stall is released once the stand-in has heard B's change, or 300 ms past the top of its band, so a hold to the stall's end still shows as out of band; then the menu is closed.

- `open-show`: judged: the focus of the menu's first item, matched by its name, reaches the stand-in while B's name change is still held (a relation), and B's change is held two connection timeouts from the `WinEvent` (a band).
## Client settings

Checked on Dickson's desktop runs of the programs of 14e569c9, with Notepad's menu as the newly opened window, by default and with each setting: neither connection recovery nor event coalescing changes the outcome. The provider program's menu has not been run yet.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `both`: Notepad's menu focus 26 to 112 ms after Alt+F while the notification was held 3,815 to 3,829 ms | 2026-11-01 | Build 26200 | `../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/recorded/both.out` | The menu's focus during the hold; a band |
| First desktop runs, programs of 14e569c9 (built from f0b18b2a), by default and with each client setting: the menu's focus reached the stand-in during the hold each time, B's change 4,016.9 to 4,026.1 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | Dickson's desktop runs | Pass |
| Dickson's desktop runs at 28b23338, by default and with each client setting, with a hidden window of the provider program shown and brought to the front in place of a menu: its focus did not reach the stand-in while B's change was held (4,005.7 to 4,027.6 ms) | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 28b23338 | A window shown is not the menu the claim names; the case now opens a menu |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\focus-for-a-newly-opened-or-shown-window-is-not-held.exe --case open-show`. It needs the unlocked desktop and takes it over: it brings a provider program's window to the front and opens a menu over it. The debugger guide for the route is in [the threads guide](../uias-client-threads-and-focus-tracking-calls/readme.md) (`route1`, `route2`).

## Evidence kept

None of its own: `both.out` stays with [the hold's folder](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md), which cites it for the listener's hold.
