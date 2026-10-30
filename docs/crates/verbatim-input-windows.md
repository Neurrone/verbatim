# verbatim-input-windows

The low-level keyboard hook thread (architecture section 5): the thin,
never-blocking imperative shell around [verbatim-input](verbatim-input.md)'s
pure `DecisionMachine`. It is a separate crate so that the machine, the key
names, and the gesture tables carry no Windows dependency.

Public API: `InputHook::start(config, map, speech, reports, holding,
foreground)`, which installs `WH_KEYBOARD_LL` on a dedicated thread; drop
uninstalls. `InputHook::injector` hands out an `Injector`, by which other
threads inject inputs for the user. The hook thread hooks nothing else
and sends nowhere but its two callbacks (architecture section 5): what it
needs to know of the window in front, `foreground`, is Core's.

The code split follows who calls it (`docs/design/ownership-input.md`,
"The ownership check on the hook"): the `hook` module holds everything
that runs on the hook thread (`HookState`, the hook procedure, `decide`,
`hold`, `report_passed`, `inject` and the message loop), and `lib.rs`
what other threads call: `InputHook` and its drop, the thread's
start-up, `KeyHolding`, `ForegroundView` and `Injector`. `cargo xtask
ci` checks `hook`, `typed` and the app's `key_reports` by the hook rule
(`xtask/src/ownership.rs`): they may name no lock, wait, log, I/O or
`SendMessageW`, and `use` only an allow-list.

## Key numbers

The hook thread is the one minter of key numbers
(`verbatim_model::KeySeq`). `HookState::last_key` is the last one given
out: a key press takes the next, a release the last, and an injected
gesture the next, each in the order the thread handles it. Every report
of a user input carries its number. Speech is fenced by key order: the
key's cancel names its number (`SpeechControl::cancel_through`), Core
stamps the speech each input causes with its number, and the speech
queue drops speech of an earlier number that arrives after a later
cancel. Only numbers one thread minted in its own handling order agree
with the order the inputs reach Core.

## The callbacks

`speech` is a `SpeechEffectFn` (a boxed `Fn(KeySpeechEffect, KeySeq) +
Send`) that carries out each key press's effect on speech, given the
press's number, before anything the press caused is reported, so speech
the press causes is never the speech it cancels. `reports` is a
`KeyReportFn` that receives, after the speech effect, everything a key or
an injected input caused, as `KeyReport`s, in the order decided:

- `Observed`, the observed gesture the key completed (a caret key,
  `Decision::observed`), its number and `pressed_at_us`, the Unix time in
  microseconds when the hook procedure ran, on the clock outposts stamp
  observations with (the latency log starts a caret key's line there).
  The application gets the key only once the hook returns, so an
  outpost's read that finished before that time came before the key. The
  key event's own `time` is not used: it counts from startup at the
  system timer's resolution of about 16 milliseconds, and converted to
  Unix time it could fall after the application handled the key.
- `Held` and `Deferred`, below.
- `Typed`, the text the key types, with a trace id minted when it was
  observed and its number (the source of `Input::CharacterTyped`).
- `Passed`, any other key-down passed to the application that types no
  text and is neither observed, a modifier, a lock key, nor a key Windows
  gives a meaning outside the text (Pause, Print Screen, Sleep, and the
  browser, volume, media and launch keys, which never edit and would only
  leave a caret watch open until its bound), with its trace id, number
  and `pressed_at_us` (the source of a caret key judged by where the
  caret landed, `CaretMotion::Other`).
- `Gesture`, a bound gesture that fired, an injected gesture, or a lock
  key's gesture (`ToggleKey::gesture`, repeat 0) when a Caps Lock, Num
  Lock or Scroll Lock press passes to the operating system, so its new
  state can be announced (not for the Verbatim key passed in share mode,
  `Decision::shared_modifier`, whose fate the screen reader behind
  Verbatim decides), with its number.
- `Handled(n)`, when the last key event of an end-to-end harness's
  numbered key stroke (`verbatim_input::harness`, read from
  `dwExtraInfo`) has been decided, so everything the stroke caused was
  reported before it.
- `Barrier(token)`, an injected idle waiter, reported behind everything
  handled before it.

Both callbacks run on the hook thread, so they must not block nor
dispatch sent messages. The app's callbacks post to the reducer's completion port
and to the speech queue's, neither of which takes a lock or waits.
`OWN_INPUT_TAG` is the `dwExtraInfo` Verbatim gives keys it injects for
its own purposes (the GUI's Control tap that unlocks the foreground): the
hook still decides them, but leaves speech alone and reports nothing, as
NVDA ignores the keys it injects itself.

## Injected inputs

`Injector::inject(Injected)` posts an input injected for the user to the
hook thread, from any thread: `Gesture(GestureId)`, the control plane's
`SendGesture`, and `Barrier(token)`, an `AwaitIdle` waiter's. The item is
boxed and posted with `PostThreadMessageW` as a private `WM_APP`
message whose `LPARAM` is the box; when the post fails (the thread has
gone, or its queue of 10,000 messages is full), the box is taken back and
the item returned as the error, so an injection reported failed never
runs later. An `Injector` holds a handle to the hook thread, duplicated
by `InputHook::start` from its join handle and closed with the last
clone, so the thread's id is not given to another thread while any
`Injector` can post to it.

The hook thread's `GetMessageW` loop (`hook::pump_messages`) takes each
such message, reclaims the box and calls `inject`: a gesture takes the
next number, carries out `KeySpeechEffect::Cancel` with it through the
same speech callback a key uses, and is reported as `Gesture`; a barrier
is reported as `Barrier`. Windows calls the hook procedure from within
that loop's waits, so keys and injected inputs are handled one at a
time, in the one order the thread takes them. The thread runs no other
loop while the hook is installed; once the loop ends and the hook is
removed, the injected messages left are taken with `PeekMessageW` and
dropped.

## Implementation notes

The hook thread runs at `THREAD_PRIORITY_HIGHEST`
(`verbatim_process::scheduling`; `docs/design/communication.md`, section
11, step 3). It keeps its state in a thread-local (the hook procedure is a
bare callback with no user pointer, and only ever runs on the thread that
installed it), tells the machine before each key whether Num Lock is on
(`DecisionMachine::set_num_lock`, from the keyboard's own toggle state, a
local read), calls `speech` with the decision's speech effect, reports,
and returns 1 to swallow or calls `CallNextHookEx` to pass. The
never-block constraint is load-bearing: Windows silently removes
low-level hooks whose procedure exceeds the system timeout, and the
reader would go deaf to the keyboard with no error. So is the never-drop
rule: a key pressed while Verbatim is backed up must still act once the
reducer reaches it. Both hold because the reports go to completion
ports, whose post never waits and whose backlog waits in memory in front
of the reducer, as every backlog in `docs/design/communication.md` does
(section 4.8). Each post allocates a box, so the allocator's own
synchronisation stays on the hook thread; the hook takes no lock of
Verbatim's. A re-entered hook procedure, should a callback ever dispatch
sent messages, passes its key untouched rather than panicking on the
state's borrow.

`decide` is the decision for each key, which the hook procedure calls
with the thread's state; unit tests drive it and `inject` directly: a
burst of a thousand Verbatim+V strokes, every gesture and every stroke's
handled mark reported in order; a key press, an injected gesture and a
second press numbered 1, 2 and 3, each cancel naming its own number
before its report; an injected barrier reported behind the keys before
it; and an injection posted to thread id 0, which no thread has, coming
back as the error. The `hook_echo` example installs the real hook with
two bound gestures and prints what each key reports, on its own thread;
it needs an interactive desktop and is never run in CI.

## Holding caret keys

`holding` is a `KeyHolding`: the caret keys the hook holds back (the
whole caret table, `verbatim_input::caret_bindings`) and an atomic
switch the app sets after every input from Core's focus
(`SrState::holds_caret_keys`, true while the focus has text or may have,
as for the caret keys whose evidence is watched); the hook reads it with
one relaxed load, so it never waits (`docs/design/key-holding.md`). While
it is on, a key-down that completes one of those gestures is swallowed and
reported as `KeyReport::Held`, with its press time and the stroke it came
from (`HeldStroke`: virtual key, scan code, extended flag), after its
effect on speech as before; the key's release is swallowed too, so the
application never sees a release without its press. The clearing keys,
typed characters, Enter and any other key are never held for a read.

Keys behind a held key: while any key held back has not yet come back
through the hook, every later transition the hook would pass, typed
characters, modifiers and releases included, is swallowed too and
reported as `KeyReport::Deferred` (its stroke and whether it is a press),
after the reports it makes as before (a typed character is still
reported `Typed`, translated with the deferred modifiers ahead of it
taken as made), so the app sends it on after the held key, in the order
pressed. Each key held or deferred gets a place in line
(`HeldStroke::sequence`), kept by the hook (`HookState::outstanding`)
until the key comes back carrying it: `dwExtraInfo` holds
`PASSED_ON_TAG` in its high half and the place in its low half
(`passed_on_info`), 0 on the modifiers and releases sent around a held
key. A key coming back clears its place and every place before it, so a
key Windows dropped is waited for only until a later one arrives. A key
carrying the tag passes straight through, decided by nothing, numbered
by nothing and reported to nobody, as NVDA ignores the keys its scripts
send; a key the end-to-end harness injects carries its stroke number
instead and is held like a typed one. The lock keys (Caps Lock, Num Lock,
Scroll Lock) are never deferred: they pass at once, since their new state
is announced from Windows' own and Windows translates numpad keys by Num
Lock's state before the hook sees them.

Elevated windows: while the foreground window's process has higher
integrity than Verbatim's, or its integrity cannot be read, nothing is
held or deferred, since Windows drops input Verbatim sends to it; a held
key's release is still swallowed. The hook does not work this out. The
focus listener reports each window that comes to the front with its
process's integrity level, Core decides whether keys may be held there
(`SrState::hook_foreground`), and the reducer's thread publishes that
after each input in `foreground`, a `ForegroundView` (a lock-free
`ArcSwapOption` of `verbatim_model::HookForeground`). The hook reads it
only for a key it would hold or defer, with one load and a
`GetForegroundWindow`, and applies it only while the window it names is
the one in front: until Core has caught up with a foreground change, or
before Core has any view, nothing is held, a key judged without its
caret costing less than a key lost. It is a temporary guard until
uiAccess (`docs/roadmap.md`, M8.1). Unit tests drive `decide` with the
hold on and off, with each tag, with keys deferred behind held ones and
coming back in and out of order, with lock keys behind a held key, with
Core's view forbidding holding, and with a view of another window or
none; the tests fix the window in front (`HookState::front`), so nothing
depends on the desktop they run on.

## Typed text (the `typed` module)

`Input::CharacterTyped` comes from the hook's translation of each key,
not from the application's own text changes, because
that is the source that is correct for dead keys and never wrong for input
methods, and the one the reducer's password rule for terminals needs
(characters arrive before the terminal shows them, and are held until it
does):

- Each key-down the hook passes to the application, with no modifier but
  Shift, or with AltGr (Control and Alt together), is translated with
  `ToUnicodeEx`, using the keyboard layout of the thread that owns the
  foreground window (each thread has its own layout) and the modifier and
  Caps Lock state read locally. A console host window is the exception:
  Windows answers which thread owns it with the first process attached to
  the console (a shell), not the console host's own thread, whose layout
  the console translates keys with. A `WinEvent` always names the real
  thread, as NVDA uses for its issue 10113: the focus listener reports
  the thread a console window's foreground event names, Core publishes
  it as the window's keyboard thread in the hook's `ForegroundView`, and
  the translation uses it while that window is in front. When the
  listener starts with a console already in front, it hooks every
  console's events until that console names its thread, then unhooks
  them (`verbatim-outpost`, the `foreground` module). A console window
  whose thread is not known yet, or a view Core has not brought up to
  date, falls back to the answer Windows gives. Control or Alt alone, or the Windows key,
  make a shortcut, which types nothing. The result counts when it is text
  other than control characters, a tab and the carriage return of Enter
  excepted.
- Dead keys. Windows keeps a pending dead key (the accent of a
  US-International key, waiting for the letter it modifies) in state every
  translation with that layout shares: one process
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
  it answers true for every layout, the US one included. The committed text is not echoed; NVDA hears it from inside
  the application (its `ime` and `tsf` hooks), which Verbatim's injection
  helper (decision D2, milestone M7) can do, and announcing compositions is
  a later milestone (`docs/parity/input.md`, "IME/composition reporting").
- Keyboard text services. Outside Chinese, Japanese, and Korean, Windows'
  text-service keyboards (Vietnamese Telex and VNI, the Indic Phonetic
  keyboards) also compose characters from several keys, but under the
  language's ordinary layout, so the layout handle cannot tell them
  apart. The hook thread enters a COM apartment and asks the text
  services' profile manager (`ITfInputProcessorProfileMgr`) for the active
  keyboard profile; when it is a text service
  (`TF_PROFILETYPE_INPUTPROCESSOR`) for the foreground layout's language,
  nothing is translated, so Verbatim stays silent rather than echoing the
  raw keys. A profile manager that cannot be made leaves that check
  out: `Typing::with_text_services` returns the error for the thread's
  start-up to log, since nothing on the hook thread logs. Windows
  switches input methods for every
  application together unless the user chose otherwise, which is why the
  hook thread's active profile stands for the foreground application's.
  Echoing the composed text needs the injection helper and is on the
  roadmap for M7.
- A key typed as a Unicode packet (`VK_PACKET`, from an on-screen keyboard
  or `SendInput` with `KEYEVENTF_UNICODE`) carries its UTF-16 unit, a
  surrogate pair as two keys, which are joined.

The application's text events were the alternative and were rejected:
they cannot tell typing from a paste, an autocompletion, or a program's own
change; an input method's composition text appears in the document as it
is composed, so it would be echoed key by key and again on commit; and a
terminal's password prompt would need no rule at all, but every other
terminal echo would wait for the screen.
