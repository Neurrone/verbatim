# Text and documents

The text layer, caret and selection reporting, segmentation, and documents.

### TextInfo-equivalent layer

TextInfo-equivalent layer, caret-key reporting via
wait-for-evidence, selection deltas. NVDA:
[TextInfo](../nvda/text-infos.md) and
[Editable text and terminals](../nvda/editable-text-and-terminals.md).
Verbatim: the text protocol (`docs/crates/verbatim-model.md`, "The text
protocol") is **matched since 2026-10-06** for UIA text and the Win32
edit and rich edit controls (the `notepad_editing` and
`notepad_review_cursor` scenarios); an MSAA object that is neither has
no text. A rich edit window that is not Unicode has its text decoded
by what its buffer holds, UTF-16 or ANSI, as NVDA decodes it ("Rich
edit text" in [Editable text and terminals](../nvda/editable-text-and-terminals.md)),
since 2026-10-08; it had been decoded by the window's flag. Caret keys pass
to the application and Core asks the outpost to watch for evidence (a
caret event, the caret leaving where Core knew it, the text at the caret
changing for Delete, or the selection changing), which it checks when
the request arrives and at each caret, text, or selection change the
application reports, never waiting for it, then speaks NVDA's unit for
the key: the character
for Left and Right Arrow, Home, and End; the provider's word for Control
with Left or Right Arrow, a word of one character (Notepad's full stop)
by its name, as NVDA spells it; the line for Up and Down Arrow, the page keys,
and Control with Home or End; the paragraph for Control with Up or Down
Arrow; what Backspace deleted; what Delete left at the caret, as NVDA
does (confirmed by Dickson on 2026-10-09: Delete is not among the keys
that speak the text they removed). A newer key
supersedes a watched one and a focus change drops it, NVDA's two
short-circuits; a key's reply read after a later key was pressed is not
spoken for it, since it may show that key's effect (the live caret
checks of 2026-10-08). Shift movement speaks the text followed by "selected" or
"unselected", NVDA's word order, a single character by its name, and 512
characters or more as their number; a movement without Shift that leaves
a selection speaks its unit and then the text unselected. Both were
corrected on 2026-10-07 after comparing transcripts with NVDA (Verbatim
had put "selected" first, NVDA's order only for text already selected
when an object is announced, and had said nothing of a selection an
arrow key left). **Different:** NVDA reports any selection change of the
focus after its caret event, such as one made with the mouse; Verbatim
reports those that keys make. **Different:** Verbatim never swallows and resends the key,
so what Backspace deleted is worked out by comparing Core's copy of the
caret's line before the key with the line after it (since 2026-10-08):
the text removed at the caret is spoken, in any language, and nothing
when nothing was removed (the classic edit control's Control+Backspace
inserts a DEL character, where NVDA would speak the word it expected to
go). **Different (since 2026-10-08, `docs/design/phase6-design.md`, "What a key did
to the text"):** a key outside NVDA's caret keys that types no text
(bash's Control+A, Control+E, Alt+B, Alt+F, Control+W, Control+K, and
Escape clearing a line) is spoken by its evidence: text it removed at
the caret, the new line when the caret left its line, and otherwise by
where the caret landed (the character, the word, or the word then the
character); NVDA says nothing for such keys (captured live on
2026-10-09 in both terminals: Escape after `echo hi` is silent in NVDA,
"echo hi" in Verbatim; Escape after inline prediction showed `git
status` is silent in NVDA, "git" in Verbatim, the text typed). Escape
on a typed line that had wrapped onto a second row says all it removed,
as on a line of one row, though the caret went up a row (since
2026-10-09; NVDA, captured live in both terminals, is silent). In a
terminal, Backspace and Control+Backspace now say what NVDA says ("o"
and "tw" after `echo one two`, both terminals, captured live); a
backspace over a line break
names the kind of break the text was last seen to use, which is what
NVDA reads, since a text uses one kind throughout, and says nothing
when Core has seen no line break in it yet. A line break a unit lands on
is named as NVDA names it, **matched since 2026-10-07** ("A line break
as a character" in [Editable text and
terminals](../nvda/editable-text-and-terminals.md)): End and Right Arrow
at a line's end say "carriage return"; the review cursor's end of line
is the line's last character, its break included, "carriage return" in
Windows 11 Notepad and "line feed" in a standard edit control, and
moving by character crosses each character of the break; Backspace
over a line break says "carriage return" in Notepad and "line feed" in
a standard edit control (the `notepad_editing` and
`notepad_review_cursor` scenarios). A terminal's row is cells, as
before, with no line break to land on. NVDA's `caretMovementFailed`
event is not offered. **Different:**
a caret event alone is not evidence when the caret's position is known,
since an application's late caret event can belong to an earlier key; the
position, the characters either side of the caret, or the selection must
change. **Different, deliberately (since 2026-10-08):** a key that does
not move the caret is silent. NVDA waits 100 ms (300 in Windows
Terminal) for evidence and, finding none, speaks the key's unit anyway:
Home at the start of a line reads the character after the caret,
Control+Home at the start of the text reads the first line, Down Arrow
on the last line reads that line. Verbatim's outpost never waits for a
caret key, so that nothing else of the application, a focus change
above all, waits behind one (the wait had held every event of the
application for its 100 ms); it watches for the evidence instead and
speaks when it comes, however late. A watch that ends without evidence,
because the next key replaced it, the focus moved, or it reached its
ten-second bound, says nothing. The same holds for a key whose
application moves the caret without reporting it: a classic edit control
reports a caret move only by the system caret's hide and show events,
which Verbatim does not hear, where NVDA's polling finds the move; a
Common Controls version 6 edit control, such as a Windows Forms text box
with visual styles, reports it with its text selection event (measured
with mockapp, 2026-10-08). An application whose Up Arrow on the first
line, or Down Arrow on the last, moves the caret to the line's start or
end has that move spoken as the key's answer, as NVDA speaks it.
The caret as it was when the key was pressed
is the baseline: the outpost's own newest report, when the read behind it
finished before the hook saw the key, else where Core knew it; a report
read after the key, from the application's caret event for that very key
arriving before Core's request, is not. Core does the same for what
Backspace deleted and the text Delete compares: the key's path to Core
(the hook, then Verbatim) can be slower than the application's caret
event (the application, then the outpost, then Core), so Core keeps its
last 8 caret reports with the time each was observed or read, and works
from the newest observed before the key was pressed, not the caret it
holds when the key arrives (fixed 2026-10-07 after `notepad_editing`
heard "n" for a Backspace over "x" under load). The caret Core holds is
the newest by the time each report was observed or read, not the last to
arrive, so a caret key's reply read before a caret event that reached
Core first does not move the caret, or the review cursor following it,
back; a report without a time is taken as it arrives. A review command
while the review cursor follows the caret, say-all from the caret, and the
caret's location read the caret as the outpost finds it, not Core's copy,
which can lag behind a key the application has already handled: NVDA
runs one script at a time, so reading the current line waits for an
earlier Down Arrow's script to finish (`nvda/input.md`, "Scripts run one
at a time"), and a live read gives the same line without holding the
command back (both fixed 2026-10-07). Terminals are recognized by
their UIA class (`TermControl`, `WPFTermControl`) and the console host's
window class, never by title. The console host's text area is announced
as "terminal" without its English-only name, "Text Area", as NVDA's
console class drops it (**matched since 2026-10-07**, found by
comparing transcripts; the terminal scenarios assert it).

### A text pattern missing at the focus

NVDA fetches a UIA object's text
pattern live, keeps what it got for that object's life, and makes a new
object for every focus event (`_get_UIATextPattern` and
`_get_TextInfo`, `nvda/source/NVDAObjects/UIA/__init__.py` lines 1766
to 1772 and 1802 to 1804); an object without a text pattern has no
navigable text, so its value is spoken in place of its line
(`_get__hasNavigableText`, `nvda/source/NVDAObjects/__init__.py` lines
1590 to 1603; `getObjectSpeech`, `nvda/source/speech/speech.py` lines
870 to 917). UIA reports a provider that fails the pattern request
exactly as one with no pattern. Verbatim: **matched**, and since
2026-10-08 the answer of no pattern is not kept past evidence that it
is out of date: the next focus on the node, or a caret or text event
from it, which only an element with text raises, has the pattern
fetched again; a caret key's watch on such a node stays open for that
event instead of being answered at once; and a fetch that fails with an
error is not kept at all. Found live in Windows 11 Notepad at launch,
whose document answered no pattern: Verbatim spoke the whole document,
as NVDA would have, and then every caret key in that window was silent,
since the answer was kept for the node's life (mockapp's
`a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event`).
**Different:** when a focus's text has a pattern but cannot be read at
all, nothing is said in place of its line until its caret is next
reported, where NVDA's read raises and nothing more of the focus is
spoken.

### Word and character segmentation

Word and character segmentation (Uniscribe grapheme clusters and
word stops) and the three-way paragraph-style setting. NVDA:
[TextInfo](../nvda/text-infos.md). Verbatim: segmentation is **matched
since 2026-10-06** in `verbatim-text` (grapheme clusters, Unicode's word
rules with dictionaries, jieba for Chinese, a run of spaces and tabs as
one segment), used by the review cursor over text and by spelling; a
zero-width space or zero-width no-break space counts as white space, not
as a word, and text of nothing else reads as blank (since 2026-10-08);
Han outside the Basic Multilingual Plane counts as Chinese for choosing
jieba, and the text's language, when the provider gives one, tailors
Unicode's word rules where ICU has a tailoring (Finnish and Swedish
"EU:n" is one word), since 2026-10-08; Japanese written only in kanji
with no language given is segmented as Chinese, as NVDA's automatic
choice does;
where
the application moves the caret by word, the provider's word is spoken.
A plain Win32 edit control's word, which the outpost finds itself, is
**matched since 2026-10-08**: the line segmented by the same rules, each
word with the white space after it, null characters and no-break spaces
read as spaces, and a carriage return or line feed a word of its own (it
had been a run of characters other than white space, so Thai or Chinese
in such a control was one word). The paragraph-style setting is
**not yet**.

### Direction marks in names and values

NVDA removes the left-to-right
and right-to-left marks from list view items, File Explorer's values,
and the clock ("Direction marks in names and values" in
[NVDAObjects](../nvda/object-model.md)). Verbatim: **different, deliberately,
since 2026-10-08**: the outposts remove them, and the embeddings,
overrides, and isolates with them, from every name, value, and
description they read, through UIA and MSAA alike, so a date reads the
same in any application. In text read by character, each is named in
the character table: NVDA's two names, and the Unicode names for the
others.

### Line breaks

NVDA speaks carriage returns and line feeds inside text as
spaces ("Line breaks in spoken text" in [Speech](../nvda/speech.md)).
Verbatim: **matched since 2026-10-06**, for every kind of line break, at
the presentation stage; text Core splits into lines itself (an object's
value reviewed as flat text) ends a line at a carriage return and line
feed, either alone (Windows 11 Notepad's bare carriage return), or
Unicode's line and paragraph separators (`verbatim-text`'s `lines`).

### The review cursor's bottom in a standard edit control

NVDA's review
bottom (Shift+Numpad 9) there is the last line with text, while
Verbatim's, like NVDA's in Windows 11 Notepad, is the empty line after a
final line break (NVDA transcripts, 2026-10-07). **Different, not yet
decided.**

### Browse mode, quick nav, pass-through rules, virtual-buffer equivalent

**not yet (M6)**; references
[Browse mode](../nvda/browse-mode.md), [Virtual buffers](../nvda/virtual-buffers.md).

### Office object-model reading

**not yet** (app-module track);
reference [Office through COM](../nvda/office-com.md) before deciding whether
Verbatim replicates the OM path or bets on modern-Office UIA.

### Document formatting reporting

Document formatting reporting (the option vocabulary and the
cache-and-diff announcement model). NVDA:
[Document formatting reporting](../nvda/document-formatting.md).
Verbatim: **partial**: the cache-and-diff model is matched for caret
keys and a focus's line ("Formatting while the caret moves" under
"Speech and audio"), and the options are theme indications.

### The role-shaped behavior layer

The role-shaped behavior layer (progress bars, dialog text
harvesting, suggestion sounds, fake table rows, tooltips/toasts).
NVDA: the behavior mixins ([Object model](../nvda/object-model.md)).
Verbatim: **partial** — selection and notification handling exist
in the reducer, and dialog text in the outpost (see "Focus and
announcements"); the rest lands per feature. Architectural question
for the review: where is Verbatim's home for this layer?

### ARIA vocabulary, annotations/details, compound documents

NVDA:
[ARIA, annotations, and compound documents](../nvda/aria-and-annotations.md).
Verbatim: **not yet (M6)**.

### Math

Math (MathML pipeline, provider seam, interactive navigation).
NVDA: [Math](../nvda/math.md). Verbatim: **not yet** (in scope,
unscheduled — maintainer decision 2026-07-17).
