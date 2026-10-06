# Videos

Demonstrations of Verbatim, each a recording of one end-to-end scenario
with Verbatim's own speech. The videos are stored with Git LFS
(`.gitattributes`), so a clone needs `git lfs install` once to get them
rather than pointers to them.

## Making one

Videos are added to this folder only when someone asks for one. Every
end-to-end run, CI's included, records each scenario to
`target/e2e-artifacts`, but nothing copies those here. On a Windows machine with ffmpeg on `PATH` and an
unlocked desktop:

`cargo xtask demo <scenario> [--name <name>]`

This builds and starts an agent of its own, runs the scenario through
the end-to-end harness while recording it, and saves the result as
`videos/<name>.mp4`. The name defaults to the scenario's, with hyphens
for underscores; `cargo xtask demo` with no arguments lists the
scenarios, and then the demonstrations: scenarios of something only some
machines have, which the suite leaves out so that it runs the same
everywhere, such as `notepad_spelling_errors`, Windows 11 Notepad's own
spell checker. A scenario that fails leaves this folder unchanged. Like any
local end-to-end run, it takes over the desktop while it runs
(`docs/tooling.md`).

A demo records at 30 frames a second, losslessly while the scenario
runs, and then encodes with a slow preset, so the encoding never slows
Verbatim down. The audio is exactly what Verbatim played, recorded by
Verbatim itself, so nothing else the machine plays ends up in it, and a
run is recorded the same way whether or not the computer has a sound
card.

## The videos

A line for each video: its file name, the scenario it records, and what
it demonstrates.

Each records an end-to-end scenario with every other window minimized
first, so only the scenario's own windows appear.

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