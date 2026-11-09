# Object navigation and review

Object navigation, the navigator and review cursor, and the commands that report and review objects.

### Navigation tree shape

NVDA default: simple review on (filtered
tree, [Focus and the navigator](../nvda/focus-and-navigator.md)). Verbatim: full tree,
matching NVDA with simple review off, the project's stated
baseline (maintainer decision). **matched
(verified)** against that baseline; simple-review-on filtering is not
planned (revisit only if the baseline changes).

### The API at a window boundary

NVDA: an object reached by navigation, or
as a focus ancestor, in a different window is read through that
window's API (`correctAPIForRelation`): an MSAA object in a UIA window
becomes the window's UIA element, and a UIA element that is another
window's root, in an MSAA window, becomes that window's MSAA object.
Verbatim: **matched (unverified)**, for object navigation and for
focus ancestry in both directions. Checked live in Notepad's Save As dialog:
the file name box's ancestry continues through the shell's UIA view,
and navigating to its parents switches to UIA there. Where a
focus's MSAA walk crosses into a UIA window, the window's element is
presented by NVDA's rule for a UIA object, as every UIA ancestor is:
only when it is presentable by role, name, and description and UI
Automation counts it both a control and content.

### Navigator follows focus; review follows navigator

NVDA coupling
rules ([Review modes](../nvda/review-modes.md)). Verbatim: **partial**.
The follow-focus default is matched, unverified. Following the caret
(on by default) is implemented in Core, and its toggle, Verbatim+6, says
NVDA's "caret moves review cursor off" and "caret moves review cursor
on", **matched (verified)** against an NVDA capture in the Windows Forms
text box (2026-11-03) and pinned by `text_box_review_cursor` and
`notepad_review_cursor`; `followMouse` is
not yet built (unscheduled). NVDA's "review cursor follows focus"
setting (on by default) has no counterpart yet: Verbatim always follows
the focus, and the setting is part of the settings package (M4.3).

### Report current object

report / spell / copy on 1st/2nd/3rd press.
NVDA: script repeat counting ([Keyboard input](../nvda/input.md)), reading
the object live. Verbatim: **matched (unverified)**. A name, value, or
state change on the focus also updates the navigator's copy while it
rests there, so the report reads the object as it is now. The spelled
text and the copy wording are covered under the messages entries below.
An object with text (an edit field, a document, a terminal) spells and
copies its name followed by its selected text or the line at its caret,
as NVDA does ([Speech](../nvda/speech.md), "What an object with text
says"), checked against NVDA transcripts in Notepad.

### Report focus

Report focus (NVDA+Tab, every layout): the focus object reported as a
query on the first press, its text included as on focus; its name
spelled on the second press ("blank" with no name), and spelled with
character descriptions on the third and later; "No focus" with none
([Focus and the navigator](../nvda/focus-and-navigator.md), "Reporting the
focus"; `globalCommands.py`, `script_reportCurrentFocus`, which speaks
`getObjectSpeech` of `api.getFocusObject()` for the query on the first
press, `speakSpelling` of its name after, and "No focus" when the focus
object is not an NVDA object). Verbatim: **matched (verified)** on
Verbatim+Tab, checked live in Windows 11 Notepad against an NVDA
transcript: both say "Text editor document focused" and the line at the
caret, NVDA in one utterance and Verbatim in two, the line as its own,
as for any focus with text (pinned by `notepad_editing`).

Verbatim+Tab asks the application for its focus live (Dickson,
2026-11-07; [ADR 0001: Focus](../../books/dev/src/reference/adr/0001-focus.md),
"Report focus"): a focus query (`Query::ReadFocus`, purpose `Report`) to
the outpost of the process that serves the foreground thread's focus
window, found by local calls, so it reports the real focus even when the
focus moved with no event. The answer is spoken only, the object then its
line, as NVDA+Tab speaks its focus object, and is never taken as Core's
focus, so Core's focus ancestry and later events are unchanged; the
outpost neither follows nor adopts it. NVDA+Tab reports NVDA's own focus
object instead, its properties read live; its history records no reason.
A second and third press while the first waits spell after it is
spoken; a press after the repeat interval replaces the waiting one, whose
answer is not spoken; a focus observed after the press ends the waiting
press, so a focus already on its way is spoken and then the answer, as
NVDA runs its script after the focus queued before it (Claude,
2026-11-08, pending Dickson's acceptance). When nothing can answer, no
window in front, an ignored process, no outpost, the outpost's end, or an
empty answer, Verbatim says "No focus", as NVDA+Tab does with no focus
object; abandoned at its deadline, as in a hung application, or replaced,
it says nothing (Claude, 2026-11-08, pending Dickson's acceptance).
Core's `verbatim_tab_speaks_the_answer_alone_then_its_line`,
`verbatim_tab_never_moves_cores_focus`,
`verbatim_tab_with_nothing_to_ask_says_no_focus` and
`verbatim_tab_abandoned_says_nothing`; the app's
`verbatim_tab_is_routed_to_the_focus_windows_process`; mockapp's
`a_report_focus_query_in_a_hung_window_waits_for_the_answer`.

### Review and navigator messages and repeated presses

Review and navigator messages and repeated presses (the list in
[Review modes](../nvda/review-modes.md), "Reading commands built on
review"). Verbatim: **matched (unverified)**: the edge messages
("Top", "Bottom", "Left", "Right") with the unit read again, character
motions kept within the line, "blank", the current line or word spelled
on a second press, the current character's code on a third, "space" in
spelling, report current object spelling the name and value, "Move to
focus", "No navigator object", and activation saying "Activate" or "No
action" after walking up the parents. A copy says "Copied to clipboard:"
with the text (its length from 1024 characters on) after reading the
clipboard back, or "Unable to copy". The current character's description
on a second press and the current line or word spelled with
descriptions on a third come from the
English character table in `verbatim-i18n` (the phonetic alphabet, spelled as NVDA spells it, "Alfa" and "Xray"), and
a punctuation character spoken on its own or spelled is named from the
same table ("comma"). Raised pitch for
capitals (not checked by ear): spelling
and reading a single character speak an uppercase letter with the pitch
setting raised by 30 and then restored, for every synthesizer, as NVDA
does by default ([Speech](../nvda/speech.md), "Capitals when spelling");
the offset is not yet configurable, and saying "cap" or beeping for
capitals is not offered. eSpeak NG and OneCore speak the pitch change
within one synthesis, as SSML prosody written as NVDA's drivers write
it, so a capital brings no pause; a synthesizer that cannot is given
the change as a pitch setting between separate synthesis calls, which
can leave a short pause. A character is composed (NFC)
before its case is judged and before the table is looked up, so "E"
written with a combining acute accent is spelled as the capital "É",
and a capital is a character with some uppercase letter and no
lowercase one, so a capital with accents that have no precomposed form
is still raised. A capital's description is its small letter's by the
text's language, so Turkish İ is described as i, and the table names
the ideographic space (U+3000).

### Toggle key announcements

Toggle key announcements ("caps lock on", "num lock off", "scroll lock
on") when a lock key reaches the operating system, including Caps Lock
passed through by a double tap of the Verbatim key. NVDA:
`KeyboardInputGesture.reportExtra`. Verbatim: **matched (verified)**,
checked live with Num Lock, and pinned by the
`lock_key_announcements` scenario with Scroll Lock. The
Verbatim key passed in share mode is not announced: the screen reader
behind Verbatim decides whether it reaches the operating system and
announces its state itself.

### Parent/next/previous/first-child moves with edge reporting

Parent/next/previous/first-child moves with edge reporting ("no
parent" etc. spoken, not silence). An MSAA `accNavigate` result that is
the same COM object counts only when its child id moves the right way,
as NVDA's `_get_next`, `_get_previous`, and `_get_firstChild` check
(Verbatim compares canonical `IUnknown`s where NVDA compares interface
pointers). The settings dialog's rate slider answers next and previous
with itself; another object, such as a
windowless sibling in the same window, is a real neighbor, except that
a first child must be in the object's own window or one inside it, and
an object whose `accNavigate` finds no first child is asked for its
children (`AccessibleChildren`), both as NVDA does. Verbatim: **matched
(verified)**; no NVDA wording comparison has been made.

### Sibling navigation resolving back to self reported as edge

Sibling navigation resolving back to self reported as edge
(MSAA `accNavigate` quirk). NVDA: fallback heuristics
([MSAA and winevent handling](../nvda/msaa.md)). Verbatim: **matched (unverified)**.

### Windowed MSAA children navigated through the window hierarchy

NVDA: same strategy. Verbatim: **matched (verified)** (msinfo32
E2E).

### SysTreeView32 via TVM messages, Tree/TreeItem roles

NVDA:
control-specific overlay. Verbatim: **matched (verified)**
(`system_information_tree` E2E). A Windows
Forms tree view (`WindowsForms10.SysTreeView32.app.…`) gets the same
handling, since NVDA matches its class rules against the normalized
class name, and so does every class rule in `verbatim-ia2`
(`native_controls` mockapp test, a real comctl32 tree view under a
Windows Forms class name; no end-to-end scenario covers it). A tree view that draws its own check boxes
(msconfig's, say) says whether an item is checked only through the
item's state image, which NVDA reads with `TVM_GETITEMSTATE`: any state
image makes the item checkable, the second checked, and the third half
checked; Verbatim reads it the same way (`native_controls` mockapp test:
"checked", "not checked", and a half-checked parent; no end-to-end
scenario covers it). A tree item outside a `SysTreeView32`
is NVDA's generic outline item, which drops its value only when it is a
number and takes no level from it, as Verbatim does (`native_controls`
mockapp test, scripted tree items). Different: a tree view with
comctl32's own check boxes (`TVS_CHECKBOXES`) reports its items to MSAA
as check boxes, which NVDA gives no tree view handling at all, reading
them as plain check boxes with MSAA's flat navigation; Verbatim still
navigates and walks them through the control's messages, which reads
more usefully and changes nothing spoken on focus.

### Dead objects

An object whose element no longer exists. NVDA: only a focus moves its
navigator, and the navigator object it has stays when the object dies; a
command on it does what the object's dead calls do. Verbatim follows NVDA
throughout (Dickson, 2026-11-09), and the review cursor reads live: every
review command is a press of the key line, reading the application first
(`Query::ReadNode` for an object's name and value, and a line read for a
text command that answers from its line), so a press costs one round
trip. Only a focus, the to-focus command and a successful object
navigation move the navigator. "No navigator object" is said only when
there is no navigator, its only source in NVDA (`globalCommands.py` 1517
to 1520). A navigation from a dead object says the direction's edge
message, "No next", "No previous", "No objects inside" or "No containing
object", as a tree edge does, the navigator staying; Parent from a dead
element answers the parent of its window, as NVDA's walker fails and it
falls to the window's parent (`NVDAObjects/UIA/__init__.py` 2136 to 2145,
`NVDAObjects/window/__init__.py` 287 to 293), the navigator moving there.
The window is the element's nearest window recorded when its object was
made (`NVDAObjects/UIA/__init__.py` 1604 to 1607), and its parent is read
as NVDA's `findBestAPIClass` chooses (`NVDAObjects/window/__init__.py` 84
to 98 and 291 to 293): the desktop and a hung window are plain windows,
any other its UIA element or MSAA client, said with its own role; the
outpost makes no connection to another process's window, which is a
stand-in from Windows' own state. A limit: the object-activation walk
goes up by the walker alone and stops at a dead element, where NVDA's
walk takes the object's parent, the same fall back to the window's
parent (`globalCommands.py` 1880 to 1901).
A dead object reviews as an object with empty text: Report Current Object
says nothing, and the review commands say "blank" or, moving past the
empty line, "Bottom" and "blank", as NVDA's name and value then read as
empty (`NVDAObjects/__init__.py` 1450 to 1463). Captured from NVDA on
mockapp's disconnected UIA nodes, 2026-11-09, on a dead button: Next
Object "No next", Previous Object "No previous", First Child "No objects
inside", Parent "Desktop window", Report Current Object nothing, review
line, word and character "blank", review next line "Bottom", "blank",
the to-focus command "Move to focus" and the focus. A destroyed window: the same
edges and "No containing object"; report nothing. The review cursor's
current line (flat review) leaves its position where it was
(`globalCommands.py` 1975 to 1990), so a column stays across a short line in every text.
A navigation the application did not answer is not `Gone`: the navigator
stays where it is and nothing is said, as NVDA's stays when a call to a
busy application is cancelled. A press of a command the application took
longer than its bound to answer, with a press behind it, gives way
(`docs/design/m42-parity.md`, "Limits"), saying nothing.

Review presses run in press order among keys, as NVDA's scripts are one
queue (`scriptHandler.py` 242 to 252), a press with something behind it
bounded by 150 ms and one alone by its read's own deadline; a focus that
moves the navigator ends a press saying nothing. Core's
`a_gone_navigation_says_the_direction_edge_and_the_navigator_stays`,
`a_dead_object_reviews_as_empty_flat_text_and_reports_nothing`,
`a_press_with_something_behind_it_gives_way_at_its_bound`,
`a_focus_ends_a_navigation_press_and_what_waits_runs_on_the_new_navigator`
and `flat_reviews_current_line_leaves_the_position_where_it_was`;
mockapp's `dead_objects` tests through a real outpost; the
`review_dead_object`, `windows_terminal_review_stays_on_line` and
`conhost_review_stays_on_line` scenarios.

### Review cursor line/word/character over object text

NVDA: object
review over TextInfo ([Review modes](../nvda/review-modes.md)). Verbatim:
**matched (unverified)**: an edit field, document, or terminal is
reviewed a line at a time through the text protocol, starting at the caret (or the
start, with no caret) as NVDA's object review does; characters are
grapheme clusters and words follow the text's language
(`verbatim-text`); previous and next word cross lines; start and end of
line speak the character there; the page commands (Verbatim+Page Up and
Page Down), the selection's start and end (Verbatim+Alt+Home and End),
and the review cursor's and caret's locations are NVDA's commands too,
and at a page edge both say "Top" or "Bottom" and leave the review cursor
where it was. A location in text measured by character offsets (a plain
Win32 edit control, such as a Windows Forms text box, which NVDA reads
by offsets; NVDA reads a rich edit through its text object model) is
said as NVDA says it, the place's offset as a whole
percentage of the text's length (rounded half to even, an empty text
taken as one character long), then the place on the screen:
"93%, at 608, 516", or "93%" alone when the control gives no place;
other text (UI Automation's, and a rich edit's) says "Positioned at x, y". **Matched
(verified)** against an NVDA capture in the Windows Forms text box
(2026-11-03) and pinned by `text_box_review_cursor` and
`notepad_review_cursor`. Four of their answers are
different, pending a live capture and Dickson's decision: a page move
reads the line it lands on, where NVDA reads the whole page; where the
text cannot move by page Verbatim says "Not supported in this document",
where NVDA says "Movement by page not supported"; the selection's start
or end with no selection moves to the caret, where NVDA says "No
selection" and leaves the review cursor; and the review cursor's
location in text not measured by offsets (UI Automation's and a rich
edit's), when the text gives no place for it, says "Not supported in
this document" (offset-measured text says its percentage alone), where NVDA says the navigator object's location instead
and "No location information" only when that is missing too. An object
with no text interface is reviewed by its value or name,
NVDA's fallback, through the M3 flat walk, which walks
the same grapheme clusters and words as text with a text pattern;
different, as a follow-up: its start and end of line still speak the
line rather than the character there. Different, not yet matched:
flat review's current-line command moves the review position to the
line's start, where NVDA's current-line command leaves the review
position where it is, so a following character or word command starts
from a different place; it is fixed with the review cursor on objects
that no longer exist ([the roadmap](../roadmap.md), M4.2).

### Review bottom on a final empty line

NVDA's last position in text measured by character offsets is the
text's last character, so in a Windows Forms text box whose text ends in
a line break, review bottom reads the last line with text and the
previous line then reads the line above it; in UI Automation text, such
as Notepad's, its last position is the end, and bottom reads the empty
line after the final break. Verbatim: **different** (Dickson,
2026-11-03): bottom reads the text's last line in both, even when it is
empty ("blank"), because NVDA's handling of a final empty line is taken
to be a defect. Pinned by `text_box_review_words`.

### Previous word across a line break

In text measured by character offsets, NVDA's previous word from a
line's first word stops on the line break before it, saying "line feed"
and then "carriage return", before the previous line's last word.
Verbatim: **different** (Dickson, 2026-11-03): previous word goes
straight to the previous line's last word, as it does in UI Automation
text. Pinned by `text_box_review_words`.

### Review cursor columns

NVDA moves the review cursor to the start of the
next or previous line. Verbatim: **different** (Dickson, 2026-10-06): moving to another line,
page, or by line from a word keeps the column, so columns of a text table
or the map of a text-based game read line by line. In a terminal the
column is a cell column: wide characters (Chinese, Japanese, Korean) take
two cells by Unicode's East Asian Width, and a position past the end of a
row's text is a blank cell, read "blank", so moving down from column 10
always lands on column 10. Elsewhere the column is counted in characters
(grapheme clusters): a shorter line puts the cursor on its last
character, and the column is remembered, so the next longer line returns
to it, as editors do with their caret. From the second column of a
table in Notepad, NVDA's next line and current character say each row's
first letter ("A", "F", "B") where Verbatim says the column's ("3", "g",
"1"), and a later move by character from the kept column, such as
previous character, reads a different character from NVDA's. The rest
of the review commands (line, word, and character, spelling and
descriptions on repeated presses, top and bottom, and the edge messages) read the same in both, on both layouts.

### Select then copy from the review cursor

Select then copy from the review cursor (Verbatim+F9, Verbatim+Shift+F9,
Verbatim+F10 once to select and twice to copy). NVDA: the same keys and
messages ("Start marked", "No start marker set", "Can't select text,
press twice to copy", "Press twice to copy or reset the start marker").
The first press selects the range in the control, from the earlier of
the start marker and the review cursor through the character at the
later, whichever of the two that is, so the characters at both ends are
included whichever way the review cursor moved, and announces the
selection's change as a selecting key's is announced ("Name selected");
where the text cannot be selected it says "Can't select text, press
twice to copy". The first press keeps the range it built until it is
copied or the start marker is set again, so a further first press says
"Press twice to copy or reset the start marker" and selects nothing,
flat text included, where the first press only tried to select. The
second, within the multi-press timeout, copies that range ("Copied to
clipboard: Name") and clears the start marker whether or not anything
was copied, so a further press says "No start marker set"; a range with
no text copies nothing and says nothing. Verbatim clears only the marker
the copy was made for, so a marker set again while the copy's read is in
flight is kept. With the navigator in another
object than the marker, Verbatim+F10 says "No start marker set" and
keeps the marker, so back in the marker's object it selects from it.
NVDA has no message of its own for that case: the messages listed
above are all of NVDA's for these commands, with no "same object"
message. The captures
`target/e2e-evidence/m42-captures-2026-10-21/1a-review-copy-messages.txt`,
`1b-start-marker-other-object-parent.txt`, and
`1b-start-marker-other-object-and-back.txt` show both.
Verbatim: **matched (verified)** by `text_box_review_cursor` and
`notepad_review_cursor`, implemented in Core: the first press's
announcement comes from the same comparison of the selection before and
after, and the same wording, as a selecting key's, and the first press
says "Can't select text, press twice to copy" when the text cannot be
selected, flat text included. The marker in another object saying "No
start marker set" is Dickson's decision (Dickson, 2026-10-26). The range
with the review cursor moved back before the marker is covered by
`select_then_copy_back_from_the_marker_runs_from_the_cursor_through_the_marker`
(`crates/verbatim-core/tests/text.rs`), with flat text by
`flat_review_copies_back_from_the_marker_through_the_marker_s_character`
(`crates/verbatim-core/tests/reduce.rs`), and the outpost's range by
`a_range_through_its_ends_holds_both_ends_characters_whichever_comes_first`.
A copy the text cannot answer says "Not supported in this document" and
clears the marker too, a recorded difference: NVDA keeps the marker and
says nothing, since the read's failure escapes its script. NVDA's "No
text to copy", for a first press whose range holds nothing, which it
says and then clears the marker, is not built: the range holds nothing
only where the marker and the review cursor are at the same place with no character there, such as the end of
the text, and Core cannot tell that two positions from different reads
are the same place, so such a press asks the control to select nothing,
and its second press copies nothing.

### Document review and screen review modes

Verbatim: **not yet
(M7)**; screen review will be a projection of the tree, different by
decision D11.

### Object activation

Object activation (do default action). NVDA: the review position's
or navigator's action, walking up the parents until one has an action,
then its name spoken, or "No action". Verbatim: **matched (unverified)**:
an MSAA object's default action is spoken by its own name
("Press"), a UIA element's Invoke as "invoke", and an action with no
name as "Activate"; a UIA element is activated by Invoke, then Toggle,
then selecting it, NVDA's order. NVDA first tries the review
position's own activation, which the M4 text model brings.

### Move focus to navigator / caret routing

Verbatim: **not yet
(M4)**.
