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
- `message_text(message)` and `phrase_text(phrase)` — the wording of the
  reader's fixed messages and of its messages with values ("selected
  hello", "Positioned at 10, 20", "speak typed characters only in edit
  controls"), NVDA's English wording; `typing_echo_name(mode)` names a
  typing echo choice.
- Themes (milestone M4): `indication_name(indication)` and
  `indication_category_name(category)`, the catalogue as the theme panel
  lists it (a role's or state's spoken name, "spelling error", "Text
  formatting"); `presentation_name` ("speech and sound");
  `theme_default_name` and `theme_default_description`, since the built-in
  default theme has no name of its own; `format_text` for formatting spans
  ("spelling error", "out of spelling error", NVDA's "bold" and "no bold",
  "italic" and "no italic", "underlined" and "not underlined",
  "strikethrough", "double strikethrough", and "no strikethrough", "link"
  and "out of link", a background color as "light grey background" or,
  after a color, "on light grey", a kind of underline by Microsoft Word's
  name ("double underline", "wave underline"), a bullet as NVDA names its
  character ("bullet", "white bullet", "black square"), and a font or
  color as the application words it); `earcon_text`, an event's words when a theme
  speaks it ("browse mode", "40 percent"); and `theme_problem_text`, a
  problem found loading a theme. `phrase_text` also words "skipped 1 line"
  and "skipped 120 lines".
- `character_name(character, language)` and
  `character_description(character, language)` (milestone M4) — the
  character table: the name a character is spoken by on its own ("comma",
  "space", "superscript minus") and its description ("Alfa" for a, a
  capital taking its small letter's, by the language's casing, so in
  Turkish and Azerbaijani I takes the dotless ı's and İ takes i's). A character is looked up in its
  composed form (`verbatim_text::composed`), so a letter written with a
  combining accent is found as the precomposed one. Both are keyed by locale: the table
  is the `character-name-` and `character-description-` messages of each
  locale's Fluent file, one per code point in lowercase hexadecimal
  (`character-name-002c = comma`), so another language's table is data, not
  code. `language` is the text's BCP 47 tag; a language with no table falls
  back to the loaded languages, English last. English is the only table
  shipped: NVDA's English symbol names, with its corrected ones ("three
  eighths", "superscript minus"), and the phonetic alphabet. NVDA's
  translations are never imported (the provenance rule).

Implementation notes: the loader disables Fluent's bidi argument isolation
globally — Fluent wraps interpolated arguments in invisible directional
isolate marks by default, which protects visually rendered mixed-direction
text but would leak invisible characters into spoken text, dictionary and
symbol processing, and braille. Languages loaded from a locale folder
come with isolation on again, so `load_locale_dir_into` turns it off
after loading them. `LocaleDirAssets` exists because
i18n-embed's own filesystem assets type yields bare file names without the
language folder, which breaks language negotiation; this implementation
yields `language/file` paths. The pseudo-locale test (required from M1) generates
a bracket-wrapped translation of every English message into a temporary
locale, loads it, and asserts every message id resolves through it to
exactly its English text in brackets — proof no string bypasses the
loader, and that no argument or isolation mark is lost or added. It parses the `.ftl` line by line, which is
why the resource file keeps every message on a single line.
