# verbatim-input-windows

The low-level keyboard hook thread (architecture section 5): the thin,
never-blocking imperative shell around [verbatim-input](verbatim-input.md)'s
pure `DecisionMachine`. It is a separate crate so that the machine, the key
names, and the gesture tables carry no Windows dependency.

Public API: `InputHook::start(config, map, events, speech)`, which
installs `WH_KEYBOARD_LL` on a dedicated thread; drop uninstalls. `speech`
is a `SpeechEffectFn` (a boxed `Fn(KeySpeechEffect) + Send`) that carries
out each key press's effect on speech; it is called on the hook thread, so
it must not block.

Implementation notes: the hook thread keeps the machine in a thread-local
(the hook procedure is a bare callback with no user pointer, and only ever
runs on the thread that installed it), calls `speech` with the decision's
speech effect before sending the gesture, so speech the gesture causes is
never the speech the key press cancels, forwards emitted gestures with a
non-blocking `try_send` that drops on a full channel, and returns 1 to
swallow or calls `CallNextHookEx` to pass. The never-block constraint is
load-bearing: Windows silently removes low-level hooks whose procedure
exceeds the system timeout, and the reader would go deaf to the keyboard
with no error. The `hook_echo` example installs the real hook with two
bound gestures and prints what fires; it needs an interactive desktop and
is never run in CI.
