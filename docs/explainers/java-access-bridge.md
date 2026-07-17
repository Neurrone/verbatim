# The Java Access Bridge

Java Swing/AWT applications implement none of the Windows accessibility
APIs ([The accessibility landscape](accessibility-landscape.md)); the
JVM has its own Java Accessibility API, and the *Java Access Bridge*
(JAB) is the translation layer that exports it to Windows assistive
technologies. This file covers the bridge as a technology; NVDA's
client is in [Java Access Bridge](../nvda/java-access-bridge.md).

## Architecture

Three pieces:

- **JVM side**: an accessibility support component the JVM loads,
  disabled by default. Enabled per user by `jabswitch -enable` (writes
  `%USERPROFILE%\.accessibility.properties`), machine-wide via
  `accessibility.properties`, or by the app bundling its own
  enablement. If it is off, the app is a black box — the first support
  question for any Java app.
- **Windows side**: `WindowsAccessBridge-64.dll` (and `-32`, matching
  the *client's* bitness), which an assistive technology loads
  directly into its own process. It is a plain C API, not COM.
- **Between them**: the bridge's own IPC over hidden windows and
  window messages — invisible to the client, but the reason a call to
  a busy JVM blocks the caller just like every other cross-process
  accessibility call ([COM](com.md) round-trip caveats apply in
  spirit, minus COM itself).

## The client model

After loading the DLL, the client calls `Windows_run()` once — this
starts the bridge's internal message plumbing, and requires the
calling thread to pump messages. From there:

- Windows are matched with `isJavaWindow(hwnd)`; the entry point is
  `getAccessibleContextFromHWND`, yielding an *AccessibleContext* — an
  opaque JVM-side object reference (`JOBJECT64`).
- **Everything is struct-filling C calls**: `getAccessibleContextInfo`
  fills a struct with name, description, *role and states as localized
  strings* (not enums — a design wart clients must map back), bounds,
  child count, and capability flags; further families cover text
  (`getAccessibleTextInfo`, items, selection, attributes), tables,
  relations, actions, and hypertext. Navigation is
  `getAccessibleChildFromContext` / `getAccessibleParentFromContext`
  plus index-based child access.
- **Reference lifetime is manual**: every AccessibleContext the bridge
  hands out pins a JVM-side object until the client calls
  `releaseJavaObject`. Leaks accumulate in the *target JVM*, not the
  client — a discipline problem every JAB client must solve with
  wrapper objects.
- **Events** are registered C callbacks (`setFocusGainedFP`,
  `setPropertyChangeFP`, caret updates, menu events…), delivered on
  the client's message-pumping thread, carrying the source
  AccessibleContext (which the client must release after use).
- **Bitness**: the client loads the DLL matching its own architecture;
  the bridge translates across to whatever the JVM is. There is no
  ARM64-native Access Bridge DLL; on ARM64 Windows the practical path
  is the x64 DLL under emulation.

## Working characteristics

Fidelity is decent for standard Swing components and as good as the
app's own accessibility work for custom ones (same story as every
toolkit). Calls are synchronous IPC into the JVM's event-dispatch
thread: a busy or blocked EDT stalls the caller, with no timeout
machinery provided — budget-guard accordingly. The API is stable to
the point of fossilization (it has barely changed since Java 8;
JDK-bundled since then), which makes it a well-bounded, low-churn
target.

## References

- [Java Accessibility Guide: the Java Access Bridge API (Oracle)](https://docs.oracle.com/javase/accessbridge/2.0.2/api.htm)
- [jabswitch (Oracle JDK tool reference)](https://docs.oracle.com/en/java/javase/21/docs/specs/man/jabswitch.html)
- The header `AccessBridgeCalls.h` in any JDK's `include` directory,
  and NVDA's Python transcription in `nvda/source/JABHandler.py`.
