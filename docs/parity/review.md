# Object navigation and review

Object navigation, the navigator and review cursor, and the commands that report and review objects.

### Navigation tree shape

NVDA default: simple review on (filtered
tree, [Focus and the navigator](../nvda/focus-and-navigator.md)). Verbatim: full tree,
matching NVDA with simple review **off** — the project's stated
baseline (maintainer decision; commit 6b7a519). **matched
(verified)** against that baseline; simple-review-on filtering is
**not planned** (revisit only if the baseline changes).

### The API at a window boundary

NVDA: an object reached by navigation, or
as a focus ancestor, in a different window is read through that
window's API (`correctAPIForRelation`): an MSAA object in a UIA window
becomes the window's UIA element, and a UIA element that is another
window's root, in an MSAA window, becomes that window's MSAA object.
Verbatim: **matched since 2026-10-03**, for object navigation and for
focus ancestry in both directions; the UIA-to-MSAA direction of focus
ancestry was already matched. Checked live in Notepad's Save As dialog:
the file name box's ancestry continues through the shell's UIA view,
and navigating to its parents switches to UIA there. Until 2026-10-03
an MSAA walk or navigation step stayed in MSAA across windows. Where a
focus's MSAA walk crosses into a UIA window, the window's element is
presented by NVDA's rule for a UIA object, as every UIA ancestor is:
only when it is presentable by role, name, and description and UI
Automation counts it both a control and content (since 2026-10-11;
before, it was reported whatever it was, and the reducer judged it by
role and name alone).

### Navigator follows focus; review follows navigator

NVDA coupling
rules ([Review modes](../nvda/review-modes.md)). Verbatim: **matched
(unverified)** for the follow-focus default. Following the caret
("caret moves review cursor", on by default, toggled by Verbatim+6 with
NVDA's messages) is **implemented in Core since 2026-10-06**,
unverified until the outpost sends caret reports; `followMouse` is
**not yet**. NVDA's "review cursor follows focus" setting (on by
default) has no counterpart yet: Verbatim always follows the focus, and
the setting is part of the settings package (decided 2026-10-08, not
yet built).

### Report current object

report / spell / copy on 1st/2nd/3rd press.
NVDA: script repeat counting ([Keyboard input](../nvda/input.md)), reading
the object live. Verbatim: **matched since 2026-10-03**; since
2026-10-03 a name, value, or state change on the focus also updates the
navigator's copy while it rests there, so the report reads the object
as it is now (it had read the copy taken when focus landed, saying "not
checked" for a box just checked). The spelled text and the copy wording
are covered under the messages entries below. An object with text (an
edit field, a document, a terminal) spells and copies its name followed
by its selected text or the line at its caret, as NVDA does
([Speech](../nvda/speech.md), "What an object with text says"): **matched
since 2026-10-07**, found by comparing transcripts with NVDA in
Notepad, where Verbatim had spelled the whole value.

### Report focus

Report focus (NVDA+Tab, every layout): the focus object reported as a
query on the first press, its text included as on focus; its name
spelled on the second press ("blank" with no name), and spelled with
character descriptions on the third and later; "No focus" with none
([Focus and the navigator](../nvda/focus-and-navigator.md), "Reporting the
focus"). Verbatim: **matched since 2026-10-07** on Verbatim+Tab,
checked live in Windows 11 Notepad against an NVDA transcript the same
day: both say "Text editor document focused" and the line at the caret,
NVDA in one utterance and Verbatim in two, the line as its own, as for
any focus with text (the Notepad scenarios, when a Notepad window was
already open). One
difference with no counterpart in NVDA: while the outpost that reported
the focus has ended and its replacement has not yet reported the focus
again, Verbatim says "No focus", as its node ids name nothing then.

### Review and navigator messages and repeated presses

Review and navigator messages and repeated presses (the list in
[Review modes](../nvda/review-modes.md), "Reading commands built on
review"). Verbatim: **matched since 2026-10-03**: the edge messages
("Top", "Bottom", "Left", "Right") with the unit read again, character
motions kept within the line, "blank", the current line or word spelled
on a second press, the current character's code on a third, "space" in
spelling, report current object spelling the name and value, "Move to
focus", "No navigator object", and activation saying "Activate" or "No
action" after walking up the parents. A copy says "Copied to clipboard:"
with the text (its length from 1024 characters on) after reading the
clipboard back, or "Unable to copy". The current character's description
on a second press and the current line or word spelled with
descriptions on a third are **matched since 2026-10-06**, from the
English character table in `verbatim-i18n` (the phonetic alphabet, spelled as NVDA spells it, "Alfa" and "Xray", since 2026-10-07), and
a punctuation character spoken on its own or spelled is named from the
same table ("comma"). Raised pitch for
capitals is **matched since 2026-10-04** (unverified by ear): spelling
and reading a single character speak an uppercase letter with the pitch
setting raised by 30 and then restored, for every synthesizer, as NVDA
does by default ([Speech](../nvda/speech.md), "Capitals when spelling");
the offset is not yet configurable, and saying "cap" or beeping for
capitals is not offered. eSpeak NG and OneCore speak the pitch change
within one synthesis, as SSML prosody written as NVDA's drivers write
it, so a capital brings no pause; a synthesizer that cannot is given
the change as a pitch setting between separate synthesis calls, which
can leave a short pause. Since 2026-10-08 a character is composed (NFC)
before its case is judged and before the table is looked up, so "E"
written with a combining acute accent is spelled as the capital "É",
and a capital is a character with some uppercase letter and no
lowercase one, so a capital with accents that have no precomposed form
is still raised. A capital's description is its small letter's by the
text's language, so Turkish İ is described as i, and the table names
the ideographic space (U+3000), since 2026-10-08.

### Toggle key announcements

Toggle key announcements ("caps lock on", "num lock off", "scroll lock
on") when a lock key reaches the operating system, including Caps Lock
passed through by a double tap of the Verbatim key. NVDA:
`KeyboardInputGesture.reportExtra`. Verbatim: **matched since
2026-10-03**, verified live with Num Lock, and covered by the
`lock_key_announcements` scenario with Scroll Lock since 2026-10-05. The
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
with itself (confirmed live on 2026-10-05); another object, such as a
windowless sibling in the same window, is a real neighbor, except that
a first child must be in the object's own window or one inside it, and
an object whose `accNavigate` finds no first child is asked for its
children (`AccessibleChildren`), both as NVDA does. Verbatim: **matched
(verified)**
(commits 9bd6bec, 0fe39f0); NVDA wording comparison still
worthwhile.

### Sibling navigation resolving back to self reported as edge

Sibling navigation resolving back to self reported as edge
(MSAA `accNavigate` quirk). NVDA: fallback heuristics
([MSAA and winevent handling](../nvda/msaa.md)). Verbatim: **matched (unverified)**
(commit 0fe39f0).

### Windowed MSAA children navigated through the window hierarchy

NVDA: same strategy. Verbatim: **matched (verified)** (msinfo32
E2E; commit d3e13b7).

### SysTreeView32 via TVM messages, Tree/TreeItem roles

NVDA:
control-specific overlay. Verbatim: **matched (verified)**
(system_information_tree E2E; commits 809214d, 1078614). A Windows
Forms tree view (`WindowsForms10.SysTreeView32.app.…`) gets the same
handling, since NVDA matches its class rules against the normalized
class name: Verbatim compared the raw name, so such a tree's items had
no position, flat navigation, and no expanded count. **Matched since
2026-10-07**: every class rule in `verbatim-ia2` compares the normalized
name (`native_controls` mockapp test, a real comctl32 tree view under a
Windows Forms class name; the end-to-end scenario is still to be
written). **Different:** a tree view with comctl32's own check boxes
(`TVS_CHECKBOXES`) reports its items to MSAA as check boxes, which NVDA
gives no tree view handling at all, reading them as plain check boxes
with MSAA's flat navigation; Verbatim still navigates and walks them
through the control's messages, which reads more usefully and changes
nothing spoken on focus. A tree view that draws its own check boxes
(msconfig's, say) says whether an item is checked only through the
item's state image, which NVDA reads with `TVM_GETITEMSTATE`: any state
image makes the item checkable, the second checked, and the third half
checked. **Matched since 2026-10-07** (`native_controls` mockapp test:
"checked", "not checked", and a half-checked parent; the end-to-end
scenario is still to be written). A tree item outside a `SysTreeView32`
is NVDA's generic outline item, which drops its value only when it is a
number and takes no level from it; Verbatim had made any tree item's
numeric value its level and dropped any other value. **Matched since
2026-10-07** (`native_controls` mockapp test, scripted tree items).

### Navigator death recovery

NVDA reports failure and stays; Verbatim
re-seeds navigator from focus on `Gone` and announces it —
**different (documented in [verbatim-core](../crates/verbatim-core.md))**; NVDA-side
behavior in [Focus and the navigator](../nvda/focus-and-navigator.md).
A navigation the application did not answer (too slow, or the read
failed) is not `Gone`: since 2026-10-03 the navigator stays where it is,
as NVDA's stays when a call to a busy application is cancelled; it had
jumped to the focus and announced it.

### Review cursor line/word/character over object text

NVDA: object
review over TextInfo ([Review modes](../nvda/review-modes.md)). Verbatim:
**implemented in Core since 2026-10-06, unverified** until the outpost
speaks the text protocol: an edit field, document, or terminal is
reviewed a line at a time through it, starting at the caret (or the
start, with no caret) as NVDA's object review does; characters are
grapheme clusters and words follow the text's language
(`verbatim-text`); previous and next word cross lines; start and end of
line speak the character there; the page commands (Verbatim+Page Up and
Page Down), the selection's start and end (Verbatim+Alt+Home and End),
and the review cursor's and caret's locations ("Positioned at x, y") are
new. An object with no text interface is reviewed by its value or name,
NVDA's fallback, through the M3 flat walk, which since 2026-10-08 walks
the same grapheme clusters and words as text with a text pattern (it had
walked code points and white space, so a Hindi vowel sign stopped alone
and a Thai name was one word); its start and end of line still speak the
line rather than the character there (**different**, a follow-up).

### Review cursor columns

NVDA moves the review cursor to the start of the
next or previous line. Verbatim: **different, deliberately** (decided
2026-10-06, `docs/design/phase6-design.md`, M4 item 5): moving to another line,
page, or by line from a word keeps the column, so columns of a text table
or the map of a text-based game read line by line. In a terminal the
column is a cell column: wide characters (Chinese, Japanese, Korean) take
two cells by Unicode's East Asian Width, and a position past the end of a
row's text is a blank cell, read "blank", so moving down from column 10
always lands on column 10. Elsewhere the column is counted in characters
(grapheme clusters): a shorter line puts the cursor on its last
character, and the column is remembered, so the next longer line returns
to it, as editors do with their caret. An NVDA transcript on 2026-10-07
confirmed the difference: from the second column of a table in
Notepad, NVDA's next line and current character say each row's first
letter ("A", "F", "B") where Verbatim says the column's ("3", "g",
"1"). The rest of the review commands (line, word, and character,
spelling and descriptions on repeated presses, top and bottom, and the
edge messages) read the same in both, on both layouts.

### Select then copy from the review cursor

Select then copy from the review cursor (Verbatim+F9, Verbatim+Shift+F9,
Verbatim+F10 once to select and twice to copy). NVDA: the same keys and
messages ("Start marked", "No start marker set", "The start marker must
reside within the same object"). Verbatim: **implemented in Core since
2026-10-06, unverified** until the outpost selects and reads ranges; the
first press says nothing when the selection was made, and "Not supported
in this document" when the text cannot be selected.

### Document review and screen review modes

Verbatim: **not yet
(M6)**; screen review will be tree-projection **different (D11)**.

### Object activation

Object activation (do default action). NVDA: the review position's
or navigator's action, walking up the parents until one has an action,
then its name spoken, or "No action". Verbatim: **matched since
2026-10-03**: an MSAA object's default action is spoken by its own name
("Press"), a UIA element's Invoke as "invoke", and an action with no
name as "Activate"; a UIA element is activated by Invoke, then Toggle,
then selecting it, NVDA's order (Verbatim had tried MSAA's default
action through UIA in place of selecting). NVDA first tries the review
position's own activation, which the M4 text model brings.

### Move focus to navigator / caret routing

Verbatim: **not yet
(M4)**.
