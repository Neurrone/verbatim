# verbatim-input-windows

The low-level keyboard hook thread (architecture section 5): the thin,
never-blocking imperative shell around [verbatim-input](verbatim-input.md)'s
pure `DecisionMachine`. It is a separate crate so that the machine, the key
names, and the gesture tables carry no Windows dependency.

Public API: `InputHook::start(config, map, events, speech, reports)`,
which installs `WH_KEYBOARD_LL` on a dedicated thread; drop uninstalls.
`events` is the gesture router's channel of `Routed` values, in the order
the keys came: `Gesture`, a gesture that fired; `Handled(n)`, sent when
the last key event of an end-to-end harness's numbered key stroke
(`verbatim_input::harness`, read from `dwExtraInfo`) has been decided, so
everything the stroke caused was sent before it; and `Barrier(token)`,
which the hook never sends but the router's other producers do, for a
control-plane idle request answered only after everything sent before it.
`speech` is a `SpeechEffectFn` (a boxed `Fn(KeySpeechEffect, u64) +
Send`) that carries out each key press's effect on speech, given the
press's key sequence number. Every key press gets the next number
(`next_key`), and the hook records under it, by trace id, everything the
press causes: its gesture, its observed caret key, the text it types
(`record_key_origin`, looked up with `key_of`, the latest 1,024 kept). The
app passes the number to `SpeechControl::cancel_through`, and the speech a
trace causes to `SpeechManager::speak_for_key`, so speech an earlier press
caused never follows a later press's cancel. `reports` is a
`KeyReportFn` that receives, after the speech effect, what a key passed to
the application did, a `KeyReport`: `Observed`, the observed gesture the
key completed (a caret key, `Decision::observed`) and `pressed_at_us`,
the Unix time in microseconds when the hook procedure ran, on the clock
outposts stamp observations with (the latency log starts a caret key's
line there); the application gets the key only once
the hook returns, so an outpost's read that finished before that time
came before the key. The key event's own `time` is not used: it counts
from startup at the system timer's resolution of about 16 milliseconds,
and converted to Unix time it could fall after the application handled
the key. Or `Typed`, the text the
key types, with a trace id minted when it was observed (the source of
`Input::CharacterTyped`, milestone M4). Both are called on the hook thread,
so they must not block. `OWN_INPUT_TAG` is the `dwExtraInfo` Verbatim
gives keys it injects for its own purposes (the GUI's Control tap that
unlocks the foreground): the hook still decides them, but leaves speech
alone and reports nothing, as NVDA ignores the keys it injects itself.

Implementation notes: the hook thread keeps the machine in a thread-local
(the hook procedure is a bare callback with no user pointer, and only ever
runs on the thread that installed it), tells it before each key whether
Num Lock is on (`DecisionMachine::set_num_lock`, from the keyboard's own
toggle state, a local read), calls `speech` with the decision's
speech effect before sending the gesture, so speech the gesture causes is
never the speech the key press cancels, forwards emitted gestures with a
non-blocking `try_send` that drops on a full channel, also sends the
lock-key gesture (`ToggleKey::gesture`, with a repeat of 0) when a Caps
Lock, Num Lock, or Scroll Lock press passes to the operating system, so
its new state can be announced (not for the Verbatim key passed in share
mode, `Decision::shared_modifier`, whose fate the screen reader behind
Verbatim decides), and returns 1 to
swallow or calls `CallNextHookEx` to pass. The never-block constraint is
load-bearing: Windows silently removes low-level hooks whose procedure
exceeds the system timeout, and the reader would go deaf to the keyboard
with no error. The `hook_echo` example installs the real hook with two
bound gestures and prints what fires and what each key reports; it needs
an interactive desktop and is never run in CI.

## Typed text (the `typed` module)

Which source `Input::CharacterTyped` comes from was the open choice M4's
Core work left: the hook's translation of each key, or the application's
own text changes. It comes from the hook, decided on 2026-10-06, because
that is the source that is correct for dead keys and never wrong for input
methods, and the one the reducer's password rule for terminals needs
(characters arrive before the terminal shows them, and are held until it
does):

- Each key-down the hook passes to the application, with no modifier but
  Shift, or with AltGr (Control and Alt together), is translated with
  `ToUnicodeEx`, using the keyboard layout of the thread that owns the
  foreground window (each thread has its own layout) and the modifier and
  Caps Lock state read locally. Control or Alt alone, or the Windows key,
  make a shortcut, which types nothing. The result counts when it is text
  other than control characters, a tab and the carriage return of Enter
  excepted.
- Dead keys. Windows keeps a pending dead key (the accent of a
  US-International key, waiting for the letter it modifies) in state every
  translation with that layout shares; measured on 2026-10-06, one process
  translating a dead key leaves it for another process's translation of a
  letter, which comes back accented. Translating keys in a hook the
  classic way consumes the application's dead key and breaks its typing,
  the well-known fault of keyboard hooks. The hook therefore
  translates with `ToUnicodeEx`'s flag that leaves the keyboard state
  unchanged (Windows 10 version 1607 and later): the dead key itself
  translates to nothing and is not echoed, and the letter after it reads
  the pending state the application's own translation of the dead key set,
  so it is echoed composed ("é"), without disturbing the application. A
  letter typed faster than the application handles the dead key before it
  reads uncomposed; nothing else is lost.
- Input methods. Chinese, Japanese, and Korean input compose several keys
  into text the hook cannot know, so with such a layout active (by the
  layout's language, or an older input method's layout handle, whose high
  word starts with hexadecimal E) nothing is translated, rather than
  echoing the romanized keys. `ImmIsIME` cannot tell: with text services
  it answers true for every layout, the US one included (found
  2026-10-06). The committed text is not echoed; NVDA hears it from inside
  the application (its `ime` and `tsf` hooks), which Verbatim's injection
  helper (decision D2, milestone M6) can do, and announcing compositions is
  a later milestone (`phase6-design.md`, "Internationalization in the text
  model").
- Keyboard text services. Outside Chinese, Japanese, and Korean, Windows'
  text-service keyboards (Vietnamese Telex and VNI, the Indic Phonetic
  keyboards) also compose characters from several keys, but under the
  language's ordinary layout, so the layout handle cannot tell them
  apart. The hook thread enters a COM apartment and asks the text
  services' profile manager (`ITfInputProcessorProfileMgr`) for the active
  keyboard profile; when it is a text service
  (`TF_PROFILETYPE_INPUTPROCESSOR`) for the foreground layout's language,
  nothing is translated, so Verbatim stays silent rather than echoing the
  raw keys (decided 2026-10-08). Windows switches input methods for every
  application together unless the user chose otherwise, which is why the
  hook thread's active profile stands for the foreground application's.
  Echoing the composed text needs the injection helper and is on the
  roadmap for M6.
- A key typed as a Unicode packet (`VK_PACKET`, from an on-screen keyboard
  or `SendInput` with `KEYEVENTF_UNICODE`) carries its UTF-16 unit, a
  surrogate pair as two keys, which are joined.

The application's text events were the alternative and were rejected:
they cannot tell typing from a paste, an autocompletion, or a program's own
change; an input method's composition text appears in the document as it
is composed, so it would be echoed key by key and again on commit; and a
terminal's password prompt would need no rule at all, but every other
terminal echo would wait for the screen.
