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
execution, and algorithms. The algorithms are the focus ancestry, for
milestone M4's terminals `terminal_tail`, for caret reports `caret_read`,
for the text protocol's other requests `text_units`, `text_range`, and
`text_location`, and for object navigation `navigation_step` (layer 3
below). Every UIA path in the outpost that makes a sequence of calls to
the application runs through one of them; "Where remote operations are
not used" lists the rest and why.

## Layer 1: instructions and the builder

- `Opcode` is the table NVDA uses and five more, 109 opcodes
  (`Opcode::ALL`): every general instruction from `0x00` to `0x54`,
  cache requests (`0x4C` to `0x50`) included, and the 19 text range
  methods, whose opcode is
  `(patternId << 16) | (relatedObject << 8) | vtableIndex`
  (`pattern_related_object_method`); and, from Microsoft's
  `RemoteOperationInstructions.h`, which NVDA does not use, the getters
  of the text patterns (`GetTextPattern` and `GetTextPattern2`, whose
  opcode is the pattern's id) and three of their methods
  (`TextPatternGetSelection`, `TextPatternGetDocumentRange`, and
  `TextPattern2GetCaretRange`, whose opcode is
  `(patternId << 10) | vtableIndex`, `pattern_method`). `Comparison`,
  `NavigationDirection`, `PointProperty`, and `RectProperty` are the
  enumerations instructions carry; `Status` is a run's outcome.
- `Instruction` is one instruction with its parameters, and
  `Instruction::encode` writes its bytes. Every instruction has a unit
  test of its exact bytes, and a second test checks that those tests
  cover every opcode.
- `Builder` writes a program. Each register is a `Reg<K>`, where `K` is a
  marker from `kind` saying what it holds: `Element`, `TextRange`, `Int`,
  `Uint`, `Bool`, `Double`, `Char`, `Str`, `Point`, `Rect`, `Array`,
  `StringMap`, `CacheRequest`, `Guid`, `TextPattern`, or `Any` for a
  value whose type is
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
that a focus event is not stale), and otherwise an `Ancestry`. A query
may also name `previous`, the element the caller holds under the focused
element's runtime id, whose `HasKeyboardFocus` is read live too: an
application can give a dead element's runtime id to a new one (File
Explorer does), and NVDA treats a focus as a duplicate only while the
element it compares equal to still has the focus. The `Ancestry`:

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
- `previous_focused`: whether the query's `previous` element has the
  keyboard focus, false when it does not or its read failed, `None` when
  the query named none.

The remote program is one round trip. It reads `HasKeyboardFocus` and
halts when it is false, then reads the previous element's. UIA fails a
whole run before its first instruction when an imported element is gone
(verified against mockapp: `ExecutionFailure` with
`UIA_E_ELEMENTNOTAVAILABLE` and no failing instruction), so no catch block
inside the program can answer for a previous element that died;
`focus_ancestry` runs such a program once more without it and answers
the previous element as not focused when that run succeeds. For a list or tab control it reads the element's
`Selection2FirstSelectedItem` property ignoring its default
(`SelectionPattern2`), fills that item's cache, and keeps it; a provider
without `SelectionPattern2` answers "not supported", and the program then
reads the
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
live read, the selected child through `SelectionPattern2` or else the
Selection pattern
(`Uia::selected_element`, which `Uia::selected_child` also
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
true it runs the program and, when that fails, runs the classic walk for
the same call; with `remote` false it runs the classic walk alone. It
returns the answer with a `Path` (whose `name()` is the word a log line
gives it): `Remote`, `Classic`, or
`Fallback(error)`, the program's error, so the caller can log it (an
`Error::Failed` prints the failing instruction, its opcode, and the Rust
line that emitted it) and stop trying for a window whose import failed.
A program that fails because the provider did not answer within UIA's
transaction timeout, or because its element is gone (once the run
without a gone `previous` element has been tried), is not answered by the
classic walk, which would fail the same way after waiting on a stalled
application a second time: its error is returned, as are the classic
walk's own failures. The classic walk fails when its read of `previous`
or a hop times out, or its read of the selected child times out or finds
an element gone, rather than taking a hop that did not answer as the
root or a selection that did not answer as nothing selected, so a walk that hit the timeout is never reported as a short but
complete ancestry; any other failed hop still ends the walk as the root,
as NVDA's parent read answers no parent. Pinned against mockapp with
`slow` and `stall` (`crates/mockapp/tests/remote_ops.rs`,
`a_focus_walk_that_times_out_fails`).

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
(`TailStart::Document`, the text pattern's document range, or
`TailStart::Text`, the element and its text pattern, from which the
program reads the document range itself, so a fresh read is one round
trip where the other is two), and says how
many of the last lines to read. With `caret` (a `CaretLineQuery`), the
caret and its line are read too, in the same program or, classically,
after the text, as `caret_read` reads them with nothing to compare and no
unit or formatting, and come back as `Tail::caret`, a `CaretAnswer`: a
terminal raises no caret event for every character typed. The answer, a `Tail`, gives the text as the provider gave it,
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
- `head_rows` and `head`: from an anchor, when more lines follow it than
  the last ones read, how many of the first of them were read too, up to
  the number asked for, and their text, read in one call from the start of
  the line after the anchor's, so a flood's start is heard; zero and empty
  otherwise.
- `last`: the last line's range, the next anchor.
- `settled`: whether the text held still while it was read, and
  `scrolled`: whether, if not, the line above where it started changed,
  the text having scrolled beneath the ranges.

The program reads the anchor's line and the one before it. When the one
before it differs from the fingerprint, it searches the whole text above
for the fingerprint by its text, with `FindText` backward, nearest first,
on a range the program made (a copy of a collapsed range with its start
moved to the text's start: `FindText` on an imported range would move the
caller's own anchor). There is no bound in lines, decided with Dickson on
2026-10-07 for a predictable cost (`docs/performance.md`, "A terminal's
upward search"); only matches are bounded, `SEARCH_MATCHES` (64) of them
checked before the fingerprint counts as not found. What is sought
(`Fingerprint::search`), without its trailing white space and line break,
since Windows Terminal's `FindText` matches neither and threw an exception
searching for padding:

- When the fingerprint's line before is not blank, that line, above the
  line before the anchor: a match counts when it starts its line and the
  line holds exactly the fingerprint's line before, and the line under it
  is the anchor's line whatever it holds now, as at the anchor itself:
  the last line read is often the one output was still being written to
  (the cursor's line, blank or half written), complete by the next read.
- Otherwise the anchor's line itself, above the anchor: a match counts
  when it starts its line, the line equals the fingerprint's line (as
  read, or with the line feed or carriage return and line feed a last
  line gains once more text follows it), and the line above it holds
  exactly the fingerprint's blank line before.
- Two blank lines are not searched for: no text search can find them, and
  the fingerprint is not found.

The distance up, `Moved(n)`, is counted as a walk line by line would count
it: the walk to the end of the text from where the fingerprint was found
less the same walk from the anchor, one more when the anchor is inside its
line. The strings compare inside the provider (an `Equal` comparison on
two strings, verified against mockapp). The count is a `Move` by a million lines from the found line,
and the last line is found from where that walk stopped: the line there,
or, when the walk stopped past the final line break, the one before; less
one when the walk ended past the last line's start, as the terminals'
moves do at the end of the text and mockapp's do not, so both read the
same. Finding the last line from the walk itself keeps the count and the
last line in agreement however the text grows meanwhile. The last lines
are read with one `GetText` from the start of the first of them to the
end of the last, and the first lines after the anchor, when more follow,
with one more. Nothing in it reads more than twice the lines asked for, so a
read's cost does not grow with the scrollback or with the lines written.

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
`verbatim-uia`'s text wrappers, so each is counted. Where the provider's
`FindText` fails, the program fails and the classic implementation
answers for that call; its `FindText` fails the same way, and it walks up
from the line before the anchor a line at a time instead, reading each
line once, up to 256 lines, until it finds a line under a line equal to
the fingerprint's line before (the anchor's line, under a blank one, must
also equal the fingerprint's line). Measured against both terminals with a
full scrollback on 2026-10-07, the search by text costs 6 to 10
milliseconds classically wherever the fingerprint is, and about 3 more
remotely for `FindText` on a range the program made (0.2 on an imported
one, which a program cannot search without changing the caller's own),
where the walk line by line took 104, 644, and 1,580 calls and up to 170
milliseconds classically at 10, 100, and 256 lines, and 0.5 to 2.5
milliseconds remotely up to 256 lines, the bound it then had. A range
from before
a terminal switched to or from its alternate screen fails to compare
with the text; the program then fails, the classic implementation fails
the same way, and the caller reads afresh.

Against mockapp's text provider (`crates/mockapp/tests/terminal.rs`), the
two implementations give the same answer afresh, after lines written past
the anchor, after the oldest lines were discarded beneath it (by 300
lines, beyond the 256 the search once covered), past matches that are
part of a longer line or under the wrong line, for an anchor's line under
a blank line, and after the text was cleared; a provider whose `FindText`
fails is answered by the walk; and a read that finds new output costs one
round trip remotely (`docs/performance.md`, "A terminal output line").

## Layer 3: the caret read

`caret_read_remote` and `caret_read_classic` share one signature
(`CaretReadFn`), and `caret_read(query, remote)` chooses between them as
`focus_ancestry` does, answering with a `Path` (milestone M4 items 3 and
7; `phase6-design.md`, "Caret responsiveness"). One difference: a run
that failed because the provider's process has gone or did not answer in
time (`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`) is returned as the
error without running the classic reads, which would meet the same
failure, a second wait for a stalled application. A `CaretQuery` gives
the element with the text (a program starts from it), its text pattern
and `TextPattern2` (the classic reads use them), where the caret was
known to be (`since`, a `RangeEnd`: a range and which of its ends), the
selection's ends as they were known, a unit to read at the caret besides
the line (a word, a paragraph, a page), whose formatting to read
(`FormatSpan`: the character at the caret, that unit, or the line),
which attributes to read (`attributes`, an `Attributes` set of
`TextAttribute`s: the annotation types for spelling and grammar errors,
the font's name, size, and weight, italic, the underline and
strikethrough styles, the foreground and background colors, the bullet
style, and the link attribute), and which of them are being learned
(`learning`: those whose support the caller does not know yet). The
`CaretAnswer`:

- `caret`, a range whose start is the caret, and whether it is known to
  be collapsed; `selection`, the selected range when text is selected.
  The caret is the selection's first range, collapsed, when nothing is
  selected; with text selected, `TextPattern2`'s caret range where the
  provider has it, else the selection's start; with no selection at all,
  the start of the document range.
- `moved`: the caret is not where `since` says; `selection_moved`: the
  selection's ends (both the caret when nothing is selected) are not the
  known ones.
- `line` and `unit`: each a `UnitRead`, the unit's range, its text, and
  how many UTF-16 code units of it come before the caret.
- `runs`: the formatting, stretch by stretch from the span's start, each
  a length in UTF-16 code units and its `RunAttributes`: whether it is a
  spelling or a grammar error (annotation types 60001 and 60002, a single
  integer or an array of them), and the font name, size, weight, italic,
  underline style, strikethrough style, color, background color, bullet
  style, and whether it is a link (any value of the link attribute, the
  range it leads to; false for none), each `None` when not read, not
  supported, mixed, or of another type. A character's one stretch covers
  it whole, and its length is not read.
- `unsupported`: of the `learning` attributes, those the span answered
  "not supported" for, the rest being supported; `None` when nothing was
  learned, as from a character's read or an empty line's, which say too
  little about the provider.

The program imports the element and gets its text pattern with the
pattern getter instructions (Microsoft's `GetTextPattern`, whose opcode
is the pattern's id, 10014), reads the selection with the text pattern's
`GetSelection` and, only when text is selected, the caret with
`TextPattern2`'s `GetCaretRange` (pattern methods, opcode
`(patternId << 10) | vtableIndex`; both added to the instruction table
from Microsoft's `RemoteOperationInstructions.h`). It compares the caret
with the imported `since` range and the selection with the imported old
ends, expands copies of the caret to the line and the unit, and reads
their text and the text before the caret, whose length is the offset.
Formatting is read by UIA's format unit, as NVDA reads it: from the
span's start, a copy's end moved one format unit on, cut at the span's
end, its text's length and each attribute read, until the span's end or
`MAX_RUNS` (64) stretches. A stretch with an attribute that answers UIA's
"mixed" (the program's `IsMixedAttribute` test, `verbatim_uia::is_mixed`
classically) is not appended: it is walked again the same way by words,
and a mixed word by characters, as NVDA walks a mixed stretch by finer
units, so a provider whose format unit does not end where an attribute
changes (Windows Terminal's and the console host's do not, for italics,
`docs/text-attributes.md`) still has each part read with its own value;
the 64 stretches bound the whole walk. Each value is appended as the
provider gave it, UIA's sentinels included, and the caller reads it by
its type, so a value of the wrong type, "not supported", or "mixed" for a
single character, comes back as none; the program spends no instructions
on testing types, which is what let it read eleven attributes in fewer
instructions than it read seven (`docs/performance.md`, "The instruction
limit"). Before the walk, the span is asked for its annotation types,
and its stretches are asked for them only when the span has some (Windows
11 Notepad answers "not supported" for text without annotations); a span
with none and nothing else to read is one stretch, never walked. Each
`learning` attribute is asked of the span too, its "not supported"
answer returned as `unsupported`; the annotation types are never
learned. Verified on Windows 11 26200
against Windows 11 Notepad (`RichEditD2DPT`), whose provider runs
programs and reports a misspelt word's annotation types as an array
holding 60001, and splits its format units at the error's ends; the
remote and classic reads gave the same answer there, and do against
mockapp (`crates/mockapp/tests/text.rs`).

The classic implementation makes the same reads one call at a time,
except that a collapsed caret serves as its own point where the program
copies it, and that a stretch's attributes are read together, in one
`IUIAutomationTextRange3::GetAttributeValues` call, as NVDA reads a
range's formatting (`verbatim_uia::text::TextRangeExt::attributes`), one
call each only where the range cannot answer that. An attribute whose
read fails is not supported on both paths: UIA answers
`GetAttributeValues` with "not supported" in its place, and the program
gets the same, checked against a mockapp provider whose `IsItalic` read
fails (`a_failing_attribute_is_not_supported` in mockapp's `text.rs`).
Classically the span's annotation types and the attributes being learned
are asked in one `GetAttributeValues` call. Both paths are checked
against mockapp's `formatting.json`, which supports every attribute, and
its `text.json`, which supports none of strikethrough, background,
bullets, and links (`every_attribute_is_read_both_ways` and
`unsupported_attributes_are_found_both_ways`).
Both read the whole answer on every call, so each check of a caret
key's watch for evidence costs one round trip remotely
(`docs/performance.md`, "A caret move, UIA").

With a previous selection, the answer also carries the selection's
changes (`changes`, `SelectionTextChange`s), worked out by the text
protocol's rule and read only when the selection moved: two selections
that neither overlap nor touch are the old text unselected and the new
selected; otherwise the text between the new and the old start, then
between the old and the new end, each selected or unselected by the
direction it moved, empty stretches left out, each read up to the query's
`max_change_text`. The program compares the ends and reads the stretches
inside the provider, so a selecting key's answer is one round trip with
the text of what changed (8 before, `docs/performance.md`, "A caret key
that selects, UIA").

## Layer 3: the text protocol's other reads

Three named operations answer the rest of the text protocol, each with a
remote program, a classic implementation behind the same signature
(`TextUnitsFn`, `TextRangeFn`, `TextLocationFn`) that makes the calls the
outpost made before, and an entry point that chooses as `caret_read`
does, gone and timed-out providers included. Each starts from a
`TextFrom`, a point as the protocol names it:

- `Caret`, `SelectionStart`, `SelectionEnd`: read as the caret read reads
  them, in the same program (the caret read's selection and caret
  instructions), once however many points use them.
- `Start` and `End`: the document range collapsed to one end; the program
  gets the text pattern from the element with the pattern getter
  instruction and reads the range itself.
- `At(Position)`: a range and the end of it the caller holds, imported.
- `After { from, prefix, counts }`: a point some text after a held one,
  the text between them being `prefix`. Each of `counts` (UTF-16 code
  units, code points, grapheme clusters: a provider's character may be any
  of them) is tried in turn, moving a copy's end by that many characters
  and comparing the text passed with `prefix`, a string comparison inside
  the provider, until one matches; the last tried is kept when none does.
  This is how a position Core found inside a chunk is resolved without a
  round trip per try.

Every point is returned (`FoundPoint`) so the caller can remember it.

- `text_units` (`UnitsQuery`, `UnitsAnswer`): from the point, an optional
  `Movement` (`By(unit, count)` from the start of the unit containing the
  point, collapsed before it moves as the review cursor moves, and left
  as the move leaves it, since UIA keeps a collapsed range collapsed when
  it moves, its count negative for a backward move even from a provider
  that answers one with a positive count, as NVDA corrects it; or
  `Document(count)`, to an end, saying it moved when the point was not
  there), then the unit containing the point reached: its
  range, its text up to `max_text`, the point's offset in it (zero when
  the movement was by that unit, landing on its start), and its language
  (`Culture`, a locale id turned into a BCP 47 tag with
  `verbatim_uia::text::locale_name`; an unsupported value is null). With a
  `count` above one it reads on, a unit at a time, a collapsed copy of the
  last one moved by one unit and expanded in place, until it has `count`,
  the text read reaches `max_total`, or a move by one does not move
  (`ended`: the last unit read is the text's last). That is say-all's
  batch: twenty lines ahead in one round trip. The language of a batch is
  read once, over a range from its first unit's start to its last one's
  end, and unit by unit only when that answers UIA's "mixed" (both
  sentinels are told apart, `verbatim_uia::text::Language`), so a batch in
  one language costs one `Culture` read rather than one per unit.
  Microsoft's guidance on `ExpandToEnclosingUnit` is that it normalizes a
  range from its start alone, so a unit is expanded from a copy whose
  start is the point, never collapsed first.
- `text_range` (`RangeQuery`, `RangeAnswer`): two points (or one, for
  moving the caret), ordered by comparing them, and the text between them
  read up to a limit or selected. A selection the provider refuses is
  caught inside the program (`try_catch`), answered as not selected, as
  the classic `Select` failure is.
- `text_location` (`LocationQuery`, `LocationAnswer`): the character at
  the point and its bounding rectangles, the first rectangle's left and
  top. The program returns them as an array of doubles (verified against
  mockapp, which now reports one rectangle per character on a fixed grid).

Against mockapp (`crates/mockapp/tests/text.rs`), the two implementations
of each agree: a line at the caret, the next line, the end of the text,
every line ahead with the end found, a word from a point found by its
text, a range read in either order, a selection made and read back, and
locations.

## Layer 3: an object-navigation step

`navigation_step(uia, query, remote)`, with `navigation_step_remote` and
`navigation_step_classic` behind it (`NavigationStepFn`), takes one
raw-view step (parent, next or previous sibling, first child) from an
element and finds the element's nearest window, as the outpost's
navigation needs to correct the neighbor's backend. The program reads the
element's own window handle from its cache, else walks raw-view parents
reading `NativeWindowHandle` until one is set, then navigates and fills
the neighbor's cache inside the provider, choosing a cache request per
`LEFT_OUT_WHEN_UNSUPPORTED` combination as the focus ancestry does. The
classic implementation is `nearest_window_handle` and a tree walker step
built with the cache, one call each; a step that fails because the
element is gone is an error, any other failure no neighbor.

A program's walk ends at the process's top-level window, whose parent it
reports as null where a tree walker reports the desktop, so the outpost
takes a step from a top-level window classically. Against mockapp's
ancestry fixture (`crates/mockapp/tests/remote_ops.rs`), both give the
same neighbor with the same cached properties and snapshot, and the same
window, for every direction and at an edge.

## Fallback rules

How the outpost chooses, per UIA focus (`uia_remote_enrichment` in its
`read.rs`), and the same way for every other named operation (the caret
read and the text reads through `UiaText`, a terminal's tail, a
navigation step):

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

## The instruction limit and counting

UIA lets one run execute 10,000 instructions and stops it past that with
`Status::InstructionLimitExceeded`, measured against mockapp on Windows
11 build 26200 (`crates/mockapp/tests/instruction_limit.rs`, which pins
it). `counting` measures programs against it: `counting::start()` turns it
on for the thread, every program the thread then runs is run in its
counting form, and `counting::stop()` returns how many of each program's
own instructions its run executed. The counting form puts an `Add` of one
to a register of its own before each instruction, doubles each jump's
offset less one so the jump lands on the `Add` before its target, and asks
for the register with the results, so its results and effects are the
program's; it executes twice as many instructions and two more, and so
counts only programs under half the limit. It is never on in Verbatim
itself. Each program's worst case against mockapp is pinned by the same
test and recorded in `docs/performance.md`, "The instruction limit": every
one stays under half the limit but the caret read's walk of a span whose
every format stretch is mixed, about 7,700 at its 64 stretches, which
runs under it; a run that exceeded it would be answered classically for
that call. Programs are not resumable (Dickson, 2026-10-07): their work is
bounded by their queries.

A provider whose process has gone and one that times out
(`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`, both as an
`ExecutionFailure`) are not program failures: the classic
implementation would fail the same way, so they are answered as the
outpost answers a gone or unresponsive element. The entry points of the
focus ancestry, the caret read, the text reads, and the navigation step
return them without running the classic calls, which would wait a second
time on a stalled application; a navigation step from an element
reported gone is then searched for by runtime id, as the classic step
always was, and a focus whose ancestry failed is reported with its
containers unknown.

### Where NVDA declines remote operations, and whether it applies here

Verbatim tries a remote operation wherever one is available and falls
back per call. NVDA instead makes each call site choose. Every place in
NVDA (as vendored, 2026-10) where a remote operation exists or could be
used but the classic path is taken under some condition, with the reason
from its code and history, and whether the reason applies to Verbatim:

- The global gate (`UIAHandler/remote.py`, `isSupported`): remote
  operations only on Windows 11 or later, a major-version check added
  with the first Word extensions (NVDA #13283; a revert, #13350, was for
  build problems, not runtime ones). Does not apply: Verbatim runs only on
  Windows 11, and a Windows without the API fails creating the operation
  (`Error::Unavailable`), which falls back per call.
- Browse mode's heading search in Word (`UIAHandler/browseMode.py`):
  remote on Windows 11, the classic paragraph walk otherwise; no runtime
  fallback. A long document exceeds the instruction limit, so NVDA reruns
  the operation up to 20 times, resuming from values kept between runs.
  The limit applies in principle: every Verbatim program is bounded by its
  query (64 format runs, 16 units ahead and 32 K code units of text, 256
  lines searched, the ancestor depth limit), and a run that exceeds the
  limit anyway is answered classically for that call (`Path::Fallback`).
  No guard proposed; if a provider is ever seen to exceed it, the
  design's unimplemented retry with a smaller bound is the fix.
- Word's custom attributes through its extended text range pattern
  (`NVDAObjects/UIA/wordDocument.py`): the text column number is never
  fetched because it crashes Word 16.0.1493 and later (#13511, #13503);
  the expand and collapse state only from Word 16.0.18226, since earlier
  versions crash on that attribute (#18279); a page number of -1 is
  ignored as not yet available (#19424); and the extension is checked for
  inside the operation because Windows Mail's Word control lacks it
  (#16689). Does not apply: Verbatim calls no provider extensions
  (`CallExtension`); its programs make only the calls a classic read
  makes. A guard would be needed only if Word's extensions were adopted,
  keyed to Word's product version and the extension's support check, as
  NVDA keys them, never to a window title.
- Word's sentence movement through extensions (`MoveBySentence` and
  relatives): classic object-model movement on older Office or Windows
  (#19367), and a collapsed range at the document's end left alone
  because Word's extensions wrap to the start (#20498). Does not apply:
  Verbatim uses no extensions, and UIA has no sentence unit.
- `IsOpcodeSupported`: wrapped by NVDA but never called. Verbatim checks
  every opcode of a program after import and falls back when one is
  missing, which is stricter.
- Timeouts: NVDA sets none for remote operations. Verbatim relies on
  UIA's process-wide transaction timeout and the outpost's watchdog, as
  for classic calls (above).
- A shutdown hang in Microsoft's old remote operations library (#16072).
  Does not apply: Verbatim calls Windows' API directly, and outposts end
  with their job.

No crash, hang, or wrong answer in NVDA's history is tied to remote
operations against Windows Terminal, the console host, Chromium, Excel,
or the standard text controls; NVDA does not run programs against them.
So trying remote operations wherever they are available, with the
per-call fallback, is a safe default, and no guard is proposed.

## Where remote operations are not used

Every UIA path in the outpost that makes more than one call in sequence
runs as a named operation above, except these:

- Reading the focused element (`GetFocusedElementBuildCache`): one call,
  and the element is what a program would start from.
- A node's text patterns, fetched once per node the first time its text
  is read (`TextPattern2`, else `TextPattern`, one or two calls): the
  classic reads need the pattern objects, and the programs get the
  pattern from the element themselves. Making the fetch lazy, so a
  remote read never makes it, is a possible follow-up.
- A selection event in a list the focus controls
  (`Uia::controlled_descendant`): the `ControllerFor` relation and a
  `FindFirst` for the element under each controlled root, two calls for
  the usual one root. There is no `FindFirst` instruction, and a program
  walking the list to find the element would cost the provider more than
  the search does.
- Activation (`Uia::activate`): each pattern is fetched live and its
  method called, two calls for an `Invoke`. The pattern methods
  (`Invoke`, `Toggle`, `Select`) have no instructions.
- The menu item's legacy checked state: one call, read for that element
  alone.
- A dialog's own text through UIA: already one call
  (`BuildUpdatedCache` over the children).
- The ancestors query (`Query::Ancestors`), a full classic walk: nothing
  sends it today.
- The tree dump (`Query::DumpTree`), a diagnostic.
- The arbitration probe and the checks of a console's and a Windows Forms
  window's provider: window messages and one-time checks made before the
  window's backend is known.

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

The text reads and the navigation step, each against its classic calls,
are timed in `docs/performance.md`, "Wall-clock gain".

## Tests

Unit tests cover each instruction's bytes and the builder's control flow
offsets, constants, and source locations. `crates/mockapp/tests/remote_ops.rs`
runs both implementations against mockapp's `ancestry.json` fixture and
asserts the same ancestors with the same cached properties and snapshots
for a deep chain, controls with a value and a checked state, a list and
a tab control with selected children (including one selected after
start), a stop at a known ancestor, and the depth limit, with the same
nearest window; that an element that lost the focus returns early; that
a navigation step reads the same both ways; and the stalled and exited
provider findings above.
`crates/mockapp/tests/call_counts.rs` pins the calls and provider hits of
a UIA focus through `focus_ancestry` as the outpost makes it, of
navigation steps, of caret moves, caret reports, and a caret key's watch through
`caret_read`, and of the review cursor's line and word reads, say-all's
read ahead and caret move, the caret's location, the selected text, and a
selecting key's answer, remotely and classically;
`crates/mockapp/tests/text.rs` checks that the two caret reads agree,
formatting and selection changes included, and that the two
implementations of each text read agree; `crates/mockapp/tests/terminal.rs`
pins a terminal's reads.
