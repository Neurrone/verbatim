# verbatim-uia-rops

UIA remote operations (architecture section 4): small programs that run
inside a UIA provider's process, so work that would take one
cross-process round trip per step takes one in all. The crate calls
Windows' own API, the WinRT class
`Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, as NVDA
does; it does not use Microsoft's `microsoft-ui-uiautomation` library.
It is Windows-specific and GPL, so it ports NVDA's framework
(`source/UIAHandler/_remoteOps/`) freely, with attribution in each
ported file; where it follows Microsoft's MIT-licensed
`RemoteOperationInstructions.h`, that file carries Microsoft's notice.
The design of record is phase 6's "UIA remote operations" section.

The crate has three layers: the instruction set and a typed builder,
execution, and algorithms. The algorithms are the focus ancestry and,
for milestone M4's terminals, `terminal_tail` (layer 3 below).

## Layer 1: instructions and the builder

- `Opcode` is the full table NVDA uses, 104 opcodes (`Opcode::ALL`):
  every general instruction from `0x00` to `0x54`, cache requests
  (`0x4C` to `0x50`) included, and the 19 text range methods, whose
  opcode is `(patternId << 16) | (relatedObject << 8) | vtableIndex`
  (`pattern_related_object_method`). `Comparison`,
  `NavigationDirection`, `PointProperty`, and `RectProperty` are the
  enumerations instructions carry; `Status` is a run's outcome.
- `Instruction` is one instruction with its parameters, and
  `Instruction::encode` writes its bytes. Every instruction has a unit
  test of its exact bytes, and a second test checks that those tests
  cover every opcode.
- `Builder` writes a program. Each register is a `Reg<K>`, where `K` is a
  marker from `kind` saying what it holds: `Element`, `TextRange`, `Int`,
  `Uint`, `Bool`, `Double`, `Char`, `Str`, `Point`, `Rect`, `Array`,
  `StringMap`, `CacheRequest`, `Guid`, or `Any` for a value whose type is
  known only at run time (a property value, an array item, a map entry),
  which `Reg::assume` names. Arithmetic takes only `Numeric` kinds,
  ordering comparisons only `Ordered` ones, and indexes only `Index`
  ones, so a program that compares an element with an integer does not
  compile.
- Constants (`Builder::int`, `uint`, `bool`, `string`) are gathered in a
  section ahead of the program and shared by every use of the same value,
  as NVDA's const section is; `new_int` and its siblings make variables
  set where they are emitted. A program is built per call, with that
  call's values as constants.
- Control flow takes closures, which run once at build time to emit their
  bodies, and computes its own jump offsets: `if_`, `if_else`, `while_`
  (with `break_loop` and `continue_loop`), `try_catch`, and `halt`. The
  layouts are NVDA's: a loop block whose continue target is the condition
  just after it, the condition's `ForkIfFalse` jumping to the
  `EndLoopBlock`, and a `ContinueLoop` ending the body; a try block whose
  catch target reads the operation status and resets it to zero before
  the catch body runs.
- `Builder::import_element` and `import_text_range` bring in the objects
  a program starts from; `add_to_results` asks for a register's value
  after the run; `finish` ends the program with a `Halt` and returns the
  `Operation`.
- Every emitting method is `#[track_caller]`, so each instruction records
  the Rust source line that emitted it; a failing instruction's index maps
  back to that line (`Operation::location`), as NVDA maps it to the
  Python line. NVDA's local emulator and Python operator overloading are
  not ported: tests run against mockapp's real provider instead.

### The encoding, as verified

The layout of each instruction is not documented officially. Verified
on Windows 11 26200 (x64): a program is the `u32` version 0 followed by
its instructions, with no padding. An instruction is its `i32` opcode and
then its parameters in order, little-endian: an operand is a `u32`
register id, an offset or enumeration an `i32`, a boolean one byte, a
character two bytes, a double eight bytes, and a string a `u32` length
that counts a terminating null, followed by that many UTF-16 code units,
the null included. Jump offsets count instructions from the jumping one,
and a failure's location is an instruction index.

Two of NVDA's instruction definitions are wrong, and Microsoft's are
right: `RemoteArrayRemoveAt` and `RemoteStringMapRemove` write a result
(the removed value) before their target, which NVDA's omit (NVDA never
uses either). Sent NVDA's way, the program fails as malformed bytecode.

## Layer 2: execution

`Operation::execute` creates a `CoreAutomationRemoteOperation`, imports
the program's elements and text ranges (an `IUIAutomationElement` casts
to the WinRT `AutomationElement` by `QueryInterface`), checks with
`IsOpcodeSupported` that the provider supports every opcode the program
uses (support is known only once something is imported, since it depends
on the provider's process), registers the requested results, and runs
`Execute`, the one cross-process round trip, which counts as one UIA call
on the thread's `verbatim_uia::calls` count. Creating the operation and
importing took about half a microsecond and each support check 18
nanoseconds in a release build: both are answered in Verbatim's process,
and neither is counted.

A failed run is an `Error`:

- `Unavailable`: Windows lacks the API.
- `Import`: an import failed. An element served by a client-side proxy
  (UIA's MSAA proxy inside Verbatim's own process) fails with
  `E_UNEXPECTED`, since there is no provider process to run in; nothing
  about that element's window will change.
- `Unsupported(opcode)`: the provider lacks an instruction.
- `Execute`: the call itself failed.
- `Failed(Failure)`: the program stopped with a failure status
  (`MalformedBytecode`, `InstructionLimitExceeded`, `UnhandledException`,
  or `ExecutionFailure`), with the extended HRESULT, the failing
  instruction's index, its opcode, the Rust line that emitted it, and the
  results computed before it stopped (`Failure::partial`).
- `MissingResult` and `ResultType`: a requested result is absent, or not
  of its register's type.
- `Uia`: a UIA call failed, in a classic implementation.

`Error::hresult` gives the HRESULT behind any of them: a provider whose
process has gone gives `UIA_E_ELEMENTNOTAVAILABLE`, and one that did not
answer in time `UIA_E_TIMEOUT`.

`Outcome::get` converts a requested register to Rust by its kind: `Int`
to `i32`, `Uint` to `u32`, `Bool` to `bool`, `Double` to `f64`, `Char` to
`u16`, `Str` to `String`, `Element` to `Option<IUIAutomationElement>`
(with the cache the program filled, readable through the `Cached*`
getters with no further call), `TextRange` to
`Option<IUIAutomationTextRange>`, `Array` to `Vec<Value>`, and `Any` to a
`Value`. Scalars come back as `IPropertyValue`s, arrays as
`IVector<IInspectable>` (the `windows-collections` crate), a runtime id
as an integer array, and a null register as a null object.

## Layer 3: the focus ancestry

`focus_ancestry_remote` and `focus_ancestry_classic` share one signature
(`FocusAncestryFn`): a `&Uia` and a `FocusQuery`, which holds the focused
element (with its cache filled; the outpost passes the focused element it
read), the runtime ids the caller already knows, a depth limit, the
properties to cache on every returned element
(`verbatim_uia::CACHED_PROPERTIES`, so they match what the snapshot code
reads), and a deadline, which only the classic walk checks, between hops.
Both answer `FocusAncestry::NotFocused` when a
live read of the element's `HasKeyboardFocus` is false (NVDA's check
that a focus event is not stale), and otherwise an `Ancestry`:

- `ancestors`: the raw-view parents, nearest first, each with the
  properties cached. The walk ends at the top-level window of the
  element's process: in a program, the top-level window's parent is null
  (verified), so the classic walk stops below the desktop root as well.
- `met_known`: which of the known runtime ids the last ancestor has, when
  the walk stopped there.
- `depth_limited`: whether the walk stopped at the depth limit with
  ancestors left.
- `out_of_time`: whether the classic walk stopped at the deadline with
  ancestors left; never set by the program.
- `selected_child`: for a list or tab control (by the element's cached
  control type), the first selected child, with the properties cached.
- `window`: the native window handle of the element or of its nearest
  raw-view ancestor that has one, which `verbatim_uia::nearest_window_handle`
  would find (NVDA's `getNearestWindowHandle`). The outpost needs it to
  arbitrate and report a focus whose event names no window, and reading
  it here saves that separate call.

The remote program is one round trip. It reads `HasKeyboardFocus` and
halts when it is false. For a list or tab control it reads the element's
`Selection` property (the Selection pattern has no method instructions;
the property is the same array of elements, verified against mockapp),
fills the first item's cache, and keeps it. Then it walks with
`Navigate` (direction parent), filling each ancestor's cache inside the
provider, turning its runtime id into a string with `Stringify`, and
stopping at the first one found in a string map of the known ids.
`Stringify` writes a runtime id as its integers in decimal, comma
separated, in square brackets (`[42,14681214,4,5]`); `runtime_id_key`
makes the same string on Verbatim's side. Setting the walking register
to the parent does not disturb the elements already appended to the
results array, though both are held by reference (verified). The window
starts as the element's own cached handle; while it is zero, each
ancestor's `NativeWindowHandle` is read as the walk passes it, and a walk
that stopped at a known ancestor or the depth limit before finding one
goes on up, returning nothing more, until it does.

The classic implementation is the reference and the fallback: the same
live read, the selected child through the Selection pattern
(`verbatim_uia::selected_element`, which `Uia::selected_child` also
uses), and one `GetParentElementBuildCache` round trip per ancestor over
the raw view, the walk `Uia::ancestor_chain` makes. `ancestor_chain`
itself is not called, because it returns filtered snapshots, not
elements: the presentable-ancestor filter, the switch to MSAA, and the
splice with the previous focus's chain stay with the outpost, which
applies them to either implementation's elements with
`Uia::ancestor_chain_from`. Its window comes from the walked ancestors'
caches, or, when it stopped before reaching one, from
`nearest_window_handle`, one more call. Every call it makes goes through
`verbatim-uia`'s safe wrappers, so it is counted, and the module has no
`unsafe` code.

### The entry point

`focus_ancestry(uia, query, remote)` is what call sites use. With `remote`
true it runs the program and, when that fails for any reason, runs the
classic walk for the same call; with `remote` false it runs the classic
walk alone. It returns the answer with a `Path`: `Remote`, `Classic`, or
`Fallback(error)`, the program's error, so the caller can log it (an
`Error::Failed` prints the failing instruction, its opcode, and the Rust
line that emitted it) and stop trying for a window whose import failed.
Only the classic walk's own failure is returned as an error.

NVDA makes each call site choose instead: code that can use remote
operations asks `remote.isSupported()` before building a program and
otherwise runs its own classic code. Here the choice and the fallback live
in one function, and a call site decides only whether to try, so a
failure is handled the same way everywhere.

### Cached properties filled remotely

A cache filled by a remote program stores a property's default where a
locally built cache stores UIA's "not supported" value, so reading a
property while ignoring defaults (`GetCachedPropertyValueEx` with
`ignoreDefault`) cannot tell an unsupported property from its default.
The snapshot code reads three properties that way, and each is handled:

- `ValueIsReadOnly` (default true) and `RangeValueValue` (default zero)
  are now read only when `IsValuePatternAvailable` and
  `IsRangeValuePatternAvailable` say the pattern is there; both flags
  joined `CACHED_PROPERTIES`. Without that, every container read
  remotely was read-only with a value of "0".
- `IsDataValidForForm` (whose default reads as false) has no pattern, so
  the program leaves it out of the cache of an element that does not
  support it, which the snapshot reads as it reads "not supported".
  `PopulateCache` replaces an element's cache rather than adding to it
  (verified), so the program builds one request per combination of
  these properties (`LEFT_OUT_WHEN_UNSUPPORTED`, one property, so two
  requests) and picks one per element after a `GetPropertyValue` with
  `ignoreDefault` and an `IsNotSupported` test.

Against mockapp, every other cached property reads the same both ways,
and the snapshots the outpost makes from the two implementations' elements
are equal.

## Layer 3: a terminal's tail

`terminal_tail_remote` and `terminal_tail_classic` share one signature
(`TerminalTailFn`), and `terminal_tail(uia, query, remote)` chooses
between them as `focus_ancestry` does, answering with a `Path` (milestone
M4 item 9; `phase6-design.md`, "How the outpost finds new lines"). A
`TailQuery` starts either from an anchor (`TailStart::Anchor`: a range
whose start is the start of the last line read, with a `Fingerprint`, the
text that line and the line before it held) or afresh
(`TailStart::Document`, the text pattern's document range), and says how
many of the last lines to read and how far up to search (`SEARCH_LINES`,
256). The answer, a `Tail`, gives the text as the provider gave it,
padding and line breaks included, so comparisons are exact and the caller
trims:

- `found`: `AtAnchor` when the line before the anchor still holds what it
  held (the anchor's own line may have changed in place, and the caller
  compares it); `Moved(n)` when the anchor's line was found `n` lines up,
  the text having scrolled beneath the anchor (a full scrollback discards
  its oldest lines while a range keeps its row); `NotFound`; or `Afresh`.
- `line` and `previous`: the anchor's line and the line before it, read at
  the anchor; `found_line`: the line where the anchor's line was found, as
  it is now.
- `count`: the lines after the anchor's line (where it was found) to the
  end of the text; afresh, every line.
- `rows`: how many of the last lines were read, up to the number asked
  for; `lines`: their text, read in one call and split at its line breaks,
  oldest first, so a line the terminal wrapped across rows is one line;
  `last_line` and `before_last`: the last line and the one before it, each
  read as a line, the next fingerprint.
- `last`: the last line's range, the next anchor.
- `settled`: whether the text held still while it was read.

The program reads the anchor's line and the one before it. When the one
before it differs from the fingerprint, it walks up a line at a time,
reading each line once, until it finds a line under a line equal to the
fingerprint's previous one. When that previous line is not blank, the
line under it is the anchor's line whatever it holds now, as at the
anchor itself: the last line read is often the one output was still being
written to (the cursor's line, blank or half written), complete by the
next read. Under a blank line the line must also equal the fingerprint's
line (as read, or with the line feed or carriage return and line feed a
last line gains once more text follows it). The strings compare inside
the provider (an `Equal` comparison on two strings, verified against
mockapp). The count is a `Move` by a million lines from the found line,
and the last line is found from where that walk stopped: the line there,
or, when the walk stopped past the final line break, the one before; less
one when the walk ended past the last line's start, as the terminals'
moves do at the end of the text and mockapp's do not, so both read the
same. Finding the last line from the walk itself keeps the count and the
last line in agreement however the text grows meanwhile. The last lines
are read with one `GetText` from the start of the first of them to the
end of the last. Nothing in it reads more than the lines asked for, so a
read's cost does not grow with the scrollback or with the lines read.

A read is settled when the line above where it started reads at the end
as it did when the start was found (with no anchor, the text's first line,
read before and after), and the one read of the last lines ends with the
last line and the one before it as each was read on its own, line breaks
aside (Windows Terminal ends each line's text with one; the console host
gives a line without it but separates lines with one in a longer range).
Live, a flood filling Windows Terminal's or the console host's full
scrollback moves the text beneath every range between two calls, so lines
read one call at a time came back twice or out of order; the outpost sets
an unsettled read aside, and the text change that disturbed it causes the
next read.
The classic implementation makes the same calls one at a time, through
`verbatim-uia`'s text wrappers, so each is counted. A range from before
a terminal switched to or from its alternate screen fails to compare
with the text; the program then fails, the classic implementation fails
the same way, and the caller reads afresh.

Against mockapp's text provider (`crates/mockapp/tests/terminal.rs`), the
two implementations give the same answer afresh, after lines written past
the anchor, after the oldest lines were discarded beneath it, and after
the text was cleared, and a read that finds new output costs one round
trip remotely (`docs/performance.md`, "A terminal output line").

## Fallback rules

How the outpost chooses, per UIA focus (`uia_remote_enrichment` in its
`read.rs`):

- A window without a native UIA provider (arbitration's
  `UiaHasServerSideProvider` verdict) is read through MSAA, so its focus
  never reaches this choice.
- An import that fails (`Error::Import`, a client-side proxy) marks the
  window the focus is in for its lifetime, and the outpost then reads it
  with its own classic walk, without trying; the mark is forgotten when
  the window is destroyed, since its handle may be reused. The window is
  the one known without a call: the focus event's own window, else the
  application's keyboard focus window when the listener captured it.
- A run that fails otherwise is answered by the classic walk for that
  call (`Path::Fallback`) and logged as a warning with the failing
  instruction and its source line.
- A developer setting (`uia.remote_operations` in `settings.toml`, on by
  default) forces the outpost's classic walk; the supervisor passes it to
  each outpost as `--classic-uia`.

Two of the design's rules are not implemented yet: marking a window after
repeated run failures, and retrying a run that exceeds the instruction
limit with a smaller depth limit. Neither has been seen to happen.

A provider whose process has gone and one that times out
(`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`, both as an
`ExecutionFailure`) are not program failures: the classic
implementation would fail the same way, so they are answered as the
outpost answers a gone or unresponsive element.

## A stalled or exited provider

Verified against mockapp's `stall` command, which blocks the window
thread its apartment-threaded providers run on:

- `Execute` blocks while the provider is stalled, exactly as a classic
  call does. The UIA connection timeout (`IUIAutomation2`'s
  `ConnectionTimeout`, which `Uia::within` shortens) does not bound it:
  with a one-second connection timeout, both a program and the classic
  walk waited out a four-second stall and then succeeded.
- UIA's transaction timeout (`IUIAutomation2`'s `TransactionTimeout`,
  20 seconds by default) does bound it. With it at one second, `Execute`
  returned after about 1.1 seconds with `ExecutionFailure` and extended
  error `UIA_E_TIMEOUT`, as the classic walk's call failed with
  `UIA_E_TIMEOUT` in the same time.
- The transaction timeout is process-wide: the last value set through any
  `IUIAutomation` object in the process applies to every client and every
  element, whichever client fetched the element, and reading it back
  through another client returns that value.
- So the outpost's worker can call `Execute` directly, as it makes
  classic calls: a stalled provider holds it no longer than it holds one
  classic hop, and its watchdog covers both the same way. A program is
  one wait where a classic walk against a provider that stalls partway is
  one wait per remaining hop.
- When the provider's process has exited, `Execute` returns at once
  (under a millisecond) with `ExecutionFailure` and extended error
  `UIA_E_ELEMENTNOTAVAILABLE`; the classic walk's first call fails with
  the same code.

## Timings

Measured in a release build against mockapp (whose providers run on one
apartment-threaded window thread, so every provider call inside a
program is itself marshaled to that thread), averaged over 200 calls:

- Five ancestors to the top-level window: 3.4 ms remotely, 25 ms
  classically. Most of the classic time is the last hop, from the
  top-level window to the desktop root, which the classic walk needs to
  find the end.
- Stopping at a known ancestor after three: 4.0 ms remotely, 4.6 ms
  classically.
- A list with its selected child and one ancestor: 3.4 ms remotely, 17 ms
  classically.
- One live property read, for scale: 0.31 ms.

## Tests

Unit tests cover each instruction's bytes and the builder's control flow
offsets, constants, and source locations. `crates/mockapp/tests/remote_ops.rs`
runs both implementations against mockapp's `ancestry.json` fixture and
asserts the same ancestors with the same cached properties and snapshots
for a deep chain, controls with a value and a checked state, a list and
a tab control with selected children (including one selected after
start), a stop at a known ancestor, and the depth limit, with the same
nearest window; that an element that lost the focus returns early; and
the stalled and exited provider findings above.
`crates/mockapp/tests/call_counts.rs` pins the calls and provider hits of
a UIA focus through `focus_ancestry` as the outpost makes it, remotely and
classically.
