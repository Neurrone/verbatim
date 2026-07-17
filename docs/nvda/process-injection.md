# nvdaHelper and process injection

`nvdaHelper/` is NVDA's C++ layer: one DLL loaded into NVDA itself
(`nvdaHelperLocal`), one that spreads into every UI process on the
desktop (`nvdaHelperRemote`), COM proxy DLLs, and the UIA remote-ops
DLL. The in-tree `nvdaHelper/readme.md` is accurate and current; this
file adds the mechanics it glosses over.

## The local side

`nvdaHelperLocal.dll` is loaded into NVDA (Python binding:
`source/NVDAHelper/__init__.py` and `localLib.py`). It provides:

- Client stubs for calling *into* app processes (the `nvdaInProcUtils`,
  `vbufBackend`, and `displayModel` RPC interfaces served by the
  injected side), and server stubs for calls coming *back*
  (`nvdaController` — the public "speak this" API any app can call —
  and `nvdaControllerInternal`, the private channel carrying live
  regions, typed characters, IME composition/candidate updates, input
  language changes, display model changes, virtual buffer change
  notifications, log messages, focus-rect draws;
  `NVDAHelper.initialize` wires each RPC entry point to a Python
  callback via `_setDllFuncPointer`).
- The cancellable `SendMessage` machinery and NVDA's own import-table
  hooks for it ([Main loop and watchdog](main-loop-and-watchdog.md)).
- The UIA event rate limiter ([The UIA client](uia.md)) and text utilities.

RPC transport is `ncalrpc` (local-only MS-RPC), endpoint name
`NvdaCtlr.<desktop-specific namespace>` — the namespace string
encodes session and desktop so secure-desktop instances get their own
channel (`nvdaHelper/remote/injection.cpp`, `DllMain`).

## How injection actually happens

Two stages, both in `nvdaHelper/remote/injection.cpp`:

1. NVDA loads `nvdaHelperRemote.dll` and calls `injection_initialize`,
   which starts a thread registering two **in-context winevent hooks**
   (`SetWinEventHook(..., WINEVENT_INCONTEXT)`) for
   `EVENT_OBJECT_FOCUS` and `EVENT_SYSTEM_FOREGROUND`
   (`outprocMgrThreadFunc`). In-context hooks mean Windows itself maps
   the DLL into any process that raises those events — so *every app
   the user focuses gets the DLL*, lazily, with no `CreateRemoteThread`
   or other aggressive technique.
2. Inside the target process, the first hook callback starts the
   in-process manager thread (`inprocMgrThreadFunc`), which:
   - registers per-thread `WH_GETMESSAGE` and `WH_CALLWNDPROC` windows
     hooks (`inProcess.cpp`) so in-process features can observe
     messages, and API-hooks selected functions (import/inline hooking
     via the vendored minhook; e.g. `SetWindowsHookExA` is wrapped so
     NVDA's W hooks stay ahead of other software's A hooks),
   - registers in-context winevent hooks for the events in-process
     features need (live regions, typed characters, display model),
   - registers the COM proxies the process may lack: IAccessible2 and
     ISimpleDOM (`COMProxyRegistration.cpp`, activation-context based,
     no registry writes),
   - starts the RPC servers (`remote/rpcSrv.cpp`) so NVDA can call in,
     and connects the client bindings back to NVDA's endpoint.

Bitness/architecture: `nvdaHelperRemote` is built per-arch (x86, x64,
ARM64). NVDA's process is x86; Windows only injects matching-arch hook
DLLs, so NVDA spawns tiny *remote loader* processes
(`nvdaHelper/remoteLoader/loader.cpp`; `_RemoteLoader` instances for
x86/AMD64/ARM64 in `NVDAHelper.initialize`) whose job is to register
the same in-context hooks from an executable of the right architecture.

Lifecycle care: on process exit the DLL unhooks everything from
`DllMain` `DLL_PROCESS_DETACH`; the in-process manager also watches
NVDA's liveness (so an NVDA crash doesn't leave hooks calling into a
dead endpoint), and `injection_terminate` tears down the desktop-wide
hooks when NVDA exits. Injection is disabled entirely when NVDA runs
as a Windows Store app (`NVDAHelper.initialize`).

## What runs in-process (the inventory)

Files under `nvdaHelper/remote/`, each a feature that *requires* being
inside the target process:

- `IA2Support.cpp`, `ia2LiveRegions.cpp`, `textFromIAccessible.cpp` —
  IA2 utilities and live-region speech ([IA2 usage](ia2.md)).
- `vbufRemote.cpp` plus `vbufBase/` and `vbufBackends/` — virtual
  buffer construction ([Virtual buffers](virtual-buffers.md)).
- `displayModel.cpp`, `gdiHooks.cpp` — the GDI text-capture display
  model ([The display model](display-model.md)).
- `typedCharacter.cpp`, `tsf.cpp`, `ime.cpp`, `inputLangChange.cpp` —
  typed-character echo, IME composition and candidate reporting, input
  language switch announcements ([Keyboard input](input.md)); these hook window
  messages and TSF from inside the app because the data never crosses
  the process boundary otherwise.
- `winword.cpp`, `WinWord/`, `excel.cpp`, `outlook.cpp` — running
  Office object-model queries in-process to avoid cross-process COM
  latency ([Office through COM](office-com.md)).
- `sysListView32.cpp` — bulk retrieval of list view item data (the
  `LVM_*` message protocol requires shared-memory gymnastics
  cross-process; in-process it is direct).
- `apiHooks.cpp`, `inProcess.cpp`, `nvdaController.cpp`,
  `rpcSrv.cpp` — plumbing for all of the above.

## Design properties worth noting

- Injection is *lazy and universal*: any focused process gets the DLL,
  whether or not any in-process feature will be used there. The
  in-process side is quiescent (hooks registered, nothing computed)
  until NVDA calls in or a hooked event fires.
- All in-process program state is per-process and dies with the app;
  NVDA re-injects on next focus. There is no persistence and no IPC
  between injected instances.
- Security posture: injection does not cross integrity levels or into
  AppContainer/protected processes; those apps simply run without
  in-process features (and NVDA's uiAccess flag governs what it can
  reach at all — [Processes and security](../explainers/processes-and-security.md)).
- Failure posture: MessageBox-on-error in the injection paths
  (visible in `injection.cpp`) reflects its status as
  must-never-fail plumbing; in-process crashes take the *host app*
  down with them, which is the fundamental risk of the technique (a
  browser crash caused by a screen reader's injected code is
  indistinguishable, to the user, from a browser bug).
