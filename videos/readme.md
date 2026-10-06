# Videos

Recordings of Verbatim, each of one end-to-end scenario, with Verbatim's
own speech. They are in two folders:

- `demos` holds demonstrations: scenarios written to show a feature to a
  viewer, at a viewer's pace.
- `tests` holds recordings of the end-to-end suite's own test scenarios.

The videos are stored with Git LFS (`.gitattributes`), so a clone needs
`git lfs install` once to get them rather than pointers to them.

## Making one

Videos are added only when someone asks for one. Every end-to-end run,
CI's included, records each test scenario to `target/e2e-artifacts`, but
nothing copies those here. On a Windows machine with ffmpeg on `PATH` and
an unlocked desktop:

`cargo xtask demo <scenario> [--name <name>]`

This builds and starts an agent of its own, runs the scenario through the
end-to-end harness while recording it, and saves the result as
`videos/tests/<name>.mp4` for a test scenario, or `videos/demos/<name>.mp4`
for a demonstration. The name defaults to the scenario's, with hyphens for
underscores, and without a demonstration's `demo_` prefix;
`cargo xtask demo` with no arguments lists the demonstrations and the test
scenarios. A scenario that fails leaves both folders unchanged. Like any
local end-to-end run, it takes over the desktop while it runs
(`docs/tooling.md`).

Demonstrations are the registry's `demo` group
(`crates/verbatim-e2e/src/registry.rs`). They are never part of the
suite: their tests are ignored, so neither CI nor a local
`cargo test -p verbatim-e2e` runs them, and `cargo xtask vm test` refuses
them. They run with the same fixed settings and speech rate as the suite,
and like the tests, they wait for evidence rather than for a fixed time,
and assert what they show, so a broken feature fails rather than
recording a misleading video. Unlike the tests, they let each announcement
be heard in full before the next action: typing, for example, is one
character at a time, each echo heard out.

A video records at 30 frames a second, losslessly while the scenario
runs, and then encodes with a slow preset, so the encoding never slows
Verbatim down. The audio is exactly what Verbatim played, recorded by
Verbatim itself, so nothing else the machine plays ends up in it, and a
run is recorded the same way whether or not the computer has a sound
card. Every other window is minimized first, so only the scenario's own
windows appear.

## Demonstrations: the demos folder

Each entry gives the video's file name and the scenario it records, then
what a viewer sees and hears, step by step.

### notepad-editing.mp4, from demo_notepad_editing

Editing a short paragraph in Windows 11 Notepad.

1. Notepad opens on a two-line paragraph, and Verbatim announces the
   window, then the text area and the line the caret is on.
2. Control+Home moves the caret to the top, and Verbatim reads the first
   line, "Verbatim reads this short note". Right Arrow twice speaks "e"
   and "r", the characters the caret reaches; Control+Right Arrow twice
   speaks the words "reads" and "this"; Down Arrow and Up Arrow speak the
   second line and the first again.
3. Home speaks "V". Shift+Control+Right Arrow selects the first word, and
   Verbatim says "Verbatim selected", then the second, "reads selected";
   Shift+Control+Left Arrow takes the second word out again, "reads
   unselected"; and Shift+End extends the selection to the end of the line,
   "reads this short note selected".
4. Control+End moves to the empty last line ("blank"), and the sentence
   "Typing is echoed." is typed. Each character is spoken as it is typed,
   the spaces as "space". It is first typed with a typo, "echoef":
   Backspace speaks the "f" it deletes, and "d." finishes the sentence,
   the full stop spoken as "dot".
5. Verbatim+3 turns on typed-word echo: "speak typed words only in edit
   controls". The sentence "So are words." is typed after it. Each
   character is still spoken, and each word, once finished, is spoken
   before the space or full stop that ends it: "S", "o", "So", "space",
   and so on, to "words" and "dot".

### review-cursor.mp4, from demo_review_cursor

The review cursor over a plain-text table in Notepad. The table has
three columns, Fruit, Color, and Price, and five rows; the Fig row has no
price, so it is shorter than the Price column.

1. Control+Home moves the caret to the top, and the review cursor follows
   it. Numpad 8 reads the header row; numpad 5 reads the current word,
   "Fruit", and numpad 6 the next words, "Color" and "Price".
2. Down the Price column: numpad 9 reads each next row, and numpad 2 the
   character in the column the review cursor keeps. On the Apple row it
   is "1", the start of the price. The Fig row is too short, so the
   review cursor is on its last character, "e". On the Banana row it is
   back in the Price column, "0", and on the Cherry row "3". Numpad 7 then
   reads back up the rows, Banana, Fig, and Apple, numpad 2 giving "0",
   "e", and "1" again: the column is kept in both directions.
3. On the Apple row, Shift+numpad 1 moves to the start of the line ("A")
   and numpad 5 reads the word "Apple". Numpad 5 pressed twice spells it,
   letter by letter. Numpad 2 pressed twice describes the character,
   "Alfa". Numpad 3 and numpad 1 move to the next character, "p", and
   back, "A", and numpad 2 pressed three times gives the character code,
   65, then in hexadecimal.
4. Verbatim+F9 marks the start of the Apple row ("Start marked"),
   Shift+numpad 3 moves to its last character ("0"), and Verbatim+F10
   pressed twice copies from the mark to the review cursor: "Copied to
   clipboard", then the row. Control+End moves to the empty last line,
   Control+V pastes the copy there, and Home and numpad 8 read the pasted
   line: the Apple row, as copied.

### say-all.mp4, from demo_say_all

Say-all, in Notepad and in a standard Win32 edit control.

1. Notepad opens on four paragraphs of prose about a lighthouse keeper.
   Control+Home moves the caret to the top, and Verbatim+Down Arrow (the
   desktop layout's say all) starts reading. Notepad's text has no
   sentence unit, so say-all reads it line by line, moving the caret as
   it goes. The first two paragraphs are read in full.
2. As the third paragraph, "Ships passing in the night", starts, Control
   stops speech. Home then speaks "S", and numpad 8 reads the line the
   caret is on, which starts the third paragraph: the caret stopped where
   speech did.
3. A window opens holding one large text box, a standard Win32 edit
   control. Windows PowerShell shows it with Windows Forms, since classic
   Notepad's edit control is not on Windows 11. Verbatim announces the
   text box, "Story", and its first line.
4. Verbatim+Down Arrow reads it from the top. Verbatim reads an edit
   control's text itself and can split it into sentences, so here say-all
   reads by sentence, the "Say all reads by" setting's default: "A letter
   arrived on Tuesday." is one piece, "It had no stamp and no return
   address." the next, and so on through the five sentences of two
   paragraphs, each heard on its own. Then the window closes.

### terminal-session.mp4, from demo_terminal_session

A session in Windows Terminal running Windows PowerShell, whose prompt is
set to "ready>". Every command is typed one character at a time, and
Verbatim echoes each character, punctuation by name, before the next is
typed.

1. The terminal opens, and Verbatim speaks the prompt as it appears;
   numpad 8 reads it again with the review cursor.
2. `Get-ChildItem -Name files` lists a folder of three files: "alpha.txt",
   "beta.txt", and "gamma.txt" are spoken as they are printed, then the
   prompt.
3. `.\table.ps1` prints a table of planets, their moons, and whether they
   have rings, and every row is spoken. Numpad 7 then reads back up the
   rows with the review cursor to the header row. Shift+numpad 1 moves to
   its start ("P") and numpad 6 to the next word, "Moons". Down the
   column, numpad 9 reads each row and numpad 5 the word in the Moons
   column: "0" for Mercury, "1" for Earth, "2" for Mars, "95" for Jupiter,
   and "146" for Saturn.
4. `echo helo` is typed with a typo. Backspace deletes the last "o", which
   is spoken, "lo" is typed, and Enter prints "hello", which is spoken,
   then the prompt.
5. `.\password.ps1` asks for a password. "secret" is typed at the
   "Password:" prompt and nothing of it is spoken, since "speak
   passwords" is off by default. The script then prints "done", and the
   prompt follows.
6. `.\flood.ps1` prints a thousand lines at once, "flood line 1" to
   "flood line 1000". Verbatim speaks the first lines, then "skipped" and
   how many lines it skipped, then the last lines, through "flood line
   1000", and the prompt.
7. Verbatim+5 turns output reporting off, "report new output off".
   `.\quiet.ps1` prints two lines; the typing is still echoed, but
   neither line nor the next prompt is spoken. Verbatim+5 turns reporting
   back on, "report new output on", and `echo back` prints "back", which
   is spoken, then the prompt.

### settings-dialog-keys.mp4, from demo_settings_dialog_keys

The keys of Verbatim's settings dialog.

1. Verbatim's menu opens with Verbatim+V, and Settings opens on the
   Speech page. Tab moves to the rate slider, its value spoken, and Up
   Arrow lowers the rate by one. Tab moves on to Cancel, and Enter on it
   cancels: the dialog closes. Opened again, the rate slider has its old
   value.
2. Up Arrow lowers the rate again, Tab moves to Apply, and Enter on it
   applies the change and keeps the dialog open: Shift+Tab moves back to
   Cancel. Escape closes the dialog, and opened again, the slider has the
   applied value.
3. Up Arrow lowers the rate once more, and Control+S, pressed on the
   slider itself, saves. Escape closes the dialog, and opened again, the
   slider has the saved value.
4. Control+Tab, pressed on the slider inside the Speech page, moves to
   the next category: the category list takes the focus on Theme. Escape
   closes the dialog.

## Test recordings: the tests folder

A line for each video: its file name, the test scenario it records, and
what it shows.

- `tabbing-through-settings.mp4`, from `menu_and_settings_dialog`:
  Verbatim opens its own context menu and Settings dialog and tabs
  through every control of the Speech page (the synthesizer, voice,
  variant, and the rate, pitch, inflection, and volume sliders, with a
  voice and a rate changed and changed back), each announced in full in
  eSpeak NG's voice. It demonstrates eSpeak NG as the default
  synthesizer, Verbatim reading its own interface, and a recording
  carrying Verbatim's speech.
- `notepad-and-verbatim-menu.mp4`, from `notepad_and_verbatim_menu`:
  Notepad opens on a file and is announced, window then text area;
  Verbatim's own menu opens over it and closes, and Notepad is announced
  again. It demonstrates following the focus between applications.
- `object-navigation-in-settings.mp4`, from
  `object_navigation_in_settings`: the navigator object moves through the
  Settings dialog (to the parent, the next and previous objects, and the
  first child), each object announced. It demonstrates object
  navigation.
- `rapid-tabbing-in-settings.mp4`, from `rapid_tabbing_in_settings`:
  tabbing quickly through the Settings dialog, each announcement cut off
  by the next key press, and the last one heard in full. It demonstrates
  speech keeping up with the focus.
- `explorer-folder-window.mp4`, from `explorer_folder_window`: a File
  Explorer folder window opens and its title is announced, then the
  arrows move through a subfolder and three files, each with its
  position, and Enter and Backspace go into the subfolder and back. It
  demonstrates reading File Explorer.
- `settings-system-page.mp4`, from `settings_system_page`: the Settings
  app opens on its System page, and Tab and the arrows move through its
  list of settings, each announced with its position. It demonstrates
  reading the Settings app.
- `start-menu-search.mp4`, from `start_menu_search`: the Start menu opens
  and its search box is announced. It demonstrates reading the Windows
  shell.
- `switch-to-onecore.mp4`, from `switch_to_onecore`: the Select
  Synthesizer dialog switches from eSpeak NG to Windows OneCore voices,
  whose voice is then read, and back. It demonstrates switching
  synthesizer, and both synthesizers speaking.
- `synth-host-crash-recovery.mp4`, from `synth_host_crash_recovery`: the
  synthesizer's host process is killed, and the next announcement is
  still heard from a new one. It demonstrates recovering from a
  synthesizer crash.
- `theme-panel.mp4`, from `theme_panel`: the Settings dialog's Theme
  page is read control by control, the indications tree is walked
  through the first entry of each category, and the find field narrows
  it to the buttons. The button role is set to be reported by a sound,
  which makes a copy of the built-in theme, and the Preview button is
  then announced with the sound in place of its role. The indication is
  reset, and the copy removed. It demonstrates themes and the Theme
  page.
- `windows-terminal-commands.mp4`, from `windows_terminal_commands`: in
  Windows Terminal, the prompt is spoken as it appears and read with the
  review cursor, `echo hello` is echoed character by character and its
  output and the next prompt are spoken, and a password typed at a
  `Read-Host -AsSecureString` prompt is never spoken. It demonstrates
  reading a terminal's new output, typed-character echo there, and the
  password rule.
- `terminal-flood.mp4`, from `terminal_flood`: ten thousand lines printed
  into Windows Terminal as fast as the shell can, spoken as their first
  lines, how many were skipped, and the last ones, with Verbatim+5
  answered during a second flood and output reporting off for a third.
  It demonstrates the flood policy and staying responsive during one.
