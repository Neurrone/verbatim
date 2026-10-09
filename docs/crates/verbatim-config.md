# verbatim-config

Portable configuration next to `verbatim.exe`. `settings.toml` is the base
configuration — global settings plus the base profile's sections — and the
`profiles` folder holds named profiles as sparse overlays, mirroring NVDA's
base-plus-diffs model. Because the base lives in `settings.toml`, any file
name in `profiles` is a legitimate user profile. The `themes` folder holds
the user's themes, one directory each, and the `sounds` folder beside the
program holds the shared sounds the built-in default theme plays.

Public API:

- `Settings` — the `settings.toml` schema: `locale`, `log_filter`,
  `verbatim_keys`, `keyboard`, `uia` (all global; profiles cannot carry
  them), `speech` (the base profile's section), `reader` (milestone M4, the
  base profile's too until profiles grow it in M8), and `theme` (milestone
  M4, the base profile's theme, see "Themes" below).
- `ReaderSettings`, `TypingEcho`, `SayAllUnit` — re-exported from
  `verbatim-model`, which the reducer reads them from; the `[reader]`
  section, every key with NVDA's default:
  - `speak_typed_characters`: `"off"`, `"edit_controls"` (only in edit
    controls and other places text can be typed), the default, or
    `"always"`. NVDA's "Speak typed characters".
  - `speak_typed_words`: the same choices, `"off"` by default. NVDA's
    "Speak typed words".
  - `follow_caret`: the review cursor follows the caret, `true` by default
    ("caret moves review cursor", toggled with Verbatim+6).
  - `say_all_unit`: `"sentence"` (by sentence where the text can be split
    into sentences, by line otherwise; the default), `"paragraph"`, or
    `"line"`. NVDA's "Say all reads by".
  - `keep_display_on`: keep the display on while say-all reads, `true` by
    default. NVDA's "Prevent display from turning off during say all".
  - `speak_terminal_passwords`: echo characters typed into a terminal at
    once rather than when the terminal shows them, `false` by default.
    NVDA's "Speak passwords in all enhanced terminals".
  - `report_terminal_output`: speak new output in terminals, `true` by
    default ("Report new output", toggled with Verbatim+5).
  - `terminal_full_lines`: "Lines spoken in full", 30 by default: up to
    this many lines of terminal output waiting to be spoken are all
    spoken.
  - `terminal_last_lines`: "Last lines to speak", 30 by default: when more
    are waiting, the older ones become "skipped N lines" and this many of
    the newest are kept. Both limits are kept between 1 and 10,000
    (`MAX_TERMINAL_LINES`), more than either terminal's history holds, so
    "Lines spoken in full" may reach past it; Core's 10 MB bound on output
    waiting protects memory.

  These four and `speak_terminal_passwords` are the Terminal settings of
  `phase6-design.md` ("M4: text, editing, and terminals", Questions),
  set on the settings dialog's Terminal page and, for "Report new
  output", with Verbatim+5.
- `VerbatimKeys` — which keys act as the Verbatim modifier (`caps_lock`,
  `insert`, `numpad_insert`) plus `share_modifier`, which passes the
  modifier's own transitions down the hook chain for a screen reader
  running behind Verbatim.
- `KeyboardConfig`, `KeyboardLayout` — the active gesture-binding layout
  (`desktop`, the default, or `laptop`), M3's `keyboard` section, and since
  M4 NVDA's two speech interrupt settings, both `true` by default:
  `speech_interrupt_for_characters` (a typed character, or Shift, cuts
  speech off) and `speech_interrupt_for_enter` (Enter cuts speech off),
  which the app maps to `verbatim-input`'s `DecisionConfig`. Exposed
  only in `settings.toml`; a GUI choice arrives with M8's gesture-remapping
  work. `verbatim-input`'s `bindings_for` consumes the resolved layout
  through its own decoupled layout enum (the app maps one to the other),
  the same pattern `DecisionConfig` already follows for `VerbatimKeys`.
- `UiaConfig` — the `uia` section, developer settings for the UIA client:
  `remote_operations` (on by default) lets outposts read a UIA focus's
  ancestry with one remote operation inside the application's provider;
  `remote_operations = false` forces the classic walk, one round trip per
  ancestor, for diagnosing a provider and for before-and-after
  measurements. Read once at startup: `verbatim-app` hands it to the
  supervisor, which passes `--classic-uia` to each outpost when it is off.
- `SpeechConfig`, `ConfigValue` — active synthesizer plus per-synthesizer
  setting values, keyed by synth id then setting id so switching synths
  never loses the other synth's values.
- `ThemeConfig` and `ProfileTheme` — the `[theme]` section of the base
  settings (the theme's `id`, `default` for the built-in default theme,
  and the `ThemeOptions` flattened beside it) and of a profile (each
  optional, so a profile overrides only what it sets).
- `Profile` — one named profile: its `speech` and `theme` sections.
  Deliberately has no global fields, so a profile file cannot smuggle in
  modifier keys or a locale; unknown sections are ignored by the schema
  (tested).
- `ConfigStore` — `load(root)`, `settings()`, `settings_mut()`,
  `save_settings()`, `ensure_files_exist()`, `active()`, `themes_dir()`,
  and `sounds_dir()` (the `THEMES_DIR` and `SOUNDS_DIR` folders beside the
  executable).
- `ActiveConfig` — the resolved read-only view: `synthesizer()`,
  `synth_setting(synth, id)`, and `synth_settings(synth)` resolve each
  setting through the active overlays (most specific first) down to the
  base; `theme_id()` is the theme of the most specific layer that names
  one, else the base's, and `theme_options()` resolves the sound volume
  and the two checkboxes the same way, each on its own. M1 activates no
  overlays; the layering is implemented and tested.
- `themes` (a public module) — theme files, described under "Theme
  files" below: `LoadedTheme` (a theme, its directory, and its
  problems; `builtin()` and `sound_path`), `ThemeError`, `MANIFEST`,
  `parse_theme`, `theme_to_toml`, `is_valid_theme_id`, `sound_path`,
  `validate_theme`, `load_theme`, `find_theme`, `list_themes`,
  `save_theme`, `theme_id_for_name`, `new_theme`, `rename_theme`,
  `remove_theme`, `add_sound`, `import_theme`, and `export_theme`.

## Themes

What a user can set, as `settings.toml` and the themes folder hold it
(`phase6-design.md`, "Themes: one model for verbosity, speech, and
sounds"). A theme decides, for everything Verbatim can report (each an
indication: a role such as "link", a state such as "checked", a
description, a spelling error, browse mode switching on, and so on),
whether it is reported, and how: in words, by a sound, by both, or not at
all. It also holds the sounds, replacement words, and voice styles that
go with them. NVDA's separate verbosity checkboxes, such as reporting
object descriptions, are indications in the theme rather than settings
of their own.

The built-in default theme speaks everything as NVDA speaks it and plays
sounds where NVDA plays them by default, with NVDA's own sounds: the
browse and focus mode sounds, suggestions appearing and going away,
errors, start and exit, and the spelling error sound alongside the words
"spelling error". Verbatim adds tones of its own for an application that
is not responding, for terminal output skipped as too much to read, and
for progress bars. It cannot be changed; a changed version of it is a new
theme. Every other theme stores only where it differs from the default,
so anything it does not mention, including what later versions of
Verbatim add, is reported as the default theme reports it.

The `[theme]` section of `settings.toml`, each key optional:

- `id`: the theme in use, `default` for the built-in default theme or
  the id of a theme in the themes folder.
- `sound_volume`: the volume of sounds relative to speech, from 0 to 100,
  100 by default.
- `sounds_during_say_all`: whether sounds play while say-all reads,
  `true` by default.
- `speak_sounded_indications`: whether indications reported by a sound
  alone are spoken as well, for learning a theme's sounds, `false` by
  default.

A configuration profile may name a different theme, such as a
proofreading theme for a word processor, and change the three settings
beside it; a profile that names none uses the theme of `settings.toml`.
Profiles hold no per-indication settings: to present something
differently in one application, make a theme for it and choose it in that
application's profile.

Theme files. A theme is a directory in the `themes` folder named by its
id (lowercase letters, digits, hyphens, and underscores): a manifest,
`theme.toml`, and its sound files. The manifest holds `id`, `name`,
`author`, `description`, `version`, `gain` (percent, applied to all its
sounds), `[voice_styles.<name>]` tables (`pitch`, `rate`, and `volume`
changes from -100 to 100; only the pitch is applied so far), and an
`[indications.<id>]` table for each indication where it differs from the
default theme, with `report` (`off`, `speech`, `sound`, or
`speech-and-sound`), `sound` (a WAV file name, or a tone such as
`{ frequency = 880, duration = 40 }` in hertz and milliseconds), `gain`
(percent, 100 by default), `words` (spoken in place of the indication's
own words, or before them when it carries content of its own, such as a
description), and `voice` (a voice style's name). Indication ids are
listed in [verbatim-model](verbatim-model.md), "Themes". A sound file the
theme's directory does not have is looked for in the shared `sounds`
folder. A theme is shared as a package, its directory zipped.

What loading a theme finds wrong is listed rather than refused: an
indication this version does not know, an indication reported by sound
alone with no sound, a sound file that is missing or cannot be decoded,
an unknown voice style, a gain above 400 percent. An indication whose
sound is unavailable is spoken instead, so nothing is lost by accident.

Theme files, implementation notes (`themes.rs` and `package.rs`). The
manifest is parsed with indications keyed by id, so an unknown id is a
problem rather than a parse error; a theme's id is its directory's name,
whatever its manifest says. `save_theme` writes the manifest atomically
and refuses the built-in default theme, which has no directory.
`new_theme` copies a theme's indications, voice styles, gain,
description, and the sound files in its directory under an id made from
the new name (numbered when taken); based on the default theme, it starts
with no differences. `rename_theme` changes only the name, so every
reference by id stays. `add_sound` copies a WAV file into a theme under
its own name, numbered when the theme has a different file by that name.
`export_theme` writes a zip of the manifest and the sound files in the
theme's directory that it uses, deflated, through a temporary file
renamed into place. `import_theme` reads a package whose manifest is at
its top level or in its single top-level folder, takes only the manifest
and WAV files with plain names there (at most 256 files, 16 MB each, and
64 MB in all), refuses an id that is invalid, taken, or `default`,
unpacks into a staging directory beside the themes, and renames it into
place, so a failure leaves nothing installed. Packages are read and
written with the `zip` crate, using a pure-Rust deflate. Unit tests round
trip a theme through its manifest and through export and import, parse
the documented example, list problems, list and find themes, and check
new, rename, remove, add-sound, a package zipped as a folder, and that
stray paths in a package are left out.

## Settings files

Implementation notes: writes are atomic (write a temporary file, rename
over the target; `std::fs::rename` replaces on Windows).
`ensure_files_exist` materializes missing files with defaults at startup
(`settings.toml`, and empty `profiles` and `themes` folders) so
they are discoverable and hand-editable, but never touches existing files —
including corrupt ones, which `load` surfaces as errors rather than
silently replacing. The corollary: fields added to the schema later do not
appear in an existing file; they read as defaults until the file is
regenerated or saved.
