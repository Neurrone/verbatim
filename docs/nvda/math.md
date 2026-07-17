# Math

How NVDA reads and navigates mathematics (`source/mathPres/`).
In scope for Verbatim, unscheduled; this documents the shape of the
problem and NVDA's current answer.

## The interchange format is MathML

Everything math flows through MathML strings. Sources: browsers
expose embedded MathML through their accessibility trees (IA2 object
attributes / dedicated interfaces on math roles), Word's OMML is
converted, and `getMathMlFromTextInfo` pulls MathML out of a text
position whose ControlField declares math content
([TextInfo](text-infos.md)). A math region appears in flowing text as
an embedded object with role math; reading past it speaks the
rendered math, and a command enters interactive navigation.

## The provider seam

`mathPres.MathPresentationProvider` is a three-method contract:
`getSpeechForMathMl(mathMl)` (a speech sequence — real commands, so
prosody and pauses are possible; [Speech](speech.md)),
`getBrailleForMathMl(mathMl)` (a braille string in a math code such
as Nemeth or UEB technical; [Braille](braille.md)), and
`interactWithMathMl(mathMl)` (enter step-by-step navigation).
Speech, braille, and interaction providers register independently
(`registerProvider`).

The bundled provider is **MathCAT** (`mathPres/MathCAT/`), a Rust
library with Python bindings, vendored since 2025 — it replaced the
add-on-era arrangement and the older MathPlayer dependency. It
implements all three roles: rule-driven speech (multiple speech
styles, verbosity levels, language rule sets), braille math codes,
and navigable structure. Its settings surface as NVDA's Math
settings panel ([NVDA's GUI and the settings framework](gui-and-settings.md)).

## Interactive navigation

`interactWithMathMl` opens a `MathInteractionNVDAObject` — a
transient focus target (the same virtual-focus pattern as OCR
results; [OCR and content recognition](ocr-and-content-recognition.md))
whose arrow-key scripts walk the expression tree via the provider
(into/out of fractions, across terms, with zoom levels), speaking
each step; Escape returns to the document. The design keeps NVDA
ignorant of math semantics: navigation state lives in the provider,
NVDA supplies the focus shell and input routing.

## Transferable observations

- Treating MathML as the single interchange format keeps every
  source (browser, Word, EPUB) converging on one pipeline; the cost
  is conversion shims at each source.
- Speech-for-math must return *command-bearing sequences*, not
  strings — pause and pitch structure is what makes nested
  expressions intelligible.
- Math braille is a separate translation domain from literary
  braille (dedicated codes), which is why the provider contract
  splits speech and braille.
- Interaction as a transient virtual focus object reuses existing
  screen reader machinery — no document-model changes were needed to
  add math navigation.
