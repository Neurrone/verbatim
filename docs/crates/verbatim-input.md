# verbatim-input

The keyboard hook (architecture section 5), split into a pure decision
state machine and a thin never-blocking hook shell.

Public API:

- `KeyEvent`, `KeyDecision` — one raw key transition, and the
  swallow-or-pass verdict returned to Windows.
- `keys` — the NVDA-style key-name vocabulary: `vk_from_name` and
  `name_from_vk` (letters and digits resolve procedurally, everything else
  through a table where the extended flag distinguishes twins like insert
  and numpad insert), plus `VERBATIM_MODIFIER_NAME`. Shared with the
  control plane's key injection so both sides speak identical names. M3
  filled out the numpad and navigation-cluster vocabulary the object- and
  review-navigation bindings need: `numpad0` through `numpad9` are the
  non-extended twins of the navigation cluster (Num Lock off reports the
  same vk as `home`, `uparrow`, and so on), matching the existing
  `insert`/`numpadinsert` pattern; `numpad5` has no navigation-cluster twin
  and names `VK_CLEAR` instead; `numpadminus`, `numpadplus`,
  `numpaddivide`, `numpadmultiply`, and `period` are ordinary untwinned
  keys.
- `DecisionConfig`, `DecisionMachine`, `Decision`, `EmittedGesture` — the
  pure state machine. `on_key(event, now)` takes a caller-supplied clock,
  so tests script entire key streams with fake time.
- `GestureMap`, `SharedGestureMap` — the bound-gesture set behind an
  arc-swap snapshot the hook reads lock-free; rebinding is one atomic store.
- `InputHook::start(config, map, events)` — installs `WH_KEYBOARD_LL` on a
  dedicated thread; drop uninstalls.
- `scripts` — the M3 script vocabulary. `KeyboardLayout` (`Desktop` or
  `Laptop`, redeclared here decoupled from `verbatim_config::KeyboardLayout`
  like `DecisionConfig` already is from `VerbatimKeys`) and `ScriptAction`
  (every M3 command: object navigation, review-cursor text reading, speak
  time, show tray list) plus `bindings_for(layout) -> Vec<(GestureId,
  ScriptAction)>`, the complete gesture table for a layout, transcribed from
  `docs/roadmap.md`'s M3 object-navigation bullet. `GestureMap` is a plain
  membership set, not generic over the bound action, so `bindings_for`
  returns gesture-and-action pairs the application adapts itself: build the
  hook's `GestureMap` from the gestures with `gesture_map_for(bindings)`,
  and keep a separate lookup (the same pairs, or a `HashMap` built from
  them) on the application side to resolve an emitted gesture to its action
  in the router. The intended activation path, wired in by `verbatim-app`'s
  reducer-side consumer rather than this crate: read
  `Settings.keyboard.layout`, map it to this crate's `KeyboardLayout`, call
  `bindings_for`, and store `gesture_map_for`'s result — rebinding, for
  example on a layout change, is one atomic store on the existing
  `SharedGestureMap`, picked up by the hook on its next keystroke.

Implementation notes, `DecisionMachine::on_key` (the intricate one;
`docs/parity.md`'s input section is the behavioural record for these
rules):

- A Verbatim-modifier key-down is always swallowed, so caps lock never
  toggles while acting as the modifier. In share mode the modifier's
  transitions pass down the hook chain instead, so a screen reader hooked
  behind Verbatim sees the modifier held and swallows it itself.
- A key-down forming a bound gesture is swallowed, recorded in the
  swallowed-downs set, and emitted with a freshly minted `TraceId`. An
  unbound companion key falls through to the application as a bare
  keypress — NVDA behavior, so an unrecognized chord is not eaten.
- Keys swallowed on the way down are swallowed on their key-up too, so an
  application never sees the release of a key whose press it never saw.
- Double-tap passthrough: releasing the modifier with no other key pressed
  during the hold arms a window (`multi_press_timeout`, 500 ms). Pressing
  the same physical key again inside the window hands that whole press to the
  operating system, auto-repeats included, so caps lock actually toggles. The
  hand-over ends at the next key-up and does not re-arm itself, so a triple
  tap makes the third press a modifier again. The three states of a lone tap
  (held alone, released with the window running, being handed over) are one
  `LoneModifier` enum inside the machine, so no combination of them can go
  out of step.
- Injected keys are processed identically to physical ones, which is what
  lets the control plane drive gestures with synthetic input.
- Chords normalize modifiers to generic names (left and right control both
  become `control`), and gesture assembly relies on `GestureId::parse` for
  ordering, so press order never matters.
- Multi-press counting (NVDA semantics, M3): every `EmittedGesture` carries
  a `repeat` count — 0 for the first press, 1 for the second, 2 for the
  third, saturating rather than overflowing. Pressing the same bound
  gesture again within `multi_press_timeout` (500 ms) of its previous
  genuine press increments the count; a different gesture or the window
  elapsing resets it to 0. Consumers dispatch on it: report-current
  (report, spell, copy), Verbatim+F12 (time, date), Verbatim+F11 (tray
  list, taskbar list). Auto-repeat — holding the gesture's key, which
  re-fires key-downs with no intervening key-up — does not advance the
  count; NVDA does not treat auto-repeat as a multi-press for
  script-repeat purposes, and every auto-repeated emission carries the
  same count as the genuine press that started the hold. The machine
  detects auto-repeat as a key-down of a key it swallowed and has not yet
  seen released.
  Control-plane gesture injection (which builds an `EmittedGesture`
  directly in `verbatim-app`, bypassing the machine) always injects
  `repeat: 0`, a single first press.

The hook shell (`hook.rs`) keeps the machine in a thread-local on the hook
thread, forwards emitted gestures with a non-blocking `try_send` that drops
on a full channel, and returns 1 to swallow or calls `CallNextHookEx` to
pass. The never-block constraint is load-bearing: Windows silently removes
low-level hooks that exceed the system timeout.
