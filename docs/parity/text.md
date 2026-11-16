# Text and documents

The text layer, caret and selection reporting, segmentation, and documents.

### TextInfo-equivalent layer

The text layer: caret-key reporting by waiting for evidence, and
selection deltas. NVDA: [TextInfo](../nvda/text-infos.md) and
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: **matched (verified)** for UIA text and the Win32 edit and
rich edit controls, through the text protocol
(`books/dev/src/reference/crates/verbatim-model.md`, "The text protocol";
`books/dev/src/reference/crates/verbatim-core.md`, "Text"; the
`notepad_editing` and `notepad_review_cursor` scenarios); an MSAA object
that is neither has no text.

- Rich edit text: a rich edit window that is not Unicode has its text
  decoded by what its buffer holds, UTF-16 or ANSI, as NVDA decodes it
  ("Rich edit text" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)).
- Rich edit paragraphs: **different** until M6.5. NVDA reads a rich edit
  control through its object model whenever it can get that model
  (`ITextDocumentTextInfo` for any `editAPIVersion` other than 0,
  `nvda/source/NVDAObjects/window/edit.py` 1041 to 1045), whose
  paragraph is the text between line breaks; Verbatim reads it through
  window messages, and its paragraph unit is the line as shown, as in a
  plain edit control, so after Control+Up or Control+Down Arrow, which
  the application moves by its own paragraphs in both, Verbatim speaks
  the wrapped line where NVDA speaks the paragraph, and its sentences,
  for say-all and for moving by sentence, stop at a wrapped line's end.
- NVDA's `caretMovementFailed` event is not offered.

The layer's cases are the entries that follow, from "Caret keys and
what each speaks" to "Not yet built". The end-to-end steps that declare
a difference from NVDA against this entry show the differences recorded
in "A caret key that moves nothing", "What counts as a caret key's
evidence", "Holding caret keys", "Keys outside the caret table",
"Selecting with Shift" and "NVDA's delayed character description at the
caret".

### Caret keys and what each speaks

NVDA speaks its unit for each caret key once the caret has moved
([Editable text and terminals](../nvda/editable-text-and-terminals.md)).
Verbatim: **matched (verified)** (the `caret_keys` scenarios,
`notepad_editing`, `notepad_review_cursor`). A caret key is held: at
its turn in Core's event queue the outpost reads the caret, Core passes
the key on and asks the outpost to watch for the key's effect, and Core
never blocks on the answer
(`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the
event queue"). Core then
speaks NVDA's unit: the character for Left and Right Arrow, Home, and
End; the provider's word for Control with Left or Right Arrow, a word of
one character (Notepad's full stop) by its name, as NVDA spells it; the
line for Up and Down Arrow, the page keys, and Control with Home or End;
the paragraph for Control with Up or Down Arrow; what Backspace deleted;
and what Delete left at the caret.

- NVDA's other forms of these keys are in the caret table too:
  Shift+Backspace as Backspace, Shift+Delete (which cuts a selection)
  and the numpad's Delete with and without Shift as Delete, and Control
  with the numpad's Delete as Control+Delete (the `caret_keys`
  scenarios, from the captures of 2026-10-23 in
  `target/e2e-evidence/m42-text-captures-2026-10-23/`, items 4a to 4c).
- Alt with Up or Down Arrow, NVDA's sentence keys, speak the line when
  the application moves the caret, as NVDA does.
- NVDA's wait ends silently when a script is queued behind the key
  (`isScriptWaiting`, `scriptHandler.py` 333 to 334, counted by
  `queueScript`, 240 to 244), which a modifier alone never queues and an
  unbound key, such as a typed letter, queues only while an intercepted
  command's script has not finished (`inputCore.py` 629 to 630 and 644
  to 648), as a caret script is; and when a focus is pending, queued but
  not yet handled ("While a focus is pending" in
  `docs/parity/focus-events.md`). Verbatim does the same: a later key
  other than a modifier, a command, or a key Core sends, joining the
  queue behind a caret key, marks it silent, and its watch is withdrawn;
  a focus pending when Core judges the key withdraws its request too.
  Either way the key says nothing, except Backspace and
  Control+Backspace, which with a focus pending say the text they
  delete before the focus, as NVDA's `_backspaceScriptHelper` speaks it
  (`editableText.py` 293 to 321; Dickson, 2026-11-16).
- A line break a unit lands on is named as NVDA names it ("A line break
  as a character" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)):
  End and Right Arrow at a line's end say "carriage return"; the review
  cursor's end of line is the line's last character, its break
  included, "carriage return" in Windows 11 Notepad and "line feed" in a
  standard edit control, and moving by character crosses each character
  of the break; Backspace over a line break says "carriage return" in
  Notepad and "line feed" in a standard edit control (the
  `notepad_editing` and `notepad_review_cursor` scenarios). A
  terminal's row is cells, with no line break to land on.

### The caret a key starts from

NVDA runs one script at a time, so reading the current line waits for an
earlier Down Arrow's script to finish (`nvda/input.md`, "Scripts run one
at a time"). Verbatim: **matched (verified)** (`notepad_editing` under
load). A caret key's starting read, made at the key's turn in Core's
event queue before the key is passed on, is the point the key is judged
from: for its caret, for what Backspace deletes and for the text Delete
compares. It is taken with no wait for the effect of the key ahead, as
NVDA's next script reads the caret as soon as the one ahead returns
(`editableText.py` 94 to 95; Dickson, 2026-11-16). Core takes each caret
answer as the caret in the order the answers arrive, since one worker
answers the event queue's requests one at a time
(`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the
event queue", owns the mechanism).

A review command while the review cursor follows the caret, say-all from
the caret, and the caret's location run at their turn in the event
queue and read the caret as the outpost finds it. A command pressed
behind a caret key marks the key silent and reads at once, so it may
read the line before the key's move, as NVDA's capture does
([holding caret keys](../../books/dev/src/explanation/design/key-holding.md),
"Limitations").

### A caret key that moves nothing

NVDA waits 100 ms (300 in Windows Terminal) for evidence and, finding
none, speaks the key's unit anyway: Home at the start of a line reads
the character after the caret, Control+Home at the start of the text
reads the first line, Down Arrow on the last line reads that line, and
Control with Down or Up Arrow in the Windows Forms text box, which does
not move its caret, reads the line (`text_box_paragraph_keys`). With
Alt with Up or Down Arrow where the application does not move the caret,
NVDA, after its wait, moves the caret by sentence itself and speaks the
sentence, "delta epsilon" after Alt+Down on "alpha beta gamma" in the
Windows Forms text box, whose sentences it finds in each paragraph
(`04d-textbox-*` in `target/e2e-evidence/m42-text-captures-2026-10-23/`);
through UI Automation, which has no sentence unit, it moves nothing
(Notepad, where its captures then say "Non Client Input Sink Window", a
window behind the document).

Verbatim: **matched** (Dickson, 2026-10-29 and 2026-11-08). The key's
bound is NVDA's, 100 ms from when the key is passed on, multiplied by
1.5 in a console host older than the improved API level and by 3 in
Windows Terminal, and carried in the watch Core asks for
(`editableText.py` 86 to 88; `NVDAObjects/UIA/winConsoleUIA.py` 365 to
376, 470, 486). A key with no effect by then has one final read,
and a read showing no effect is the bound answer: the line after Up and
Down Arrow, the page keys and Control with Home or End; the character
after Left and Right Arrow, Home and End, "blank" where there is none;
the word after Control with Left or Right Arrow; the paragraph after
Control with Up or Down Arrow; the unit at the caret after Delete,
Shift+Delete and Control+Delete. Backspace and Control+Backspace say
nothing where the caret stayed, and a key with Shift, which is not held,
says nothing for itself. Alt with Up or Down Arrow where the application
moves nothing moves the caret by sentence from the key's starting read,
by Unicode's rules within each paragraph, with the ordinary due time of
a request, as NVDA blocks until it can move (`editableText.py` 245 to
262; Dickson, 2026-11-16), and through UI Automation, which has no
sentence unit, says nothing.

The differences: a move later than the bound is not spoken for the key,
and in a classic edit control or MSAA text, where the watch checks the
caret's position rather than taking the caret event (see "What counts as
a caret key's evidence"), a control that raises a caret event for a key
that moved nothing is heard at the bound where NVDA speaks at once. A
focused classic edit control's system caret
location change and show are caret events to Verbatim; a Common Controls
version 6 edit control, such as a Windows Forms text box with visual
styles, reports it with its text selection event too. An application
whose Up Arrow on the first line, or Down Arrow on the last, moves the
caret to the line's start or end has that move spoken as the key's
answer, as NVDA speaks it.

### What counts as a caret key's evidence

NVDA finds the effect of a key on a password field by the caret event
the control raises within 60 ms of the key, which it takes as evidence
("Protected (password) fields" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)).
NVDA takes such events wherever `caretMovementDetectionUsesEvents` holds,
which is every editable text but its `Terminal` behaviour
(`editableText.py` 57, 104 to 120 and 194 to 206;
`NVDAObjects/behaviors.py` 527 to 530), and elsewhere checks the caret's
position.

Verbatim: **matched** in UI Automation text fields other than terminals
(Dickson, 2026-11-16): a caret or text-change event of the watched node
within 60 ms of the key's pass ends the watch as the key's effect,
whether or not the caret moved, and the outpost reads the caret at once.
Terminals check the caret's position, as NVDA does. **Different** in a
classic edit control and MSAA text, whose caret events reach the focus
listener rather than the outpost that watches, so the watch checks the
caret's position there: the position, the characters either side of the
caret, or the selection must change, and after Delete or Control+Delete
the word at the caret, compared once 30 ms have passed since the pass,
as NVDA compares it for its delete script. For Backspace and Delete,
alone or with Control, the caret's line differing from the line at the
key's starting read is an effect too (Dickson, 2026-10-26), since
deleting one of a run of the same character, such as a password field's
mask, need not change the caret's neighbors.

### Holding caret keys

NVDA's caret scripts read the caret before passing the key on. A caret
script stops waiting for its evidence, and says nothing, the moment
another script is queued, and the next script reads the caret at once
(`_hasCaretMoved`'s short-circuits in
[Editable text and terminals](../nvda/editable-text-and-terminals.md));
a typed character overtakes a held key. 100 ms is NVDA's default wait
for a caret to move (`caretMoveTimeoutMs`,
`nvda/source/config/configSpec.py` 364), which NVDA multiplies by 3 in
Windows Terminal (`_caretMovementTimeoutMultiplier`,
`nvda/source/NVDAObjects/UIA/winConsoleUIA.py` 470 and 486), so 300 ms
there.

Verbatim: **different** ([holding caret keys](../../books/dev/src/explanation/design/key-holding.md),
which owns the mechanism; Dickson, 2026-10-08 and 2026-10-22). Verbatim
holds every caret key while the focus has text, as NVDA's caret scripts
do: the hook swallows it, and at its turn in Core's event queue the
outpost reads the caret and its line and the key is sent on. A held key
behind another takes its starting read at its own turn, with no wait for
the effect of the key ahead, as NVDA's next script does (Dickson,
2026-11-16). The selection keys, the Shift forms of the moves and
Control+A, are not held, as NVDA binds no script to them in a control
that detects its own selection changes. The differences:

- Keys pressed while a held key waits, typed characters included, wait
  behind it and reach the application in the order pressed, one step
  beyond NVDA; the lock keys alone pass at once, since their state is
  announced from Windows' and Num Lock's decides what a numpad key is
  (Dickson, 2026-10-26).
- A held key is sent on 150 ms after its press, whatever waits ahead of
  it in Core's event queue, and then says nothing, so a slow read never
  holds the user's typing (Dickson, 2026-11-13;
  [ADR 0003](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
  "Keys and commands").
- While a window of higher integrity than Verbatim's is in front,
  nothing is held.

### What Backspace and Delete say

NVDA's Backspace and Delete scripts speak what they expect the key to
remove: Control+Backspace in a Windows Forms password box says "4 star"
(`winforms-password-deleting-keys.txt` in
`target/e2e-evidence/m42-protected-fixes-2026-10-22/`).
Verbatim: **matched** (Dickson, 2026-11-16): Backspace and
Control+Backspace say what the key's starting read shows them about to
delete, the character or the text from the word's start before the
caret, or before a selection's start, "blank" at the text's start, once
the key's effect shows, and nothing at the bound, as NVDA's scripts do
([holding caret
keys](../../books/dev/src/explanation/design/key-holding.md); Core's
`backspace_says_what_it_deleted_from_its_starting_read`). Delete and
Control+Delete say the unit now at the caret.

- A backspace over a line break, with or without Control, names the
  kind of break the text was last seen to use, which is what NVDA reads,
  since a text uses one kind throughout, and says nothing when Core has
  seen no line break in it yet: "line feed" in the text box and
  "carriage return" in Notepad, at a later line's start with a selection
  there or none (`selection_keys` scenarios; NVDA's Control+Backspace
  with no selection was captured in Notepad only, where the classic edit
  control's Control+Backspace inserts a DEL character instead).
- In a terminal, Backspace and Control+Backspace say what NVDA says
  ("o" and "tw" after `echo one two`, both terminals).

### Keys outside the caret table

NVDA says nothing for a key outside its caret keys that types no text.
Verbatim: **different** (Dickson, 2026-10-08): such a key (bash's
Control+A, Control+E, Alt+B, Alt+F, Control+W, Control+K, and in a
terminal the editing chords, Escape clearing a line among them) is spoken
by its evidence: text it removed at the caret, the new line when the
caret left its line, and otherwise by where the caret landed (the
character, the word, or the word then the character).

- Escape after `echo hi` in a terminal is silent in NVDA and "echo hi"
  in Verbatim.
- Escape after inline prediction showed `git status` is silent in NVDA
  and "git", the text typed, in Verbatim.
- Escape on a typed line that had wrapped onto a second row says all it
  removed, as on a line of one row, though the caret went up a row; NVDA
  is silent.
- A key outside the caret table that removes a selection, something
  selected before it and nothing after, says only the selection's change,
  never the text it removed, which was the selection's: NVDA has no
  script for these keys.

In a text field the editing chords, Control+Z, Control+Y, Control+X,
Control+V and Escape, match NVDA (Dickson, 2026-10-25): they say nothing
for the key, as NVDA binds no script to them, and a selection change the
application's caret event shows is spoken from that event, as "alpha
selected" after Control+Z restores a cut word (the `caret_keys`
scenarios).

### Selecting with Shift

NVDA: "Selection changes" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: **matched (verified)**. A selection change is said by the
caret event the application raises, as NVDA's is (mockapp's
`a_selecting_keys_selection_is_said_by_its_caret_tell`). Shift movement
speaks the text
followed by "selected" or "unselected", NVDA's word order, a single
character by its name, and 512 characters or more as their number; a
selection
made where nothing was selected, or removed leaving nothing selected, is
said whole, once, wherever the caret was in it (a double-click on "beta"
says "beta selected" in both editors, captured 2026-10-23, item 21a;
mockapp's `a_selection_around_the_caret_is_one_change_each_way`); a
movement without Shift that leaves a selection speaks its unit and then
the text unselected. Each text selected or unselected, and "selection
removed", is an utterance of its own, as NVDA speaks each
(Shift+Control+Home across the selection's anchor says the first line
selected and then the second unselected, `document_keys` scenarios).

Different (Dickson, 2026-10-22): a selection's characters are counted as
grapheme clusters, for the count and for naming a single character,
where NVDA counts code points, so a carriage return and line feed are
one character: Control+A over forty lines of "alpha beta gamma" says
"680 characters selected" in the Windows Forms text box, where NVDA says
720 (`text_box_select_all_count`; Notepad, whose line break is one
character, says 680 in both). A selected carriage return and line feed
has no name of its own and is spoken as white space, which sounds as
NVDA's "selected" with no text (`text_box_caret_keys`).

### A selection no key made

NVDA reports any selection change of a UIA or classic edit focus after
its caret event, whatever made it ("Selection changes" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)),
comparing with the selection it last saw, which it takes when the object
gains the focus and forgets when it loses it. Its event limiter keeps
one caret event per object
(`IAccessibleHandler/orderedWinEventLimiter.py`), so it compares only
the last selection with the one it last saw: Shift+Right followed at
once by Right says only the character, its Shift+Right script saying
nothing while Right's waits (`selection_keys` scenarios, captured
2026-10-22 in
`target/e2e-evidence/m42-key-holding-merge-2026-10-22/captures/`).

Verbatim: **matched** (Dickson, 2026-11-16). Every selection change, a
key's or one no key made, such as one made with the mouse or by the
application, is said from the caret event the application raises: at
the event's turn in Core's event queue, on the committed focus with no
focus pending, the outpost compares the selection now with the selection
baseline, the selection last spoken from, which Core names, and Core
speaks the changes, "alpha selected" and "alpha unselected" (mockapp's
`a_selection_no_key_made_is_said_by_its_caret_tell`). The baseline is
taken when the object gains the focus. Caret events waiting for one
object merge into one entry, as NVDA's limiter keeps one, so only the
last selection is compared. A caret key's own reads compare no
selection, as NVDA's caret scripts compare only the caret. A selection
Verbatim makes itself, select then copy's first press, or a caret it
moves, is spoken by its own reply, if at all, and becomes the next
baseline, so the caret event after it says no change (mockapp's
`a_selection_verbatim_made_is_the_next_baseline`).

### Selection over changed text in a classic edit control

NVDA has one rule for every selection change in an edit control,
whatever key caused it: at each caret event it compares the selection it
last saw with the current one, replaces it, and speaks the change, in
general terms when its note that the content changed is set ("Selection
changes" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)).
NVDA's Enter script reads the new line's text before the caret, which is
empty in every case captured.

Verbatim: **matched (verified)** (the UIA case is the next entry),
against the captures in
`target/e2e-evidence/m42-selection-keys-2026-10-22/` (each named by the
control and the key, such as `textbox-word-backspace.txt`) and
`target/e2e-evidence/m42-desktop-2-2026-10-22/` (`part3b-textbox-*.txt`).
Core keeps one selection baseline for the committed focus, and only a
caret event's read compares with it; a selection Verbatim makes itself
or a caret it moves becomes the next baseline (mockapp's
`a_selection_after_verbatims_compares_with_it`). A caret event whose
selection offsets equal the previous caret event's says nothing, as
NVDA's edit control returns before comparing (`NVDAObjects/window/edit.py`
1068 to 1074). A comparison is in general terms when the text-changed
flag is set (a value change of the focus judged, as NVDA's
`hasContentChangedSinceLastSelection`, cleared at each comparison with a
baseline and by a focus with no caret), or when the control's value
change waits in the event queue at the caret read's answer, which is
then taken as counted, as NVDA runs it inside its caret event
(`NVDAObjects/window/edit.py` 1075 to 1080;
`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and
Core's text facts"). General terms
are the whole new selection followed by "selected" when anything was
selected, then, when anything was unselected, the whole new selection
followed by "selected instead", or "selection removed" when nothing is
selected now. A key's own words come first and the selection's change
after, since the caret event joins the event queue behind the key, as
NVDA speaks its script's words and then its caret event's. Backspace and
Control+Backspace over a selection say what comes before the
selection's start, NVDA's caret (mockapp's
`a_selections_start_is_read_in_the_line_it_starts_in`).

In the Windows Forms text box, over "alpha " or "beta " selected with
Shift+Control+Right Arrow: Backspace says the character before the
selection's start, read before the key, "blank" when there is none ("blank" at the text's
start, "line feed" at a later line's start, and over "beta " "space");
Control+Backspace the text from the word's start to the caret ("blank"
at the text's start, "line feed" at a later line's start, and over
"beta " "alpha"); Delete and Shift+Delete the
character now at the caret ("b"); Control+Delete the word now at the
caret ("beta"); each then "selection removed" (the line breaks from
`01a-*` and `02a-*` in
`target/e2e-evidence/m42-text-captures-2026-10-23/`). Typing over the
selection echoes the character, then "selection removed"; Control+X,
Control+V and Enter say only "selection removed", over a word and over
Shift+End's rest of a line; Control+C says nothing. For Enter neither
says anything more: Verbatim has no read of the new line, since Enter
reaches it as typed text, "\r". The `text_box_selection_keys` and
`text_box_editing` scenarios; mockapp's
`an_edit_controls_delete_over_a_selection_says_its_unit_then_the_selection_removed`,
`an_edit_controls_caret_tell_before_its_value_change_is_said_in_general_terms`
and
`an_edit_controls_value_change_judged_first_has_the_next_change_said_in_general_terms`.

### Selection over changed text through UIA

NVDA notes no UIA text change but for Word and the two terminals, and
compares the old selection's range as it now is, which the provider
moved with the edit, so it names what the range now holds or says
nothing. In Windows 11 Notepad: "x" typed over the selection, "x" then
"x unselected"; Backspace, Delete, Shift+Delete and Control+Delete over a
word, only their own words; Enter over a word or a line's rest,
"carriage return unselected"; Control+X and Control+V over a word or a
line's rest, nothing (the line's rest captured on 2026-10-23, item 25a,
where an earlier capture had "kiwi unselected", the pasted word).
Backspace over "alpha " at a later line's start says only "carriage
return" (`01b-*` and `02b-*` in
`target/e2e-evidence/m42-text-captures-2026-10-23/`; Backspace over
"alpha " was captured again because the first capture had NVDA reading
Notepad as a classic edit control, not through UIA).

Verbatim: **matched** (Dickson, 2026-11-16): the caret event's read
compares the selection baseline's range as it now is, its ends where the
provider moved them with the edit, and names what that range now holds,
or says nothing when the old range and the new selection are both empty,
as NVDA's `speakSelectionChange` compares its kept old range
(`speech/speech.py` 1340 to 1418; mockapp's
`a_uia_selection_typed_over_says_what_its_range_now_holds` and Core's
`an_old_range_now_holding_other_text_is_said_as_it_now_is`). The
deleting keys' case is pinned by `notepad_selection_keys`, since
mockapp's ranges do not follow a deletion. Control+Backspace over "beta
", which Notepad answers by deleting the word before the selection, says
"alpha" then "beta unselected", and over "gamma" to the line's end
"beta" then "gamma unselected"; over "alpha " at the text's start
Notepad deletes nothing and nothing is said; over "alpha " at a later
line's start Notepad deletes the line break before the selection, and
"carriage return" then "alpha unselected" is said
(`notepad-second-word-control-backspace.txt`,
`notepad-third-word-to-end-control-backspace.txt` and
`notepad-word-control-backspace.txt`, NVDA's captures).

### NVDA's delayed character description at the caret

NVDA speaks a delayed character description after the character a key
reaches, "Bravo" after the "b" Delete reaches; for a character with no
description, such as the star or circle Left Arrow reaches in a password
field, it is an utterance with no words, a pause before nothing
(`getSingleCharDescription`, `nvda/source/speech/speech.py` 558).
Verbatim: **different**: a setting Verbatim does not have yet
(`text_box_password`, `wpf_password`).

### Not yet built

Selection detection cases not built. Verbatim: **not yet**.

- Unscheduled: terminals are unresolved: NVDA's console class replaces the detection
  (`NVDAObjects/UIA/winConsoleUIA.py` line 416), which is not yet
  traced, and Verbatim reports no selection change in a terminal, which
  has no selection baseline.

### Terminals recognized by their class

Verbatim: **matched (verified)** (the terminal scenarios assert it).
Terminals are recognized by their UIA class (`TermControl`,
`WPFTermControl`) and the console host's window class, never by title.
The console host's text area is announced as "terminal" without its
English-only name, "Text Area", as NVDA's console class drops it.

### The system caret's events

NVDA handles the system caret's location change and show (`OBJID_CARET`)
as a caret event on the focus. Verbatim: **matched (verified)**, whether
the focus is an edit control read through MSAA or a UIA node: some UIA
documents, such as Visual Studio's, raise no UIA caret event of their
own (mockapp's `the_system_carets_move_is_a_caret_tell_of_a_uia_focus`),
and a caret show from the focused classic edit control is its caret
event (`a_classic_edit_controls_caret_show_is_a_caret_tell`). The
system caret's event is for the focus last told by the readers, whatever
its window, as NVDA maps it onto its last queued focus
(`IAccessibleHandler/__init__.py` 634 to 666), and is read only when
that is Core's committed focus. It is taken from any window NVDA's
acceptance test takes a location change from (the foreground
application's windows, those sharing its root owner, and the rest of
`docs/parity/focus-events.md`, "Event acceptance"), not only from the
focus's own window. The caret's
show from a classic edit control comes with every repaint, since
painting hides and shows the caret, so each repaint reads the caret, as
in NVDA. The differences are in "The system caret beside other caret
events".

NVDA's three exceptions are not built, each recorded for where it
belongs: a focus read through IAccessible2, which Verbatim does not read
before milestone M7; Word's UIA document, which Chinese input methods
flood with these events, a rule of Word's own support; and a focus whose
class refuses the caret's shows and hides
(`shouldAcceptShowHideCaretEvent`), which only Scintilla's editor does
at the pin (`NVDAObjects/IAccessible/scintilla.py`, line 15), since its
caret blinks by them, a rule of Scintilla's support. The last two are on
the roadmap's app-module track, with Word (tier D) and Notepad++ (tier
A). Until then a focused Scintilla editor, such as Notepad++'s, tries to
read its caret at each blink.

### The system caret beside other caret events

A UIA focus's own caret events and the system caret's are both
handled, as NVDA handles both: each reads the caret and compares the
selection with the baseline, so the second event of one move finds no
change and says nothing (mockapp's
`a_system_caret_tell_after_the_uia_one_of_the_same_move_says_nothing`).
Verbatim: **different**, in three cases:

- In a focused console host terminal, the system caret's events are
  never used: the console's caret event (`EVENT_CONSOLE_CARET`) is its
  caret event (`books/dev/src/reference/crates/verbatim-outpost.md`, "Terminals"), read
  once per move, because it also comes for a move of a selection's end
  and Verbatim's terminal reading is built on it (`conhost_editing`;
  `the_console_hosts_system_caret_is_not_its_caret_event`). NVDA does the
  reverse on this build. Windows 11's console host is FORMATTED, so
  NVDA's default console setting ("auto") reads it as its UIA console
  (`_shouldUseUIAConsole`, `UIAHandler/utils.py` 337 to 347), whose focus
  is a UIA object; the system caret's location change is mapped onto
  that focus as its caret event (`IAccessibleHandler/__init__.py` 636 to
  668), and NVDA's console handler (`winConsoleHandler.py`), which only
  its legacy console connects, does not run. The console host also
  raises UIA's text selection changed on its text area for each move,
  which NVDA takes as a caret event too once it has registered its
  focus's local event handlers on the text area
  (`UIAHandler/__init__.py` 278 to 287 and 697 to 739), so NVDA handles
  two caret events for each move. Checked on 2026-10-21 with NVDA's
  debug log of the console opened as `conhost_editing` opens it (both
  events for every move in one launch; in two other launches NVDA did not
  register on the text area and took the system caret's alone), and a
  UIA probe that heard the text area's event at every step
  (`target/e2e-evidence/fixes-round-eight-2026-10-21/`, `nvda-debug.log`
  and `uia-probe-conhost.txt`). The console host raises the system
  caret's location change about 130 microseconds before its console
  caret event, at the same blink tick, for the same move.
- The event's window is checked as readable when the focus listener
  receives the event, before it tells Core: it must be a window, have no ghost window
  standing in for it, and belong to an application that is responding,
  so that a hung application's caret event never leads to a read that
  would wait on it. NVDA checks that the event's window exists for every
  event as its hook receives it
  (`IAccessibleHandler/internalWinEventHandler.py` 113 to 133), then maps
  the caret event onto its focus and makes the ghost window and hung
  application checks only for other events
  (`IAccessibleHandler/__init__.py`, 549 to 576 against 636 to 668).
- A caret show is tested for acceptance as a location change is; NVDA
  skips the test for it and asks only its focus's
  `shouldAcceptShowHideCaretEvent`. The two differ only for a show from
  a window unrelated to the foreground window.

### A text pattern missing at the focus

NVDA fetches a UIA object's text pattern live and keeps it for the
object's life, with a new object for every focus event; an object
without a text pattern has its value spoken in place of its line, and
UIA reports a provider that fails the pattern request exactly as one
with no pattern ([The UIA client](../nvda/uia.md), "Element wrapping").
Verbatim: **matched**: a caret event from the element, which only an
element with text raises, is read through the event's element when the
focus's node had no text pattern (mockapp's
`a_caret_tells_read_adopts_its_element_when_the_node_had_no_text_pattern`);
a caret key on a node with no text is sent on at once and says nothing,
as NVDA's script sends the key when its caret read fails
(`editableText.py` 182 to 186).
Different: when a focus's text has a pattern but cannot be read at all,
nothing is said in place of its line until its caret is next reported,
where NVDA's read raises and nothing more of the focus is spoken.

### Word and character segmentation

Word and character segmentation (Uniscribe grapheme clusters and
word stops) and the three-way paragraph-style setting. NVDA:
[TextInfo](../nvda/text-infos.md). Verbatim: **partial**.

- Segmentation is matched in `verbatim-text` (grapheme clusters,
  Unicode's word rules with dictionaries, jieba for Chinese, a run of
  spaces and tabs as one segment), used by the review cursor over text
  and by spelling; a zero-width space or zero-width no-break space
  counts as white space, not as a word, and text of nothing else reads
  as blank; Han outside the Basic Multilingual Plane counts as Chinese
  for choosing jieba, and the text's language, when the provider gives
  one, tailors Unicode's word rules where ICU has a tailoring (Finnish
  and Swedish "EU:n" is one word); Japanese written only in kanji with
  no language given is segmented as Chinese, as NVDA's automatic choice
  does; where the application moves the caret by word, the provider's
  word is spoken.
- A plain Win32 edit control's word, which the outpost finds itself,
  is matched: the line segmented by the same rules, each word with the
  white space after it, null characters and no-break spaces read as
  spaces, and a carriage return or line feed a word of its own.
- The paragraph-style setting is not yet built (unscheduled).

### Direction marks in names and values

NVDA removes the left-to-right
and right-to-left marks from list view items, File Explorer's values,
and the clock ("Direction marks in names and values" in
[NVDAObjects](../nvda/object-model.md)). Verbatim: **different** (deliberately, Dickson, 2026-10-15): the
outposts remove them, and the embeddings, overrides, and isolates with
them, from every name, value, and description they read, through UIA
and MSAA alike, so a date reads the same in any application. In text
read by character, each is named in the character table: NVDA's two
names, and the Unicode names for the others.

### Line breaks

NVDA speaks carriage returns and line feeds inside text as
spaces ("Line breaks in spoken text" in [Speech](../nvda/speech.md)).
Verbatim: **matched (unverified)**, for every kind of line break, at
the presentation stage; text Core splits into lines itself (an object's
value reviewed as flat text) ends a line at a carriage return and line
feed, either alone (Windows 11 Notepad's bare carriage return), or
Unicode's line and paragraph separators (`verbatim-text`'s `lines`).

### The review cursor's bottom in a standard edit control

NVDA's review bottom (Shift+Numpad 9) there is the line before the
empty line a final line break leaves, while Verbatim's, like NVDA's in
Windows 11 Notepad, is that empty line. Verbatim: **different**
(Dickson, 2026-10-15): in every control, the review cursor's bottom is
the last line the caret can occupy, including the empty line after a
final line break. NVDA's behavior is verified by capture in a Windows
Forms text box (`target/e2e-evidence/m42-captures-2026-10-21/`): for
"one" and "two" NVDA says "two" with no final line break
(`2-review-bottom-no-final-line-break.txt`) and with one
(`2-review-bottom-one-final-line-break.txt`), and "blank", the empty
line between the two breaks, with two
(`2-review-bottom-two-final-line-breaks.txt`).

### Browse mode, quick nav, pass-through rules, virtual-buffer equivalent

**not yet (M7)**; references
[Browse mode](../nvda/browse-mode.md), [Virtual buffers](../nvda/virtual-buffers.md).

### Office object-model reading

**not yet (M8.5)**: the app-module track's Tier D, gated on the
object-model bridge planned around M8.5 (`docs/roadmap.md`); reference
[Office through COM](../nvda/office-com.md) before deciding whether
Verbatim replicates the OM path or bets on modern-Office UIA.

### Document formatting reporting

Document formatting reporting (the option vocabulary and the
cache-and-diff announcement model). NVDA:
[Document formatting reporting](../nvda/document-formatting.md).
Verbatim: **partial**: the cache-and-diff model is matched for caret
keys and a focus's line (`docs/parity/speech-audio.md`, "Formatting
while the caret moves"), and the options are theme indications.

### The role-shaped behavior layer

The role-shaped behavior layer (progress bars, dialog text
harvesting, suggestion sounds, fake table rows, tooltips/toasts).
NVDA: the behavior mixins ([Object model](../nvda/object-model.md)).
Verbatim: **partial** — selection and notification handling exist
in the reducer, and dialog text in the outpost
(`docs/parity/focus-announcement.md`, "Dialog text, which NVDA reads on
entering a dialog"); the rest lands per feature. Architectural question
for the review: where is Verbatim's home for this layer?

### ARIA vocabulary, annotations/details, compound documents

NVDA:
[ARIA, annotations, and compound documents](../nvda/aria-and-annotations.md).
Verbatim: **not yet (M7)**.

### Math

Math (MathML pipeline, provider seam, interactive navigation).
NVDA: [Math](../nvda/math.md). Verbatim: **not yet (unscheduled)**, in
scope (Dickson, 2026-07-17).
