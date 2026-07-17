# Mouse and touch

Two pointer-driven exploration surfaces: mouse tracking (speak what
the pointer moves over) and touch interaction (a full gesture system
for touchscreens). Implementation: `source/mouseHandler.py`,
`source/touchHandler.py` and `touchTracker.py`, with the shared
"speak what is at this point" logic in `source/screenExplorer.py`.

## Mouse tracking

`mouseHandler` receives every mouse event from the low-level mouse
hook (`WH_MOUSE_LL`, same hook thread as the keyboard;
[Keyboard input](input.md)) via `internal_mouseEvent`, and on
movement — throttled per core pump — resolves the point and speaks:

- Resolution and speech are `screenExplorer.ScreenExplorer.moveTo`:
  hit-test the point to an object (each API's point resolution;
  [Object model](object-model.md)), and if the object exposes text
  with point geometry, drill to the text at the point and speak it by
  `mouseTextUnit` (default paragraph; character/word/line/paragraph
  configurable) — the reason moving along a line rereads only when
  crossing unit boundaries. Object changes announce the new object;
  `reportObjectRoleOnMouseEnter` adds the role.
- `audioCoordinatesOnMouseMove` plays positional beeps
  (`playAudioCoordinates`): pitch encodes the y coordinate, stereo
  pan the x, and volume follows *screen brightness under the pointer*
  (`detectBrightness` samples the pixels) — the distinctive NVDA
  audio-mouse behavior.
- `enableMouseTracking` gates all of it; injected/synthetic mouse
  input is ignored (`ignoreInjection`) so NVDA's own click commands
  do not narrate themselves; mouse *shape* changes can be announced
  (`updateMouseShape`, off by default).
- Commands, not tracking: route mouse to navigator and click
  commands live in `globalCommands.py`
  (`executeMouseMoveEvent`/`executeMouseEvent` do the moving and
  clicking, restricted to real screen bounds across multiple
  monitors — `getMouseRestrictedToScreens`).

## Touch

`touchHandler` registers NVDA as a touch consumer (the
`RegisterPointerInputTarget` machinery with the raw
`POINTER_TOUCH_INFO` structures) on Windows touchscreens, so *all*
touch input goes to NVDA first while enabled — the screen-reader
touch model where touching explores instead of activating.

- `touchTracker.py` turns raw pointer events into a gesture
  vocabulary: taps, flicks (directional), hovers (finger resting and
  moving — the "explore by touch" primitive), holds, and multi-finger
  and hold-plus-tap combinations; each becomes a `TouchInputGesture`
  through the normal input system ([Keyboard input](input.md)), so
  touch gestures are bindable in the gesture map like keys.
- *Touch modes* (`TouchMode`) namespace the gesture set — text mode
  and object mode ship by default (a three-finger tap cycles) — so
  the same flick means next-character in text mode and next-object in
  object mode; entering browse mode switches touch bindings
  accordingly (`_browseModeStateChange`).
- Hover exploration routes through the same `ScreenExplorer` as the
  mouse, with an immediate core pump requested per hover event but
  deliberately not for *every* hover to avoid pump starvation (the
  `_immediate` note in `TouchInputGesture`).
- Windows 11 constraint tracked in NVDA: registering as a global
  touch consumer requires uiAccess there
  ([Secure mode](secure-mode.md) context; the handler refuses to
  initialize without it on Windows 11).

## What this means for parity planning

Mouse tracking is cheap once point-to-node and point-to-text
resolution exist (both needed anyway); its NVDA-specific behaviors
worth copying exactly are the text-unit throttling, brightness-scaled
audio coordinates, and injection filtering. Touch is a much bigger
lift (raw pointer capture, a tracker state machine, a mode system)
and is hardware-gated for testing — the natural read of NVDA's design
is that everything except the capture layer reuses machinery a screen
reader already has (gesture maps, explorer, review).
