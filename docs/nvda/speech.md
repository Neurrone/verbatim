# Speech: sequences, the manager, priorities, say-all

NVDA's speech subsystem (`source/speech/`) sits between announcement
generation and the synth driver ([Synth drivers](synth-drivers.md)). Its currency is
the *speech sequence*; its scheduler is `SpeechManager`, whose own
docstring (`speech/manager.py`, class `SpeechManager`) is a 15-step
flow-of-control description and the single best reference — this file
condenses it.

## Speech sequences and commands

A speech sequence is a list of strings interleaved with command
objects (`speech/commands.py`): `IndexCommand` (a numbered marker the
synth reports back when reached), `CallbackCommand` (run a function
when reached), `BeepCommand`/`WaveFileCommand` (sounds at points in
speech), `EndUtteranceCommand` (force an utterance break),
`CharacterModeCommand` (spell mode), `LangChangeCommand` (language
per run), `BreakCommand`, and parameter changes
(`PitchCommand`, `RateCommand`, `VolumeCommand` — `SynthParamCommand`
family; the capital-letter pitch change is one of these),
`ConfigProfileTriggerCommand` (switch config profile mid-stream, e.g.
say-all profiles), and `_CancellableSpeechCommand` (attaches a
validity check; used to drop expired focus announcements —
[Event handling](events.md)).

Announcement content is *generated* into sequences by
`speech/speech.py`: `speakObject(obj, reason)` (property order and
wording via `getPropertiesSpeech` and per-reason logic — the
reference for what a focus announcement contains),
`speakTextInfo(info, unit, reason)` (text with control/format fields
from `getTextWithFields` — [TextInfo](text-infos.md)), `speakMessage`,
`speakSpelling`, plus configurable verbosity (`speechModes`: off,
beeps, talk, on-demand).

### When the role is spoken

Every announcement carries a reason: focus, caret movement, say-all,
quick navigation, a query such as reporting the current object or the
focus on request, and so on. Object navigation (to the parent, the next
or previous object, the first child, or back to the focus) speaks the
new navigator object with the focus reason, and so is a container
entered as focus moves into it: apart from leaving out some properties,
focus entered is spoken like focus. The object's role is spoken except when three things hold at once:

- the reason is focus, caret movement, say-all, or quick navigation;
- the object has a name, a value, or table cell coordinates, so there
  is something else to hear; and
- its role is one of the roles NVDA leaves silent on focus: pane, root
  pane, frame, unknown, application, table cell, list item, menu item,
  check menu item, tree view item, static text, and border.

So focusing a named list item says "alpha.txt, 1 of 3", not "alpha.txt,
list item, 1 of 3", and so does navigating to it as an object; an
unnamed list item with no value still says "list item"; entering a list
as a container still says "list", since a list is not one of these
roles; and reporting the current object or the focus on request keeps
the role. A custom role text, where an
object supplies one, is always spoken. Toast and alert objects are
spoken with the focus reason, and so are items selected in a list the
focus controls. (`getPropertiesSpeech` and `silentRolesOnFocus` in
`controlTypes/role.py`.)

### Which states are spoken, and in what order

States are spoken in one fixed order, whatever order the object
reported them in: unavailable, focused, selected, busy, pressed, checked,
half checked, read only, expanded, collapsed, submenu (has popup),
protected, required, invalid entry, off screen. A negated state ("not selected", "not checked",
"not pressed") takes the same place in that order as its positive form,
so an unchecked, unavailable check box is "check box unavailable not
checked".

Some states are never worth hearing, whatever the reason: that an
object is focusable, selectable, or checkable says nothing the role and
the negated states do not. A combo box always has a popup, so its
"submenu" is dropped, and so are expanded and collapsed on a menu item
that opens a submenu.

A query, such as reporting the current object, speaks every remaining
state. Any other reason leaves out a few more:

- focused and offscreen, which describe where the object is rather than
  what it is;
- selected, on a list item, tree view item, menu item, table row, or
  check box that can be selected: selection is the expected state of a
  focused item, so only its absence is worth hearing;
- read only, on anything but an edit field or a check box, since most
  roles cannot be changed anyway.

Three states are spoken by their absence:

- "not selected", for a list item, tree view item, table row, table
  cell, row or column header, or check box that is both selectable and
  focusable, when it gains the focus or changes state while focused;
- "not checked", for a check box, a radio button, or anything else that
  says it is checkable, unless it is half checked; a change of state
  says it only on the focus;
- "not pressed", for a toggle button.

A change of state speaks only the states that changed: the ones gained,
and of the ones lost, those that would be spoken by their absence.
(`processAndLabelStates` in `controlTypes/processAndLabelStates.py`,
with the order from `controlTypes/state.py`.)

### When values and descriptions are spoken

A check box, radio button, link, menu item, application, or busy
indicator never speaks its value: its states, or its target in the case
of a link, are what the user needs, and the value is often a URL or an
internal string. A description identical to the name is dropped, since
it would only say the name twice; a description that changed is still
spoken, even when it now matches the name.

A change of value is spoken only for the focus, and only when the value
is different from the one last spoken for that object. An edit field
never speaks its changes of value, since typing already echoes the
characters and the caret reports the text; speaking the whole field
after every keystroke would drown both. (`speakObjectProperties` and
`silentValuesForRoles` in `controlTypes/role.py`; the edit field rule is
`event_valueChange` on NVDA's editable text classes.)

## The manager

`SpeechManager` (all on the main thread, by design):

- **Priorities** (`speech/priorities.py`): three — `NORMAL`, `NEXT`
  (speak after the current lower-priority utterance), `NOW`
  (interrupt lower-priority speech immediately; interrupted speech
  *resumes afterward*). One queue per priority; the manager always
  serves the highest non-empty queue. A NOW arrival mid-utterance
  interrupts the synth, and the interrupted utterance's remainder is
  resumed later with its parameter state re-applied
  (`ParamChangeTracker`, docstring steps 12–14).
- **Indexing**: the manager owns all index numbers. It splits input
  sequences at utterance ends, guarantees an index terminates every
  utterance, and maps indexes to callbacks; `synthIndexReached`
  notifications (from the driver, any thread) are queued back to the
  main thread, where `_handleIndex` fires callbacks and pushes the
  next utterance (`_pushNextSpeech`). Index-reached is also how
  say-all knows to keep feeding (below) and how braille stays in
  step where applicable.
- **Profile switches** ride the queue as commands and are applied
  only between utterances, waiting for the synth to report done
  speaking first (steps 5, 9) — the mechanism behind "different voice
  for say-all" without mid-word switches.
- **Cancellation**: `speech.cancelSpeech()` clears queues and calls
  the driver's `cancel()`; cancellable commands are additionally
  culled when their validity check fails while still queued
  (`removeCancelledSpeechCommands`) — the expired-focus case.

## Say-all

`speech/sayAll.py` (`SayAllHandler`): continuous reading from caret
or review position, and object say-all. Implementation shape: a
generator (registered with `queueHandler`; [Main loop and watchdog](main-loop-and-watchdog.md))
walks the text one `UNIT_READINGCHUNK` at a time, speaking each chunk
with a `CallbackCommand` at its end; the callback advances the caret
(or review position) to the spoken chunk and requests the next chunk
only when playback nears the end of what is queued — so the caret
tracks the audio, lookahead stays bounded, and stopping (any key)
both cancels speech and leaves the caret where reading stopped.
Structure changes mid-read (the document mutating) surface as the
TextInfo failing to move, ending the run gracefully.

## Automatic language switching

`speech/languageHandling.py` decides whether the `LangChangeCommand`s
that content generation produces (from IA2/UIA language attributes,
document language, Unicode-range detection for some scripts) actually
reach the synthesizer: with `autoLanguageSwitching` off they are
stripped from the sequence (and a reset command appended) so the
configured voice language always speaks; with it on they pass
through, and `autoDialectSwitching` separately controls whether
same-language dialect changes (en-US to en-GB) count. The driver
maps surviving language changes to voice or language selection as it
supports ([Synth drivers](synth-drivers.md)). Related normalization:
Unicode normalization of spoken text is itself configurable and
suppressible per sequence (`SuppressUnicodeNormalizationCommand`),
with normalized-character reporting during spelling.

## Behavioral details worth copying exactly

- Speech is *serialized through one manager*; nothing speaks around
  it, so ordering is global and deterministic given queue arrivals.
- Focus announcements attach cancellation validity checks; value
  changes and command echoes do not — the asymmetry is deliberate
  (stale focus is harmful, stale value announcements merely late).
- Interruption semantics are per-priority, not global: NOW does not
  cancel other NOW speech already queued.
- The speech viewer (`source/speechViewer.py`) and the
  `speech.extensions` action hooks (`speech/extensions.py`,
  `filter_speechSequence`) tap the stream post-manager — the
  extension point add-ons use to observe or rewrite speech.
