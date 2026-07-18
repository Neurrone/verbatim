# ARIA: web semantics for accessibility APIs

ARIA (Accessible Rich Internet Applications, formally WAI-ARIA) is the
W3C standard that lets web authors attach accessibility semantics to
HTML. It exists because a `<div>` styled and scripted into a tree view
carries none of the meaning a real tree control has; ARIA is the
vocabulary for asserting that meaning so the browser can expose it. A
screen reader never consumes ARIA directly — it consumes what the
*browser maps ARIA into*, over IA2 or UIA — but the vocabulary leaks
through everywhere: in IA2 object attributes, in NVDA's role mappings,
and in any conversation about web parity. This file gives a
non-web-programmer just enough of it.

## The model: authored overrides on a computed tree

A browser computes an accessibility tree from the DOM: native HTML
elements come with built-in semantics (`<button>` is a push button,
`<h2>` a heading of level 2). ARIA is a set of *attributes* that
override or augment that computation:

- `role="..."` replaces the computed role. The role vocabulary covers
  widget roles (`button`, `checkbox`, `tab`, `treeitem`, `slider`,
  `combobox`, …), composite widgets (`tree`, `grid`, `listbox`,
  `menu`, `tablist`), document structure (`heading`, `list`, `table`,
  `region`, `article`), and *landmarks* (`navigation`, `main`,
  `search`, `banner`, `contentinfo`) — the page-level wayfinding
  vocabulary browse modes surface.
- `aria-*` properties and states carry everything else: names and
  descriptions (`aria-label`, `aria-labelledby`, `aria-describedby`),
  widget state (`aria-checked`, `aria-expanded`, `aria-selected`,
  `aria-pressed`, `aria-disabled`, `aria-current`, `aria-invalid`,
  `aria-required`), ranges (`aria-valuenow`/`min`/`max`/`valuetext`),
  set position (`aria-posinset`, `aria-setsize`, `aria-level`), and
  relationships (`aria-controls`, `aria-owns`, `aria-details`,
  `aria-errormessage`).

Two structural mechanisms deserve special note because clients must
handle them:

- **`aria-activedescendant`**: DOM focus stays on a container (a
  listbox, say) while this attribute names the *logically* focused
  child. The browser fires focus events on the child without the child
  ever having DOM focus — the pattern behind most autocomplete and
  listbox widgets, and a standing source of focus-event edge cases.
- **`aria-hidden`**: subtrees the author excludes from the
  accessibility tree entirely, even though they are visible — and the
  reverse trap, visible content hidden from the tree by mistake.

## Live regions

The ARIA feature with the most screen reader machinery behind it.
Marking a container `aria-live="polite"` (or `assertive`, or using the
shorthand roles `status` and `alert`) asks that *changes inside it be
announced without focus moving there* — chat messages, validation
errors, "3 results found". Modifiers: `aria-atomic` (announce the whole
region or just the changed node), `aria-relevant` (which mutation kinds
count), `aria-busy` (hold announcements while updating). Browsers
surface all this over IA2 as text-inserted events plus object
attributes (`container-live`, `atomic`, `relevant` — [IA2](ia2.md)),
and the client implements the announcement policy; NVDA's
implementation is in [ARIA, annotations, and compound documents](../nvda/aria-and-annotations.md).

## The mapping layer

How each role/state/property becomes an IA2 role, UIA control type,
object attribute, or event is itself specified — the *Core Accessibility
API Mappings* (Core-AAM) W3C document, per platform API. Browsers
follow it closely but not identically, which is one reason two
browsers can expose the same page differently and both be conformant
([IA2](ia2.md)'s working characteristics). When NVDA needs the authored
ARIA rather than the mapped result, it reads it from IA2 object
attributes (`xml-roles`, `live`, and friends) — the escape hatch the
mappings deliberately leave open.

## Working characteristics

- ARIA is *authored*, by page authors of wildly varying skill; unlike a
  Win32 control's semantics it is frequently wrong. The first rule of
  the spec's own authoring guidance is that no ARIA is better than bad
  ARIA. A screen reader must be robust to roles that lie, states that
  never update, and live regions that spam.
- ARIA only asserts semantics; it changes no behavior. An element with
  `role="button"` still needs the author to implement keyboard
  handling. The gap between claimed role and actual behavior is a
  routine source of "screen reader bug" reports that are page bugs.
- The vocabulary versions slowly (ARIA 1.2 is current, 1.3 in draft);
  browsers ship mappings for draft features early, so attributes not
  in any published mapping table still show up in object attributes.

## References

- [WAI-ARIA 1.2 (W3C)](https://www.w3.org/TR/wai-aria-1.2/) — the role
  and attribute vocabulary.
- [Core Accessibility API Mappings (W3C)](https://www.w3.org/TR/core-aam-1.2/) —
  ARIA to IA2/UIA/MSAA mapping tables.
- [ARIA Authoring Practices Guide (W3C)](https://www.w3.org/WAI/ARIA/apg/) —
  the widget patterns authors are supposed to follow; useful as a map
  of what well-formed widgets look like.
