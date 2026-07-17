# The display model

The display model is NVDA's legacy screen-scraping layer: injected
hooks capture text as applications *draw* it with GDI, building a
per-window spatial model of what is on screen. NVDA's own design note
(`nvda/projectDocs/design/displayModel.md`) labels it "a legacy
mechanism"; it survives because it is the only text source for
GDI-drawn apps with no accessibility support, and because *screen
review* ([Review modes](review-modes.md)) is defined over it.

## Capture: GDI hooks in-process

`nvdaHelper/remote/gdiHooks.cpp` (running injected;
[Process injection](process-injection.md)) API-hooks the GDI text output family —
`TextOut`, `ExtTextOut` (A/W variants, template `hookClass_TextOut`,
`ExtTextOutHelper`), `PolyTextOut`, plus glyph-level Uniscribe
(`ScriptTextOut`) — and records, per device context resolved to its
window: the string drawn, its screen rectangle, per-character extents,
transforms, and formatting facts recoverable at draw time (font,
size, colors including transparency). Erasure/overdraw handling
(background fills, `BitBlt`-style moves) updates or invalidates
stored chunks (`displayModel.cpp`, `displayModel_t`). Changes are
pushed to NVDA (`nvdaControllerInternal_displayModelTextChangeNotify`
with the changed rectangle) — which NVDA surfaces as, among other
things, reporting of dynamic content for apps configured for it.

Fundamental limitations, inherent to the approach: only GDI is
visible — DirectWrite/Direct2D rendering (all modern toolkits:
WPF, UWP, Chromium, Office 2013+) bypasses the hooks entirely, so
the model sees nothing in those windows; text drawn before injection
is missing until redraw (NVDA forces a `redraw()` when entering
screen review — `review.getScreenPosition`); and the model knows
glyphs and rectangles, not semantics.

## Consumption: DisplayModelTextInfo

`source/displayModel.py` wraps retrieval:
`getWindowTextInRect(bindingHandle, windowHandle, rect, …)` RPCs into
the injected model and returns text chunks plus rectangles;
`DisplayModelTextInfo` (an `OffsetsTextInfo`; [TextInfo](text-infos.md))
assembles them into a readable *screen-ordered* text: chunks sorted
into lines by baseline with configurable ordering, whitespace
synthesized between spatially separated chunks, offsets mapped
bidirectionally to screen rectangles (`_getStoryFieldsAndRects`,
`_getStoryOffsetLocations`) — which is what makes review-by-line
match the visual layout, and mouse/touch text reporting possible in
GDI apps. `EditableTextDisplayModelTextInfo` layers caret/selection
detection on top by *color inversion analysis* (selection colors) —
the historical trick for edit controls with no better API.

## Users of the model

- Screen review mode: entirely this ([Review modes](review-modes.md)).
- Mouse tracking in GDI apps: text under the pointer.
- Fallback content for windows whose accessibility surface is empty
  (some owner-drawn and legacy controls) via overlay classes that
  adopt `DisplayModelTextInfo` as their TextInfo.
- The focus highlight's rectangle refinement historically consulted
  it (`displayModel_getFocusRect`).

## Status

Maintenance-only: NVDA gates parts behind app-module opt-ins because
hooking every GDI call in every process has a real performance and
stability cost, and the covered app population shrinks yearly. Any
implementation decision informed by this file should weigh that the
mechanism's value is now concentrated in legacy line-of-business
apps and consoles-adjacent tooling, and that screen review in modern
apps is served (poorly) by location-sorted accessibility text
instead (`textInfos.UNIT_SCREEN` handling), not by this model.
