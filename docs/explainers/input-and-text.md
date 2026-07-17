# Keyboard input and text entry on Windows

A screen reader sits between the keyboard and every application: it must
intercept gestures meant for itself, pass everything else through
faithfully, and *announce* what typing produces. This file gives the
minimum model of Windows input and text entry needed to reason about that.

## From key press to message

A hardware key press produces a *scan code* (position on the physical
keyboard), which the current *keyboard layout* maps to a *virtual key*
(`VK_*`: layout-independent meaning like `VK_F1`, `VK_LEFT`, `VK_A`), which
`TranslateMessage` in the focused app's message loop may further cook into
`WM_CHAR` characters using layout, shift states, and dead keys. Key events
arrive in the focused thread's queue as `WM_KEYDOWN`/`WM_KEYUP` (plus
`WM_SYSKEY*` variants when Alt is involved).

Three facts with screen reader consequences:

- Layouts are per-thread, switchable at runtime (`WM_INPUTLANGCHANGE`), so
  "what character does this key make" has no global answer; NVDA announces
  layout changes from *inside* the app because only there is the change
  visible at the right moment.
- Modifier state is queryable (`GetKeyState` in-thread,
  `GetAsyncKeyState` globally) but racy by nature; serious interception
  tracks modifier state itself from the hook stream.
- Some keys never reach apps (Win+L and other system-reserved chords), and
  hooks see others only partially — the gesture space a screen reader can
  bind is bounded by what the hook layer actually surfaces.

## Interception: the low-level hook

`WH_KEYBOARD_LL` ([Windows and messages](windows-and-messages.md)) is the interception point:
a callback in the screen reader's own process sees every keyboard event
system-wide *before* the focused app, synchronously, and returning nonzero
swallows the event. Everything a screen reader does with the keyboard is
built on this: command gestures are recognized and swallowed; modifier keys
used as screen reader modifiers (Insert, Caps Lock) are swallowed and
replayed when they turn out to be part of a passthrough combination;
everything else is passed on unmodified. The constraints: the hook thread
must service the callback fast (the system silently removes hooks that
exceed `LowLevelHooksTimeout` — do nothing blocking on the hook thread,
ever), and injected input (`SendInput`) flows through the same hook, so a
screen reader must tag its own injections via `dwExtraInfo` to avoid
reacting to itself.

## Text entry beyond keystrokes: TSF and IMEs

For Latin scripts, `WM_CHAR` is most of the story. For CJK and modern input
(handwriting, speech, the emoji panel, cloud suggestions), text enters via
the **Text Services Framework (TSF)**: an in-app COM framework where *input
processors* compose text — building a provisional *composition string*
(displayed inline, underlined), offering *candidate lists*, and finally
*committing* text into the document, possibly without any per-character key
messages at all. The older IMM32 IME API underlies/coexists for legacy
apps.

The screen reader consequences, which shape NVDA's design
([Keyboard input](../nvda/input.md)):

- **Typed-character echo cannot rely on `WM_CHAR` alone**: committed text
  from an IME never appears as keystrokes. NVDA reads composition and
  candidate state from *inside* the app process via injected TSF/IMM32
  hooks, because that state is only exposed in-process.
- **Candidate lists and composition changes are announcements of their
  own**: reading "typing k, o produces こ, candidates 1 of 7…" is a
  first-class feature with no out-of-process API in the classic world.
  (UIA has since grown selective coverage for standard IME UI.)
- Even for plain keystroke echo, the trustworthy signal is "the app
  *inserted* character X" (observed at the text control) rather than "key
  X was pressed" — dead keys, autocorrect, and rejected input make the
  hook stream an approximation of what the document received.

## Where the caret is

The blinking insertion point is per-control state, surfaced three ways:
the legacy system caret (`GetGUIThreadInfo` reports its window and
rectangle; winevents on `OBJID_CARET` report moves — many custom controls
draw their own instead, making this unreliable), the accessibility text
models (IA2 `caretOffset` / text-caret-moved events; UIA Text pattern's
selection and `ActiveTextPosition`), and per-control message protocols
(`EM_GETSEL` for edit controls). A screen reader's "review the character
under the caret / read by line" features are built by preferring whichever
of these the control actually supports — the compatibility-layered mess
that motivates a normalized text model.

## References

- [Keyboard Input overview (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/inputdev/keyboard-input)
- [SendInput (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
  and [LowLevelKeyboardProc (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)
- [Text Services Framework (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/tsf/text-services-framework)
