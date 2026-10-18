# Experiments: a hung window named in a system focus event holds a process's UIA events

These experiments establish the fault recorded in `docs/uia-bugs.md` under [A hung window named in a system focus event holds a process's UIA events](../../docs/uia-bugs.md#a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events). Three series ran on Dickson's x64 machine (Windows 11, build 26200), in debug builds:

- 2026-10-10: the mockapp experiments E-delivery, E-hung-proxy, `e_mapping`, `e_hung_cause` and `e_cross`, whose code is in [`../mockapp-experiments-2026-10-10/`](../mockapp-experiments-2026-10-10/readme.md) (commits 8b491dc and b0a1989). `docs/design/focus-pipeline.md`, sections 11 and 12.11, holds their account.
- 2026-10-18 morning: P4, in `p4/`, one of the option (c) prototype experiments ([`../focus-events-cannot-be-limited-to-one-application/`](../focus-events-cannot-be-limited-to-one-application/readme.md) describes the prototype).
- 2026-10-18 later: `h1` to `h6`, at the top of this folder, which found the mechanism and what bounds it.

Terms used here:

- A focus WinEvent is `EVENT_OBJECT_FOCUS`; a focus registration is any UIA registration that includes focus events, desktop-wide (`AddFocusChangedEventHandler`) or in an event handler group scoped to one window.
- The hold is the time from an application raising a UIA event to a client process receiving it, beyond the baseline (1.7 to 2.0 ms in `h1` to `h6`).
- TV is mockapp's comctl32 tree view (MSAA backend, which UIA reads through its proxy), stalled with mockapp's `stall` command, a sleep on its window thread. B is a mockapp with the UIA backend. D is the client process under test.
- The connection timeout and the transaction timeout are `IUIAutomation2::ConnectionTimeout` and `TransactionTimeout`; their defaults are 2,000 and 20,000 ms.

## The October experiments (2026-10-10)

Each ran once, in a process of its own on a desktop of its own, from `crates/mockapp/tests/experiments.rs`. Its clients were made with `verbatim_uia::Uia::new()`, which then set the connection timeout to 10 seconds for the whole process; this explains the long holds below.

- E-delivery: a group-scoped focus registration on two mockapps' windows and a name-change registration on B's, in one test process. With mockapp A's provider held while raising its own focus event (the hold caught UIA's `ProviderOptions` call inside A), B's focus event reached the handler 1 ms after B raised it and B's name change 0 ms after; A's focus event arrived once A was released. A held server-side provider holds only its own events.
- E-hung-proxy (`e_hung_proxy`, run twice): P1, mockapp's tree view, stalled 15 s, then `EVENT_OBJECT_NAMECHANGE` and `EVENT_OBJECT_FOCUS` raised for its tree view; P2, a UIA mockapp, raised its focus 200 ms later. P2's focus reached the test process's scoped handler 14,800 ms after it was raised, at the end of P1's stall; P2's name change never arrived within 25 s at the desktop-wide property registration. A read of P2 on another thread with its own client took 1 ms throughout. With both timeouts set to 1 second: identical, 14,803 ms (`h3` later found why: a later client creation set 10 s again). Controls: with P1 answering, P2's focus arrived in 1 ms; with P1 hung and no WinEvent naming it, in 0 ms.
- `e_mapping`: the client's proxy factory mapping, after Verbatim's own clearing, maps focus only in two entries, both for class `SysListView32`, from `EVENT_OBJECT_FOCUS`. Clearing it is per client object: another client on another thread, a client created afterwards and another process still mapped focus. With the focus entries removed the stall was unchanged: 14,789 ms against 14,801 ms.
- `e_hung_cause`, an 8-second stall, case by case: a focus WinEvent for the hung tree view held another application's focus event 7.8 s with the desktop focus handler, with only a group-scoped focus registration, and with the focus mapping cleared; a name-change WinEvent held nothing (2 to 3 ms); with no focus registration in the process, a focus WinEvent held nothing (another application's name change arrived in 1 ms), and adding a desktop focus handler made the same name change wait 7.8 s.
- `e_cross`: a listener stand-in process (the test binary started again) held the desktop focus registration, a name registration on B's window, and out-of-context focus and foreground hooks for every process; the test process, an outpost stand-in, held no focus registration, only B's name and text-changed registrations, explicit reads, and a process-scoped hook on an MSAA mockapp. With the tree view hung and named in a focus WinEvent, the outpost stand-in received B's name change in 1 ms, its text change in 0 ms, an explicit read in 0 ms and its hook's name change in 0 ms, as in the baseline; the listener stand-in received its UIA name event 9,697 to 9,699 ms late, while its MSAA focus and foreground hooks delivered in 0 ms.

## P4 (2026-10-18, `p4/`)

TV stalled for 8 s. During the stall the script raised `EVENT_OBJECT_FOCUS` naming the tree view, and 200 ms later B raised a focus event and a name change and mockapp A raised a focus event. Two runs, both alike:

- O_A, a process whose only registration was a focus group scoped to A, received A's focus late, 4,054 and 4,070 ms after the stall began.
- O_B, a focus group on B plus B's name changes, received both late by the same amount.
- N_B, which had B's name changes and no focus registration, received the name change 1 ms after it was raised.
- Delivery resumed about 4.05 s into the stall, not at its end: the prototype client set no timeouts, so the hold was twice the 2-second default.

Holding any focus registration, even one scoped to another application's window, exposes all of a process's UIA event delivery.

## h1 to h6 (2026-10-18)

The usual setup: TV stalled; D holds a desktop focus handler and a name-change registration on B; at the start of the stall a focus WinEvent (`OBJID_CLIENT`, `CHILDID_SELF`) names TV's tree view; 200 ms later B renames `btn1`. Every process ran on the desktop `protohold`, never on the user's desktop; no window took the foreground and no key was injected.

- `h1_base` (`h1_stacks.py`): thread dumps of D, idle, at 0.6, 2.0 and 3.3 s into the hold, and after it, by a non-invasive `cdb -pv` attach. UIA's client runs all event work in the process on one `uiautomationcore!WorkItemQueue` thread (thread 12 in the dumps). During the hold that thread was in `FocusTracker` handling the WinEvent (`WinEventTracker::CallbackWorkItem::DoWork`, `FocusTracker::WinEventThreadProcCallback`, `HandleFocusChange`, `GetFocusedElementFromWinEvent`, `ClientProxyManager::GetCompleteProxyNode`); the other UIA threads were idle.
- `h2_base` (`h2_trace.py`, `h2_trace.cdb`): breakpoint traces of every `SendMessageTimeoutW`. The focus work item made two calls in turn to the hung window: `WM_GETOBJECT` with lParam -25 (`UiaRootObjectId`), flags `0x22` (`SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT`) and timeout 2,000, which returned 0 after 2.01 s with last error `ERROR_TIMEOUT`; then the registered message `0xC199`, `UiaServerConnection_HookMessage`, with flags 0 and timeout 2,000, after which the next work item started 2.01 s later. B's name change was then delivered, 3.82 s after it was raised; building its sender also sent `CB_GETCOMBOBOXINFO` to the event source's own window with the transaction timeout, so a hung event source would hold the queue there too (not tested). `h2_first500_then10000` traces the same with the connection timeout set to 500 and then 10,000 ms; `h2_cui` with the older `CUIAutomation` coclass, whose calls are `WM_GETOBJECT` with flags 0 and 5,000 ms, then `UiaServerConnection_HookMessage` with timeout 0, which waited until the window answered.
- `h3_*` (`h3_variants.py`; `H3_REPEAT` sets the number of WinEvents): the hold by client variant, each variant's output `h3_<variant>.out`, each in an 8-second stall unless named:
  - connection timeout 500 ms (`cto500`): 812 ms; both timeouts 500 ms (`to500_1`): 809 ms; the default (`default_1`): 3,817 ms; the default with the WinEvent 6 s into a 12-second stall (`default_hungfirst`): 3,821 ms;
  - both timeouts 10,000 ms (`to10000_1`): 7,773 ms, the end of the stall; in a 20-second stall (`to10000_stall20`): 19,767 ms; with the WinEvent 6 s into a 20-second stall (`to10000_hungfirst`): 13,765 ms, the end of the stall; Verbatim's own client settings (`verbatimlike`: 10,000 ms, coalescing and recovery on): 7,769 ms;
  - transaction timeout 500 ms only (`h3_tto500`): 3,810 ms;
  - the timeout settings of several client objects in one process (`first500_then10000` 1,820 ms, `first10000_then500` 7,771 ms, `namefirst500_focus10000` 7,760 ms, and 500 ms set and then a client object created that sets nothing, `set500_thenplainclient`, 816 ms): the hold followed the last value set, whichever client object held the focus registration, and a later client object that sets nothing does not reset it;
  - three focus WinEvents at 1,000 ms (`repeat3_cto1000`): 5,814 ms; one (`repeat1_cto1000`): 1,824 ms;
  - each client object holding a focus registration runs its own focus tracking on the same thread: two client objects, each with a desktop handler, made four calls (1,820 ms at 500 ms; which output records this case was not noted), while two handlers on one client object made two (`twodesk_500`, 807 ms); focus on a separate client object on the same thread (`focusnew500_namemain`) 815 ms, or on a thread of its own (`focusthr500_namemain`) 817 ms; the name registration on a thread of its own (`focusmain_namethr500`) 819 ms;
  - a single-threaded apartment (`sta`) 3,809 ms; connection recovery off and on (`recovery0`, `recovery1`) 3,821 and 3,818 ms; coalescing off and on (`coalesce0`, `coalesce1`) 3,819 and 3,814 ms; an empty cache request (`desknone`) 3,806 ms; a group-scoped focus registration in place of the desktop handler (`grouponly`) 3,809 ms;
  - the proxy factory mapping emptied (`clearmap`, `ClearTable`): 1,812 ms, one call removed;
  - `CUIAutomation` (`cui`): 7,772 ms, to the end of the stall; `cui_cto500` failed at once, because that coclass's objects have no `IUIAutomation2` (`E_NOINTERFACE`), so its timeout cannot be set.
- `h4_*` (`h4_ishung.py`): `IsHungAppWindow` on the stalled window each second for 9 s answered 0 throughout, and also after a `WM_NULL` was posted to it (`h4_ishung_post`). Windows did not flag it hung, presumably because no input reaches a hidden desktop, so `SMTO_ABORTIFHUNG` never took effect. The column for the tree view's own window used a class name that matched nothing; only the top-level column counts.
- `h5_*` (`h5_read.py`): an explicit read (`ElementFromHandle` and `CurrentName`) of the stalled window on D's main client object. Default: `UIA_E_TIMEOUT` after 2,014 ms; main object at 500 ms: 514 ms; main object at 500 and another client object in the process then set to 10,000 (`main500_other10000`): 10,013 ms; the reverse (`main10000_other500`): 508 ms. Each read of the answering window took 13 to 16 ms. [`../the-connection-timeout-applies-to-the-whole-process/`](../the-connection-timeout-applies-to-the-whole-process/readme.md) uses these as its evidence.
- `h6_*` (`h6_lost.py`): an MSAA mockapp raised its focus from inside a 3-second stall. With the connection timeout at 10,000 ms the focus arrived 3,003 ms into the stall; at 500 ms and at the 2,000 ms default it was never delivered within 8 s. Focus raised while the application answered arrived in 10 to 12 ms in every case.

## Established and inferred

Established, on this machine: in a process holding any UIA focus registration, a focus WinEvent naming a hung window that UIA proxies holds all of that process's UIA event delivery, because UIA's focus tracking makes two `SendMessageTimeoutW` calls to the window on the process's one event thread, each waiting the connection timeout; the hold is twice the connection timeout per focus WinEvent and per client object holding a focus registration, or until the window answers. Name-change WinEvents, processes with no focus registration, explicit reads on other threads and MSAA hooks are not held. No client setting tested removes it.

Inferred, not measured: that there is one event thread per process rather than per client object (seen with up to two client objects); and that for a window Windows does flag hung, the first call returns at once and the second still waits one connection timeout.

## Files

- `focusproto/`: the prototype tool as `h1` to `h6` ran it (`main.rs`, and its manifest as `Cargo.toml.txt` so that the workspace does not take it for a crate). It is the option (c) prototype extended with client flags: `to=`, `cto=` and `tto=` set the timeouts, `coalesce=` and `recovery=` the `IUIAutomation6` options, `cui` uses `CUIAutomation`, `sta` joins a single-threaded apartment, `clearmap` empties the proxy factory mapping; a spec `new@<flags>+<spec>` registers on a separate client object on the same thread and `thr@<flags>+<spec>` on one with a thread of its own; `read <hwnd>` times an explicit read.
- `proto.py`: the shared helpers, including `stacks()`.
- `h1_stacks.py` and `h1_base/`; `h2_trace.py`, `h2_trace.cdb`, `h2_base/`, `h2_first500_then10000/` and `h2_cui/`; `h3_variants.py` and `h3_*.out`; `h4_ishung.py` and `h4_*.out`; `h5_read.py` and `h5_*.out`; `h6_lost.py` and `h6_*.out`.
- `p4/`: `p4_hung.py`, its outputs `p4_run1.out` and `p4_run2.out`, and the `proto.py` it ran with, which uses the earlier prototype in `../focus-events-cannot-be-limited-to-one-application/focusproto/`.

## Rerunning

`focusproto` was built beside `mockapp` in a scratch worktree of 01254292. To rebuild it, copy `focusproto/` to `crates/focusproto` in a scratch worktree, rename `Cargo.toml.txt` to `Cargo.toml`, move `main.rs` under `src/`, and run `cargo build -p focusproto -p mockapp`; then set `BIN` in `proto.py` to the build's output folder (it was `target\proto-hold\debug`, and `target\proto-c\debug` for `p4/`) and run a script with `python -I`. Every process runs on a desktop of its own, so the user's desktop is not touched. The October experiments are rerun as [their folder](../mockapp-experiments-2026-10-10/readme.md) says.
