# Text and documents

The text layer, caret and selection reporting, segmentation, and documents.

### TextInfo-equivalent layer

The text layer: caret-key reporting by waiting for evidence, and
selection deltas. NVDA: [TextInfo](../nvda/text-infos.md) and
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: **matched (verified)** for UIA text and the Win32 edit and
rich edit controls, through the text protocol
(`docs/crates/verbatim-model.md`, "The text protocol";
`docs/crates/verbatim-core.md`, "Text (milestone M4)"; the
`notepad_editing` and `notepad_review_cursor` scenarios); an MSAA object
that is neither has no text.

- Rich edit text: a rich edit window that is not Unicode has its text
  decoded by what its buffer holds, UTF-16 or ANSI, as NVDA decodes it
  ("Rich edit text" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)).
- NVDA's `caretMovementFailed` event is not offered.

The layer's cases are the entries that follow, from "Caret keys and
what each speaks" to "Not yet built". The end-to-end steps that declare
a difference from NVDA against this entry show the differences recorded
in "A caret key that moves nothing", "What counts as a caret key's
evidence", "What Backspace and Delete say", "Keys outside the caret
table", "A selection no key made", "Selection over changed text through
UIA" and "NVDA's delayed character description at the caret".

### Caret keys and what each speaks

NVDA speaks its unit for each caret key once the caret has moved
([Editable text and terminals](../nvda/editable-text-and-terminals.md)).
Verbatim: **matched (verified)** (the `caret_keys` scenarios,
`notepad_editing`, `notepad_review_cursor`). Caret keys pass to the
application, and Core asks the outpost to watch for evidence of the
key's effect, never waiting for it
(`docs/crates/verbatim-core.md`, "Text (milestone M4)"). Core then
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
- A newer key supersedes a watched one and a focus change drops it,
  NVDA's two short-circuits; a key's reply read after a later key was
  pressed is not spoken for it, since it may show that key's effect.
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
load). The caret as it was when the key was pressed is the key's
baseline, for its caret, for what Backspace deleted and for the text
Delete compares, since the key's path to Core can be slower than the
application's caret event; the caret Core holds is the newest by the
time each report was observed or read, not the last to arrive, so a
caret key's reply read before a caret event that reached Core first does
not move the caret, or the review cursor following it, back
(`docs/crates/verbatim-core.md`, "Text (milestone M4)", owns the
mechanism).

A review command while the review cursor follows the caret, say-all from
the caret, and the caret's location read the caret as the outpost finds
it, not Core's copy, which can lag behind a key the application has
already handled; a live read gives the same line as NVDA's waiting
script without holding the command back.

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

Verbatim: **different** (Dickson, 2026-10-08): a key that does not move
the caret is silent. The outpost never waits for a caret key, so that
nothing else of the application, a focus change above all, waits behind
one; it watches for the evidence and speaks when it comes, however late.
A watch that ends without evidence, because the next key replaced it,
the focus moved, or it reached its ten-second bound, says nothing. The
same holds for a key whose application moves the caret without
reporting it at all, where NVDA's polling finds the move. A focused
classic edit control's system caret location change and show are caret
events to Verbatim; a Common Controls version 6 edit control, such as a
Windows Forms text box with visual styles, reports it with its text
selection event too. An application whose Up Arrow on the first line, or
Down Arrow on the last, moves the caret to the line's start or end has
that move spoken as the key's answer, as NVDA speaks it. Alt with Up or
Down Arrow that moves nothing says nothing in both controls, since
Verbatim never decides that a key moved nothing; moving by sentence
itself awaits Dickson's decision.

### What counts as a caret key's evidence

NVDA finds the effect of a key on a password field by the caret event
the control raises within 60 ms of the key, which it takes as evidence
("Protected (password) fields" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)).
Verbatim: **different** (the wait-for-evidence design, Dickson,
2026-10-08): a caret event alone is not evidence when the caret's
position is known, since an application's late caret event can belong to
an earlier key; the position, the characters either side of the caret,
or the selection must change. For Backspace and Delete, alone or with
Control, the caret's line differing from the line of the last caret
reported before the key is evidence too (Dickson, 2026-10-26), since
deleting one of a run of the same character, such as a password field's
mask, changes neither the caret's neighbors nor, in a WPF `PasswordBox`,
whose positions follow edits, where the caret is (`wpf_password`).

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

Verbatim: **different** ([holding caret keys](../design/key-holding.md),
which owns the mechanism; Dickson, 2026-10-08 and 2026-10-22). Verbatim
holds every caret key while the focus has text, as NVDA's caret scripts
do: the hook swallows it, the focus's outpost reads the caret and its
line, and the key is sent on. The differences:

- Keys pressed while a held key waits, typed characters included, wait
  behind it and reach the application in the order pressed, one step
  beyond NVDA; the lock keys alone pass at once, since their state is
  announced from Windows' and Num Lock's decides what a numpad key is
  (Dickson, 2026-10-26).
- A held key behind another has its caret read once the key ahead's
  evidence has been answered, or 100 ms after the key ahead was sent on,
  whichever comes first, so that the read sees the application after the
  key ahead (Dickson, 2026-10-22); Verbatim's bound is 100 ms everywhere.
- While a window of higher integrity than Verbatim's is in front,
  nothing is held.

### What Backspace and Delete say

NVDA's Backspace and Delete scripts speak what they expect the key to
remove: Control+Backspace in a Windows Forms password box says "4 star"
(`winforms-password-deleting-keys.txt` in
`target/e2e-evidence/m42-protected-fixes-2026-10-22/`).
Verbatim: **different** (Dickson, 2026-10-08): what Backspace deleted is
worked out by comparing the line read before the key with the line
after it ([holding caret keys](../design/key-holding.md)). The text
removed at the caret is spoken, in any language, and nothing when
nothing was removed: the classic edit control's Control+Backspace
inserts a DEL character, where NVDA would speak the word it expected to
go (`text_box_password`).

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
Control+A, Control+E, Alt+B, Alt+F, Control+W, Control+K, and Escape
clearing a line) is spoken by its evidence: text it removed at the
caret, the new line when the caret left its line, and otherwise by where
the caret landed (the character, the word, or the word then the
character). Both editors, the `caret_keys` scenarios:

- Escape after `echo hi` is silent in NVDA and "echo hi" in Verbatim.
- Escape after inline prediction showed `git status` is silent in NVDA
  and "git", the text typed, in Verbatim.
- Control+Z after typing "x" is silent in NVDA and "x" in Verbatim.
- Pasting "kiwi" and "mango" on two lines at a line's start is silent
  in NVDA and reads the line the caret is left on, "mangoalpha beta
  gamma", in Verbatim.
- Escape on a typed line that had wrapped onto a second row says all it
  removed, as on a line of one row, though the caret went up a row; NVDA
  is silent.
- A key outside the caret table that removes a selection (Control+X,
  Control+V), something selected before it and nothing after, says only
  the selection's change, never the text it removed, which was the
  selection's: NVDA has no script for these keys.

### Selecting with Shift

NVDA: "Selection changes" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: **matched (verified)**. Shift movement speaks the text
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

Verbatim: **different** (Dickson, 2026-10-26): speaking every
intermediate change. A selection no key made, such as one made with the
mouse or by the application, is reported as NVDA reports it (Dickson,
2026-10-15): the outpost compares the selection each caret event finds
with the one in its last caret report for the focus, and Core speaks the
changes with a selecting key's words, "alpha selected" and "alpha
unselected" (mockapp's
`a_selection_no_key_made_is_reported_with_its_change`). A key's
selection is spoken once, by the key's answer, or, when the key's caret
event was reported before the watch arrived, by the report, after the
key's own words. One Verbatim makes itself, select then copy's first
press, is spoken by its own reply, and the caret event after it carries
no change, as does the one after say-all moves the caret. When the
outpost merges a caret report waiting to be written with a newer one for
the same node, the merged report carries both reports' changes, so
Shift+Right followed at once by Right says the selection, the character,
and the selection cleared.

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
The outpost keeps one old selection per node, and every comparison uses
it, a caret event's report and every key's answer alike; a selection
Verbatim makes itself or a caret it moves is kept in its place (mockapp's
`a_shift_key_after_verbatims_selection_compares_with_it`). A comparison
is in general terms when the note is set (a classic edit control's value
change handled, as NVDA's `hasContentChangedSinceLastSelection`, read
and cleared at each comparison with a selection, and cleared by a
focus), when
the control's value change waits once the caret is read, or when the old
selection's text no longer matches what its range holds
(`docs/crates/verbatim-core.md`, "Text (milestone M4)"). General terms
are the whole new selection followed by "selected" when anything was
selected, then, when anything was unselected, the whole new selection
followed by "selected instead", or "selection removed" when nothing is
selected now; the outpost carries them as `content_changed`. A key's
own words come first and the selection's change after, as NVDA speaks
its script's words and then its caret event's; a report that came
before the key's watch is said after the key's words, and a superseded
answer's change after the later key's, so no change compared is lost. Backspace and Control+Backspace over a selection say what comes
before the selection's start, NVDA's caret (mockapp's
`a_selections_start_on_the_carets_line_is_reported_in_it`).

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
`an_edit_controls_deleting_key_over_a_selection_is_answered_in_general_terms`,
`an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms`,
`an_edit_controls_value_change_handled_first_has_the_next_change_said_in_general_terms`,
`an_edit_controls_value_change_raised_during_its_caret_read_is_said_in_general_terms`,
`a_caret_report_before_a_keys_watch_leaves_its_answer_no_selection_change`
and, for a selection extended by Shift and a click after a key that
changed nothing, said at once as NVDA says it,
`a_selection_moved_before_a_keys_watch_is_evidence_by_its_ends`.

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

Verbatim: **different** (Dickson, 2026-10-22), who judges NVDA's words
wrong, since they name the new text as though it had been selected; NVDA's
behavior is not offered as a setting. Verbatim keeps the selection's
text when it is reported and, when the old range no longer holds it,
says the change in general terms: "x" then "selection removed", and
"selection removed" after each of those keys' own words. Backspace over
"alpha " at the text's start says "blank" then "selection removed"
(`target/e2e-evidence/m42-selection-fixes-2026-10-22/notepad-first-word-backspace.txt`;
NVDA's captures are in `m42-selection-keys-2026-10-22/` and
`part3b-notepad-*.txt` in `m42-desktop-2-2026-10-22/`, beside the
classic edit control's);
over "beta ", and over "gamma" selected with Shift+End to the line's end,
which in Notepad selects the line break too and puts the caret at the
next line's start, "space" then "selection removed"; at a later line's
start, "carriage return" then "selection removed" (`notepad_selection_keys`
and `notepad_editing`; mockapp's
`a_uia_selection_typed_over_is_said_in_general_terms` and
`a_uia_deleting_key_over_a_selection_is_answered_in_general_terms`).

Both texts are read in the caret read's one round trip, each up to 513
UTF-16 code units, one more than the 512 characters from which a
selection is spoken as a count (Dickson, 2026-10-22), so a longer
selection is compared by its first 513 units and by being longer, and
only an edit that leaves both as they were, such as one past its 513th
unit, is compared as NVDA compares it, in specific terms. The new
selection's text is read only when its ends moved; when they did not,
what the old ends hold now is its text and is kept (mockapp's
`remote_and_classic_caret_reads_agree`). A range that still holds its
text is compared as NVDA compares it, the same words in both:
Control+Backspace over "beta ", which Notepad answers by deleting the
word before the selection, says "alpha" then "beta unselected", and over
"gamma" to the line's end "beta" then "gamma unselected"; over "alpha "
at the text's start Notepad deletes nothing and both say nothing; over
"alpha " at a later line's start Notepad deletes the line break before
the selection, and both say "carriage return" then "alpha unselected"
(`notepad_selection_keys`; `notepad-second-word-control-backspace.txt`,
`notepad-third-word-to-end-control-backspace.txt`,
`notepad-word-control-backspace.txt`).

### NVDA's delayed character description at the caret

NVDA speaks a delayed character description after the character a key
reaches, "Bravo" after the "b" Delete reaches; for a character with no
description, such as the star or circle Left Arrow reaches in a password
field, it is an utterance with no words, a pause before nothing
(`getSingleCharDescription`, `nvda/source/speech/speech.py` 558).
Verbatim: **different**: a setting Verbatim does not have yet
(`text_box_password`, `wpf_password`).

### Not yet built

Selection detection cases not built. Verbatim: **not yet (unscheduled)**.

- A kept selection with no text, as the by-parts select path keeps it,
  is not checked for its range no longer holding its text until its
  ends move.
- NVDA's classic edit control drops a caret event whose selection
  offsets equal the last one's (`NVDAObjects/window/edit.py` 1068 to
  1075); Verbatim reads each caret event.
- Terminals are unresolved: NVDA's console class replaces the detection
  (`NVDAObjects/UIA/winConsoleUIA.py` line 416), which is not yet
  traced, and Verbatim reports no selection change in a terminal.

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
own (mockapp's `the_system_carets_move_is_the_uia_focuss_caret_event`),
and a caret show from the focused classic edit control is its caret
event (`a_classic_edit_controls_caret_show_is_its_caret_event`). For a
UIA focus they are taken from any window NVDA's acceptance test takes a
location change from (the foreground application's windows, those
sharing its root owner, and the rest of `docs/parity/focus-events.md`,
"Event acceptance"), not only from the focus's own window. The caret's
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

Verbatim: **different**, in four cases:

- Once a UIA focus has raised a UIA caret event of its own, Verbatim
  drops the system caret's events until the next focus is reported,
  where NVDA handles both. The ones before that first UIA caret event
  are still read: Windows 11 Notepad raises the system caret's events
  first, so the first move after each focus on its document reads the
  caret twice (`notepad_editing`). A focus that later stops raising UIA
  caret events is followed again from its next focus
  (`a_new_focus_uses_the_system_caret_again`).
- In a focused console host terminal, the system caret's events are
  never used: the console's caret event (`EVENT_CONSOLE_CARET`) is its
  caret event (`docs/crates/verbatim-outpost.md`, "Terminals"), read
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
- The event's window is checked as readable before the caret event is
  mapped onto the focus: it must be a window, have no ghost window
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
Verbatim: **matched (verified)**: the answer of no pattern is not kept
past evidence that it is out of date: the next focus on the node, or a
caret or text event from it, which only an element with text raises,
has the pattern fetched again; a caret key's watch on such a node stays
open for that event instead of being answered at once; and a fetch that
fails with an error is not kept at all (mockapp's
`a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event`).
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

**not yet (M8)**: the app-module track's Tier D, gated on the
object-model bridge planned around M8 (`docs/roadmap.md`); reference
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
