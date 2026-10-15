# Terminals

How new output in a terminal is read.

### New terminal output

NVDA: [Editable text and
terminals](../nvda/editable-text-and-terminals.md), "Terminals": diffing by
default, the whole document per text change, on UIA's text changes
alone for the console host (`UIAHandler/__init__.py` 145 to 147 and 674
to 679). **Different (since 2026-10-08):** in the console host,
Verbatim also takes the console's own update `WinEvents` as text
changes, since UIA's stopped reaching the outpost in the middle of large
writes in 2 of 14 runs; Windows Terminal's output
notifications only behind a flag; every new line spoken, queued, until a
key cancels speech; a one-character change dropped as probably typed.
Verbatim: **implemented since 2026-10-06** (M4 item 9); since
2026-10-08 each read diffs the screen with the screen last read
(`docs/design/phase6-design.md`, "Terminal reading by diffing the screen";
`docs/crates/verbatim-outpost.md`, "Terminals"), where NVDA diffs the
whole document: as NVDA's diff speaks only what was inserted
(`diffHandler.py` 36 to 48, `DiffMatchPatch.diff` keeping only `+`
pieces and dropping white-space ones), lines only deleted, and a line
that only lost text or gained white space, say nothing. Windows
Terminal's notifications from a terminal, and the console host's from
its text area, are all ignored, whatever their activity id, so nothing
is spoken twice (as NVDA's overlays block them, `winConsoleUIA.py` 431
to 440 and 474 to 476); a redraw with the same text
speaks nothing; blank lines are not spoken but every line counts,
blank ones included (Dickson, 2026-10-07, confirmed in the coherence
review of 2026-10-09): rows a program passed over without writing (a
cleared screen's rows above a footer drawn on its last row) are not
lines, being blank before as they are now, and a line that wrapped is
one line however many rows it took; newer
output never cancels older; a key, or anything else that cuts speech
off, drops output still waiting, as in NVDA, and output read before
the key is never spoken after it, while output read after the key is
spoken, as in NVDA (Dickson, 2026-10-10; the `*_control_flood`
scenarios). **Different:** the backlog is capped
("30 and 30", `docs/design/phase6-design.md`, "The flood policy, reconsidered"):
with more than "Lines spoken in full" (30) lines waiting once a
group's last line has been heard, the older ones become "skipped N
lines" ("skipped lines" when the count is lost, "skipped more than N
lines" when the terminal's history overflowed past the screen last
read, N the history's rows left unspoken) and the newest "Last lines to
speak" (30) are kept; NVDA speaks them all
(`behaviors.py` 455 to 466, every line queued). The terminal is read on
demand while Core's queue is full: the outpost only notes changes until
Core asks, when a group's last line is handed to speech, where NVDA's
monitor thread diffs on every change (`behaviors.py` 468 to 505). Nothing is lost for good: every
line is still there for the review cursor. **Different:** in place of
NVDA's rule dropping any one-character change, the typing Core holds is
echoed when the terminal shows it at the end of the line and not spoken
again as output, and a line changed some other way (a password prompt's
asterisks) drops the held typing unspoken, so a tab completion's added
text is read and a password is never spoken. A line rewritten in place
echoes the typing only when what it gained is exactly the typing (a
character typed in the middle of a command), so a clock ticking on a
password prompt's line never speaks the password, where NVDA speaks its
queued characters on any text change (`behaviors.py` 591 to 593).
Typing the text cannot show (a space at the end of a line, which is
padding until something follows it, or a character typed over inline
prediction that showed it already) is echoed when the caret moves over
it; typing that wrapped past the right margin is echoed, not spoken as
a new line. Held typing is forgotten on Enter and Tab, and on Escape,
Control+C, Control+D, and Control+Break; NVDA forgets its queued
characters on Enter, Tab, Control+C, Control+D, and Control+Pause
(`behaviors.py` 595 to 613). **Different, deliberate** (proposed
2026-10-09, kept by Dickson's decision of 2026-10-09 in the coherence
review): Escape forgetting held typing is Verbatim's addition: captured live,
NVDA spoke the two characters typed at a prompt that showed nothing
("a", "b") when Escape then changed the line, before the line's new
text, since it dispatches its queued characters on any text change
(`behaviors.py` 591 to 593 and 615 to 619); Verbatim speaks only the
new text. The console host's keys are translated with the
console host's own keyboard layout, its thread learned from the console
window's `WinEvents`, as NVDA does (`IAccessibleHandler/internalWinEventHandler.py`
151 to 153 and 266 to 273, `NVDAObjects/UIA/winConsoleUIA.py` 378 to
388, issue 10113).

Checked against NVDA captured live on 2026-10-09 (NVDA alpha-57645, both
terminals, the keys of `conhost_typing` and `windows_terminal_typing`
and of the NVDA study's cases, each typed a character at a time;
replayed on 2026-10-12 with alpha-58022, the build the pinned source
is, which says the same, its typed echo split by timing only):

- The same: every character typed is echoed as typed; a space typed at
  a line's end is echoed at once ("space"); characters typed past the
  right margin are echoed one by one; a character typed in the middle
  of a command after `cls` is echoed; Left Arrow says the character
  ("o"); Backspace and Control+Backspace say "o" and "tw"; `cls` says
  the prompt; typing over inline prediction echoes each character.
- **Different, recorded:** at a prompt that rewrites its line on each
  key and shows nothing typed (a ticking clock), NVDA spoke the
  characters typed ("a", "b", "c": the password) and not the count,
  which as a one-line change shorter than three characters it drops as
  probably typed (`behaviors.py` 559 to 566); Verbatim speaks the count
  ("1", "2", "3") and never the characters. Escape and other keys
  outside the caret table, as above.
- **Not Verbatim's terminal handling:** NVDA says the prompt `ready>` as
  "ready greater", and Verbatim as "ready>" until symbol processing
  (M8); NVDA also says "Oscar" after "o" for Left Arrow, its delayed
  character description, a setting this NVDA has on, which Verbatim
  does not have yet (the settings package).
- Found and fixed by the comparison: a line cleared with Escape and
  typed again was not echoed (it was compared with what the line had
  said, which still held the cleared text; typing is now matched with
  the line's change since it was read, `LineChange::since_read`); `cls`
  in Windows Terminal was silent where NVDA says the prompt (a screen
  cleared down to its prompt paired the prompt with an old line as a
  rewrite; new lines fewer than the old ones they replace are now new).

Long lines and a full-screen program, captured live from NVDA on
2026-10-09 in both terminals: a line of forty words is spoken whole by
both, and a line growing by ten words a key speaks each key's words in
both; opening the alternate screen, NVDA spoke all thirty rows in
Windows Terminal but only the last six in the console host, where
Verbatim speaks all thirty in both (within the flood limit); a row
changed says "changed" in both, and the rows scrolled down and up say
"row 0" and "row 30" in both; closing the alternate screen, NVDA spoke
the whole main screen again (PowerShell's notice, the command, then
"closed"), where Verbatim speaks only "closed" and the prompt, the main
screen being as it was (**different, since 2026-10-09**; kept as a
deliberate difference by Dickson's decision of 2026-10-09 in the
coherence review). NVDA echoes
each key a script reads without showing it ("space"); Verbatim does
not, as for any typing the terminal does not show.

The review cursor while output is written, captured live from NVDA on
2026-10-09 in both terminals: reviewing a line above the caret, then
letting a script print two lines, both follow the caret to the prompt,
so the current line is the prompt and the line before it the new
output's last (matched). A footer below a scroll region that a flood
scrolls above, redrawn unchanged during the flood and changed after it,
captured the same day: both say the footer, "status: busy", when it is
drawn; for the flood NVDA alpha-57645, with no flood policy, said lines
35 to 38, 98 to 100 and "status: done" in the console host, and lines 2
to 100 and "done" in Windows Terminal, leaving lines out as that build
did; NVDA alpha-58022, which speaks every new line (the floods replayed
on 2026-10-12, `docs/design/terminal-reading.md`, "Known weaknesses"),
is still to be captured on this script. Verbatim applies the flood
policy (lines 1 to 30, "skipped 41 lines", lines 72 to 100) and says
the footer only as it changed, "done", in both (**different**: the
flood policy).

Two windows of each terminal and two tabs of Windows Terminal, captured
live from NVDA on 2026-10-09 (an alpha build, alpha-57645; the switches
replayed on 2026-10-12 with alpha-58022, the build the pinned source
is, say the same). Switching to
the other window, as clicking its taskbar button does, both say the
window's title and "window", then the terminal and the caret's line:
"<title> terminal" in Windows Terminal, whose text area is named for its
tab, and "terminal" in the console host, whose text area's name both
drop (matched; NVDA says the terminal and its line as one utterance,
Verbatim as two). A second tab opened from the command line says
"<title> terminal" and then the prompt as output; Verbatim says
"blank" before the prompt (**different, deliberately**: NVDA's
behaviour judged a bug by Dickson on 2026-10-09). A new terminal
window or tab says "blank" for the empty line its caret is on as it
takes the focus, in Windows Terminal as in the console host, as an
empty line says everywhere else. NVDA reads the caret's line for a
focused terminal, "blank" for an empty one (`speech/speech.py`
898-917 and 1929-1933), and says it in the console host in all 14
captures, yet in Windows Terminal it said no line for a window or tab
with nothing written yet in 17 of 19: it does not recognise the
terminal's focus there, which is not something to copy.
`windows_terminal_two_windows`, `windows_terminal_tabs` and
`windows_terminal_close_tab` assert "blank" for the second window
and the second tab, and every Windows Terminal scenario for its first
window. Returning to a tab or window, NVDA said its prompt
in some captures and no line in others; Verbatim says the prompt.
Up Arrow and a character typed at once (`*_up_typing`, captured
live on 2026-10-09): NVDA says the character, "x", as it is typed and
then the recalled line with it, "echo onex"; Verbatim reads the line's
change once, both in it, and says "echo onex" without the character's
own echo (**different, deliberately**: kept as Verbatim's behaviour by
Dickson on 2026-10-09, since it does not occur in normal use,
`docs/design/phase6-design.md`).
Typing and
its output after a switch are echoed and spoken as in any terminal
(matched). An earlier report that Verbatim left the window's title out
of a console's terminal was not borne out: NVDA leaves it out too.
Switching tabs with Control+Tab or Control+Shift+Tab: **matched since
2026-10-11** (stage A step 6, `docs/design/focus-pipeline.md`, section
10.1, items 2 and 3; awaiting its first build and live runs). Windows
Terminal gives the tab the keyboard focus only until it moves it to the
tab's terminal, a few milliseconds later, and on Control+Tab gives it
first to the tab it leaves. NVDA, captured live on 2026-10-09, queued
"list" and the tab ("<title> 1 of 2"; on Control+Tab the tab it left,
then "<title> two 2 of 2"), then "<title> terminal", and culls a focus's
speech once the focus has moved on. NVDA judges a UIA focus event by the
sender's keyboard focus as its event thread receives it
(`IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent` and
`shouldAllowUIAFocusEvent`, `nvda/source/UIAHandler/__init__.py` lines
948 to 953 and `nvda/source/NVDAObjects/UIA/__init__.py` lines 1632 to
1637), on its UIA flusher thread a few milliseconds after the event,
and queues every UIA focus event it accepts. Verbatim's focus listener
now does the same: it judges and reads each focus from the event's own
element on its thread for the application, a few milliseconds after the
event, and the outpost reports every focus it read, in the order the
events arrived; Core culls the speech of a focus no longer current. The
tab's ancestors are its own element's, the terminal's container above
its list, so Verbatim says "list" and the tab, as NVDA does, and the tab
left is announced and then culled, as NVDA queues it. Heard, either key
says "list", the tab or tabs (culled), "<title> terminal", and the line
(`windows_terminal_tabs`). Before step 6 the outpost judged the focus
when its worker reached it, by reading the system's focused element,
found the tab, whose focus event comes from an element that is in no
tree, by its name and position, so said "tab control" before "list"
(accepted by Dickson for the time being, 2026-10-10), and dropped the tab
left, which had lost its selection by then (8be2223, Dickson,
2026-10-10, withdrawn with the design's decision 2 of section 10.1).
A single Control+Shift+Tab, Control released at once, gives NVDA's
speech by timing: of two captures on 2026-10-09, one had only the
terminal, the other the list and the tab. With Control held through the
tab switcher, as a person switches, NVDA's speech is fixed: captured on
2026-10-12 (alpha-57645, the portable Windows Terminal 1.24, two tabs,
and again three times the same day with alpha-58022, the build the
pinned source is, which says the same),
Control held then Shift+Tab says "Tab switcher grouping", "list", and
the tab moved to ("<title> 1 of 2") in full, and releasing Control the
tab's terminal and its line; Control held then Tab the same for the
second tab ("<title> two 2 of 2"). Since 2026-10-12
`windows_terminal_tabs` switches that way (`Scenario::hold_key`) and
expects exactly that speech; whether Verbatim says "Tab switcher
grouping" is to be seen in its first run. The two-window scenarios
switch windows by bringing them to the front (see above).

Up and Down Arrow in a terminal, a selection list a program draws
among them: **matched since 2026-10-09** (Dickson, 2026-10-09,
`docs/design/phase6-design.md`, "Terminal line keys as NVDA has them", reversing
the earlier deliberate difference). NVDA reads the caret's line at the
first change of the caret's position after the key, or, finding none
within its wait, at the end of it (`source/editableText.py`, lines 70
to 163 and 181 to 192, the line spoken by lines 165 to 178; a
terminal's caret events are not used, `NVDAObjects/behaviors.py` lines
527 to 530); what the program redraws elsewhere is new output to its
diff. Verbatim answers the key with the caret's line once the caret
has moved, as in a text field, and speaks what the program redrew
elsewhere at once as output, through the screen diff; a key whose
answer said what it did to the caret's line keeps that line's change
from being spoken again as output, and until the answer comes the
change is output like any other. Captured live on 2026-10-09 with
scripts that move a list's ">" marker in one write: when the caret
follows the marker, both say "> banana", "> cherry" and "> banana"
(NVDA "greater banana") in both terminals (`windows_terminal_marker_list`,
`conhost_marker_list`); when the program redraws the list and leaves
the caret below it, NVDA says "blank" for each key, the caret's line at
the end of its wait, and drops the marker's change as a single
character inserted on one line, where Verbatim says the line that
gained the marker as output, from the word that changed, and nothing
for the key, which did not move the caret (`windows_terminal_redrawn_list`,
`conhost_redrawn_list`; **different**, by the two deliberate
differences recorded above and below). A program that redraws a little
at a time is read part-way, by both: with the earlier scripts, which
wrote each cell on its own, NVDA said "greater banana", "greater
banana" and "cherry" in the console host and, for a list redrawn row by
row, "apple" and "blank" by turns. A pager moving a line on its
alternate screen (`windows_terminal_pager`, `conhost_pager`, captured
the same day): NVDA says the prompt, ":", at the end of its wait and
then the line the move brought onto the screen; Verbatim says that
line alone. The alternate screen's text scrolls through rows that stay
put, so Verbatim finds how far it scrolled from the text.

Live trials on 2026-10-09 in both terminals, each against NVDA captured
the same day (the pager of `git log`, Git's `less.exe`, `vim --clean`
with and without `:set relativenumber`, tig, Microsoft Edit's file list,
the prompt of `gh repo create`, PSReadLine's MenuComplete and nano's
file browser): where a program writes each move in one go, both say the
caret's line or the new line as above. The differences left: Verbatim
speaks a rewritten status line from the word that changed (vim's ruler,
"2,1 Top"; tig's commit count), where NVDA drops a change of one or two
characters on one line as probably typed (`behaviors.py` 559 to 566);
Verbatim says nothing for a key that leaves the caret where it was (the
pagers' ":" prompt, tig's and Microsoft Edit's lists, PSReadLine's menu
after its first move), where NVDA says the caret's line at the end of
its wait; with `relativenumber` every visible line changes on each
move, which NVDA speaks as a run of numbers and Verbatim as the changed
lines from their numbers, both within their limits; and programs that
write a move a little at a time (the pagers' prompt in the console
host, the prompt of `gh`, nano's browser) are read part-way by both,
differently.

**Different:** a line the
terminal rewrites in place speaks from the start of the word where it
first differs (Unicode's word rules with ICU's dictionaries, whole
graphemes), where NVDA's character diff speaks the inserted characters;
a single symbol replaced in place (a spinner) is silent until the line
changes to text (Dickson, 2026-10-07); and a rewrite while the earlier
version is still waiting replaces it. Verbatim+5 toggles "Report new output", on NVDA's key for
its "report dynamic content changes" toggle, saying "report new output
on" and "off". NVDA transcripts on 2026-10-07 showed how NVDA reads the
terminal scenarios' flood of 10,000 numbered lines: in Windows
Terminal it queued 690 of them, in 53 runs from line 323 to line
10,000, as its monitor diffed whatever text was current, and all of
them stayed queued until a key; in the console host its reading slowed
the output itself, queuing a few lines at a time, and was still
speaking the flood after the window closed. At a password prompt NVDA
echoed each typed character of the password (its typed-character echo
covers terminals), where Verbatim speaks the asterisks the console
shows. Commands, their output, the prompt, and the review cursor over
the output read the same in both, in both terminals, apart from symbol
names in flowing text ("ready greater" for the prompt `ready>`), which
wait for symbol processing (M8).
