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
- `remoteAlgorithms.py` — shared remote-side algorithms (bulk text
  range walking, attribute-run iteration).

The programming model's key restriction (from the readme): the
decorated build function must express all logic through the `RemoteAPI`
object — remote control flow, remote comparisons — because the Python
function runs *once at build time*; ordinary Python control flow would
be baked into the recording. Static typing plus runtime validation
police this.

## What NVDA uses it for

`source/UIAHandler/remote.py` is the consumer-facing module. Current
production uses:

- `msWord_getCustomAttributeValue` — fetching Word's custom text
  attributes (via Word's extended-text-range custom pattern GUID) in
  one round trip.
- Bulk text-content and attribute-run extraction for UIA documents
  (Word, Terminal): collecting a range's text plus formatting runs in a
  single operation instead of one call per run — the difference between
  usable and unusable "say all" latency in big UIA documents.
- General utilities like collecting ancestor names (the readme's
  example) where a loop of parent fetches would otherwise be a loop of
  round trips.

## Constraints and failure modes

- OS-gated: requires a Windows 11 UIA core new enough to accept remote
  programs; NVDA falls back to local execution otherwise
  (`localExecute.py`), preserving behavior at the old latency.
- The remote VM is sandboxed and instruction-limited (the platform
  bounds execution); a program that exceeds limits fails with a status
  in the result set — `OperationException.errorLocation` maps it back
  to the emitting Python line, which the framework goes to some length
  to make debuggable (`operation.py` records per-instruction source
  locations).
- Only UIA-typed data can cross: elements, ranges, and plain values in;
  chosen operands out. No callbacks mid-operation.
