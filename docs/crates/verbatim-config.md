# verbatim-config

Portable configuration next to `verbatim.exe`. `settings.toml` is the base
configuration — global settings plus the base profile's sections — and the
`profiles` folder holds named profiles as sparse overlays, mirroring NVDA's
base-plus-diffs model. Because the base lives in `settings.toml`, any file
name in `profiles` is a legitimate user profile.

Public API:

- `Settings` — the `settings.toml` schema: `locale`, `log_filter`,
  `verbatim_keys`, `keyboard`, `uia` (all global; profiles cannot carry
  them), `speech` (the base profile's section), and `reader` (milestone M4, the
  base profile's too until profiles grow it in M8).
- `ReaderSettings`, `TypingEcho`, `SayAllUnit` — re-exported from
  `verbatim-model`, which the reducer reads them from; the `[reader]`
  section, every key with NVDA's default:
  - `speak_typed_characters`: `"off"`, `"edit_controls"` (only in edit
    controls and other places text can be typed), or `"always"`, the
    default. NVDA's "Speak typed characters".
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
- `Profile` — one named profile. Deliberately has no global fields, so a
  profile file cannot smuggle in modifier keys or a locale; unknown
  sections are ignored by the schema (tested).
- `ConfigStore` — `load(root)`, `settings()`, `settings_mut()`,
  `save_settings()`, `ensure_files_exist()`, and `active()`.
- `ActiveConfig` — the resolved read-only view: `synthesizer()`,
  `synth_setting(synth, id)`, and `synth_settings(synth)` resolve each
  setting through the active overlays (most specific first) down to the
  base. M1 activates no overlays; the layering is implemented and tested.

Implementation notes: writes are atomic (write a temporary file, rename
over the target; `std::fs::rename` replaces on Windows).
`ensure_files_exist` materializes missing files with defaults at startup so
they are discoverable and hand-editable, but never touches existing files —
including corrupt ones, which `load` surfaces as errors rather than
silently replacing. The corollary: fields added to the schema later do not
appear in an existing file; they read as defaults until the file is
regenerated or saved.
