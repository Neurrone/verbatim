# Java Access Bridge

Java Swing/AWT applications implement none of the Windows
accessibility APIs; the *Java Access Bridge* (JAB) is Oracle's
translation layer, and `source/JABHandler.py` is NVDA's client for
it. The bridge itself — architecture, C API, reference lifetime — is
covered in
[The Java Access Bridge](../explainers/java-access-bridge.md); this
file is NVDA's side.

## How the bridge works

The JVM side (enabled per-user with `jabswitch -enable`, or by the
app shipping it) loads a bridge component that exports the Java
Accessibility API out of the JVM. The Windows side is
`WindowsAccessBridge-64.dll` (or -32), which NVDA loads directly
(`bridgeDll` in `JABHandler.py`) — a plain C API, not COM:
`Windows_run()` starts a hidden-window message pump the bridge uses
for its own IPC with JVMs, and thereafter NVDA calls functions like
`getAccessibleContextFromHWND`, `getAccessibleContextInfo`,
`getAccessibleTextItems`, `getAccessibleTableInfo`, receiving
filled C structs (`AccessibleContextInfo` and the other Structure
definitions at the top of the file — role and states as *strings*,
name/description, bounds, text info, table info, relations,
actions).

Object identity is a `JOBJECT64` (a JVM-side reference NVDA must
release with `releaseJavaObject` — leak-prone, and `JABHandler`
wraps contexts in `JABContext` objects managing that lifetime).
Events arrive via registered C callbacks (focus gained, caret
update, property changes…), which NVDA re-queues as NVDA events for
`NVDAObjects.JAB` objects; a `pumpAll` step in the core cycle
drains pending JAB events ([Main loop and watchdog](main-loop-and-watchdog.md) lists
`JABHandler.pumpAll`).

## NVDA specifics worth knowing

- Detection: a window is JAB-capable if the bridge reports
  `isJavaWindow(hwnd)`; NVDA checks this in API-class selection
  ([Object model](object-model.md)) before falling back to MSAA for the generic
  `SunAwtFrame` windows.
- `NVDAObjects/JAB/__init__.py` maps the string roles/states to
  `controlTypes`, implements TextInfo over the JAB text calls
  (offsets-based; [TextInfo](text-infos.md)), tables over the table structs,
  and fires the usual events.
- The bridge is chatty and synchronous: every call is IPC to the
  JVM with the same blocking hazards as everything else in this
  folder (a busy JVM stalls the caller; nvaccess/nvda issue [#16749](https://github.com/nvaccess/nvda/issues/16749)'s
  reproduction is precisely a Java app sleeping its UI thread).
- Bitness matters: the bridge DLL must match NVDA's architecture,
  and the JVM must have the bridge enabled or NVDA sees an empty
  window — the first support question for any Java-app user.
