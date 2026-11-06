# UIA's client threads and focus tracking's calls

## Fact

These are internals seen with a debugger and Microsoft's public symbols (`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events", and `books/dev/src/explanation/windows/uia.md`, "Events into the client"):

- A client process's first registration, or the element lookup before it, starts UIA's WinEvent thread (`WinEventManager::ThreadProc`), its I/O thread (`OverlappedIOManager::StaticIoThreadProc`) and a `NavigationHelperProxyOneCore` thread (`s_CoreUIThreadProc`). The I/O thread reads each event and queues it to a work-item thread (`WorkItemQueue`), which it creates when the first event arrives (`threads_name`, `threads_desk`). What the `NavigationHelperProxyOneCore` thread does is not known.
- The work-item thread runs every event of the process in turn, and UIA's focus tracking on it: for a focus `WinEvent`, `WinEventTracker::CallbackWorkItem::DoWork`, `FocusTracker::HandleFocusChange`, `GetFocusedElementFromWinEvent` and `ClientProxyManager::GetCompleteProxyNode` (`h1_base`).
- Focus tracking's two calls to the named window: `SendMessageTimeoutW(hwnd, WM_GETOBJECT, wParam, UiaRootObjectId, SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT, connection timeout)` from `HwndUtils::SendWmGetObject` under `ProviderEntryPoint::GetNativeUiaNode`; then, only when that finds no provider of the window's own, `SendMessageTimeoutW(hwnd, RegisterWindowMessageW("UiaServerConnection_HookMessage"), 0, lParam, 0, connection timeout)` from `PerformOperationOnHwndThread` under `ProviderEntryPoint::GetProxyUiaNode` (`calls_focus`, `calls_reg`). Registering a desktop focus handler makes the same pair of calls to top-level windows. That `ElementFromHandle` sends `WM_GETOBJECT` the same way is inferred.
- Focus for a newly opened or shown window is raised on a thread-pool thread (`ClientEventManager::OpenShowWinEventCallbackThreadProc`, then `EventManager::AssociateWithRecord` and `RaiseLocalEvent`), not on the work-item thread (`route1`, `route2`).

The binary of [the hung-window fact](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md) checks the observable outcomes: the hold, the two messages a stalled window receives one connection timeout apart, and the open-and-show route.

## What Verbatim relies on

No code; the observable outcomes are checked by the hung-window and flood folders.

## Method

A guide, with no program of its own: each trace is taken on a run of the hung-window program paused under `--pause`, which prints each process id and waits for Enter.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `h1_base`: thread dumps idle, at 0.6, 2.0 and 3.3 s into a hold, and after it; the work-item thread in focus tracking | 2026-10-18 | Build 26200 | `recorded/h1_base/` | The frames below |
| `h2_base`, `h2_first500_then10000`, `h2_cui`: breakpoint traces of every `SendMessageTimeoutW` during a hold | 2026-10-18 | Build 26200 | `recorded/h2_base.out`, `recorded/h2_first500_then10000.out`, `recorded/h2_cui.out` | The calls below |
| `calls_reg`, `calls_focus`: the two calls with their stacks and flags; the registered message named `UiaServerConnection_HookMessage` (0xC1AA that boot) | 2026-11-01 | Build 26200 | `recorded/run_focus.txt`, `recorded/run_focus2.txt` | The calls below |
| `threads_name`, `threads_desk`: the threads a registration creates | 2026-11-01 | Build 26200 | `recorded/run_name.txt`, `recorded/run_desk.txt` | The threads above |
| `route1`, `route2`: the menu's focus on a thread-pool thread while the work-item thread is held | 2026-11-01 | Build 26200 | `recorded/focusthr.txt`, `recorded/focusthr2.txt` | The frames above |
| `q3`'s stacks: the work-item thread in `HwndUtils::SendWmGetObject` under `FocusTracker::HandleFocusChange` during a slow provider's hold | 2026-11-01 | Build 26200 | `recorded/eventthread-stacks/` | The frames above |

## How to rerun

`uia-facts/build.ps1`. The debugger is cdb, the WinDbg app's `cdbX64.exe` (`books/dev/src/how-to/debugging.md`, "Where cdb is"). Symbols: `-y srv*C:\symbols*https://msdl.microsoft.com/download/symbols`.

## Debugger guide

The threads a registration creates (`threads_name`, `threads_desk`):

1. Run `target\debug\a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events.exe --case calls --pause`. It prints the process ids of P1, P2 and client D, which has already registered, and waits.
2. For the threads of a registration from its start, attach before the client registers: start a listening client by hand under cdb, `cdb -y <symbols> -c "bp kernelbase!CreateThread \"k 20; gc\"; g" target\debug\a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events.exe --role listen name=<B's handle>`, B's handle being P2's window from the run's `--verbose` output.
3. Expect, from the registration: `NavigationHelperProxyOneCore::s_CoreUIThreadProc` and `OverlappedIOManager::StaticIoThreadProc` from the `ElementFromHandle` before it, and `WinEventManager::ThreadProc` from `WinEventManager::Init`; and, only once the first event arrives, `WorkItemQueue::StaticThreadProc`, created from `OverlappedIOManager::IoThreadProc`, `LocalEventCallbackStub::OnMessage`, `WorkItemQueue::AddItem`.

Focus tracking's calls (`calls_focus`) and the work-item thread (`h1_base`):

1. Run the same with `--pause`; at the pause, attach to client D: `cdb -p <client D> -y <symbols> -c "bp user32!SendMessageTimeoutW \".printf \\\"msg %x lParam %p flags %x timeout %d\\\\n\\\", @rdx, @r9, poi(@rsp+28), poi(@rsp+30); k 20; gc\"; g"`, then press Enter in the run.
2. Expect two hits on D's `WorkItemQueue` thread after the focus `WinEvent`: `WM_GETOBJECT` (0x3d) with lParam `0xffffffffffffffe7` (`UiaRootObjectId`), flags 0x22 and timeout 2,000, under `HwndUtils::SendWmGetObject`, `ProviderEntryPoint::GetNativeUiaNode`, `FocusTracker::HandleFocusChange`; then, about 2 s later, a registered message (`0xC000` or above) with flags 0 and timeout 2,000, under `PerformOperationOnHwndThread`, `ProviderEntryPoint::GetProxyUiaNode`, `UiaNodeFactory::InProcProxyFromHwnd`. `GetClipboardFormatNameW` of its number names it `UiaServerConnection_HookMessage`.
3. For `h1_base`'s dumps, attach non-invasively during the hold instead, `cdb -pv -p <client D> -y <symbols> -c "~*k 30; qd"`, and expect the work-item thread in `WinEventTracker::CallbackWorkItem::DoWork`, `FocusTracker::WinEventThreadProcCallback`, `HandleFocusChange`, `GetFocusedElementFromWinEvent`, `ClientProxyManager::GetCompleteProxyNode`, and the other UIA threads idle.

The older `CUIAutomation` coclass's calls (`h2_cui`), which no case of the hung-window program makes, since its first wait is a fixed 5 s and its second has no timeout:

1. Start two provider programs by hand, `target\debug\uia-facts-provider.exe --windows plain:0` for T and `target\debug\uia-facts-provider.exe --windows uia:0` for B; each prints `window 0 <kind> <handle> <thread>` and `ready <process id>`, then reads commands on its input.
2. Start a listening client with the older coclass under cdb: `cdb -y <symbols> -c "bp user32!SendMessageTimeoutW \".printf \\\"msg %x lParam %p flags %x timeout %d\\\\n\\\", @rdx, @r9, poi(@rsp+28), poi(@rsp+30); k 20; gc\"; g" target\debug\a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events.exe --role listen older desk name=<B's handle>`, and wait for its `ready` line.
3. In T's provider type `busy 0 8000`, then `winevent focus 0`; in B's, `rename 0 1 During`, then `raise name 0 1`.
4. Expect on the client's `WorkItemQueue` thread `WM_GETOBJECT` (0x3d) with lParam `UiaRootObjectId`, flags 0 and timeout 5,000, under `HwndUtils::SendWmGetObject` and `FocusTracker::HandleFocusChange`; 5 s later the registered message with flags 0 and timeout 0, under `PerformOperationOnHwndThread` and `ProviderEntryPoint::GetProxyUiaNode`; and B's name change delivered only once T's 8 s stall ends.

The open-and-show route (`route1`, `route2`): run `target\debug\focus-for-a-newly-opened-or-shown-window-is-not-held.exe --case open-show --pause`, which needs the unlocked desktop; attach to the client with breakpoints on `UIAutomationCore!FocusTracker::HandleFocusChange` and on the program's `HandleFocusChangedEvent` with `k 20`, and expect the shown window's focus events under `ClientEventManager::OpenShowWinEventCallbackThreadProc` on a thread-pool thread while the work-item thread is in `HandleFocusChange`.

It matches when the frames and arguments are as listed. An invasive attach changes timing; the dumps of a hold are taken with `-pv`.

## Evidence kept

The traces of the 2026-11-01 series and of `h1` and `h2`, named in "Recorded results", each cited for the frames, calls and threads listed with it.
