# Ancestors cannot be cached or listened to

## Fact

An event registration scoped to an element's ancestors alone (`TreeScope_Ancestors` or `TreeScope_Parent`) receives no ancestor's events: on Windows 11's taskbar, a registration on the clock button's child does not hear the clock button's name change each minute, while one on the clock button itself does, and a provider's tree gives the same result, made as an event handler group and directly. With the element itself in the scope it depends on the application: on the clock button's child, the element and its ancestors, NVDA's scope, heard one of the button's two name changes each minute, while in the provider program's tree no such scope heard an ancestor's change (`books/dev/src/reference/uia-bugs.md`, the limitation of that name). That a cache request cannot take in the parent or ancestors is Microsoft's documentation of [`IUIAutomationCacheRequest::TreeScope`](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope), cited, not reproduced.

## What Verbatim relies on

Ancestors are read by a walk or a remote operation, and Verbatim registers its local group on the focus with `TreeScope_Element` alone (`docs/parity/backends.md`, the local group), so it hears no ancestor's events where NVDA's scope hears some, as on the taskbar.

## Method

The program is the client; every registration is for name changes, caching the sender's name and class name.

- `provider`: in the provider program's tree node 5's parent is node 1, whose parent is the root. Registrations on node 5 with each of `TreeScope_Parent`, `TreeScope_Ancestors`, element and parent, and element and ancestors, each made directly (`AddPropertyChangedEventHandlerNativeArray`) and as an event handler group; and the control, element scope on node 1. Node 1 and then the root are renamed, raising their name changes. Once the control has heard node 1's change, the program waits 0.5 s more for any other delivery, since the one worker delivers them in turn.
- `taskbar`, run only when named, since it waits for the clock's next minute: the taskbar's clock button (class `SystemTray.OmniButton` under `Shell_TrayWnd`), whose name changes each minute. Registrations on its first child in the raw view, one with ancestors scope and one with element and ancestors scope, and the control on the button itself, element scope, made anew each trial. Each trial waits up to 62 s for the minute, then 0.5 s more.

Judged: the count of events each registration in `provider` heard, and the ancestors-scoped registration's in `taskbar` (deterministic); the minutes in which the element-and-ancestors registration heard the button, a rate against 8e3ef116's 1 of 2.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The clock button's child, ancestors scope, did not hear the button's name change; a registration on the button did; mockapp the same, as a group and directly | Before 2026-11-04 | Build 26200 | `docs/parity/backends.md`, the local group; the probes were not kept | Deterministic |
| First uia-facts run, `provider`: the control heard node 1's change; none of the eight ancestor-scoped registrations heard node 1's or the root's | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |
| First uia-facts run, `taskbar`: the button's registration heard its change at the minute; the child's, ancestors scope, none | 2026-11-06 | As above | As above | Pass |
| The child's registration with element and ancestors scope heard the button's name change in 1 of 2 runs | 2026-11-06 | As above | 8e3ef116's commit message | A rate |
| `taskbar --trials 4`: the button's registration heard each minute, twice; the child's with ancestors scope none; with element and ancestors scope the button's change once in each of 4 minutes | 2026-11-06 | As above | d0a8d430 | Ancestors none; element and ancestors 4 of 4, inconclusive against 1 of 2 |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\ancestors-cannot-be-cached-or-listened-to.exe`, which runs `provider`; `--case taskbar --trials 4` runs the clock case for four minutes. `taskbar` needs a signed-in session with the taskbar's clock shown, and waits for the clock's next minute; neither case takes over the desktop.

## Evidence kept

None: the original probes were not kept.
