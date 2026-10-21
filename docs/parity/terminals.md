# Terminals

How new output in a terminal is read.

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

Verbatim: **different** (`docs/design/phase6-design.md`, "Terminal
reading by diffing the screen"; `docs/design/terminal-reading.md`;
`docs/crates/verbatim-outpost.md`, "Terminals"), case by case below,
some matched and some deliberately different. The terminal scenarios
pin it: the `conhost_` and `windows_terminal_` scenarios named below,
each run in both terminals.

#### Reading output

- Each read diffs the screen with the screen last read, where NVDA
  diffs the whole document. As NVDA's diff speaks only what was
  inserted, lines only deleted, and a line that only lost text or gained
  white space, say nothing.
- In the console host, the console's own update `WinEvents` are taken
  as text changes as well as UIA's, merged per window, and a change
  observed before the last read began is covered by it, since UIA's text
  changes stopped reaching the outpost in the middle of large writes in
  2 of 14 runs (different; `docs/design/phase6-design.md`).
- Windows Terminal's notifications from a terminal, and the console
  host's from its text area, are all ignored, whatever their activity
  id, so nothing is spoken twice, as NVDA's overlays block them.
- A redraw with the same text speaks nothing; blank lines are not spoken
  but every line counts, blank ones included (Dickson, 2026-10-07): rows
  a program passed over without writing (a cleared screen's rows above a
  footer drawn on its last row) are not lines, being blank before as
  they are now, and a line that wrapped is one line however many rows it
  took.
- Newer output never cancels older; a key, or anything else that cuts
  speech off, drops output still waiting, as in NVDA, and output read
  before the key is never spoken after it, while output read after the
  key is spoken, as in NVDA (Dickson, 2026-10-10; the `*_control_flood`
  scenarios).
- A line the terminal rewrites in place speaks from the start of the
  word where it first differs (Unicode's word rules with ICU's
  dictionaries, whole graphemes), where NVDA's character diff speaks the
  inserted characters; a single symbol replaced in place (a spinner) is
  silent until the line changes to text (Dickson, 2026-10-07); and a
  rewrite while the earlier version is still waiting replaces it
  (different). So Verbatim speaks a rewritten status line from the word
  that changed (vim's ruler, "2,1 Top"; tig's commit count), where NVDA
  drops a change of one or two characters on one line as probably typed.
- Verbatim+5 toggles "Report new output", on NVDA's key for its "report
  dynamic content changes" toggle, saying "report new output on" and
  "off".
- Commands, their output, the prompt, and the review cursor over the
  output read the same in both screen readers, in both terminals, apart
  from symbol names in flowing text ("ready greater" for the prompt
  `ready>`, Verbatim "ready>"), which wait for symbol processing (M8).
  A line of forty words is spoken whole, and a line growing by ten
  words a key speaks each key's words, as in NVDA (`*_long_lines`).

#### The backlog

- The backlog is capped ("30 and 30", `docs/design/phase6-design.md`,
  "The flood policy, reconsidered"; different): with more than "Lines
  spoken in full" (30) lines waiting once a group's last line has been
  heard, the older ones become "skipped N lines" ("skipped lines" when
  the count is lost, "skipped more than N lines" when the terminal's
  history overflowed past the screen last read, N the history's rows
  left unspoken) and the newest "Last lines to speak" (30) are kept;
  NVDA speaks them all. Nothing is lost for good: every line is still
  there for the review cursor (the `*_flood` and `*_scrollback_overflow`
  scenarios).
- The terminal is read on demand while Core's queue is full: the
  outpost only notes changes until Core asks, when a group's last line
  is handed to speech, where NVDA's monitor thread diffs on every
  change.
- A footer below a scroll region that a flood scrolls above, redrawn
  unchanged during the flood and changed after it: both say the footer,
  "status: busy", when it is drawn; Verbatim applies the flood policy
  (lines 1 to 30, "skipped 41 lines", lines 72 to 100) and says the
  footer only as it changed, "done" (different: the flood policy;
  `*_footer_flood`).

#### Typing

- Every character typed is echoed as typed; a space typed at a line's
  end is echoed at once ("space"); characters typed past the right
  margin are echoed one by one; a character typed in the middle of a
  command after `cls` is echoed; Left Arrow says the character ("o");
  `cls` says the prompt; typing over inline prediction echoes each
  character. All as NVDA (`*_typing`).
- In place of NVDA's rule dropping a change of one or two characters,
  the typing Core holds is echoed when the terminal shows it at the end
  of the line and not spoken again as output, and a line changed some
  other way (a password prompt's asterisks) drops the held typing
  unspoken, so a tab completion's added text is read and a password is
  never spoken (different). A line of one or two characters is output
  like any other (`*_short_output`).
- A line rewritten in place echoes the typing only when what it gained
  is exactly the typing (a character typed in the middle of a command),
  so a clock ticking on a password prompt's line never speaks the
  password (Dickson, 2026-10-08), where NVDA speaks its queued characters
  on any text change. At such a prompt Verbatim speaks the count ("1",
  "2", "3") and never the characters; NVDA speaks the characters and
  not the count. At a password prompt that shows
  asterisks Verbatim speaks the asterisks the console shows, where NVDA
  echoes each typed character.
- Typing the text cannot show (a space at the end of a line, which is
  padding until something follows it, or a character typed over inline
  prediction that showed it already) is echoed when the caret moves over
  it; typing that wrapped past the right margin is echoed, not spoken as
  a new line. Each key a script reads without showing it is not echoed,
  where NVDA echoes it ("space").
- Held typing is forgotten on Enter and Tab, and on Escape, Control+C,
  Control+D, and Control+Break; NVDA forgets its queued characters on
  Enter, Tab, Control+C, Control+D, and Control+Pause. Escape
  forgetting held typing is Verbatim's addition (Dickson, 2026-10-09):
  NVDA speaks characters typed at a prompt that showed nothing when
  Escape then changes the line, before the line's new text; Verbatim
  speaks only the new text.
- Escape and other keys outside the caret keys are spoken by their
  evidence (`docs/parity/text.md`, "TextInfo-equivalent layer").
- Up Arrow and a character typed at once (`*_up_typing`): NVDA says the
  character, "x", as it is typed and then the recalled line with it,
  "echo onex"; Verbatim reads the line's change once, both in it, and
  says "echo onex" without the character's own echo (Dickson,
  2026-10-09; `docs/design/phase6-design.md`).
- The console host's keys are translated with the console host's own
  keyboard layout, its thread learned from the console window's
  `WinEvents`, as NVDA does.
- NVDA's delayed character description ("Oscar" after "o") is a
  setting Verbatim does not have yet (the settings package).

#### The alternate screen

- Opening the alternate screen, Verbatim speaks all thirty rows in both
  terminals (within the flood limit), where NVDA speaks all thirty in
  Windows Terminal but only the last six in the console host; a row
  changed says "changed" in both, and the rows scrolled down and up say
  "row 0" and "row 30" in both (`*_full_screen`).
- Closing the alternate screen, Verbatim speaks only "closed" and the
  prompt, the main screen being as it was, where NVDA speaks the whole
  main screen again (Dickson, 2026-10-09).
- The alternate screen's text scrolls through rows that stay put, so
  Verbatim finds how far it scrolled from the text.

#### The review cursor while output is written

Reviewing a line above the caret, then letting a script print two
lines: both screen readers follow the caret to the prompt, so the
current line is the prompt and the line before it the new output's
last (matched; `*_review_output`).

#### Windows and tabs

- Switching to another window, as clicking its taskbar button does,
  says the window's title and "window", then the terminal and the
  caret's line: "<title> terminal" in Windows Terminal, whose text area
  is named for its tab, and "terminal" in the console host, whose text
  area's name both drop (matched; NVDA says the terminal and its line as
  one utterance, Verbatim as two). Returning to a tab or window,
  Verbatim says the prompt. Typing and its output after a switch are
  echoed and spoken as in any terminal (matched). NVDA leaves the
  window's title out of a console's terminal, as Verbatim does.
- A new terminal window or tab says "blank" for the empty line its
  caret is on as it takes the focus, in Windows Terminal as in the
  console host, as an empty line says everywhere else; a second tab
  opened from the command line says "<title> terminal", "blank", and
  then the prompt as output. NVDA reads a focused terminal's caret line
  in the console host, but in Windows Terminal says no line for a window
  or tab with nothing written yet, not recognising the terminal's focus,
  a bug not copied (Dickson, 2026-10-09). `windows_terminal_two_windows`,
  `windows_terminal_tabs` and `windows_terminal_close_tab` assert
  "blank" for the second window and the second tab, and every Windows
  Terminal scenario for its first window. The two-window scenarios
  switch windows by bringing them to the front.
- A second window opened in the running Windows Terminal has its
  terminal's focus raised under more than one runtime id in some
  launches, and both readers announce each repeat (matched). Verbatim
  reads the caret's line with each, so a repeat says "<title> terminal"
  and "blank", or the terminal alone for an identity whose element the
  outpost cannot find, before or after the announcement heard in full.
  NVDA, in its compare runs of 2026-10-21, said "<title> window",
  "<title> terminal" twice, and "<title> terminal blank", the line only
  with the identity whose text it could read
  (`target/e2e-evidence/m42-desktop-2-2026-10-22/`,
  `part2c-windows_terminal_two_windows-run1.txt`,
  `part2c-windows_terminal_leave_flood-run1.txt` and `-run6.txt`, step
  5); in its other runs, a single "<title> terminal", in two of them
  with "blank".
  `windows_terminal_two_windows` and `windows_terminal_leave_flood`
  accept the repeats as optional utterances (`docs/testing.md`,
  "Optional utterances") and require the window, the terminal heard in
  full with its line, and then the prompt. Telling the identities apart
  is left to Windows Terminal's app module (`docs/roadmap.md`, M5).
- Switching tabs with Control+Tab or Control+Shift+Tab (matched;
  `docs/design/focus-pipeline.md`, section 10.1, items 2 and 3).
  Windows Terminal gives the tab the keyboard focus only until it moves
  it to the tab's terminal, a few milliseconds later, and on Control+Tab
  gives it first to the tab it leaves. NVDA judges a UIA focus event by
  the sender's keyboard focus on its UIA flusher thread a few
  milliseconds after the event, queues every UIA focus event it
  accepts, and culls a focus's speech once the focus has moved on.
  Verbatim's focus listener does the same: it judges and reads each
  focus from the event's own element on its thread for the application,
  a few milliseconds after the event, and the outpost reports every
  focus it read, in the order the events arrived; Core culls the speech
  of a focus no longer current. The tab's ancestors are its own
  element's, the terminal's container above its list, so Verbatim says
  "list" and the tab, as NVDA does, and the tab left is announced and
  then culled, as NVDA queues it. Either key says "list", the tab or
  tabs (culled), "<title> terminal", and the line. With Control held
  through the tab switcher, as a person switches, NVDA says "Tab
  switcher grouping", "list", and the tab moved to ("<title> 1 of 2") in
  full, and on releasing Control the tab's terminal and its line;
  `windows_terminal_tabs` switches that way (`Scenario::hold_key`) and
  expects exactly that speech.

#### Line keys and lists a program draws

- Up and Down Arrow in a terminal, a selection list a program draws
  among them, are matched (Dickson, 2026-10-09,
  `docs/design/phase6-design.md`, "Terminal line keys as NVDA has
  them"). NVDA reads the caret's line at the first change of the
  caret's position after the key, or, finding none within its wait, at
  the end of it, without using a terminal's caret events; what the
  program redraws elsewhere is new output to its diff. Verbatim answers
  the key with the caret's line once the caret has moved, as in a text
  field, and speaks what the program redrew elsewhere at once as
  output, through the screen diff; a key whose answer said what it did
  to the caret's line keeps that line's change from being spoken again
  as output, and until the answer comes the change is output like any
  other.
- When the caret follows a list's ">" marker, both say "> banana",
  "> cherry" and "> banana" (NVDA "greater banana") in both terminals
  (`windows_terminal_marker_list`, `conhost_marker_list`).
- When the program redraws the list and leaves the caret below it,
  Verbatim says the line that gained the marker as output, from the
  word that changed, and nothing for the key, which did not move the
  caret, where NVDA says "blank" for each key and drops the marker's
  change as probably typed (`windows_terminal_redrawn_list`,
  `conhost_redrawn_list`; different, following from the silent key in
  `docs/parity/text.md`, "TextInfo-equivalent layer", and the rewritten
  line above).
- A program that redraws a little at a time is read part-way by both,
  differently (the pagers' prompt in the console host, the prompt of
  `gh repo create`, nano's file browser).
- A pager moving a line on its alternate screen (`windows_terminal_pager`,
  `conhost_pager`): Verbatim says the line the move brought onto the
  screen alone, where NVDA says the prompt, ":", at the end of its wait
  and then that line.
- With programs that write each move in one go (the pager of `git log`,
  Git's `less.exe`, `vim --clean` with and without `:set
  relativenumber`, tig, Microsoft Edit's file list, PSReadLine's
  MenuComplete and nano's file browser), both say the caret's line or
  the new line as above. The differences left: Verbatim says nothing
  for a key that leaves the caret where it was (the pagers' ":" prompt,
  tig's and Microsoft Edit's lists, PSReadLine's menu after its first
  move), where NVDA says the caret's line at the end of its wait; and
  with `relativenumber` every visible line changes on each move, which
  NVDA speaks as a run of numbers and Verbatim as the changed lines from
  their numbers, both within their limits.
