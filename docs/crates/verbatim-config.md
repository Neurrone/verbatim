# verbatim-config

Portable configuration next to `verbatim.exe`. `settings.toml` is the base
configuration — global settings plus the base profile's sections — and the
`profiles` folder holds named profiles as sparse overlays, mirroring NVDA's
base-plus-diffs model. Because the base lives in `settings.toml`, any file
name in `profiles` is a legitimate user profile.

Public API:

- `Settings` — the `settings.toml` schema: `locale`, `log_filter`,
  `verbatim_keys`, `keyboard` (all global; profiles cannot carry them), and
  `speech` (the base profile's section).
- `VerbatimKeys` — which keys act as the Verbatim modifier (`caps_lock`,
  `insert`, `numpad_insert`) plus `share_modifier`, which passes the
  modifier's own transitions down the hook chain for a screen reader
  running behind Verbatim.
- `KeyboardConfig`, `KeyboardLayout` — the active gesture-binding layout
  (`desktop`, the default, or `laptop`), M3's `keyboard` section. Exposed
  only in `settings.toml`; a GUI choice arrives with M8's gesture-remapping
  work. `verbatim-input`'s `bindings_for` consumes the resolved layout
  through its own decoupled layout enum (the app maps one to the other),
  the same pattern `DecisionConfig` already follows for `VerbatimKeys`.
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
