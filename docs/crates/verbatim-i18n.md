# verbatim-i18n

Fluent localization (decision D10). English resources compile into the
binary as the permanent fallback; other locales load at startup from a
`locale` folder next to the executable.

Public API:

- `loader()` — the process-wide `FluentLanguageLoader`; `new_loader()`
  builds an isolated one for tests.
- `load_locale_dir(dir, requested)` — negotiates and loads locale-folder
  languages over the embedded fallback.
- `messages` — one typed accessor per statically known UI string
  (`menu_settings()`, `settings_title_with_category(category)`, and so on),
  each compile-time checked by the `fl!` macro, so a message-id typo fails
  the build.
- `message(id)` — runtime lookup for ids that arrive as data, such as
  setting-descriptor label keys.
- `role_name(role)`, `state_name(state)`, `negated_state_name(state)` — the
  localized spoken words for utterance tokens; states that are never spoken
  (focused, focusable, selectable, offscreen) return `None`.
- `position_in_set(position, set_size)` and `level(n)` — the localized
  "2 of 5" and "level 3" phrases for the corresponding utterance spans.
  Both pass their numbers as pre-rendered strings so no locale applies
  digit grouping to an ordinal position.

Implementation notes: the loader disables Fluent's bidi argument isolation
globally — Fluent wraps interpolated arguments in invisible directional
isolate marks by default, which protects visually rendered mixed-direction
text but would leak invisible characters into spoken text, dictionary and
symbol processing, and braille. `LocaleDirAssets` exists because
i18n-embed's own filesystem assets type yields bare file names without the
language folder, which breaks language negotiation; this implementation
yields `language/file` paths. The pseudo-locale test (required from M1) generates
a bracket-wrapped translation of every English message into a temporary
locale, loads it, and asserts every message id resolves through it — proof
no string bypasses the loader. It parses the `.ftl` line by line, which is
why the resource file keeps every message on a single line.
