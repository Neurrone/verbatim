# Experiments: focus events cannot be limited to one application

These experiments establish the limitation recorded in `docs/uia-bugs.md` under [Focus events cannot be limited to one application](../../docs/uia-bugs.md#focus-events-cannot-be-limited-to-one-application). They ran on Dickson's x64 machine (Windows 11, build 26200), in debug builds: E-scope and E-cost on 2026-10-10 as mockapp experiments, and P1, P2 and P5 on 2026-10-18 with a scratch prototype. They were run to judge option (c) of the focus pipeline's design, in which each outpost registers for its own application's focus (`docs/design/focus-pipeline.md`, sections 2 and 11).

Terms used here:

- A scoped focus registration is the focus event (`UIA_AutomationFocusChangedEventId`) in an event handler group registered on one window's element with `TreeScope_Subtree`; a desktop-wide one is `AddFocusChangedEventHandler`, or the same group registered on the desktop's root element.
- mockapp A and B use the UIA backend (a provider of their own, which raises focus events itself); mockapp M uses the MSAA backend, so UIA reads it through its proxy and derives its focus events from WinEvents.
- A limit of every experiment here: on a desktop of its own, no window has the system's keyboard focus. Provider-raised focus events then reach only scoped registrations, and desktop-wide registrations receive only the focus events UIA derives from a WinEvent (M's).

## E-scope and E-cost (2026-10-10)

`e_scope` and `e_cost` in [`../mockapp-experiments-2026-10-10/experiments.rs`](../mockapp-experiments-2026-10-10/readme.md), each run once in a mockapp test process on a desktop of its own, with the `small.json` fixture.

- E-scope: `IUIAutomation::AddAutomationEventHandler(UIA_AutomationFocusChangedEventId, A's window, TreeScope_Subtree)` was refused with `E_INVALIDARG` and UIA's message "Focus events must be registered using IUIAutomation::AddFocusChangedEventHandler". The same event in an event handler group registered on A's window (`AddEventHandlerGroup`, as `verbatim_uia::Registration` does) was accepted, and delivered A's two focus events, with the sender and its cache, and not B's. A desktop-wide `AddFocusChangedEventHandler` in the same process, and a group with the focus event on the root, received none of mockapp's focus events.
- E-cost: provider calls counted inside mockapp for one focus event: with no registration, 1 (`ProviderOptions`, from raising the event); with one scoped registration and a four-property cache, 90 (4 `WM_GETOBJECT`, 39 `ProviderOptions`, 16 `HostRawElementProvider`, 10 `Navigate`, 8 `FragmentRoot`, 7 `GetPropertyValue`, 6 `GetRuntimeId`), the callback running 1.9 ms after the command; with three such registrations, each its own client, 185, the callbacks after 2.4, 1.7 and 0.9 ms. The first scoped registration, including its thread and client, took 22 to 25 ms; later ones on the same window 1 to 3 ms.

## P1: scoping (`p1_scope.py`, three runs)

- `IUIAutomation::AddAutomationEventHandler` for the focus event on a window was refused with `E_INVALIDARG` (0x80070057) and the message above, 3 of 3.
- A scoped registration on A's window received exactly A's provider-raised focus events and none of B's; the same for B; 3 of 3 runs.
- For M, a scoped registration on M's window received none of M's focus events in 3 of 3 runs, where the desktop-wide `AddFocusChangedEventHandler` received both in every run and the root group 5 of 6.

## P5: the core API (`p5_core.py`, two runs)

- `UiaAddEvent(node, UIA_AutomationFocusChangedEventId, ..., TreeScope_Subtree, ...)` on a window's node (from `UiaNodeFromHandle`) was accepted and delivered that window's provider-raised focus events, 2 of 2 in each run, and none of M's.
- On the root node it received nothing here.

## P2: what one focus event costs, by number of registered processes (`p2_cost.py`)

Provider calls counted inside mockapp per focus event, the average over 10 events, for 0, 1, 2, 4 and 8 client processes (`p2_cost.out`, summarised in `p2_cost.summary.txt`):

- Scoped to A: with an empty cache 1, 70, 117, 210 and 397 calls; with the process id cached 1, 71, 119, 214 and 405; with NVDA's base cache (thirteen properties and the Text pattern) 1, 86, 149, 274 and 525. About 47 to 49 calls per registered process with any cache, most of them `ProviderOptions`, `HostRawElementProvider`, `Navigate` and `FragmentRoot`, plus about 16 `GetPropertyValue` for the base cache. Delivery P50 was 1.4, 2.0, 3.1 to 3.4, and 5.3 to 5.6 ms.
- Desktop-wide, focus WinEvents from M: empty cache 0, 13, 15, 51 and 90; process id 0, 13, 15, 55 and 79; base 0, 16, 19, 67 and 112. About 10 to 13 calls per process.
- Not measurable here: the desktop-wide registration's cost for a provider-raised focus (no delivery without keyboard focus), and processor time (`GetProcessTimes` advances in 15.6 ms ticks, too coarse for 10 events).

P2's finding that every process after the first receives desktop-wide focus late is a fault of its own, described in [`../desktop-wide-focus-handlers-in-a-second-process-deliver-late/`](../desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md).

## Established and inferred

Established, on this machine: the documented call to limit focus events to one window is refused; two undocumented routes (a scoped event handler group, and `UiaAddEvent` on a window's node) deliver that window's provider-raised focus and no other application's, but neither received any focus of a window UIA proxies; each registered process adds about 47 to 49 provider calls per focus event on the application's UI thread.

Inferred, not shown: that a scoped focus registration cannot serve classic Win32 applications. Confirming it needs a real Win32 application with real keyboard focus.

## Files

- `focusproto/`: the prototype (`main.rs`, and its manifest as `Cargo.toml.txt` so that the workspace does not take it for a crate). `focusproto client <tag> <flags> <spec>...` is a UIA client process whose specs are `desk:<cache>` (`AddFocusChangedEventHandler`), `group:<hwnd>:<cache>` (a scoped registration), `rootgroup:<cache>`, `direct:<hwnd>:<cache>` (`AddAutomationEventHandler` for focus on that window) and `name:<hwnd>`; the cache is `none`, `pid`, `min` (process id, window handle and name) or `base`. The `hold` flag keeps every sender and the `held` command reads them live. `focusproto coreevent <tag> <hwnd|root>` registers with `UiaAddEvent`. `focusproto ondesk <desktop> <argv>` runs a process on a desktop of its own.
- `proto.py`: the shared helpers; each `pN_*.py` script writes its raw log to `pN_*.out`.
- `p1_scope.py` and `p1_scope.run1.out` to `run3.out`; `p2_cost.py`, `p2_cost.out` and `p2_cost.summary.txt`; `p5_core.py`, `p5_core1.out` and `p5_core2.out`.

The same prototype ran P3 ([elements cannot be passed to another process](../elements-cannot-be-passed-to-another-process/readme.md)), P4 ([the hung-window fault](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md)) and P2's delivery timings ([late delivery](../desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md)).

## Rerunning

`focusproto` was built beside `mockapp` in a scratch worktree of 01254292, into `target\proto-c`. To rebuild it, copy `focusproto/` to `crates/focusproto` in a scratch worktree, rename `Cargo.toml.txt` to `Cargo.toml`, move `main.rs` under `src/`, and run `cargo build -p focusproto -p mockapp`; set `BIN` in `proto.py` to the build's output folder and run a script with `python -I`. Every process runs on the desktop `protoc`, never on the user's desktop; no window takes the foreground and no key is injected. E-scope and E-cost are rerun as [their folder](../mockapp-experiments-2026-10-10/readme.md) says.
