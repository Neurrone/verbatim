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
entered as focus moves into it: focus entered is spoken like focus,
states and position included, apart from leaving out the value, the
level, and the keyboard shortcut (kept for a list, the one container whose
shortcut is known to work). The object's role is spoken except when three things hold at once:

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

### Where the level goes

A tree view item or list item that reports a level speaks it as "level"
and the number, and where it goes depends on the level spoken before it.
NVDA remembers the last such level it put first. When an item's level
differs from that one, the level is spoken first, before the name, and
becomes the remembered level; when it is the same, the level is spoken
last, after the position. So moving into a tree says "level 1 Hardware
Resources collapsed 1 of 3", the next item at the same depth "Components
collapsed 2 of 3 level 1", and moving back out to the root "level 0 System
Summary". The remembered level belongs to speech as a whole, not to one
tree or one reason: a focus, object navigation, a selection spoken in a
list, and reporting the current object all read and update it, and nothing
resets it. Any other role that reports a level speaks it last, without
reading or updating the remembered level, and a container the focus enters
speaks no level at all. (`getPropertiesSpeech` in `speech/speech.py`, with
`_speechState.oldTreeLevel`.)

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

For a change of state, "focused" means the object's own new states
still include focused, not merely that it is the object the screen
reader last announced as the focus. Moving through a list or tree,
many controls take the selection and the focus from the old item
before the focus event for the new one arrives, so the old item's
change still reaches the screen reader while it is the last focus;
it no longer reports itself focused, and its "not selected" or "not
checked" is not spoken. Any state it gained, such as expanded, still
is.

A change of state speaks only the states that changed: the ones gained,
and of the ones lost, those that would be spoken by their absence.
(`processAndLabelStates` in `controlTypes/processAndLabelStates.py`,
with the order from `controlTypes/state.py`.)

### How many items an expanded tree view item holds

When an item of a Win32 tree view (the `SysTreeView32` common control,
read through MSAA) is expanded while it is the focus, NVDA says how many
items it now holds: after the change of state, which says "expanded",
it speaks the number of the item's own children as a message of its
own, "1 item" for one and "52 items" otherwise, "0 items" included.
Only the item's direct children are counted, not their descendants; NVDA
counts them by walking the control's items, from the item's first child
through each next sibling.

It does so only on a change of state, never on the focus or object
navigation, and only when the change makes the item expanded: the item
is the focus, its new states include expanded, and the states last
spoken for it did not. A further change of state while it stays
expanded, or collapsing it, says no count. No other tree view says it:
not a tree item read through UIA, not one of a Qt or web tree, and not
any other role. (`TreeViewItem.event_stateChange` in
`NVDAObjects/IAccessible/sysTreeView32.py`.)

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

### What an object with text says

An object with navigable text speaks its text in place of its value. Such
an object is an edit field, a document, a terminal, or anything else
marked editable, and only when it has a real text interface: an object
whose only text is its name and value, read through NVDA's generic
fallback, does not count, and speaks its value like any other object.

For one of these, the announcement leaves the value out, and after
everything else it would say (name, role, states, description, shortcut,
position) it speaks the text at the caret:

- when text is selected, "selected" followed by the selected text, or by
  its number of characters when there are 512 or more; this order, with
  "selected" first, warns that typing would replace text the user has not
  heard being selected;
- otherwise, the line containing the caret, read as a caret movement would
  read it, so an empty line says "blank". A control that cannot report its
  selection reads its first line. When the whole text is empty and the
  object has a placeholder, the placeholder is spoken too.

For a single-line edit field the line is the whole value, so the field
says what it would have said with its value, only later in the
announcement. This applies to focus, to object navigation (which speaks
with the focus reason), and to reporting the current object or the focus
on request. It does not apply to a container entered as the focus moves
into it, or to an object under the mouse, which speak neither value nor
text. NVDA does no masking of its own here: a protected field's text is
read through its text interface, which gives the masked characters the
control shows. (`getObjectSpeech` and `_objectSpeech_calculateAllowedProps`
in `speech/speech.py`; `_hasNavigableText` on the base NVDA object.)

The object does not need the focus. Object navigation and reporting the
current object read the same text for an edit field the user has not
focused, through the same calls: the selection is asked of the control
itself, which keeps its selection, and so its caret, while it does not
have the focus. A Win32 edit control answers from its own selection; a UI
Automation control answers from its text pattern's selection, which most
providers report without the focus. When the control reports no selection
at all (UI Automation gives an empty array, or the call fails), the first
line is read instead, so a field the user never entered usually reads its
first line. (`UIATextInfo` for `POSITION_SELECTION` in
`NVDAObjects/UIA/__init__.py`.)

Reporting the current object a second and third time spells and copies
the same text for such an object: the name followed by the selected text,
or by the line at the caret when nothing is selected, rather than the name
and the value. An object without a real text interface spells and copies
its name and value. (`script_navigatorObject_current` in
`globalCommands.py`.)

### Line breaks in spoken text

Before any text reaches the synthesizer, NVDA replaces every carriage
return, line feed, and null character with a space, after symbol
processing (`processText` in `speech/speech.py`). Text with line breaks
in it, such as a selection over two lines or a multi-line value, is
spoken with a space between one line's last word and the next line's
first, never as one run-together word. A text made only of spaces
and line breaks is blank.

### A word of one character

When NVDA speaks a word or a character of text, such as the word at the
caret after Control with Left or Right Arrow, or the review cursor's
word, and that text is a single character once surrounding white space
is set aside, it spells it rather than speaking it as text: the
character is spoken by its name, as a character is when the caret moves
over it. A full stop that the application's word unit counts as a word of
its own (Windows 11 Notepad's does, after the last word of a sentence)
is therefore spoken "dot", where as text, at the default symbol level,
it would say nothing. A word of two or more characters is spoken as
text. (`getTextInfoSpeech` and `_getTextInfoSpeech_considerSpelling` in
`speech/speech.py`.)

### Capitals when spelling

When NVDA spells, whether spelling a word or line on request or speaking
a single character as the review cursor or caret moves over it, each
uppercase letter is spoken with the pitch raised: the letter is preceded
by a pitch command offset by the synthesizer's "capital pitch change"
setting (`capPitchChange`, default 30, from -100 to 100) and followed by
a pitch command that returns to the configured pitch. The offset is added
to the user's pitch setting, so with pitch at 50 a capital is spoken at
80; the synthesizer limits the result to its range. Two other settings,
both off by default, can also mark capitals: saying "cap" before the
letter (`sayCapForCapitals`) and a short beep (`beepForCapitals`).
(`_getSpellingCharAddCapNotification` in `speech/speech.py`.)

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
  (`removeCancelledSpeechCommands`) — the expired-focus case. What
  cancels speech is listed under "What a key press does to speech" in
  [Keyboard input](input.md) and "The focus gate" in
  [Event handling](events.md): every key press but a few, a change of
  foreground, and entering a menu. Focus speech is otherwise queued,
  never interrupting.
- **Expired focus speech**: a focus announcement carries a
  `FocusLossCancellableSpeechCommand` (`eventHandler.py`) for the object
  it is about. That speech stays valid while any of these holds:
  - the object is the focus;
  - the object never had the focus, which is the case for an entered
    container announced by `focusEntered` (the object is marked as
    having had the focus when the command is made while it is the
    focus);
  - the object is an ancestor of the focus;
  - the object is the foreground object, so a dialog's title survives
    the focus moving into the dialog;
  - the object is an MSAA menu item, check menu item, or radio menu item
    with a parent, and the focus is now a popup menu: some applications
    focus a submenu's first item and then the submenu itself, and the
    item must still be spoken (NVDA issues 12624 and 14550).

  The check runs at two moments. On each focus change, after the new
  focus and its ancestors are set (`doPreGainFocus`), the manager looks
  at the utterances it has already handed to the synthesizer, finds the
  newest whose check fails, removes everything in the queue up to and
  including it, cancels the synthesizer, and pushes the next speech, so
  later queued speech is still heard. And an utterance still waiting in
  the queue is checked when it comes up to be handed to the
  synthesizer (`_checkForCancellations`); if it has expired it is
  dropped and the next one is tried. A queued utterance that has expired
  therefore never costs speech queued before it. The advanced setting
  "Attempt to cancel speech for expired focus events"
  (`cancelExpiredFocusSpeech`, on by default) turns both checks off.

## Say-all

`speech/sayAll.py` (`SayAllHandler`): continuous reading from caret
or review position, and object say-all. Implementation shape: a
generator (registered with `queueHandler`; [Main loop and watchdog](main-loop-and-watchdog.md))
walks the text one `UNIT_READINGCHUNK` at a time, speaking each chunk
with a `CallbackCommand` at its start (`_TextReader.nextLine`); when
playback reaches the callback, it moves the caret (or review position) to
the start of that chunk (`lineReached`, then `updateCaret`, which collapses
the chunk's range to its start before selecting it) and asks for the next
chunk, so the caret tracks the audio, lookahead stays bounded, and
stopping (any key) both cancels speech and leaves the caret at the start
of the chunk whose callback playback reached last. The first chunk runs
from the caret to the end of its unit (the range's end is moved by a
chunk, not expanded), so reading starts at the caret, not at the start of
its line.
Structure changes mid-read (the document mutating) surface as the
TextInfo failing to move, ending the run gracefully.

The reading chunk is configurable: `UNIT_READINGCHUNK` resolves through
`TextInfo.unit_readingChunk` (`textInfos/__init__.py`) to the
`speech.sayAllReadingUnit` feature flag, whose options are "Sentence
where possible" (the default), Paragraph, and Line ("Say all reads by"
in the Speech settings panel). Offsets-based TextInfos find sentences
with ICU (UAX 29) over the containing paragraph
(`OffsetsTextInfo._getSentenceOffsets` in `textInfos/offsets.py`) and
fall back to the line where a unit is not implemented; UIA TextInfos
always read by line, because `UIAHandler.NVDAUnitsToUIAUnits` maps the
reading chunk to `TextUnit_Line` (UIA has no sentence unit).

### Say-all speaks without pauses

Every chunk say-all reads, whatever the unit, passes through
`SpeechWithoutPauses` (`speech/speechWithoutPauses.py`) before it reaches
the synthesizer, so what one call to the synthesizer speaks is not the
chunk but a run of text ending at a sentence end. This is how NVDA reads
UIA text, which it reads by line, sentence by sentence: in Windows 11
Notepad a line holding the end of one sentence and the start of the next
is spoken in two calls, the first ending at the full stop.

- A sentence end is a full stop, exclamation mark, or question mark that
  directly follows a character that is neither whitespace nor one of those
  three marks, optionally followed by one closing character (a straight or
  curly double or single quotation mark, or a closing parenthesis), and
  then followed by whitespace or the end of the text.
- In each chunk's text, the last sentence end is found. Everything up to
  it, with the whitespace after it, is spoken now, after anything held
  back from earlier chunks; what follows it is held back. A chunk with no
  sentence end is held back whole.
- What is held back is spoken at the start of the next call, so a
  sentence that runs from one line to the next is spoken in one call: the
  rest of the first line, then the second line up to its last sentence
  end. The second line's callback sits in that call where the second
  line's text starts, so the caret moves to the second line when its text
  starts playing, not when the call starts. A line's callback is held back
  with its text, so it always plays just before that line's words.
- Only the last sentence end in a chunk splits it. A line with two
  complete sentences and the start of a third is spoken as the two
  sentences together, then the third with whatever follows it.
- Because the rule is only these characters, there is no list of
  abbreviations: "Dr. Smith" splits after "Dr. ", as does "e.g. this".
  A full stop inside a number ("3.14") is followed by a digit, not
  whitespace, so it does not split, nor does an ellipsis ("wait..."),
  whose last full stop follows another. One closing quotation mark or
  parenthesis between the mark and the whitespace still splits; two
  closing characters, a quotation mark and then a parenthesis, do not.
  Full-width terminators such as the ideographic full stop are not
  sentence ends here.
- Ten chunks in a row with no sentence end are not held back further:
  after the tenth (`_TextReader.MAX_BUFFERED_LINES`), everything held back
  is spoken in one call. Blank chunks count among the ten, and say nothing.
- When the text ends, whatever is held back is spoken
  (`_TextReader.finish` flushes it with an end of utterance).

While a say-all runs, NVDA keeps the system awake: `_Reader.start`
(`speech/sayAll.py`) calls `systemUtils.preventSystemIdle(persistent=True)`,
which also keeps the display on when the general setting
`preventDisplayTurningOff` is set (the default), and `_Reader.stop`
releases it (`systemUtils.resetThreadExecutionState`).

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
