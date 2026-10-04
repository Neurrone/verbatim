# Keyboard input, gestures, and scripts

The pipeline: a low-level hook thread sees every key, the keyboard
handler decides trap-or-pass and builds a gesture, the input core
resolves the gesture to a *script* through a priority chain, and the
script runs on the main thread. IME/composition reporting rides a
separate in-process channel.

## The hook layer

`source/winInputHook.py`: a dedicated thread installs
`WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks and pumps messages
(`hookThreadFunc`); callbacks forward to registered functions with
(vkCode, scanCode, extended, injected). The callback path is kept
minimal — everything heavy is deferred — because a slow low-level
hook is silently removed by the OS.

## The keyboard handler

`source/keyboardHandler.py` (`internal_keyDownEvent` /
`internal_keyUpEvent`) implements the trap/pass policy:

- NVDA-modifier handling (`isNVDAModifierKey`: Insert variants and/or
  Caps Lock per config): the modifier is trapped
  (`trappedKeys`) and replayed as a real key only if released without
  a companion key (the double-press pass-through and "sticky"
  handling: `stickyNVDAModifier` for one-shot/locked states).
- "Pass next key through" (NVDA+F2) sets `passKeyThroughCount` so the
  next key skips NVDA entirely.
- Injected input (the `injected` flag, plus NVDA's own injections) is
  ignored by default so NVDA does not react to itself.
- Everything else becomes a `KeyboardInputGesture` (same file; it
  computes the display name, tracks modifier state itself, and knows
  how to `send()` itself — key echo of characters and words also
  hangs off this path, fed by typed-character reports; see below).
  The gesture goes to `inputCore.manager.executeGesture` *on the hook
  callback*, which queues script execution to the main thread; if a
  script claims the gesture, the key is swallowed (hook returns
  nonzero), else it passes to the app.

## Gestures and mapping

`source/inputCore.py`: an `InputGesture` is source-agnostic (keyboard,
braille display keys, touch); `GlobalGestureMap` holds user and
default bindings (gesture identifier to (module, class, script
name)), loaded from `gestures.ini` and add-ons; the Input Gestures
dialog edits it. `executeGesture` also implements *input help mode*
(announce the gesture and its script instead of running it) and each
gesture's effect on speech (`speechEffectWhenExecuted`; see the next
section).

## What a key press does to speech

Every key-down that reaches the input core changes speech before its
gesture does anything else, whether or not the gesture is bound to a
script. `executeGesture` reads the gesture's `speechEffectWhenExecuted`
and queues the effect on the main thread before it queues the script,
so the cancel always runs first and never cuts off speech the script
itself produces. Modifiers count: pressing Control alone, the NVDA
modifier, or Alt cancels speech, and so does a typed character that
NVDA passes straight to the application. Key-ups have no effect. Keys
NVDA never sees as gestures, such as those passed through after "pass
next key through" or injected keys NVDA is set to ignore, change
nothing.

For a keyboard gesture the effect is decided by
`_get_speechEffectWhenExecuted` in `keyboardHandler.py`, in this order:

- While input help is on, every key cancels.
- The volume keys (mute, volume down, and volume up, as extended keys)
  leave speech alone, so the user can adjust the volume of what is being
  said.
- Virtual-key code `0xFF`, a key Windows does not know, leaves speech
  alone: some devices report events such as a gyroscope moving with it
  (NVDA issue 3468).
- With the setting "Speech interrupt for typed characters"
  (`speechInterruptForCharacters`) turned off, a typed character, and
  Shift, leave speech alone. A typed character here is a key whose name
  is a single character, or Space, pressed with no modifier or with
  Shift alone; a lock key also counts here, since it is not reported as
  a command. So with the setting off, typing does not cut speech off,
  and Shift no longer pauses it, while command keys, such as arrows,
  Tab, Enter, and modified letters, still cancel.
- With the setting "Speech interrupt for Enter" (`speechInterruptForEnter`)
  turned off, Enter leaves speech alone, so the user can submit a line
  without cutting off what is being read.
- Shift (left, right, or generic) pauses speech, or resumes it when
  speech is paused.
- Every other key cancels speech.

Both settings are on by default, so by default every key except the
volume keys, the unknown key, and Shift cancels speech.

Holding Shift does not stutter. Windows repeats a held key's key-down,
and NVDA's key-down handler drops a repeated press of a modifier whose
effect is pause or resume when that key is already among the held
modifiers, so one physical press pauses or resumes once.

Pausing and cancelling interact through the speech state (`speech.py`):

- `pauseSpeech(switch)` tells the synthesizer to pause or resume and
  records whether speech is paused.
- `cancelSpeech()` cancels the speech manager and the synthesizer and
  clears the paused flag, so a key press while paused ends the pause by
  discarding what was paused.
- `speak()` cancels first when speech is paused: new speech arriving
  while paused, for example from a focus change, throws the paused
  speech away rather than queueing behind it, and is heard at once.

## Script resolution

`scriptHandler.findScript` — the priority chain, exactly
(`_yieldObjectsForFindScript`):

1. the gesture's own scriptable object (e.g. a braille display's
   modal state),
2. global plugins,
3. the focus's app module,
4. the braille display driver, then active vision providers,
5. the focus's tree interceptor when ready — *filtered by
   pass-through*: in focus mode the interceptor's scripts are skipped
   unless marked `ignoreTreeInterceptorPassThrough`; browse mode can
   also substitute alternative scripts (`getAlternativeScript` —
   how quick-nav letters exist only in browse mode; [Browse mode](browse-mode.md)),
6. the focus NVDAObject itself,
7. focus *ancestors* (innermost last), only scripts marked
   `canPropagate`,
8. configuration-profile activation scripts, then global commands
   (`globalCommands.commands` — the big default command set).

While locked, only allow-listed "safe" scripts run
(`utils.security.getSafeScripts`). `executeScript` tracks repeat
counts (`getLastScriptRepeatCount`) — the once/twice/thrice behaviors
throughout NVDA (spell on second press, copy on third) are all
time-window repeat counting here, not per-feature timers.

## Typed characters, IME, and composition

Out-of-process key events cannot tell NVDA what an app actually
inserted ([Input and text](../explainers/input-and-text.md)), so this comes from
the injected side ([Process injection](process-injection.md)):

- `typedCharacter.cpp` hooks `WM_CHAR` delivery in-process and
  reports each real inserted character
  (`nvdaControllerInternal_typedCharacterNotify`) — the source for
  speak-typed-characters/words in classic controls.
- `ime.cpp` and `tsf.cpp` hook IMM32 and TSF composition:
  composition string updates
  (`nvdaControllerInternal_inputCompositionUpdate`), candidate list
  changes (`..._inputCandidateListUpdate`), IME open/conversion mode
  changes; `source/NVDAObjects/inputComposition.py` materializes the
  composition and candidate UI as NVDAObjects so reading behavior is
  normal object/text reading.
- `inputLangChange.cpp` reports keyboard layout switches
  (`..._inputLangChangeNotify`), announced as language changes.

Modern (UIA-era) IME candidate UI additionally arrives through UIA
events; the MSAA-side menu events from the candidate window are
deliberately dropped to avoid double handling ([MSAA and winevent handling](msaa.md)).
