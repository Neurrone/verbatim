# IA2 usage

NVDA consumes IAccessible2 as an upgrade layer over its MSAA objects.
API background: [IA2](../explainers/ia2.md). The consumers are the
`IAccessible` NVDAObjects, the virtual buffer backends
([Virtual buffers](virtual-buffers.md)), and the in-process live-region code.

## Discovery

`IAccessibleHandler.normalizeIAccessible(pacc, childID)`
(`source/IAccessibleHandler/__init__.py`): every `IAccessible` NVDA
obtains is immediately upgraded if possible — QueryInterface to
`IServiceProvider`, then `QueryService(IID_IAccessible, IID_IAccessible2)`.
Only for real objects: IA2 has no simple children, so childID != 0 stays
plain MSAA ([#2558](https://github.com/nvaccess/nvda/issues/2558)). Misbehaving apps returning a null pointer from
QueryService are treated as unsupported. From then on the object's
`IAccessibleObject` is an `IAccessible2` when the app offers one, and
`NVDAObjects.IAccessible` reads IA2-first:

- Roles: `IAccessibleObject.role()` (IA2 extended roles) consulted
  before the MSAA role; the mapping tables at the top of
  `IAccessibleHandler/__init__.py` translate both `ROLE_SYSTEM_*` and
  `IA2_ROLE_*` into `controlTypes.Role`.
- States: MSAA states plus IA2 states
  (`IAccessible2StatesToNVDAStates`) merge into one
  `controlTypes.State` set.
- Identity: `uniqueID` becomes the object's identity for equality and
  event matching (IA2 events carry it as the child parameter).
- `IA2Attributes` (parsed `attribute:value;` pairs) feed formatting,
  landmarks, ARIA properties; relations feed label/description
  resolution.

## Text

`NVDAObjects.IAccessible`'s TextInfo (`IA2TextTextInfo` in
`source/NVDAObjects/IAccessible/__init__.py`) implements the offsets
TextInfo contract ([TextInfo](text-infos.md)) over `IAccessibleText`:
`caretOffset`, `nCharacters`, `text(start, end)`, `textAtOffset` for
unit expansion, `characterExtents`/`offsetAtPoint` for geometry,
selections via `nSelections`/`Selection`, `setCaretOffset` for caret
movement. Formatting comes from `IAccessibleText::attributes` runs.
Embedded objects appear as U+FFFC characters resolved through
`IAccessibleHypertext` — in *browse mode* this resolution happens at
buffer build time in C++ instead ([Virtual buffers](virtual-buffers.md)); the Python path
serves focus mode and non-buffer IA2 text (for example, editable fields).

## Events

IA2's extension events arrive as winevents (registered in the standard
map; [MSAA and winevent handling](msaa.md)): `IA2_EVENT_TEXT_CARET_MOVED` becomes the NVDA `caret`
event, `IA2_EVENT_DOCUMENT_LOAD_COMPLETE` drives browse-mode buffer
loading, `IA2_EVENT_OBJECT_ATTRIBUTE_CHANGED` and `IA2_EVENT_PAGE_CHANGED`
map to their NVDA events. Text-changed events are *not* consumed
out-of-process: document text change handling lives in the virtual
buffer backends and the live region code, both in-process.

## Live regions are handled in-process

ARIA live regions are implemented entirely inside the injected helper
(`nvdaHelper/remote/ia2LiveRegions.cpp`), not in Python: an in-context
winevent hook for `EVENT_OBJECT_LIVEREGIONCHANGED` runs *inside the
browser process*, reads the IA2 object attributes (`container-live`
polite/assertive, `container-relevant`, `container-atomic`,
`container-busy`), walks up to find the live region root, extracts the
changed text with `textFromIAccessible.cpp` (recursively assembling
name/text of the subtree, resolving embedded objects), and pushes the
finished string directly to NVDA over the helper's RPC channel
(`nvdaControllerInternal_reportLiveRegion`), where it is spoken at the
appropriate politeness. Rationale worth noting for parity thinking: by
the time an out-of-process client could react to the winevent and walk
the region, the DOM may have changed again; in-process handling reads
the tree synchronously at event time. Consequence: *without injection,
NVDA has no IA2 live region support* (its UIA path has separate
notification/live-region handling).

## Where IA2 quirks are absorbed

Per-toolkit differences are handled in overlay classes:
`NVDAObjects.IAccessible.mozilla` (Gecko), `.chromia` (Chromium),
`.ia2Web` (shared web behavior), each adjusting role/state/attribute
interpretation. The `IAccessibleApplication` interface (toolkit name and
version) is how objects self-identify for these overlays. The general
pattern to remember: NVDA treats IA2 data as *mostly trustworthy* within
Gecko/Chromium and full of exceptions elsewhere, with the exceptions
living in overlays, not in the handler.
