# Terminals

How new output in a terminal is read.

Verbatim's reading of terminals is [the terminal
model](../design/terminal-model.md), which states each rule with its
reason; `docs/crates/verbatim-core.md`, "Core's terminal model", and
`docs/crates/verbatim-outpost.md`, "Terminals", describe the code. The
terminal scenarios named below each run in both terminals, as
`conhost_` and `windows_terminal_` scenarios. A rule cited as "rule N"
is that reading rule of the terminal model.

### New terminal output

NVDA: [Editable text and
terminals](../nvda/editable-text-and-terminals.md), "Terminals",
"Terminal details at the pinned source", and "What NVDA says in
terminals, captured live": the whole document diffed on each text
change, speaking only what was inserted; the console host read on UIA's
text changes alone; terminal notifications blocked, Windows Terminal's
used only behind a flag; every new line spoken, queued, until a key
cancels speech; a change of one or two characters on one line dropped as
probably typed; typed characters spoken on the terminal's next text
change of any kind.

Verbatim: **different**, case by case in the entries that follow, some
matched and some deliberately different: "Terminal output read row by
row", "Redrawn rows", "The backlog", "Typing", "The alternate screen", "The review cursor while output is written",
"Windows and tabs", "Switching tabs" and "Line keys and lists a program
draws". End-to-end steps declaring a difference against this entry show
the flood policy ("The backlog"), output a key's answer leaves out
("Line keys and lists a program draws"), and short output ("Typing").

### Terminal output read row by row

Verbatim: **different** (Dickson, 2026-10-23; rules 5, 12 and 13).

- Only terminals with UIA text are read: Windows Terminal, the terminal
  embedded in Visual Studio, and the console host, each recognised by
  its control in Verbatim's core rather than by an app module, as NVDA's
  overlays are. NVDA's legacy console support, for consoles without UIA
  text, is not ported or tested (Dickson, 2026-10-08), since every
  supported Windows has the console host with UIA.
- Each read is aligned with the rows last read, row by row, so that
  every row is new, kept wherever it moved, or rewritten, where NVDA
  diffs the whole document by character. As NVDA's diff speaks only what
  was inserted, lines only deleted, and a line that only lost text or
  gained white space, say nothing.
- A line still being written speaks what it gained, but holds back its
  unfinished last word until the line grows past it, the caret leaves
  the line, or the quiet period passes, so a streamed word is not spoken
  in pieces.
- In the console host, the console's own update `WinEvents` are taken as
  text changes as well as UIA's, merged per window, and a change observed
  before the last read began is covered by it, since UIA's text changes
  stopped reaching the outpost in the middle of large writes in 2 of 14
  runs (different).
- Windows Terminal's notifications from a terminal, and the console
  host's from its text area, are all ignored, whatever their activity
  id, so nothing is spoken twice, as NVDA's overlays block them.
- A redraw with the same text speaks nothing; blank lines are not spoken
  but every line counts, blank ones included (Dickson, 2026-10-07): rows
  a program passed over without writing (a cleared screen's rows above a
  footer drawn on its last row) are not lines, and a line that wrapped is
  one line however many rows it took.
- Newer output never cancels older; a key, or anything else that cuts
  speech off, drops output still waiting, as in NVDA, and output read
  before the key is never spoken after it, while output read after the
  key is spoken, as in NVDA (Dickson, 2026-10-10; the `*_control_flood`
  scenarios).
- Verbatim+5 toggles "Report new output", on NVDA's key for its "report
  dynamic content changes" toggle, saying "report new output on" and
  "off".
- A program that erases the screen and the history and prints them
  again, as Pi's regular mode does on Control+O to expand or collapse its
  tool output, is heard only within the screens Verbatim last read: Pi's
  "Tool output: expanded" is spoken, and lines the program added above
  them are not, read with the review cursor instead (Dickson, 2026-10-25,
  a listed limit; pi-changed-above recording). NVDA's character diff of
  the whole document would speak those added lines too, with its usual
  fragments; no capture exists.
- Commands, their output, the prompt, and the review cursor over the
  output read the same in both screen readers, in both terminals, apart
  from symbol names in flowing text ("ready greater" for the prompt
  `ready>`, Verbatim "ready>"), which wait for symbol processing (M8).
  A line of forty words is spoken whole, and a line growing by ten
  words a key speaks each key's words, as in NVDA (`*_long_lines`).

### Redrawn rows

NVDA's character diff speaks the inserted characters of a changed row,
and it drops a change of one or two characters on one line as probably
typed. Verbatim: **different** (Dickson, 2026-10-07 and 2026-10-23;
rules 16 and 17).

- Any row a program rewrites in place is a redrawn row, wherever it is
  on the screen. Its change speaks from the start of the word where it
  first differs (Unicode's word rules with ICU's dictionaries, whole
  graphemes); a single symbol replaced in place (a spinner) is silent;
  and a rewrite while the earlier version is still waiting replaces it.
  So Verbatim speaks a rewritten status line from the word that changed
  (vim's ruler, "2,1 Top"; tig's commit count).
- A redrawn row that keeps changing (a volatile row: one that changed
  twice within 2 seconds, each a change the program owns) has its first
  change heard, so the user knows it exists, and is then silent until it
  has been unchanged for the quiet period, 1 second or 1.5 times the
  interval between its last two changes if that is longer, when its text
  is spoken once. So a timer says "Working... 1s" and then "Working...
  34s" when the work ends, where NVDA speaks every change it reads.
- Colours that change with no key and no text change are not heard, and
  panes a program draws side by side (tmux) are not told apart, since
  they share rows (Dickson, 2026-10-23; the model's "Limits").

### The backlog

NVDA speaks every new line, queued, until a key cancels speech, and its
monitor thread diffs on every change. Verbatim: **different** ("30 and
30", Dickson, 2026-10-06 and 2026-10-07; the model's "Features and
settings", Core's flood policy).

- With more than "Lines spoken in full" (30) lines waiting once a
  group's last line has been heard, the older ones become "skipped N
  lines" and the newest "Last lines to speak" (30) are kept. The summary
  is "skipped lines" when the count is lost; "skipped more than N lines"
  when the lines went by past the terminal's history, as when one write
  larger than the history follows a fresh or cleared screen
  (`conhost_fresh_flood`, `windows_terminal_clear_flood`), N the
  history's rows left unspoken; and "skipped about N lines" when a lost
  read counted them from text, as after Windows Terminal's clear followed
  by more than a screen of output before the next read, such as
  `clear; dir` typed as one line, whose every line after the clear is new
  (Dickson, 2026-10-25; `windows_terminal_cleared_flood`). Nothing is
  lost for good: every line is still there for the review cursor (the
  `*_flood` and `*_scrollback_overflow` scenarios).
- The terminal is read on demand while Core's queue is full: the outpost
  only notes changes until Core asks, when a group's last line is handed
  to speech (rule 1).

### Typing

NVDA speaks typed characters on the terminal's next text change of any
kind, however late, drops a change of one or two characters on one line
as probably typed, and forgets its queued characters on Enter, Tab,
Control+C, Control+D, and Control+Pause. Verbatim: **different** (rule
20; Dickson, 2026-10-08, 2026-10-09 and 2026-10-23).

- Matched (`*_typing`): every character typed is echoed as typed; a space
  typed at a line's end is echoed at once ("space"); characters typed
  past the right margin are echoed one by one; a character typed in the
  middle of a command after `cls` is echoed; Left Arrow says the
  character ("o"); `cls` says the prompt; typing over inline prediction
  echoes each character.
- In place of NVDA's rule dropping a change of one or two characters,
  the typing Core holds is echoed when the terminal shows it at the end
  of the line and not spoken again as output, and a line changed some
  other way (a password prompt's asterisks) drops the held typing
  unspoken, so a tab completion's added text is read and a password is
  never spoken. A line of one or two characters is output like any other
  (`*_short_output`).
- A line rewritten in place echoes the typing only when what it gained
  is exactly the typing (a character typed in the middle of a command),
  so a clock ticking on a password prompt's line never speaks the
  password, where NVDA speaks its queued characters on any text change.
  At such a prompt Verbatim speaks the count ("1", "2", "3") and never
  the characters; NVDA speaks the characters and not the count. At a
  password prompt that shows asterisks Verbatim speaks the asterisks the
  console shows, where NVDA echoes each typed character.
- A typed character the terminal has not shown within 300 milliseconds
  of reaching the program is dropped unspoken, so a late echo over a
  slow connection is heard as output.
- An input line the shell reprints at the caret, below a completion list
  or below output that arrived while the user typed, is not spoken
  again; characters typed since are echoed as usual (rule 14).
- Typing the text cannot show (a space at the end of a line, which is
  padding until something follows it, or a character typed over inline
  prediction that showed it already) is echoed when the caret moves over
  it; typing that wrapped past the right margin is echoed, not spoken as
  a new line. Each key a script reads without showing it is not echoed,
  where NVDA echoes it ("space").
- Held typing is forgotten on Enter and Tab, and on Escape, Control+C,
  Control+D, and Control+Break. Escape forgetting held typing is
  Verbatim's addition (Dickson, 2026-10-09): NVDA speaks characters typed
  at a prompt that showed nothing when Escape then changes the line,
  before the line's new text; Verbatim speaks only the new text.
- Escape and other keys outside the caret keys are spoken by their
  evidence (`docs/parity/text.md`, "Keys outside the caret table").
- Matched: Enter in a terminal is answered only by new output, never by
  the caret's line (rule 19). NVDA speaks the caret's new line after
  Enter in an edit field by binding Enter to its new-line script, but
  only where the object announces new lines, and every terminal turns
  that off: its live-text behaviour, which the console's and Windows
  Terminal's objects take, and Windows Terminal's notification overlay
  both set `announceNewLineText` to false at the pinned source. Enter is
  then passed to the program with no script, and only what NVDA's change
  monitor finds inserted is spoken. So a read between Windows Terminal's
  two writes of `cls` from Windows PowerShell, which homes the caret onto
  the screen's first row before it erases, says nothing of that row
  (Dickson, 2026-10-25), and the prompt the erase then writes on that row
  is said, as `cls` says the prompt: a line that only lost text while the
  caret moved forward over text both reads show unchanged is not taken
  for the caret's line (Dickson, 2026-10-25). With no read between the
  homing and the erase, `cls` typed on a prompt on the screen's first row
  reads as Control+W erasing a word and says nothing (a limit of
  `docs/design/terminal-model.md`).
- Up Arrow and a character typed at once: NVDA says the character, "x",
  as it is typed and then the recalled line with it, "echo onex";
  Verbatim reads the line's change once, both in it, and says "echo
  onex" without the character's own echo (Dickson, 2026-10-09). When a
  read shows the recalled line before the "x", as at human speed, the
  line is spoken and the "x" echoed (Dickson, 2026-10-26; `*_up_typing`).
- The console host's keys are translated with the console host's own
  keyboard layout, its thread learned from the console window's
  `WinEvents`, as NVDA does.
- NVDA's delayed character description ("Oscar" after "o") is a setting
  Verbatim does not have yet (the settings package).

### The alternate screen

Verbatim: **different**.

- Opening the alternate screen, Verbatim speaks all thirty rows in both
  terminals (within the flood limit), where NVDA speaks all thirty in
  Windows Terminal but only the last six in the console host; a row
  changed says "changed" in both, and the rows scrolled down and up say
  "row 0" and "row 30" in both (`*_full_screen`).
- Closing the alternate screen, Verbatim speaks the shell's screen as it
  comes back, at most a screen, through the flood policy, as NVDA speaks
  the shell's screen when a pager closes (`nv-con-pager.txt`; Dickson,
  2026-10-24). Over a long history it tells the screen coming back from a
  flood by the shell's screen it set aside when the program opened, so it
  says no skip there either (Dickson, 2026-10-25; `*_pager_history`; rule
  8). A program already open when Verbatim first reads the terminal has
  no such screen set aside, so its close over a long history says the
  first 30 lines, "skipped more than N lines" and the last 30, a limit of
  the design (Dickson, 2026-10-25).
- The alternate screen's text scrolls through rows that stay put, so
  Verbatim finds how far it scrolled from the text.

### The review cursor while output is written

NVDA's review cursor follows the caret to the prompt when a script
prints lines below a line being reviewed. Verbatim: **different**
(Dickson, 2026-10-23; rule 24 and the model's "The review cursor"):
with "review follows caret" on, the review cursor follows the caret only
when a key moved the caret or the focus arrived, and otherwise stays on
its text, moving with it as output scrolls; with it off, only the user
moves it. So reviewing a line above the caret and letting a script print
two lines leaves Verbatim's review cursor on the line reviewed. The
`*_review_output` scenarios still assert NVDA's behaviour; they are among
the terminal scenarios `docs/roadmap.md`, "The terminal redesign",
brings back to passing.

### Windows and tabs

Verbatim: **matched (verified)**.

- Switching to another window, as clicking its taskbar button does,
  says the window's title and "window", then the terminal and the
  caret's line: "<title> terminal" in Windows Terminal, whose text area
  is named for its tab, and "terminal" in the console host, whose text
  area's name both drop (NVDA says the terminal and its line as one
  utterance, Verbatim as two). Returning to a tab or window, Verbatim
  says the prompt. Typing and its output after a switch are echoed and
  spoken as in any terminal. NVDA leaves the window's title out of a
  console's terminal, as Verbatim does.
- A new terminal window or tab says "blank" for the empty line its caret
  is on as it takes the focus, in Windows Terminal as in the console
  host, as an empty line says everywhere else; a second tab opened from
  the command line says "<title> terminal", "blank", and then the prompt
  as output. Different (Dickson, 2026-10-09): NVDA reads a focused
  terminal's caret line in the console host, but in Windows Terminal says
  no line for a window or tab with nothing written yet, not recognising
  the terminal's focus, a bug not copied. `windows_terminal_two_windows`,
  `windows_terminal_tabs` and `windows_terminal_close_tab` assert "blank"
  for the second window and the second tab, and every Windows Terminal
  scenario for its first window. The two-window scenarios switch windows
  by bringing them to the front.
- A second window opened in the running Windows Terminal has its
  terminal's focus raised under more than one runtime id in some
  launches, and both readers announce each repeat. Verbatim reads the
  caret's line with each, so a repeat says "<title> terminal" and
  "blank", or the terminal alone for an identity whose element the
  outpost cannot find, before or after the announcement heard in full.
  NVDA, in its compare runs of 2026-10-21, said "<title> window",
  "<title> terminal" twice, and "<title> terminal blank", the line only
  with the identity whose text it could read
  (`target/e2e-evidence/m42-desktop-2-2026-10-22/`,
  `part2c-windows_terminal_two_windows-run1.txt`,
  `part2c-windows_terminal_leave_flood-run1.txt` and `-run6.txt`, step
  5); in its other runs, a single "<title> terminal", in two of them
  with "blank". `windows_terminal_two_windows` and
  `windows_terminal_leave_flood` accept the repeats as optional
  utterances (`docs/testing.md`, "Optional utterances") and require the
  window, the terminal heard in full with its line, and then the prompt.
  Telling the identities apart is left to Windows Terminal's app module
  (`docs/roadmap.md`, M5).

### Switching tabs

Windows Terminal gives the tab the keyboard focus only until it moves it
to the tab's terminal, a few milliseconds later, and on Control+Tab
gives it first to the tab it leaves. NVDA judges a UIA focus event by
the sender's keyboard focus on its UIA flusher thread a few milliseconds
after the event, queues every UIA focus event it accepts, and culls a
focus's speech once the focus has moved on. With Control held through
the tab switcher, as a person switches, NVDA says "Tab switcher
grouping", "list", and the tab moved to ("<title> 1 of 2") in full, and
on releasing Control the tab's terminal and its line.

Verbatim: **matched (verified)** for Control+Tab and Control+Shift+Tab
(`docs/design/focus-pipeline.md`, section 10.1, items 2 and 3). The
focus listener judges and reads each focus from the event's own element
on its thread for the application, a few milliseconds after the event,
and the outpost reports every focus it read, in the order the events
arrived; Core culls the speech of a focus no longer current. The tab's
ancestors are its own element's, the terminal's container above its
list, so Verbatim says "list" and the tab, as NVDA does, and the tab
left is announced and then culled, as NVDA queues it. Either key says
"list", the tab or tabs (culled), "<title> terminal", and the line.
`windows_terminal_tabs` switches with Control held
(`Scenario::hold_key`) and expects exactly NVDA's speech above.

### Line keys and lists a program draws

NVDA reads the caret's line at the first change of the caret's position
after the key, or, finding none within its wait, at the end of it,
without using a terminal's caret events; what the program redraws
elsewhere is new output to its diff, and it drops a marker's change of
one or two characters as probably typed.

Verbatim: **matched (verified)** for Up and Down Arrow in a terminal, a
selection list a program draws among them (Dickson, 2026-10-09). Verbatim
credits a key with the changes found by reads that begin within 300
milliseconds after the key reached the program; later changes are
output (rule 18). It answers the key with the caret's line, or with what
the line gained or lost. A row away from the caret that gained text or
emphasis and that the keys own (Dickson, 2026-10-26) is spoken whole at
once; every other row the keys changed is spoken once, from the word
that changed, when no key has been pressed for the quiet period (rule
21). So fzf says the current entry after each typed character and its
count when typing pauses. Colours are read with the reads in each key's
window, in the same remote operation as the text, and never before a
key, so a list shown only by colour says the row that gained the
highlight (rule 23). When the caret follows a list's ">" marker, both
say "> banana", "> cherry" and "> banana" (NVDA "greater banana") in
both terminals (`windows_terminal_marker_list`, `conhost_marker_list`).
With programs that write each move in one go (the pager of `git log`,
Git's `less.exe`, `vim --clean` with and without `:set relativenumber`,
tig, Microsoft Edit's file list, PSReadLine's MenuComplete and nano's
file browser), both say the caret's line or the new line.

Different (Dickson, 2026-10-23):

- When the program redraws the list and leaves the caret below it,
  Verbatim says the line that gained the marker, whole, as the key's
  answer, where NVDA says "blank" for each key and drops the marker's
  change as probably typed (`windows_terminal_redrawn_list`,
  `conhost_redrawn_list`).
- A pager moving a line on its alternate screen (`windows_terminal_pager`,
  `conhost_pager`): Verbatim says the line the move brought onto the
  screen alone, where NVDA says the prompt, ":", at the end of its wait
  and then that line.
- For a key that leaves the caret where it was, Verbatim says the row
  that gained the marker or the highlight (tig's and Microsoft Edit's
  lists, PSReadLine's menu after its first move) and nothing when no row
  gained one (the pagers' ":" prompt), where NVDA says the caret's line
  at the end of its wait.
- With `relativenumber` every visible line changes on each move, which
  NVDA speaks as a run of numbers and Verbatim as the changed lines from
  their numbers once the keys pause, within its limits.
- A program that redraws a little at a time is read part-way by both,
  differently (the pagers' prompt in the console host, the prompt of
  `gh repo create`, nano's file browser).

### Reading a terminal on demand

NVDA: "report status bar" (NVDA+End, NVDA+Shift+End in the laptop
layout) reads on the first press, spells on the second and copies on
the third (`globalCommands.py`, `script_reportStatusLine` and
`_getStatusBarText`). It looks in three places in turn:

- A status bar object: the application module's `statusBar`, or else
  the object at the foreground window's bottom-left corner, or the
  nearest ancestor of it with the status bar role (`api.getStatusBar`).
  Its text is its name and its children's names and values, joined by
  spaces (`api.getStatusBarText`).
- With none, the application module's `statusBarTextInfo`.
- Where the module does not implement that, the foreground object's
  `flatReviewPosition`, expanded to the whole story, collapsed to its
  end (`collapse(True)`) and expanded to a line: the story's last line.

Only when all three find nothing does it say "No status line found". It
has no command to move between commands entered.

What the pinned source shows for the two terminals, uncaptured:

- No application module in the pinned source covers the console host or
  Windows Terminal, so neither has a `statusBar` or `statusBarTextInfo`
  of its own, and both reach the defaults.
- Whether the object at the window's bottom-left corner has a status
  bar ancestor depends on the live tree and the window's frame, which
  the source cannot show; neither terminal's own tree is known to hold
  a status bar.
- `flatReviewPosition` (`NVDAObjects/__init__.py`) starts at the
  foreground object's `simpleParent`, not at the object itself, and
  walks up its content ancestors, answering only from one that is the
  root of a tree interceptor or whose `TextInfo` is a display-model
  one. For a top-level window that parent is the desktop, which
  `NVDAObjects.window.Window` keeps as a plain window (its
  `getPossibleAPIClasses` yields no API class for the class "#32769",
  and the `Desktop` overlay sets no `TextInfo`, leaving the base
  `NVDAObjectTextInfo`), and the desktop has no parent. So by the
  source the fallback finds nothing in either terminal, and NVDA says
  "No status line found" there unless the corner lookup finds a status
  bar.
- The display-model classes (`GenericWindow`, `WindowRoot` and
  `ContentGenericClient` in `NVDAObjects/IAccessible`) would apply to a
  window object of MSAA's, but the walk examines only the foreground
  object's ancestors. Were the foreground object NVDA records for a
  terminal below its top-level window, the walk would pass that window,
  and its display-model text, the text drawn in it as NVDA's display
  model captured it, would be read; the source cannot tell which object
  is the foreground, nor what the display model captures of either
  terminal's rendering.

Verbatim: **different** (Dickson, 2026-10-23 and 2026-10-26;
`docs/design/terminal-model.md`, "Features and settings"), Verbatim's
own commands for a terminal, so no capture is taken:

- "Report status bar", on NVDA's gestures (NVDA+End in the desktop
  layout, NVDA+Shift+End in the laptop layout), reads the footer of a
  terminal as it is now, top to bottom: the lines below the caret's
  line, or else the rows at the screen's bottom a program has rewritten,
  or else the last line with text, so a row that never stops changing
  can be heard; a second press spells it and a third copies it, as
  NVDA's command does with a status bar, and a screen with no text says
  "no status bar information". This is Verbatim's own: by the pinned
  source NVDA finds no status line in either terminal (above), which no
  capture has confirmed. In vim and less the same rule finds their last
  rows, which are a footer as worded. Outside a terminal Verbatim says
  NVDA's "No status line found" until the general status bar of M5.
- Verbatim+Alt+Up Arrow and Verbatim+Alt+Down Arrow move the review
  cursor to the previous and next command the user entered and speak
  its line, as a line move does, from marks Verbatim sets when Enter is
  pressed on a line a key changed; a mark whose line is gone is dropped
  and the next one tried, nothing is said past the oldest or newest,
  commands entered before Verbatim watched the terminal have none, and
  no prompt is guessed. NVDA has no such command, and uses NVDA+Alt+Up
  and Down Arrow to pan its magnifier.
