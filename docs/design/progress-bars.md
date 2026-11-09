# Progress bars, foreground and background

Status: approved by Dickson on 2026-11-09.

## In short

Today a progress bar that is not the focus is heard only through the foreground application's outpost, which subscribes on its window. Every UIA handler on a classic window starts UI Automation's WinEvent thread in that outpost, which costs 34 to 44 wake-ups per caret key in a text box, whatever the scope or property.

This design listens once, in the listener, as NVDA does. All bars take one path, with no outpost and no per-outpost subscription. A UIA bar's change arrives with its value and range in the event's cache, and the reading thread reads where the bar is. A classic bar's value change reaches the listener's global WinEvent hook, and the application's reading thread reads its percentage, as a toast is read. The listener reports the facts, and Core decides whether it is heard.

**This is a revert of recent work.** It removes the foreground-window subscription built today, a per-outpost subscription: `BarWatch`, `CoreToOutpost::Front`, `Group::RangeValue` and the bar branch of `uia_event_window`. The toast path built in the same commit stays.

## NVDA

- UIA: `UIA_RangeValueValuePropertyId` is in the desktop-wide group (`nvda/source/UIAHandler/__init__.py` 241 to 245, 598 to 603), dropped afterwards unless the object is a progress bar.
- MSAA: one out-of-context hook per event type with no process filter (`nvda/source/IAccessibleHandler/__init__.py` 217), `EVENT_OBJECT_VALUECHANGE` among them (`internalWinEventHandler.py` 57); a native UIA window's event is dropped there (570 to 583).
- Acceptance: with `reportBackgroundProgressBars` on, every value change is accepted; otherwise the window must be related to the foreground (`nvda/source/eventHandler.py` 499 to 503, 540 onward).
- `ProgressBar.event_valueChange` (`nvda/source/NVDAObjects/behaviors.py` 44 to 85): an invisible or off-screen bar, or a non-numeric value, is an ordinary value change; a bar not in the foreground window is dropped unless the setting is on (`isInForeground`, `NVDAObjects/window/__init__.py` 295 to 297); the last percentage is kept by the bar's centre.
- Settings (`nvda/source/config/configSpec.py` 146 to 152): `reportBackgroundProgressBars` false, `progressBarOutputMode` beep, `speechPercentageInterval` 10, `beepPercentageInterval` 1, `beepMinHZ` 110.

## Where each part runs

- UIA, foreground and background: the listener's desktop-wide group, filtered from the cached control type on its callback thread, as the toast path does.
- MSAA, foreground and background: the listener's global WinEvent hook, which adds `WinEventKind::ValueChange` to `LISTENER_SUBSCRIPTIONS` (`crates/verbatim-ia2/src/hook.rs`), filtered there with no call into the application, then the application's reading thread, pushed as `desktop_event` pushes, with `MarkerOwed::nothing()` so that no outpost is started or asked and no fact is made of the event, intercepted before `fact_of_win_event`.
- Core judges whether a change is heard, and keeps the last percentage per place.
- The listener is the one owner of a progress bar's changes. An outpost reports none, and its value-change handling stays for text and for keeping the focus's snapshot current.

## Types

```rust
// verbatim-uia: the part's cached shape, on the listener's registration.
pub struct ListenerProgress {
    event: ListenerEvent,
    value: f64,
    minimum: f64,
    maximum: f64,
}

// verbatim-model: built only by the listener's callback, from a progress
// bar that is on screen and has a number, so Core re-checks neither.
pub struct BarChange {
    percent: Percent,           // 0 to 100, parsed at the boundary
    place: (i32, i32),          // the bar's centre
    in_foreground: bool,        // its window is the foreground window or beneath it
}

// A message from the listener to Core, with no node: the bar is not held.
ListenerToCore::Progress(BarChange)

// verbatim-listener-logic: the reading thread's job for a classic bar.
FocusEvent::ClassicBar { window: isize, object: i32, child: i32 }
FocusEvent::UiaBar { element, value: Percent }
```

`NormalizedEvent::ProgressChanged`, carrying a node, is removed: a focused bar's value still updates its snapshot as any focused value does, and is not heard from that path. `Percent` is the same type on every path.

A UIA bar's percentage is computed from the cached value, minimum and maximum, as NVDA's overlay does (`NVDAObjects/UIA/__init__.py` 2936 to 2944), so a range of 0 to 1000 at 500 is 50. Its bounds and off-screen state are not cached, since a desktop-wide cache would fill them for every range change: the reading thread reads the bar's `Place` and its nearest window once per change, the window memoised by runtime id, and drops an off-screen bar. An event from a window whose UIA provider is not native is dropped in the callback, as NVDA drops a non-native sender (`UIAHandler/__init__.py` 1019), so a classic bar proxied through UIA is not heard twice.

A bar of a listed class, by the name NVDA normalises (`NVDAObjects/window/__init__.py` 353, so `WindowsForms10.msctls_progress32.app...` matches), has its percentage read on the application's reading thread, by three window messages bounded as that thread's reads are: `PBM_GETPOS`, then `PBM_GETRANGE` for the low and for the high end, giving `(position - low) * 100 / (high - low)`, with an empty range dropped at the boundary. These return plain integers, so no object is built and no COM call is made. The window's centre, visibility and relation to the foreground are local calls. A hung window is dropped by the listener's existing record before the read, and a read abandoned at its deadline counts in that application's record as any abandoned read does, so a stuck installer cannot hold up another application's events. It needs an application record for a process with no outpost, which no supervisor exit ends because `hold_target` never runs for it, so such a record is ended when it goes idle, as a record with nothing left is dropped.

A bar of any other class is recognised by its role, as NVDA recognises it (`NVDAObjects/IAccessible/__init__.py` 2856). The application's record keeps a capped memo of (window, object, child), under the `Readers` lock, whether that object is a bar. A known non-bar drops in the hook with no call. A miss queues one read: the raw probe for a native UIA window, then `accRole`. A bar found so has its percentage read by `accValue`, about four MSAA calls per change. An edit control therefore costs one wake and one read in its life, not one per typed character.

## Rules

Core, for every bar's change, the focused bar's included:

| Bar | Setting off | Setting on |
|---|---|---|
| In the foreground window or beneath it | Heard, by the existing rule | Heard |
| Anywhere else | Dropped | Heard |

"Heard" is today's `reduce_progress_changed`: the theme's progress indication (a rising tone by default, D12), once the percentage has moved by a percent from the last one at that place.

A focused progress bar with a number and on screen is heard only as a bar: Core keeps its ordinary value change silent, since the outpost's value change for that focus would otherwise say "41" beside the tone. A focused bar off screen or with no number speaks as any value change does, as NVDA's `event_valueChange` falls through (`behaviors.py` 49 to 58).

The listener's hook, for each `EVENT_OBJECT_VALUECHANGE`, with no call into the application:

| Window and object | What it does |
|---|---|
| A class NVDA's table names as a bar, by normalised name, visible | Queued as `FocusEvent::ClassicBar` on its application's reading thread, one held per window, a newer change replacing it |
| In the memo as not a bar | Dropped, with no call |
| In the memo as a bar | Queued the same way |
| Neither | Queued once for the read that fills the memo |

Whether a window is a native UIA window sends a window message that a busy window can hold (`crates/verbatim-uia/src/probe.rs`), so it is asked on the reading thread only, never on the hook's thread: a native UIA window's event is then dropped there, as NVDA drops it, its bar coming through the group.

The hook needs value changes always, not only while the background setting is on, because foreground bars take this path too. Core alone applies the setting. An outpost's hook still receives the same event for its own application, for text, and reports no bar from it.

Settings: `report_background_progress_bars`, false by default, is read by Core alone, so it reaches neither the listener nor the outposts. `progressBarOutputMode` and the two intervals are not part of this: the theme chooses tone or speech (D12), and M4.3 builds the speech interval as its roadmap entry says.

## Removed and kept

Removed: every report of a progress bar by an outpost, focused or not, with `NormalizedEvent::ProgressChanged`, `BarWatch` and its cells, `CoreToOutpost::Front` and `FrontWindow`, Core's sending of it, `Group::RangeValue` and `Alone::RangeValue`, the outpost's bar lookup, the bar's clause in `follows`, and the bar branch of `uia_event_window`.

Kept: `Earcon::Progress`, `progress_reported`, the focus's value subscription for its snapshot and the `foreground_progress_bar` scenario, whose `await-advised` wait on a window registration that no longer exists is replaced by a wait for the listener's subscription.

## Costs

| | This design |
|---|---|
| Text box and Notepad, per caret key | To be measured; none expected from the UIA part, since the listener already holds desktop-wide handlers |
| Foreground change | Nothing |
| Each UIA range value change anywhere | One callback on the listener's UIA thread, the cache filled in the application; dropped there unless a progress bar |
| Each UIA progress bar change | One wake of a reading thread, the bar's `Place` and, the first time, its nearest window, then one message to Core |
| Each `EVENT_OBJECT_VALUECHANGE` anywhere | One wake of the listener's WinEvent thread and local calls, a typed character in a classic edit control included; this is new, always on, and on the event path |
| Each classic bar of a listed class | One wake of a reading thread, three window messages, one message to Core |
| Each classic bar known only by its role | The same wake, about four MSAA calls, one message to Core |
| A first value change of any other object | One wake and one read, the raw probe and `accRole`, once per object while the memo holds it |

No new timer.

The first step measures the listener's wake-ups and processor time against `scratchpad\measure-main\results.md`, on `measure_keys_text_box`, `measure_keys_windows_terminal` and `measure_keys_notepad`, under typing and streaming text, and on a new workload of sliders and bars changing in a background window, including a role-only bar's calls per change, and on a hung window raising value changes, watching that focus delivery is unaffected (`books/dev/src/reference/uia-bugs.md`). Whether the part adds a WinEvent thread cost to the listener is verified there and not assumed. A measurable added cost goes back to Dickson with its figures.

## Limits

- A slider dragged anywhere wakes the listener's UIA thread for every change.
- A bar in the foreground reported by the listener carries no node, so review commands do not reach it through this path.

## Tests

Hand-written, one per table cell: Core's rules; the callback against a progress bar, a slider, an off-screen bar and a non-numeric value; the hook's filter for each row, the memo and its cap included; a WinForms-named class; the percentage from a UIA range of 0 to 1000; a proxied classic bar heard once; Core's silence for a focused numeric on-screen bar; the percentage from position and range, an empty range included; a hung window and an abandoned read in the application's record; a native UIA window's MSAA event dropped; mockapp for a UIA and a classic bar in a background window with no outpost. No generated test: the state is a rule table, with no interactions whose combinations matter.

Scenarios: `foreground_progress_bar`; a new `background_progress_bar` with the setting off and on, for a UIA and a classic bar, comparing with NVDA.

## Claude's decisions

- The listener's fact is a parsed `BarChange`, with no node, because the bar need not be held to be heard.
- The foreground fact is read where the event arrives and Core judges it, because the window relation is a local call only there.
- A windowless element's nearest window is read on the reading thread, for bars only, memoised by runtime id, as a Settings page's content needs.
- Roles are recognised by a capped memo per application, so that a value change on a known non-bar costs no call.

## Decisions (Dickson, 2026-11-09)

- The UIA value subscription is in the listener's desktop-wide group, filtered from the cached control type, conditional on the first step's measurement showing no added cost on the key workloads.
- `report_background_progress_bars` in `settings.toml`, false by default, as NVDA's `reportBackgroundProgressBars` (`nvda/source/config/configSpec.py` 147), its place in the dialog left to M4.3.
- A classic bar uses the listener's global WinEvent hook. No outpost is started for it.
- The listener owns every progress bar: its hook takes value changes always, for classic bars foreground and background, and its UIA group takes range values. An outpost reports no progress bars, and keeps its value-change handling for text. One owner, because an outpost for the foreground and the listener for the background would each have to judge whether a bar is its own against the foreground, and a foreground change could report a bar twice or not at all.
- One listener wake per value change anywhere on the desktop, a typed character in a classic edit control included, is accepted subject to the first step's measurement of the listener's wake-ups and processor time under typing and streaming text; if that adds a measurable cost, the result goes back to Dickson. NVDA's one hook has the same cost (`nvda/source/IAccessibleHandler/__init__.py` 217).
