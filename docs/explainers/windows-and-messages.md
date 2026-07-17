# Windows, messages, hooks, and winevents

Win32 UI is built on windows (`HWND`s) exchanging messages. Accessibility
clients live off this substrate: they resolve events to windows, send
messages to fetch data, and receive notifications through hooks. This file
covers exactly the parts a screen reader touches.

## Windows and window classes

Every on-screen UI element of the classic world is a *window*: a kernel-side
object identified by an `HWND`, owned by the thread that created it, arranged
in a parent/child hierarchy per top-level window, with a *window procedure*
(message handler) attached. A *window class* names the procedure plus
defaults; `GetClassName` on an `HWND` returns strings like `"Button"`,
`"SysListView32"`, `"Chrome_RenderWidgetHostHWND"`, `"#32768"` (the built-in
menu popup class). Window class names are the primary heuristic every screen
reader uses to decide how to treat a window — which API to prefer, which
overlay behavior to apply. Modern frameworks (WPF, UWP, Chromium) draw all
their content inside one big `HWND`, so the window tree tells you nothing
about their internals; that is precisely the gap the accessibility APIs fill.

Useful inspection functions you will meet constantly: `GetForegroundWindow`,
`GetGUIThreadInfo` (per-thread focus/caret/menu state — works cross-process),
`GetWindowThreadProcessId`, `GetAncestor`, `EnumChildWindows`,
`IsWindowVisible`, `GetWindowText`.

## Messages and the message loop

A thread that creates windows must run a *message loop*: repeatedly call
`GetMessage`, `TranslateMessage`, `DispatchMessage`. Two delivery paths
matter:

- `PostMessage` — enqueue and return; asynchronous.
- `SendMessage` — synchronous call of the window procedure. Same thread:
  a direct function call. **Cross-thread: the sender blocks until the
  receiving thread retrieves and processes the message.**
  [Microsoft's own documentation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagew)
  is blunt: "The sending thread is blocked until the receiving
  thread processes the message." A hung receiver hangs the sender, no
  timeout. This is hang mechanism number one for accessibility clients
  (COM-into-STA, [COM](com.md), is the same failure shape one layer up — in fact
  it is delivered via this mechanism).
- `SendMessageTimeout` — the survivable variant: a timeout in milliseconds,
  plus `SMTO_ABORTIFHUNG` to fail fast if the target is already marked
  not-responding by the system. Screen readers should never use bare
  cross-process `SendMessage`; NVDA goes further and hooks its *own* process
  so stray `SendMessage` calls become cancellable timeouts
  ([Main loop and watchdog](../nvda/main-loop-and-watchdog.md)).

While a thread is blocked in `SendMessage` it will still process *incoming*
sent (nonqueued) messages — the same re-entrancy hazard as COM's STA
pumping: your thread can run other code while "blocked."

Messages a screen reader cares about by number: `WM_GETTEXT` (fetch a
window's text cross-process), `WM_GETOBJECT` (the system sends this to a
window when an accessibility client asks for its object — implementers of
MSAA/IA2/UIA answer it), control-specific protocols (`LVM_*` list view,
`TVM_*` tree view, `EM_*` edit), and `WM_APPCOMMAND`.

## Hooks

`SetWindowsHookEx` installs system-wide or per-thread callbacks on input and
message processing. Two flavors matter here:

- **Low-level keyboard/mouse hooks** (`WH_KEYBOARD_LL`, `WH_MOUSE_LL`): run
  in the *installing* process — no injection — called synchronously for
  every key event before the focused app sees it, may swallow events. This
  is how NVDA (and Verbatim) intercept the keyboard. The OS enforces a
  timeout (`LowLevelHooksTimeout`); a slow hook gets silently unhooked,
  which surfaces as "my modifier keys stopped being intercepted."
- **In-process hooks** (`WH_GETMESSAGE`, `WH_CALLWNDPROC` on another
  thread): the hook procedure must live in a DLL, and the system *loads
  that DLL into the target process*. This documented side effect is the
  standard, supported code-injection mechanism used by screen readers to
  get in-process (see [Process injection](../nvda/process-injection.md)). It only works
  matching bitness/architecture, and does not reach higher-integrity or
  AppContainer processes without the privileges in
  [Processes and security](processes-and-security.md).

## WinEvents

`SetWinEventHook` is the accessibility notification channel of the
MSAA/IA2 world — a separate mechanism from `SetWindowsHookEx` despite the
name. You register for an event-ID range (`EVENT_OBJECT_FOCUS`,
`EVENT_OBJECT_NAMECHANGE`, `EVENT_SYSTEM_FOREGROUND`,
`EVENT_SYSTEM_MENUPOPUPSTART`, …) and receive callbacks identifying the
source as a triple: `HWND`, object ID (`OBJID_CLIENT`, `OBJID_SYSTEM`, …),
child ID. Crucially the callback carries *no data* — just the identity; the
client must call `AccessibleObjectFromEvent` and then query properties,
paying the cross-process round trips of [COM](com.md) after every event it cares
about.

Delivery mode is chosen at registration:

- **In-context** (`WINEVENT_INCONTEXT`): your callback DLL is injected into
  every process raising events, and runs there, synchronously. Fast, and
  the only mode where you can touch in-process data; requires all the
  injection caveats.
- **Out-of-context** (`WINEVENT_OUTOFCONTEXT`): the system posts events to
  your own thread — the callback runs at your leisure, *but only when your
  registering thread pumps messages* (delivery rides the message loop), and
  events are asynchronous: by the time you query the source, the UI may
  have moved on, or the window may be gone. Slow-but-safe; this is what an
  uninjected client uses.

There is no filtering beyond the event-ID range and an optional process ID:
a busy application (a terminal repainting, a progress bar, a spreadsheet
recalculating) can raise thousands of events per second, and an
out-of-context client both wades through the backlog and pays a synchronous
query per event it acts on. Event floods are hang-adjacent mechanism number
two, and every serious MSAA client rate-limits and coalesces
([MSAA and winevent handling](../nvda/msaa.md) documents NVDA's limiter precisely).

## References

- [SendMessageW remarks (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagew)
- [About Messages and Message Queues (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winmsg/about-messages-and-message-queues)
- [SetWindowsHookExW (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowshookexw)
  and [SetWinEventHook (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook)
- [In-Context and Out-of-Context Hook Functions (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winauto/in-context-and-out-of-context-hook-functions)
