# verbatim-input

The operating-system-free half of keyboard input (architecture section 5):
the pure decision state machine, the key-name vocabulary, and the gesture
tables. The thin never-blocking hook thread that feeds the machine is
[verbatim-input-windows](verbatim-input-windows.md); this crate has no
Windows dependency.

Public API:

- `KeyEvent`, `KeyDecision` — one raw key transition, and what the hook
  tells Windows: swallow or pass.
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
- `DecisionConfig`, `DecisionMachine`, `KeyInput`, `KeySource`,
  `HoldPermit`, `Decision`, `Act`, `Verdict`, `PressReport`, `RaisedGesture`,
  `EmittedGesture` — the pure state machine, which owns its `GestureMap`,
  given at construction, and reads no clock, trace id or shared state.
  It decides each key transition whole; the hook gathers its inputs,
  carries out the verdict and reports. `on_key(input)` takes a `KeyInput`: the
  event; the time in microseconds on the run's shared clock, which the
  hook reads once for each key, so tests script entire key streams with
  fake time; whether Num Lock is on, with which the numpad's operator keys
  type their characters and complete no gesture, as NVDA treats Num Lock
  as their modifier, so binding numpad plus does not take the plus sign
  from a user typing numbers; the `source`, `User` or `Own` (the keys
  Verbatim injects for itself); and the `permit` Core's view of the window
  in front gives, `None`, `Defer` or `HoldAndDefer`, the last when the
  focus also has text (for Enter, a field's other than a terminal's, which
  the hook judges); and `may_mask`, whether that view lets keys be
  held in the window it names, read without checking the window is still
  in front. A `Decision` carries `speech`, the transition's effect on
  speech, if any, and one `Act`, a variant for each combination that can
  occur: `Swallowed`, raising nothing; `Gesture`, swallowed, raising a
  bound gesture (a `RaisedGesture`, which the hook stamps with a trace id
  as it reports it, `stamped(trace_id)`, making an `EmittedGesture`) with
  the places in line of its `mask`'s press and release (below); `Held`, a
  caret key's press held back, with the observed gesture, the `CaretKey`
  the map gives it and its place; `Reaches`, passed now or deferred to a
  place, with the `observed` gesture the key completed if any (milestone
  M4's caret keys), `typing`, what a press that reaches the application
  reports (the text it types, which the hook translates, or, typing none,
  that it may have edited the text), and `toggle`, a lock key passed to
  the operating system, whose new state is announced; and `Shared`, the
  Verbatim key's own transition passed in share mode, now or deferred.
  `verdict()` (`Pass`, `Swallow`, `Hold(place)` or `Defer(place)`),
  `emitted()`, `observed()`, `typing()`, `toggle()` and `mask()` read
  those views. For `Own` input it gives no speech effect, observed
  gesture or typing. `alt_down()` says whether either Alt key is down, as
  the transitions decided left it, which the hook reports to Core.
  `passed_on(place)` takes the line of keys held back up to a
  key Core passed on, and changes nothing else; `desktop_switched(alts_down)`
  forgets every key down, the modifiers among them, and the line, after a
  switch away from the user's input desktop whose releases never came,
  keeping down only the Alt keys the caller reads as physically down, as
  NVDA judges the task switcher by the keyboard's physical Alt; `has_outstanding()` and
  `deferred_transitions()` read the line, for the hook's permit and its
  translation of typed text. The config's `interrupt_for_characters` and
  `interrupt_for_enter` are NVDA's two speech interrupt settings, both on
  by default.
- `KeySpeechEffect` — what a key press does to speech: `Cancel` (current
  and queued speech) or `TogglePause` (Shift on its own).
- `GestureMap` — the gestures the hook consults, which the decision
  machine owns: nothing changes the bindings while Verbatim runs. It is
  built from the one gesture table (`Tables::gesture_map`).
  `with_observed(gestures)` adds gestures that are observed rather than
  bound: the key is passed to the application as usual, and the
  `Decision` reports the gesture as observed, so the reducer can speak
  what the application did with it. A gesture both bound and observed is
  bound. `with_held(pairs)` adds observed gestures that are also held,
  the caret keys, each with its `CaretKey`, and `holds(gesture)` gives
  the caret key of one that is, which a held key's report carries.
  `with_held_only(pairs)` adds gestures held like them but observed
  only when held, Enter's: not held, the key is reported as an unobserved
  key, the text it types (`holds_only(gesture)`).
- `Tables` — the one gesture table: the caret and selection keys with
  their `CaretKey`, the clearing keys, the editing chords, the menu's
  gesture (given by the app, `for_layout(layout, show_menu)`), the
  layout's scripts and the lock keys, looked up on the hook thread for
  what a gesture or an observed key is to the reducer. `gesture_map()`
  builds the hook's map from it: the menu gesture and the scripts bound,
  the clearing keys, editing chords and selection keys observed, Enter
  held only and the other caret keys held.
- `scripts` — the M3 script vocabulary. `KeyboardLayout` (`Desktop` or
  `Laptop`, `verbatim-model`'s, which the configuration stores) and `ScriptAction`
  (every M3 command: object navigation, review-cursor text reading, speak
  time, show tray list) plus `bindings_for(layout) -> Vec<(GestureId,
  ScriptAction)>`, the complete gesture table for a layout, transcribed from
  `docs/roadmap-done.md`'s M3 binding lists; the laptop layout has the
  numpad bindings as well, which NVDA binds for every layout. `toggle` —
  `ToggleKey` (Caps Lock, Num Lock, Scroll Lock) and the unbound gesture a
  lock key reaching the operating system is reported as, for its new state
  to be announced. `verbatim-app` reads `Settings.keyboard.layout` once at
  startup and builds `Tables` for it, from which both the hook's map and
  the lookup its hook callbacks make (`key_reports`) come. Rebinding at
  runtime, for example on a layout change, is not implemented; a new
  layout takes effect on the next start.
- M4's commands (`docs/parity/review.md`, with NVDA's keys). Every layout: Verbatim+Alt+Home and Verbatim+Alt+End (the
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
- Report status bar (`ScriptAction::ReportStatusBar`), NVDA's keys:
  Verbatim+End in the desktop layout and Verbatim+Shift+End in the
  laptop layout, whose Verbatim+End stays the review cursor's end of
  line.
- The previous and next command entered in a terminal
  (`ScriptAction::PreviousCommand` and `NextCommand`), Verbatim+Alt+Up
  Arrow and Verbatim+Alt+Down Arrow in every layout, Verbatim's own,
  where NVDA pans its magnifier.
- `ScriptAction::review_command` — the reducer command an action runs, or
  `None` for speak time and the tray list, which run as Core's scripts; the
  one place the two vocabularies meet.
- `caret_bindings()` — the caret keys of NVDA's editable-text commands
  that the hook holds, as `(GestureId, CaretKey)` pairs: Left and Right
  Arrow, Control with them, Up and Down Arrow, Control with them, Home,
  End, Page Up, Page Down, and Control with Home and End; Backspace,
  Delete and the numpad's Delete, each alone and with Control or Shift
  (Shift deletes as the key alone does, and Shift+Delete cuts a
  selection); Alt with Up and Down Arrow, NVDA's sentence keys; and Enter
  and the numpad's Enter, NVDA's new-line script's keys
  (`CaretMotion::NewLine`), which have no Shift form among the selection
  keys. They are observed and held, never bound: the table's map holds
  them with `with_held`, Enter's with `with_held_only`, and the shell
  posts each reported gesture as a caret key's press, or, while the focus
  has text, the machine holds them back until the caret has been read and
  the shell posts each as a held caret key's press, with its stroke
  (`books/dev/src/explanation/design/key-holding.md`).
- `selection_bindings()` — the selection keys, the Shift forms of the
  moves and Control+A, as `(GestureId, CaretKey)` pairs with `select`
  set. They are observed, never held nor bound: each passes at once,
  deferred only behind a held key, and is reported, so Core ends say-all
  at its turn.
- `clearing_keys()` — the keys that end or clear the command line being
  typed: Escape, Control+C, Control+D, and Control+Break, both as NVDA
  binds it (Control+Pause) and as Windows reports it (the `break` key,
  virtual key 3). Observed like the caret keys; the shell posts each as a
  clearing key's press.
- `editing_chords()` — the closed list of Control+Z, Control+Y, Control+X,
  Control+V and Escape, observed like the caret keys; the shell posts each
  as an editing chord's press.
- `harness` — the number an end-to-end test harness puts on each key it
  injects, so Verbatim can say when it has handled that key: evidence for
  "nothing more was said" that is not a period of silence. The agent
  numbers every key stroke and carries the number in each key event's
  `dwExtraInfo`: `encode(number, last)` puts a fixed tag in the top sixteen
  bits, a flag marking the stroke's last event (a stroke is modifiers
  down, the key down and up, modifiers up) in the next bit, and the number
  in the rest, up to `MAX_NUMBER`; `decode(extra)` reads a `NumberedKey`
  back, or `None` for a key the harness did not number. The Windows hook
  (`verbatim-input-windows`) reports the number once it has handed on the
  stroke's last event, and Core reports it handled once everything the
  stroke caused is queued (the control plane's `AwaitIdle` and
  `Frame::InputHandled`).

Implementation notes, `DecisionMachine::on_key` (the intricate one;
`docs/parity/input.md` is the behavioural record for these
rules):

- A Verbatim-modifier key-down is always swallowed, so caps lock never
  toggles while acting as the modifier. In share mode the modifier's
  transitions pass down the hook chain instead, so a screen reader hooked
  behind Verbatim sees the modifier held and swallows it itself.
- A key-down forming a bound gesture is swallowed and raised, for the
  hook to stamp with a freshly minted `TraceId`. An
  unbound companion key falls through to the application as a bare
  keypress — NVDA behavior, so an unrecognized chord is not eaten.
- One record of the keys down: every key the machine has seen go down and
  not come up, with whether a press of it has reached the application, or
  is deferred to reach it, since it went down or since its last held
  press, which Core sends with its own release (`reached`). A release is
  swallowed while
  `reached` is false, so an application never sees the release of a key
  whose press it never saw, and delivered once it is true, so a key one of
  whose presses reached the application is seen to come up, where NVDA
  leaves it down in Windows until it is next pressed. The entry goes with
  the release.
- Holding (`books/dev/src/explanation/design/key-holding.md`): a press completing a held
  gesture, with the permit `HoldAndDefer` and the source `User`, is
  `Hold(place)`. While a place waits in the line, any other transition
  that would pass is `Defer(place)` when the permit is not `None`, unless
  it is a lock key, whose state is announced from Windows' own, or
  Verbatim's own key; a bound gesture is swallowed, never deferred. The
  line's places count from 1; `passed_on(place)` takes the line up to that
  place, since Core passes keys on in order and one Windows dropped is not
  waited for once a later one arrives. Every short sequence of keys,
  pass-ons, permit changes and pauses past the multi-press timeout, up to
  four steps over both Verbatim keys and every modifier, with each
  layout's map, built by `Tables`, is checked by `tests/hold_sequences.rs`
  against invariants that need no model (no release before its press, no
  key left down, no acting modifier released before its mask, the line
  matching the places given out); the unit tests and the walk's examples
  state each verdict, gesture naming, counting and the double tap. The
  walk leaves out sequences where the permit falls to `None` while
  a deferred press or a mask waits, whose release then passes at once
  (`books/dev/src/explanation/design/key-holding.md`, "Limitations", "The hook's hold
  decision").
- The menu guard (`docs/parity/input.md`, "A modifier released after a
  command"): a bound gesture the user pressed, swallowed while `may_mask`
  is set and while Alt, a Windows key, or Control and Shift together are
  held with `reached` set, gets a `mask`: two places in line for
  `VK_MASK` (0xFF, a key Windows does not know), pressed then released,
  which the app sends on as deferred transitions. The modifier's release,
  which would pass, is deferred behind them like any transition behind a
  place in line, so the application sees a key between the modifier's
  press and release and does not open its menu bar, Start, or switch the
  keyboard layout. A modifier whose press was swallowed is left out, since
  its release is swallowed too. Every auto-repeat of the gesture gets one,
  and the mask passes back through the hook with its place, so it leaves
  the multi-press count alone.
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
- Multi-press counting (NVDA semantics, M3): every `RaisedGesture` carries
  a `repeat` count — 0 for the first press, 1 for the second, 2 for the
  third, saturating rather than overflowing. Pressing the same bound
  gesture again within `multi_press_timeout` (500 ms) of its previous
  press increments the count; a different gesture or the window elapsing
  resets it to 0. Consumers dispatch on it: report-current (report,
  spell, copy), Verbatim+F12 (time, date), Verbatim+F11 (tray list,
  taskbar list). An auto-repeat — holding the gesture's key, which
  re-fires key-downs with no intervening key-up — counts as a further
  press, measured from the one before, as NVDA counts it, for every
  command (`docs/parity/input.md`, "Script repeat counting"). Any key
  other than a modifier that completes no bound gesture ends the streak,
  as NVDA forgets its last script when an unbound gesture comes between.
  A test's gestures are pressed as keys, so they are counted as a
  person's are.

The hook shell that drives the machine is described in
[verbatim-input-windows](verbatim-input-windows.md).
