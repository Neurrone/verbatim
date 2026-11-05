# Ancestors cannot be cached or listened to

## Fact

An event registration scoped to an element's ancestors (`TreeScope_Ancestors` or `TreeScope_Parent`, with or without the element itself) receives no ancestor's events: on Windows 11's taskbar, a registration on the clock button's child does not hear the clock button's name change each minute, while one on the clock button itself does, and a provider's tree gives the same result, made as an event handler group and directly (`books/dev/src/reference/uia-bugs.md`, the limitation of that name). That a cache request cannot take in the parent or ancestors is Microsoft's documentation of [`IUIAutomationCacheRequest::TreeScope`](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationcacherequest-get_treescope), cited, not reproduced.

## What Verbatim relies on

Ancestors are read by a walk or a remote operation, and Verbatim registers its local group on the focus with `TreeScope_Element` alone (`docs/parity/backends.md`, the local group). A change would only reopen an optimization.

## Method

The program is the client; every registration is for name changes, caching the sender's name and class name.

- `provider`: in the provider program's tree node 5's parent is node 1, whose parent is the root. Registrations on node 5 with each of `TreeScope_Parent`, `TreeScope_Ancestors`, element and parent, and element and ancestors, each made directly (`AddPropertyChangedEventHandlerNativeArray`) and as an event handler group; and the control, element scope on node 1. Node 1 and then the root are renamed, raising their name changes. Once the control has heard node 1's change, the program waits 0.5 s more for any other delivery, since the one worker delivers them in turn.
- `taskbar`: the taskbar's clock button (class `SystemTray.OmniButton` under `Shell_TrayWnd`), whose name changes each minute. A registration on its first child in the raw view, element and ancestors scope, and the control on the button itself, element scope. The program waits up to 62 s for the minute, then 0.5 s more.

Judged: the count of events each registration heard (deterministic).

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The clock button's child, ancestors scope, did not hear the button's name change; a registration on the button did; mockapp the same, as a group and directly | Before 2026-11-04 | Build 26200 | `docs/parity/backends.md`, the local group; the probes were not kept | Deterministic |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\ancestors-cannot-be-cached-or-listened-to.exe`, or one case with `--case`. `taskbar` needs a signed-in session with the taskbar's clock shown, and waits for the clock's next minute; neither case takes over the desktop.

## Evidence kept

None: the original probes were not kept.
