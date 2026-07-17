# Office through the COM object models

Word, Excel, and Outlook are the apps where NVDA bypasses
accessibility APIs and reads the *application object model* — the
same COM automation surface VBA macros script. This is a core-level
capability (it needed C++ in-process support), which is why it gets a
file here despite the per-app rule.

## Why the object model at all

Historically Office's MSAA was near-useless for documents and its
UIA arrived late and incomplete ([The UIA client](uia.md) documents the still-current
referee logic: Word/Excel prefer the object model when injection is
available, UIA otherwise per `shouldUseUIAInMSWord`). The object
model exposes everything (text, formatting, tables, comments,
revisions, spelling errors, charts, formulas) — at the cost of
per-call cross-process COM latency and zero screen-reader
affordances (no events, no caret notifications; those still come
from MSAA/UIA around the edges).

## Word

`source/NVDAObjects/window/winword.py`: `WordDocument` objects get a
TextInfo (`WordDocumentTextInfo`) implemented over `Range` objects
from the window's `Document` (obtained via
`accessibleObjectFromWindow` for the native OM entry point). Unit
movement maps to `Range.Move*`/`Expand`; formatting reads run
properties; browse mode over Word is quick-nav iterators over OM
collections (headings via outline levels, comments, revisions,
fields, footnotes, spelling errors, charts — the
`WordDocument*QuickNavItem` classes).

The latency fix: hot paths run *inside Winword* via the injected
helper (`nvdaHelper/remote/winword.cpp`, RPC surface
`nvdaInProcUtils_winword_expandToLine`, `_getTextInRange`,
`_moveByLine`): NVDA sends one RPC; the in-process side performs the
many OM calls at in-process speed (same STA, no marshaling) and
returns the result — including whole formatted-text extraction with
control fields assembled in C++. A registered window message
(`wm_winword_expandToLine`) lets the in-process code run on Word's
UI thread. This in-process-OM pattern is the defining trick of
NVDA's classic Office support.

## Excel

`source/NVDAObjects/window/excel.py`: sheets and cells are
NVDAObjects built over OM objects (`ExcelWorksheet`, `ExcelCell` —
obtained by `Dispatch` on the native OM); cell navigation follows
selection-change events (MSAA) but reads cell content, formulas,
comments, merged-range geometry, validation, and charts from the OM;
quick-nav iterates OM collections (`ExcelCellInfoQuicknavIterator`).
The bulk-read RPC (`nvdaHelper/remote/excel.cpp`,
`nvdaInProcUtils_excel_getCellInfos`) fetches batched
`EXCEL_CELLINFO` structs (text, formatting facts, coordinates) for a
range in one call — again, many OM calls collapsed into one
round trip.

## Outlook

`source/appModules/outlook.py` reads message lists (the `SUPERGRID`
window) and message metadata through the Outlook OM, with
`nvdaHelper/remote/outlook.cpp` providing in-process acceleration
for the message list's columned data.

## Failure characteristics

OM calls freeze when Office is busy ([Main loop and watchdog](main-loop-and-watchdog.md):
Word/Excel windows are in the watchdog's patient
`safeWindowClassSet`; issues [#10247](https://github.com/nvaccess/nvda/issues/10247)/#10276 are OM-call freezes), and
the OM can throw `COMError` at any moment mid-read (dialog open,
document closing, "the message you're reading was deleted") — code
in these modules is saturated with try/except COMError for that
reason. When injection is unavailable (Store Office, injection
failure, security contexts), NVDA silently degrades to the UIA path,
so *two full implementations are maintained in parallel* — a cost
worth weighing against the OM's fidelity when deciding whether to
replicate this design.
