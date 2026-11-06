# Ancestors cannot be cached or listened to

## Fact

An event registration scoped to an element's ancestors alone (`TreeScope_Ancestors` or `TreeScope_Parent`) receives no ancestor's events, and adding the ancestors to an element's own scope hears nothing beyond what the element's scope alone hears. In a provider's tree, registrations on a node with parent or ancestors scope, with or without the element, hear neither its parent's nor the root's name change, made as an event handler group and directly; in a tree view whose item has the keyboard focus, a registration on the focused item with element and ancestors scope (NVDA's), element scope alone (Verbatim's) or ancestors scope hears none of the tree view's name changes. On Windows 11's taskbar, a registration on the clock button's child with ancestors scope alone does not hear the button's name change, while registrations on the child with element scope alone and with element and ancestors scope hear it in the same minutes, the element and ancestors one in 11 of 12 minutes recorded; so the two scopes hear the same (`books/dev/src/reference/uia-bugs.md`, the limitation of that name). That a cache request cannot take in the parent or ancestors is Microsoft's documentation of [`IUIAutomationCacheRequest::TreeScope`](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope), cited, not reproduced.

## What Verbatim relies on

Ancestors are read by a walk or a remote operation, and Verbatim registers its local group on the focus with `TreeScope_Element` alone (`docs/parity/backends.md`, the local group), and the outcome matches NVDA's element and ancestors scope because adding the ancestors hears nothing more. If an ancestors-scoped registration began to receive them, Verbatim would miss changes NVDA hears.

## Method

The program is the client; every registration is for name changes, caching the sender's name and class name.

- `provider`: in the provider program's tree node 5's parent is node 1, whose parent is the root. Registrations on node 5 with each of `TreeScope_Parent`, `TreeScope_Ancestors`, element and parent, and element and ancestors, each made directly (`AddPropertyChangedEventHandlerNativeArray`) and as an event handler group; and the control, element scope on node 1. Node 1 and then the root are renamed, raising their name changes. Once the control has heard node 1's change, the program waits 0.5 s more for any other delivery, since the one worker delivers them in turn.
- `tree`, run only when named, since it brings a window to the front on the unlocked desktop: the provider program's tree with node 1 a tree view (control type `TreeView`) and its children tree items (`TreeItem`). The window is brought to the front and node 5, an item, takes the keyboard focus with its focus event raised. The program checks that `GetFocusedElement` answers node 5 and that the control view's parent of it is the tree view, the path a scope over ancestors needs, printed as evidence. Registrations on the focused item with element and ancestors scope, element scope and ancestors scope, and the control on the tree view, element scope; the tree view renamed and its name change raised through `UiaRaiseAutomationPropertyChangedEvent`; then 0.5 s more after the control hears it.
- `taskbar`, run only when named, since it waits for the clock's next minute: the taskbar's clock button (class `SystemTray.OmniButton` under `Shell_TrayWnd`), whose name changes each minute. Registrations on its first child in the raw view with ancestors scope, element and ancestors scope, and element scope, and the control on the button itself, element scope, made anew each minute. It watches two minutes, whatever `--trials` says, the fewest at which no minute heard would differ from the record; each waits up to 62 s for the minute, then 0.5 s more.

Judged: the count of events each registration in `provider` heard, and the ancestors-scoped registration's in `taskbar` (deterministic); in `tree`, the path, the control, and each scope's count against none; in `taskbar`, the minutes in which the element-and-ancestors registration heard the button, a rate against 19 of 20, that in each minute it heard the button exactly as often as the element registration (a relation), and the element scope's minutes, for trends.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The clock button's child, ancestors scope, did not hear the button's name change; a registration on the button did; mockapp the same, as a group and directly | Before 2026-11-04 | Build 26200 | `docs/parity/backends.md`, the local group; the probes were not kept | Deterministic |
| First uia-facts run, `provider`: the control heard node 1's change; none of the eight ancestor-scoped registrations heard node 1's or the root's | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |
| First uia-facts run, `taskbar`: the button's registration heard its change at the minute; the child's, ancestors scope, none | 2026-11-06 | As above | As above | Pass |
| The child's registration with element and ancestors scope heard the button's name change in 1 of 2 runs | 2026-11-06 | As above | 8e3ef116's commit message | A rate |
| `taskbar --trials 4`: the button's registration heard each minute, twice; the child's with ancestors scope none; with element and ancestors scope the button's change once in each of 4 minutes | 2026-11-06 | As above | 93fed1a0, and again at de138138 | Ancestors none; element and ancestors 4 of 4 each run |
| `taskbar` with the element scope beside: ancestors scope none; element and ancestors scope 2 of 2 minutes; element scope 2 of 2 | 2026-11-06 | As above | cdb6b4d6 | Pass; element scope new |
| `tree --trials 3`: the focused element node 5, its parent the tree view, 3 of 3; the control heard the rename once each; the item's element and ancestors, element and ancestors scopes none | 2026-11-06 | As above | cdb6b4d6 | Pass; the scopes new |
| Desktop runs at 8e4c9d50 by default and with each client setting: `tree` the path, the control 1, every scope on the item none; `taskbar` the button's own registration each minute, ancestors scope none, element and ancestors 2 of 2, element 2 of 2 | 2026-11-06 | As above | Dickson's desktop runs | Pass; 19 of 20 element and ancestors minutes in all |
| First desktop runs of `tree` as judged now, programs of 14e569c9 (built from f0b18b2a), by default and with each client setting: the path, the control 1, and none in every scope on the item, in each | 2026-11-06 | As above | Dickson's desktop runs | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\ancestors-cannot-be-cached-or-listened-to.exe`, which runs `provider`. `--case taskbar` watches the clock for two minutes and needs a signed-in session with the taskbar's clock shown; `--case tree` needs the unlocked desktop and brings the provider's window to the front.

## Evidence kept

None: the original probes were not kept.
