# Text and documents

The text layer, caret and selection reporting, segmentation, and documents.

### TextInfo-equivalent layer

TextInfo-equivalent layer, caret-key reporting via
wait-for-evidence, selection deltas. NVDA:
[TextInfo](../nvda/text-infos.md) and
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: **matched (verified)** for UIA text and the Win32 edit and
rich edit controls, through the text protocol
(`docs/crates/verbatim-model.md`, "The text protocol"; the
`notepad_editing` and `notepad_review_cursor` scenarios); an MSAA object
that is neither has no text.

- Rich edit text: a rich edit window that is not Unicode has its text
  decoded by what its buffer holds, UTF-16 or ANSI, as NVDA decodes it
  ("Rich edit text" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)).
- Caret keys pass to the application, and Core asks the outpost to
  watch for evidence (a caret event, the caret leaving where Core knew
  it, the text at the caret changing for Delete, or the selection
  changing), which it checks when the request arrives and at each caret,
  text, or selection change the application reports, never waiting for
  it. Core then speaks NVDA's unit for the key: the character for Left
  and Right Arrow, Home, and End; the provider's word for Control with
  Left or Right Arrow, a word of one character (Notepad's full stop) by
  its name, as NVDA spells it; the line for Up and Down Arrow, the page
  keys, and Control with Home or End; the paragraph for Control with Up
  or Down Arrow; what Backspace deleted; and what Delete left at the
  caret, as NVDA does.
- A newer key supersedes a watched one and a focus change drops it,
  NVDA's two short-circuits; a key's reply read after a later key was
  pressed is not spoken for it, since it may show that key's effect.
- Shift movement speaks the text followed by "selected" or
  "unselected", NVDA's word order, a single character by its name, and
  512 characters or more as their number; a movement without Shift that
  leaves a selection speaks its unit and then the text unselected.
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
- The caret as it was when the key was pressed is the baseline: the
  outpost's own newest report, when the read behind it finished before
  the hook saw the key, else where Core knew it; a report read after the
  key, from the application's caret event for that very key arriving
  before Core's request, is not. Core does the same for what Backspace
  deleted and the text Delete compares: the key's path to Core (the
  hook, then Verbatim) can be slower than the application's caret event
  (the application, then the outpost, then Core), so Core keeps its last
  8 caret reports with the time each was observed or read, and works
  from the newest observed before the key was pressed (pinned by
  `notepad_editing` under load). The caret Core holds is the newest by
  the time each report was observed or read, not the last to arrive, so
  a caret key's reply read before a caret event that reached Core first
  does not move the caret, or the review cursor following it, back; a
  report without a time is taken as it arrives.
- A review command while the review cursor follows the caret, say-all
  from the caret, and the caret's location read the caret as the
  outpost finds it, not Core's copy, which can lag behind a key the
  application has already handled: NVDA runs one script at a time, so
  reading the current line waits for an earlier Down Arrow's script to
  finish (`nvda/input.md`, "Scripts run one at a time"), and a live read
  gives the same line without holding the command back.
- Terminals are recognized by their UIA class (`TermControl`,
  `WPFTermControl`) and the console host's window class, never by
  title. The console host's text area is announced as "terminal"
  without its English-only name, "Text Area", as NVDA's console class
  drops it (the terminal scenarios assert it).
- The system caret's location change and show (`OBJID_CARET`) are a
  caret event on the focus, as NVDA handles them, whether the focus is
  an edit control read through MSAA or a UIA node: some UIA documents,
  such as Visual Studio's, raise no UIA caret event of their own
  (mockapp's `the_system_carets_move_is_the_uia_focuss_caret_event`),
  and a caret show from the focused classic edit control is its caret
  event (`a_classic_edit_controls_caret_show_is_its_caret_event`). For a
  UIA focus they are taken from any window NVDA's acceptance test takes a
  location change from (the foreground application's windows, those
  sharing its root owner, and the rest of `docs/parity/focus-events.md`,
  "Event acceptance"), not only from the focus's own window. The caret's
  show from a classic edit control comes with every repaint, since
  painting hides and shows the caret, so each repaint reads the caret,
  as in NVDA. Different:
  - Once a UIA focus has raised a UIA caret event of its own, Verbatim
    drops the system caret's events until the next focus is reported,
    where NVDA handles both. The ones that come before that first UIA
    caret event are still read: Windows 11 Notepad raises the system
    caret's events first, so the first move after each focus on its
    document reads the caret twice (`notepad_editing`, two focuses on the
    document: 3 system caret events read and 16 to 18 caret reads per
    run in three runs on 2026-10-20, against 16 when every move was read
    once). A focus that later stops raising UIA caret events, as one
    raising them only for selections would, is followed again from its
    next focus (`a_new_focus_uses_the_system_caret_again`).
  - In a focused console host terminal, the system caret's events are
    never used: the console's caret event (`EVENT_CONSOLE_CARET`) is its
    caret event (`docs/crates/verbatim-outpost.md`, "Terminals"). NVDA
    does the reverse on this build. Windows 11's console host is
    FORMATTED, so NVDA's default console setting ("auto") reads it as
    its UIA console (`_shouldUseUIAConsole`, `UIAHandler/utils.py` 337
    to 347), whose focus is a UIA object; the system caret's location
    change is mapped onto that focus as its caret event
    (`IAccessibleHandler/__init__.py` 636 to 668), and NVDA's console
    handler (`winConsoleHandler.py`), which only its legacy console
    connects, does not run. The console host also raises UIA's text
    selection changed on its text area for each move, which NVDA takes
    as a caret event too once it has registered its focus's local event
    handlers on the text area (`UIAHandler/__init__.py` 278 to 287 and
    697 to 739), so NVDA handles two caret events for each move. Checked
    on 2026-10-21 with NVDA's debug log of the console opened as
    `conhost_editing` opens it, typing and arrows (both events for every
    move in one launch; in two other launches NVDA did not register on
    the text area and took the system caret's alone), and a UIA probe on
    the desktop in the launch that registered, which heard the text
    area's event at every step
    (`target/e2e-evidence/fixes-round-eight-2026-10-21/`,
    `nvda-debug.log` and `uia-probe-conhost.txt`). The console host raises the system caret's
    location change about 130 microseconds before its console caret
    event, at the same blink tick, for the same move. Verbatim keeps the
    console's event, because it also comes for a move of a selection's
    end and Verbatim's terminal reading is built on it, so each move is
    read once (`conhost_editing`: 7 console caret events and 8 caret
    reads, none from the system caret's 7 events;
    `the_console_hosts_system_caret_is_not_its_caret_event`).
  - The event's window is checked as readable before the caret event is
    mapped onto the focus: it must be a window, have no ghost window
    standing in for it, and belong to an application that is responding.
    NVDA checks that the event's window exists for every event, the
    caret's included, as its hook receives it
    (`IAccessibleHandler/internalWinEventHandler.py` 113 to 133), then
    maps the caret event onto its focus and makes the ghost window and
    hung application checks only for other events
    (`IAccessibleHandler/__init__.py`, 549 to 576 against 636 to 668). Verbatim checks first so that a hung
    application's caret event never leads to a read of its caret that
    would wait on it.
  - A caret show is tested for acceptance as a location change is; NVDA
    skips the test for it and asks only its focus's
    `shouldAcceptShowHideCaretEvent`. The two differ only for a show from
    a window unrelated to the foreground window.

  NVDA's three exceptions are not built, each recorded for where it
  belongs: a focus read through IAccessible2, which Verbatim does not
  read before milestone M7; Word's UIA document, which Chinese input
  methods flood with these events, a rule of Word's own support; and a
  focus whose class refuses the caret's shows and hides
  (`shouldAcceptShowHideCaretEvent`), which only Scintilla's editor does
  at the pin (`NVDAObjects/IAccessible/scintilla.py`, line 15), since its
  caret blinks by them, a rule of Scintilla's support. The last two are
  on the roadmap's app-module track, with Word (tier D) and Notepad++
  (tier A). Until then a focused Scintilla editor, such as Notepad++'s,
  tries to read its caret at each blink.
- NVDA's `caretMovementFailed` event is not offered.

Differences:

- A key that does not move the caret is silent (Dickson, 2026-10-08).
  NVDA waits 100 ms (300 in Windows Terminal) for evidence and, finding
  none, speaks the key's unit anyway: Home at the start of a line reads
  the character after the caret, Control+Home at the start of the text
  reads the first line, Down Arrow on the last line reads that line.
  Verbatim's outpost never waits for a caret key, so that nothing else
  of the application, a focus change above all, waits behind one; it
  watches for the evidence instead and speaks when it comes, however
  late. A watch that ends without evidence, because the next key
  replaced it, the focus moved, or it reached its ten-second bound, says
  nothing. The same holds for a key whose application moves the caret
  without reporting it at all, where NVDA's polling finds the move. A
  focused classic edit control's system caret location change and show
  are caret events to Verbatim; a Common Controls
  version 6 edit control, such as a Windows Forms text box with visual
  styles, reports it with its text selection event too. An application
  whose Up Arrow on the first line, or Down Arrow on the last, moves the
  caret to the line's start or end has that move spoken as the key's
  answer, as NVDA speaks it.
- A caret event alone is not evidence when the caret's position is
  known, since an application's late caret event can belong to an
  earlier key; the position, the characters either side of the caret,
  or the selection must change (the wait-for-evidence
  design, Dickson, 2026-10-08, `docs/design/phase6-design.md`, "What a
  key did to the text").
- Verbatim never swallows and resends the key, so what Backspace deleted
  is worked out by comparing Core's copy of the caret's line before the
  key with the line after it (Dickson, 2026-10-08,
  `docs/design/phase6-design.md`, "What a key did to the text"): the
  text removed at the caret is spoken, in any language, and nothing when
  nothing was removed (the classic edit control's Control+Backspace
  inserts a DEL character, where NVDA would speak the word it expected
  to go). A backspace over a line break names the kind of break the
  text was last seen to use, which is what NVDA reads, since a text uses
  one kind throughout, and says nothing when Core has seen no line break
  in it yet. In a terminal, Backspace and Control+Backspace say what
  NVDA says ("o" and "tw" after `echo one two`, both terminals).
- A key outside NVDA's caret keys that types no text (bash's Control+A,
  Control+E, Alt+B, Alt+F, Control+W, Control+K, and Escape clearing a
  line) is spoken by its evidence (Dickson, 2026-10-08,
  `docs/design/phase6-design.md`, "What a key did to the text"): text
  it removed at the caret, the new line when the caret left its line,
  and otherwise by where the caret landed (the character, the word, or
  the word then the character). NVDA says nothing for such keys: Escape
  after `echo hi` is silent in NVDA and "echo hi" in Verbatim; Escape
  after inline prediction showed `git status` is silent in NVDA and
  "git", the text typed, in Verbatim. Escape on a typed line that had
  wrapped onto a second row says all it removed, as on a line of one
  row, though the caret went up a row; NVDA is silent.
- NVDA reports any selection change of a UIA or classic edit focus after
  its caret event, whatever made it ("Selection changes" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)),
  comparing with the selection it last saw, which it takes when the
  object gains the focus and forgets when it loses it. Verbatim does the
  same for a selection no key made, such as one made with the mouse or by
  the application (Dickson, 2026-10-15): the outpost compares the
  selection each caret event finds with the one in its last caret report
  for the focus, in the caret read's own round trip, the report after a
  focus setting it, and Core speaks the changes with a selecting key's
  words, "alpha selected" and "alpha unselected" (mockapp's
  `a_selection_no_key_made_is_reported_with_its_change`). A key's
  selection is spoken once, by the key's answer, since its watch takes
  the key's caret event; one Verbatim makes itself, select then copy's
  first press, is spoken by its own reply, and the caret event after it
  carries no change, as does the one after say-all moves the caret. When
  the outpost merges a caret report waiting to be written with a newer one
  for the same node, the merged report carries both reports' changes, so
  every intermediate change is spoken, where NVDA, whose event limiter
  keeps one caret event per object (`IAccessibleHandler/orderedWinEventLimiter.py`),
  compares only the last selection with the one it last saw.
- A selection change that follows a change of the focus's text:
  **matched** in both backends, against the captures in
  `target/e2e-evidence/m42-desktop-2-2026-10-22/` (`part3b-textbox-*.txt`
  and `part3b-notepad-*.txt`). The selection compared with is never
  forgotten when the text changes. In a classic edit control, the text
  having changed since the selection compared with was reported, the
  change is said in general terms: the whole new selection followed by
  "selected" when anything was selected, then, when anything was
  unselected, the whole new selection followed by "selected instead", or
  "selection removed" when nothing is selected now, both for a disjoint
  replacement ("Selection changes" in
  [Editable text and terminals](../nvda/editable-text-and-terminals.md)).
  The control's text counts as changed once its value change is handled,
  and at a caret event while its value change still waits, so both orders
  of the two events give the general words, as NVDA's do (`x` typed over
  the selected "hello ": "x" then "selection removed"; Control+X on a
  selected line: "selection removed"; mockapp's
  `an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms`
  and
  `an_edit_controls_value_change_handled_first_has_the_next_change_said_in_general_terms`).
  A selecting key's answer is said the same way when the text changed
  before it. The outpost carries the general terms to Core as
  `content_changed` with the whole new selection's text, read in the same
  caret read in place of the changed pieces' text. A UIA text's change
  never counts, as in NVDA, which handles UIA's text change as one only for
  Word and the two terminals: the selections are compared as always, the
  old selection's range read as it now is, so "x" typed over the selected
  "hello " in Windows 11 Notepad says "x" then "x unselected", the old
  range now holding the typed character, and Control+X on a selected line
  says nothing, the old range now empty (mockapp's
  `a_caret_event_before_its_text_change_compares_the_old_ranges_text`).
  Not yet built:
  - NVDA's classic edit control drops a caret event whose selection
    offsets equal the last one's (`NVDAObjects/window/edit.py` 1068 to
    1075); Verbatim reads each caret event.
  - NVDA speaks the whole new selection when the old one held nothing,
    and the whole old one when the new one holds nothing; Verbatim splits
    either at the caret by the rule a selecting key's answer follows,
    which differs only when the caret was inside the selection, as a
    click inside a selection or a double-click inside a word leaves it:
    two changes where NVDA speaks one.
  - Terminals are unresolved: NVDA's console class replaces the detection
    (`NVDAObjects/UIA/winConsoleUIA.py` line 416), which is not yet
    traced, and Verbatim reports no selection change in a terminal.

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

NVDA's review
bottom (Shift+Numpad 9) there is the last line with text, while
Verbatim's, like NVDA's in Windows 11 Notepad, is the empty line after a
final line break. Verbatim: **different** (Dickson, 2026-10-15): in
every control, the review cursor's bottom is the last line the caret can
occupy, including the empty line after a final line break. The claim
about NVDA's Shift+Numpad 9 in a standard edit control is from reading
its source and awaits a capture (M4.2).

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
