# UIA remote operations

Remote operations are UIA's newer answer to the cross-process round-trip
tax: instead of caching known properties up front, the client authors a
small *program* that is shipped to the provider process and executed
there in one round trip. NVDA has built a substantial Python framework
over this; it lives in `source/UIAHandler/_remoteOps/` and has its own
in-tree readme (`_remoteOps/readme.md`) worth reading verbatim.

## The platform layer

Windows (Insider-era `UIAutomationCore`, Windows 11) exposes a
low-level *Remote Operations* facility: a client submits a bytecode
program plus imported operands (elements, text ranges, values); the UIA
core executes it inside the provider's process; results chosen by the
program come back as one result set. The raw surface NVDA binds
(`_remoteOps/lowLevel.py`) is the bytecode instruction set
(`instructions/`), operand IDs, and `RemoteOperationResultSet` (status,
error location, extended error, operand retrieval). This is the same
machinery
[Microsoft's own UIA remote operations spec](https://github.com/microsoft/microsoft-ui-uiautomation/blob/master/docs/RemoteOperations.md)
describes; it is
officially sanctioned but sparsely documented — NVDA's framework is one
of the few serious consumers.

## NVDA's framework

The layers, bottom up:

- `builder.py` — assembles instruction lists with labels and offsets.
- `remoteTypes/` — remote proxies for ints, strings, arrays, GUIDs,
  variants, elements, text ranges; operations on them *emit
  instructions* rather than executing.
- `remoteAPI.py` — the authoring surface: `ra.newElement(...)`,
  `ra.newArray()`, control flow as context managers
  (`ra.whileBlock(...)`, `ra.ifBlock(...)`), `ra.Return(...)`.
- `operation.py` — `Operation.buildFunction` decorates a Python
  function that, when called once, *records* the program; `op.execute()`
  ships and runs it, marshalling results back (`OperationException`
  carries remote error location on failure).
- `localExecute.py` — a local interpreter for the same instruction set:
  the fallback when the OS lacks remote operations support, and the
  test double (the same program can run locally against live UIA,
  slower but semantically identical).
- `remoteAlgorithms.py` — shared remote-side algorithms; at this pin
  the only one is `remote_forEachUnitInTextRange`, which walks a text
  range one unit (such as a paragraph) at a time, forwards or in
  reverse.
- Cache requests (`instructions/cacheRequest.py`, commit `9fccec044`,
  #20621): the `NewCacheRequest`, `CacheRequestAddProperty`,
  `CacheRequestAddPattern`, and `PopulateCache` instructions, exposed as
  `ra.newCacheRequest()`, `RemoteCacheRequest.addProperty` and
  `addPattern` (`remoteTypes/cacheRequest.py`), and
  `RemoteElement.populateCache`. An element returned or yielded from the
  operation carries the populated cache, so its cached getters need no
  further cross-process calls. The readme's caveat: a remotely
  populated cache stores default values for properties the element does
  not support, not the reserved "not supported" value a locally built
  cache returns, so `getCachedPropertyValueEx` behaves as if
  `ignoreDefault` were unset; to test pattern support, cache the
  `IsXPatternAvailable` property, or fetch explicitly with
  `getPropertyValue` and `ignoreDefault` and test the variant with
  `RemoteVariant.isNotSupported`. The same commit taught the result
  marshalling in `UIARemote.dll` to return UInt32, Int64, Single, and
  Double values, beyond the Int32, string, and boolean scalars it
  handled before. Nothing outside the framework and its tests uses
  cache requests yet at this pin.

The programming model's key restriction (from the readme): the
decorated build function must express all logic through the `RemoteAPI`
object — remote control flow, remote comparisons — because the Python
function runs *once at build time*; ordinary Python control flow would
be baked into the recording. Static typing plus runtime validation
police this.

## What NVDA uses it for

`source/UIAHandler/remote.py` is the consumer-facing module. Its
production uses at this pin, all gated on `remote.isSupported()`
(Windows 11 or later):

- `msWord_getCustomAttributeValue` — fetching Word's custom text
  attributes (via Word's extended-text-range custom pattern GUID) in
  one round trip; `NVDAObjects/UIA/wordDocument.py` uses it for line,
  page, section, and text column numbers and expand/collapse state.
- Word sentence movement: `msWord_textRange_moveBySentence`,
  `msWord_textRange_moveEndpointBySentence`, and
  `msWord_textRange_expandToEnclosingSentence`, which call Word's
  custom extended-text-range sentence methods remotely (used by
  `wordDocument.py`).
- Heading quick navigation in UIA documents:
  `collectAllHeadingsInTextRange` and `findFirstHeadingInTextRange`
  walk a range paragraph by paragraph with
  `remote_forEachUnitInTextRange` and test each paragraph's `StyleId`
  attribute for Heading 1 to 9, instead of one cross-process call per
  paragraph (`UIAHandler/browseMode.py`).

NVDA does not use remote operations for bulk text extraction in
Windows Terminal or for collecting ancestor names; the latter appears
only as an example in the readme.

## Constraints and failure modes

- OS-gated: requires a Windows 11 UIA core new enough to accept remote
  programs. Where `remote.isSupported()` is false, NVDA's callers take
  their ordinary non-remote code path; `localExecute.py` (the
  `localMode` option of `Operation`) serves unit tests rather than
  production fallback.
- The remote VM is sandboxed and instruction-limited (the platform
  bounds execution); a program that exceeds limits fails with a status
  in the result set — `OperationException.errorLocation` maps it back
  to the emitting Python line, which the framework goes to some length
  to make debuggable (`operation.py` records per-instruction source
  locations).
- Only UIA-typed data can cross: elements, ranges, and plain values in;
  chosen operands out. No callbacks mid-operation.
