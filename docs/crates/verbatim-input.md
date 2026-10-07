# verbatim-input

The operating-system-free half of keyboard input (architecture section 5):
the pure decision state machine, the key-name vocabulary, and the gesture
tables. The thin never-blocking hook thread that feeds the machine is
[verbatim-input-windows](verbatim-input-windows.md); this crate has no
Windows dependency.

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
  so tests script entire key streams with fake time. A `Decision` carries
  the swallow-or-pass verdict, the gesture raised if any, `speech`, the
  transition's effect on speech, if any, and `observed`, an observed
  gesture the passed key completed (milestone M4's caret keys). The
  config's `interrupt_for_characters` and `interrupt_for_enter` are NVDA's
  two speech interrupt settings, both on by default. `set_num_lock(on)`
  tells the machine whether Num Lock is on: with it on, the numpad's
  operator keys type their characters and complete no gesture, as NVDA
  treats Num Lock as their modifier, so binding numpad plus does not take
  the plus sign from a user typing numbers. The hook reports the state
  before each key; it is off until it does.
- `KeySpeechEffect` — what a key press does to speech: `Cancel` (current
  and queued speech) or `TogglePause` (Shift on its own).
- `GestureMap`, `SharedGestureMap` — the bound-gesture set behind an
  arc-swap snapshot the hook reads lock-free; rebinding is one atomic store.
  `with_observed(gestures)` adds gestures that are observed rather than
  bound: the key is passed to the application as usual, and the
  `Decision` reports the gesture in `observed`, so the reducer can speak
  what the application did with it. A gesture both bound and observed is
  bound.
- `scripts` — the M3 script vocabulary. `KeyboardLayout` (`Desktop` or
  `Laptop`, redeclared here decoupled from `verbatim_config::KeyboardLayout`
  like `DecisionConfig` already is from `VerbatimKeys`) and `ScriptAction`
  (every M3 command: object navigation, review-cursor text reading, speak
  time, show tray list) plus `bindings_for(layout) -> Vec<(GestureId,
  ScriptAction)>`, the complete gesture table for a layout, transcribed from
  `docs/roadmap-done.md`'s M3 object-navigation bullet; the laptop layout has the
  numpad bindings as well, which NVDA binds for every layout. `toggle` —
  `ToggleKey` (Caps Lock, Num Lock, Scroll Lock) and the unbound gesture a
  lock key reaching the operating system is reported as, for its new state
  to be announced. `GestureMap` is a plain
  membership set, not generic over the bound action, so `bindings_for`
  returns gesture-and-action pairs the application adapts itself: build the
  hook's `GestureMap` from the gestures with `gesture_map_for(bindings)`,
  and keep a separate lookup (the same pairs, or a `HashMap` built from
  them) on the application side to resolve an emitted gesture to its action
  in the router. `verbatim-app` activates a layout this way, though it
  builds the hook's map itself, with the menu gesture added, rather than
  through `gesture_map_for`: it reads `Settings.keyboard.layout` once at
  startup, maps it to this crate's `KeyboardLayout`, and calls
  `bindings_for` for both the hook's map and the router's lookup. Rebinding
  at runtime, for example on a layout change, is not implemented; a new
  layout takes effect on the next start.
- M4's commands (`phase6-design.md`, "The autonomous run", M4 item 5, with
  NVDA's keys). Every layout: Verbatim+Alt+Home and Verbatim+Alt+End (the
  review cursor to the selection's start and end), numpad Plus (say all
  from the review cursor), Verbatim+F9 (set the start marker),
  Verbatim+Shift+F9 (the review cursor to the start marker), Verbatim+F10
  (select from the marker to the review cursor; twice, copy), Verbatim+6
  (caret moves review cursor), Verbatim+2 and Verbatim+3 (speak typed
  characters and words), and Verbatim+5 (report new output in terminals,
  M4 item 9; NVDA's key for its "report dynamic content changes" toggle).
  The desktop layout's own: Verbatim+Page Up and
  Page Down (the review cursor by page), Verbatim+Down Arrow (say all from
  the caret), Verbatim+numpad Delete (the caret's location), and
  Verbatim+Shift+numpad Delete (the review cursor's location). The laptop
  layout's own: Verbatim+Shift+Page Up and Page Down, Verbatim+A (say all),
  Verbatim+Shift+A (say all from the review cursor), Verbatim+Delete and
  Verbatim+Shift+Delete (the locations). The tables are now the layout's
  own bindings followed by those of every layout, since desktop and laptop
  give Verbatim+Down Arrow different meanings, as NVDA's `kb(desktop):`
  and `kb(laptop):` bindings do.
- Report focus (`ScriptAction::ReportFocus`), Verbatim+Tab in every
  layout, as NVDA's NVDA+Tab.
- `ScriptAction::review_command` — the reducer command an action runs, or
  `None` for speak time and the tray list, which the router handles; the
  one place the two vocabularies meet.
- `caret_bindings()` — the caret keys of NVDA's editable-text commands, as
  `(GestureId, CaretKey)` pairs: Left and Right Arrow, Control with them,
  Up and Down Arrow, Control with them, Home, End, Page Up, Page Down, and
  Control with Home and End, each with and without Shift; Backspace,
  Delete, and Control with each; and Control+A. They are observed, never
  bound: the shell adds them to the hook's map with `with_observed` and
  turns each reported gesture into `Input::CaretKey`.

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
- What a key press does to speech (`docs/nvda/input.md`, "What a key press
  does to speech"): every key-down sets `Decision::speech`, whether it is
  bound, swallowed, or passed on, modifiers, the Verbatim modifier, and
  typed characters included, to `Cancel`, with these exceptions. The
  volume keys (extended `VK_VOLUME_MUTE` through `VK_VOLUME_UP`) and the
  unknown key `0xFF` leave speech alone. Shift (generic, left, or right)
  is `TogglePause`, except while that Shift is already held, where Windows
  repeats its key-down, so holding Shift does not toggle again. Key-ups
  never touch speech. The effect is worked out before the press is recorded
  as held, and the hook carries it out before the gesture is sent, so a
  key never cancels the speech its own gesture causes. NVDA's settings to
  turn off cancelling for typed characters and for Enter are
  `interrupt_for_characters` and `interrupt_for_enter`: with the first off,
  a typed character (a letter, digit, punctuation key, or Space, with no
  modifier held or Shift alone, or a lock key not serving as the Verbatim
  modifier) and Shift leave speech alone; with the second off, Enter does.
- Injected keys are processed identically to physical ones, which is what
  lets the control plane drive gestures with synthetic input; they cancel
  speech too.
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
  count, and every auto-repeated emission carries the same count as the
  genuine press that started the hold; NVDA counts auto-repeat as presses,
  so this is a deliberate difference. The machine detects auto-repeat as a
  key-down of a key it swallowed and has not yet seen released. Any key
  other than a modifier that completes no bound gesture ends the streak,
  as NVDA forgets its last script when an unbound gesture comes between.
  Control-plane gesture injection (which builds an `EmittedGesture`
  directly in `verbatim-app`, bypassing the machine) always injects
  `repeat: 0`, a single first press.

The hook shell that drives the machine is described in
[verbatim-input-windows](verbatim-input-windows.md).
