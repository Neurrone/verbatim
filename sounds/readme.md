# Sounds

The sounds of Verbatim's built-in default theme, shared by every platform
and installed beside the program (`phase6-design.md`, "Earcons"). The
platform-neutral code only loads files from this directory, so every
build uses the same sounds. A theme names a sound by its file name; a
user theme that does not carry a sound of that name uses the one here.

## Origin and licence

Every file here is copied unchanged from NVDA's `source/waves` directory
(the `nvda` submodule, at the commit the submodule records). NVDA is
distributed under the GNU General Public License version 2 or later
(`nvda/copying.txt`), and these sounds are distributed with it under that
licence; NVDA's repository states no separate licence for them. Verbatim
is licensed under the GNU General Public License version 3 or later, which
the "or later" of NVDA's licence allows.

The files, and what the default theme plays them for:

- `browseMode.wav`: browse mode turned on.
- `focusMode.wav`: focus mode turned on.
- `error.wav`: an error was logged.
- `start.wav`: Verbatim started.
- `exit.wav`: Verbatim is exiting.
- `suggestionsOpened.wav`: suggestions appeared for the focused field.
- `suggestionsClosed.wav`: the suggestions went away.
- `textError.wav`: a spelling error in text being read.

Verbatim's own cues (an application not responding, skipped terminal
lines, progress bars, and the capital letter tone) are tones generated in
code, not files.
