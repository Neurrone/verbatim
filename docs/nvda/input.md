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
(announce the gesture and its script instead of running it) and
gesture-to-speech echo (`speechEffectWhenExecuted`, e.g. speech
interruption on any key press when `speechInterruptForCharacters`).

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
