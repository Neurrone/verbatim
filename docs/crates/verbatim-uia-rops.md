# verbatim-uia-rops

UIA remote operations: small programs that run inside a UIA provider's
process, so work that would take one cross-process round trip per step
takes one in all. Why Verbatim uses them is in `docs/architecture.md`,
section 4, "UIA"; how their caches are filled is in
`docs/design/uia-shapes.md`, section 7. The crate calls Windows' own API,
the WinRT class
`Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, as NVDA
does, not Microsoft's `microsoft-ui-uiautomation` library. It is
Windows-specific and GPL, so it ports NVDA's framework
(`source/UIAHandler/_remoteOps/`) freely, with attribution in each
ported file; where it follows Microsoft's MIT-licensed
`RemoteOperationInstructions.h`, that file carries Microsoft's notice.

The crate has three layers: the instruction set and a typed builder,
execution, and algorithms. Each algorithm is a named operation with a
remote program, a classic implementation of the same signature (a type
named `...Fn`, with `..._remote` and `..._classic` functions), and an
entry point that chooses between them and answers with a `Path`. Every
UIA path in the outpost that makes a sequence of calls to the application
runs through one of them; "Where remote operations are not used" lists
the rest.

## Layer 1: instructions and the builder

- `Opcode` is the table NVDA uses and five more, 109 opcodes
  (`Opcode::ALL`): every general instruction from `0x00` to `0x54`,
  cache requests included, and the 19 text range methods, whose opcode is
  `(patternId << 16) | (relatedObject << 8) | vtableIndex`
  (`pattern_related_object_method`); and, from Microsoft's
  `RemoteOperationInstructions.h`, which NVDA does not use, the text
  pattern getters (`GetTextPattern` and `GetTextPattern2`, whose opcode is
  the pattern's id) and three of their methods (`TextPatternGetSelection`,
  `TextPatternGetDocumentRange`, and `TextPattern2GetCaretRange`, whose
  opcode is `(patternId << 10) | vtableIndex`, `pattern_method`).
  `Comparison`, `NavigationDirection`, `PointProperty`, and `RectProperty`
  are the enumerations instructions carry; `Status` is a run's outcome.
- `Instruction` is one instruction with its parameters, and
  `Instruction::encode` writes its bytes. `OperandId` is a register's id,
  by which an instruction names it and a result is read back; `TypeTest`
  is the instruction that tests whether a register holds a value of one
  type. Every instruction has a unit test of its exact bytes, and a
  second test checks that those tests cover every opcode.
- `Builder` writes a program. Each register is a `Reg<K>`, where `K` is a
  marker from `kind`: `Element`, `TextRange`, `Int`, `Uint`, `Bool`,
  `Double`, `Char`, `Str`, `Point`, `Rect`, `Array`, `StringMap`,
  `CacheRequest`, `Guid`, `TextPattern`, or `Any` for a value whose type
  is known only at run time, which `Reg::assume` names. Arithmetic takes
  only `Numeric` kinds, ordering comparisons only `Ordered` ones, and
  indexes only `Index` ones, so a program that compares an element with
  an integer does not compile.
- Constants (`Builder::int`, `uint`, `bool`, `string`) are gathered in a
  section ahead of the program and shared by every use of the same value,
  as NVDA's const section is; `new_int` and its siblings make variables.
- Control flow takes closures, which run once at build time to emit their
  bodies, and computes its own jump offsets: `if_`, `if_else`, `while_`
  (with `break_loop` and `continue_loop`), `try_catch`, and `halt`. The
  layouts are NVDA's: a loop's continue target is the condition just
  after its block, whose `ForkIfFalse` jumps to the `EndLoopBlock`, and a
  `ContinueLoop` ends the body; a try block's catch target reads the
  operation status and resets it to zero before the catch body runs.
- `Builder::import_element` and `import_text_range` bring in the objects
  a program starts from; `add_to_results` asks for a register's value
  after the run; `finish` ends the program with a `Halt` and returns the
  `Operation`.
- Every emitting method is `#[track_caller]`, so a failing instruction's
  index maps back to the Rust line that emitted it
  (`Operation::location`), as NVDA maps it to the Python line. NVDA's
  local emulator is not ported: tests run against mockapp's real provider
  instead.

### The encoding

The layout is not documented officially; this one is checked against
Windows 11. A program is the `u32` version 0 followed by its
instructions, with no padding. An instruction is its `i32` opcode and
then its parameters in order, little-endian: an operand is a `u32`
register id, an offset or enumeration an `i32`, a boolean one byte, a
character two bytes, a double eight bytes, and a string a `u32` length
that counts a terminating null, followed by that many UTF-16 code units.
Jump offsets count instructions from the jumping one, and a failure's
location is an instruction index. Two of NVDA's instruction definitions
are wrong and Microsoft's right: `RemoteArrayRemoveAt` and
`RemoteStringMapRemove` write a result (the removed value) before their
target, which NVDA's omit.

## Layer 2: execution

`Operation::execute` creates a `CoreAutomationRemoteOperation`, imports
the program's elements and text ranges (an `IUIAutomationElement` casts
to the WinRT `AutomationElement` by `QueryInterface`), checks with
`IsOpcodeSupported` that the provider supports every opcode the program
uses (known only once something is imported, since it depends on the
provider's process), registers the requested results, and runs
`Execute`, the one cross-process round trip, counted as one UIA call on
the thread's `verbatim_uia::calls` count. The steps before it are
answered in Verbatim's process in under a microsecond each and are not
counted. `Operation::bytecode`, `len`, `is_empty`, and `instruction`
expose the program for tests.

A program is built per call, with that call's values as constants, but
for the two a typing user runs most, a terminal's screen and the caret
read, whose building costs 22 to 78 microseconds against 0.4 to 1.1
milliseconds for `Execute`. Those are kept per thread (`ProgramCache`, at
most 16 of each, all forgotten when full) by a key naming everything the
program was built from but its imports, and a query with the same key
runs the kept program bound to its own imports (`Program::bind`, which
checks they are as many and of the same kinds). The cache is per thread
because a remote operation and its imports are per client and thread. A
program whose run on the thread found every opcode supported skips the
support check on later runs; any failed run has the next check again.
`work::programs_built` and `work::support_checks` count both per thread
for tests.

A failed run is an `Error`:

- `Unavailable`: Windows lacks the API.
- `Import`: an import failed. An element served by a client-side proxy
  (UIA's MSAA proxy inside Verbatim's own process) fails with
  `E_UNEXPECTED`, since there is no provider process to run in.
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

`Outcome::get` converts a requested register to Rust through the `Read`
trait each kind implements: `Int` to `i32`, `Uint` to `u32`, `Bool` to
`bool`, `Double` to `f64`, `Char` to `u16`, `Str` to `String`, `Element`
to `Option<IUIAutomationElement>` (with the cache the program filled),
`TextRange` to `Option<IUIAutomationTextRange>`, `Array` to `Vec<Value>`,
and `Any` to a `Value`. `Outcome::has` says whether a register was
returned, and `Outcome::get_utf16` gives a `Str` register as the
provider's UTF-16, never decoded, for text the caller trims before it
decodes it.

## Layer 3: the focus ancestry

`focus_ancestry(uia, query, remote)`, with `focus_ancestry_remote` and
`focus_ancestry_classic` behind it (`FocusAncestryFn`), reads a focused
element's raw-view ancestors and what a focus report needs besides. A
`FocusQuery` holds:

- `element`, a `verbatim_uia::Cached<FocusInput>`, whose control type and
  window are in its cache (decision 8 of `docs/design/uia-shapes.md`);
  `known`, the runtime ids the caller already has; and `depth_limit`;
- `fill`, the `verbatim_uia::Fill<Ancestor>` every ancestor is filled by,
  and `element_fill`, the element's and its selected child's, which adds
  the node's place since each is read alone (decision 9 of
  `docs/design/uia-shapes.md`);
- `require_focus`, for a live read of `HasKeyboardFocus` (NVDA's check
  that a focus event is not stale), and `previous`, the element the
  caller holds under the same runtime id, whose `HasKeyboardFocus` is
  read live too, because an application can give a dead element's
  runtime id to a new one (File Explorer does);
- `refresh`, for the focused element's own cache filled afresh in the
  same round trip, so the focus listener reports its value, states, and
  details without a second call;
- for the classic walk only, `out_of_time`, checked before each call
  after the first; `before_call`, told each call's name as it begins so
  the caller can time it; and `nearest_window` and `value_available`,
  answers the caller already has, each saving a call.

It answers `FocusAncestry::NotFocused` when `require_focus` is set and
the element does not have the focus, and otherwise an `Ancestry`:

- `ancestors`: the raw-view parents, nearest first, each a
  `Cached<Ancestor>`, ending at the top-level window of the element's
  process or below the process boundary
  (`docs/design/listener-without-connections.md` defines it;
  `verbatim_uia::parent_across_process_boundary` judges it by local
  calls).
- `met_known`: which known runtime id the last ancestor has, when the
  walk stopped there; `depth_limited`: the walk stopped at the depth limit
  with ancestors left; `out_of_time`: the classic walk stopped when the
  query's time was up.
- `failed_hop`: how a step to a parent failed below the top, a
  `FailedHop` (`verbatim_uia::FailedHop`, re-exported): `Error` with the
  failing code, or `NoParent`. `Ancestry::ancestors_unknown` is true for a
  walk out of time or with a failed step, and callers report such a focus
  with its ancestors unknown, as NVDA falls back to the window hierarchy
  when its parent read fails.
- `selected_child`: for a list or tab control, the first selected child,
  which the outpost reports as its node; for a combo box without the
  `Value` pattern, its first selected item, which is its value
  (`verbatim_uia::map::value_from_selection`).
- `window`: the native window handle of the element or of its nearest
  raw-view ancestor that has one (NVDA's `getNearestWindowHandle`).
- `previous_focused`: whether `previous` has the keyboard focus, `None`
  when the query named none.
- `element`: with `refresh`, the focused element's fresh cache.
- `legacy_state`: for a menu item, its legacy MSAA state bits, read by
  the remote program, `Some(None)` when unsupported; always `None` from
  the classic walk, whose caller makes one live read. A menu item that no
  pattern makes checkable, as a Windows Forms one, says it is checked only
  there.

Only reaching the top ends a walk as known. In the classic view a
top-level window's parent is the desktop root, so the classic walk stops
below the root, and any step below it that fails or answers none sets
`failed_hop`. Inside a program the parent of a top-level window, and of a
window below the process boundary, is null, so the program's walk ends at
any null parent and then checks, with local calls
(`verbatim_uia::is_top_level_window`,
`verbatim_uia::parent_across_process_boundary`), that the last element
read has as its window one of those two; otherwise it sets `NoParent`. An
element's window is the one its runtime id `[42, handle]` names, since a
read inside the provider can leave the handle out (`docs/uia-bugs.md`,
"Inside a remote operation, conhost's window has no handle or title"), or
the one its cached `NativeWindowHandle` reports. A provider whose
`Navigate` fails reaches the client as no parent.

The remote program reads `HasKeyboardFocus` and halts when it is false,
then reads the previous element's. UIA fails a whole run before its first
instruction when an imported element is gone, so `focus_ancestry` runs
the program once more without a gone `previous` and answers it as not
focused. For a list or tab control the program reads
`Selection2FirstSelectedItem` (`SelectionPattern2`), else the `Selection`
property; for a combo box it asks `IsValuePatternAvailable` itself. Then
it walks with `Navigate` to the parent, filling each ancestor's cache
inside the provider and stopping at the first whose runtime id, turned
into a string with `Stringify` (`[42,14681214,4,5]`), is in a string map
of the known ids; `runtime_id_key` makes the same string on Verbatim's
side. While the window is unknown each ancestor's `NativeWindowHandle` is
read as the walk passes it, and a walk that stopped before finding one
goes on up, returning nothing more, until it does.

The classic walk is the focus listener's read, which makes no remote
operation (`docs/design/listener-without-connections.md`, section 2), and
the outposts' fallback: the same live reads, the selected child through
`Uia::selected`, and one `GetParentElementBuildCache` per ancestor filled
by `Fill::request`. It returns elements, not the filtered snapshots of
`Uia::ancestors`: the presentable-ancestor filter, the switch to MSAA,
and the splice with the previous focus's chain stay with the outpost,
which applies them to either implementation's elements with
`Uia::ancestors_from`. It checks the process boundary before each step,
and after each step drops and stops at a parent whose own window is
another process's, as an owner shown between the check and the step
makes it. Every call goes through `verbatim-uia`'s safe wrappers, so it
is counted, and the module has no `unsafe` code.

The entry point runs the program when `remote` is true and, when that
fails, the classic walk for the same call; with `remote` false it runs
the classic walk alone. Its `Path` (whose `name()` is the word a log line
gives it) is `Remote`, `Classic`, or `Fallback(error)`, so the caller can
log the program's error and stop trying for a window whose import
failed. A program that timed out or whose element is gone is not answered
classically, which would wait on a stalled application a second time:
its error is returned. The classic walk itself fails when its read of
`previous`, a hop, or the selected child times out, or the selected child
is gone, so a walk that hit the timeout is never reported as a short but
complete ancestry. NVDA makes each call site check `remote.isSupported()`
and run its own classic code instead; here the choice and the fallback
live in one function, so a failure is handled the same way everywhere.

### Cached properties filled remotely

A cache filled by a remote program stores a property's default where a
locally built cache stores "not supported", so a read that ignores
defaults cannot tell the two apart (`docs/design/uia-shapes.md`, section
7). `ValueIsReadOnly` and `RangeValueValue` are therefore read only when
`IsValuePatternAvailable` and `IsRangeValuePatternAvailable` say the
pattern is there. `IsDataValidForForm` has no pattern, so the program
leaves it out of the cache of an element that does not support it: since
`PopulateCache` replaces an element's cache, it builds one request per
combination of such properties (`verbatim_uia::LEFT_OUT_WHEN_UNSUPPORTED`,
one property, so two requests), picks one per element after an
`IsNotSupported` test, and returns which were left out
(`verbatim_uia::Absent`), so the element, wrapped as a `Cached<S>` with
that record (`Fill::claim`), answers the property as "not supported"
without asking UI Automation (decision 3 of `docs/design/uia-shapes.md`).
Every program here that fills a cache builds its requests from the
`Fill`'s properties and patterns (`RemoteCache`), and the classic
fallbacks fill with its request, so both come from one shape.

## Layer 3: a terminal's snapshot

`terminal_snapshot(uia, query, remote)` (`TerminalSnapshotFn`) is the
read of `docs/design/terminal-model.md`, "The read", which gives the
reasons for its fields: the outpost reads every focused terminal with
it, and `cargo xtask regenerate-terminal-conformance-expectations` reads
the real terminals with it. A `SnapshotQuery` names the element and its
text pattern; `start`, where the last read said to start (none for a
first read); `screen_rows`, the screen's height as last found, 0 when not
known; `whole`, to read the whole text instead; `above`, how many rows
above the start to begin (`docs/design/terminal-model.md`, "Positions");
`caret`, a `CaretLineQuery`, for the caret and its line; and `colours`,
for the screen's colours, with the screen's width as last found.

The unit of reading is one `GetText`: both terminals answer one from one
locked state of their buffer, while separate calls are not atomic with
each other. So the snapshot's text is the only thing anything spoken or
counted comes from, and every other field is a hint. The `Snapshot` gives
text as the provider gave it, padding and line breaks included (the
conformance expectations record each provider's,
`docs/crates/verbatim-term-sim.md`):

- `text`: from the start to the document's end, in one call. The start is
  the query's `start` when it lies no lower than where the next read
  starts (`from_start`); otherwise it is refused (`shrank`), since the
  text was cleared or switched, and the snapshot starts where the next
  read will. A first read starts at the screen's top, a `whole` read at
  the document's start, and the snapshot then starts `above` rows higher,
  as far as the document's start allows.
- `above`: how many rows the snapshot holds above its start.
- `screen`: the first visible range's text.
- `next_start`: the document's end moved back by `screen_rows` lines, the
  top of the screen output is written to, wherever the view is scrolled.
- `at_document_start` and `alternate`: whether the snapshot, and the
  screen, start at the document's start.
- `first_row`: the document's first row; when it reads otherwise, the
  history rotated.
- `caret_rows_to_end`: how many lines the caret is from the document's
  end.
- `caret`: when asked, the caret and its line, and whether it is not where
  the query's `since` says. A terminal refuses to compare the caret with a
  `since` on the other screen, which means it moved; the comparison runs
  in a try block, so the refusal does not fail the read.
- `colours`: when asked, each screen row's runs of one foreground and
  background (`ColourRun`: first column, length in cells, and two colours
  as `0x00BBGGRR`), the form the terminal reading rules take
  (`docs/design/terminal-model.md`, "Lists and colours").

Without colours the program has no loop: 50 instructions for a first read
and 90 with a start and the caret.

### Rows by their place

`terminal_rows(uia, query, remote)` (`TerminalRowsFn`) reads runs of a
terminal's history rows by their place. A `RowsQuery` gives the element,
its text pattern, the start the snapshot was asked for, and the runs,
each how many rows above that start it begins and how many it holds, at
most `MOST_RUNS` (16). For each run the program moves a collapsed copy of
the start up and its end down by the run's rows and reads its text; a run
the document's start came before is read as nothing. It then reads the
document's first row, so the caller can tell whether the buffer moved
since the snapshot. `RowsAnswer` holds each run's text, UTF-16 as given,
and the first row. The program has no loop, ten instructions a run and
twelve besides, and is kept per thread for each list of runs.

### The colours

A cell's colour attribute is its own colour; a stretch of cells answers
its first cell's colour when every cell but its last has it, and UIA's
"mixed" otherwise, since neither terminal compares a stretch's last cell;
their format unit stops only at rows; and Windows Terminal does not
implement `FindAttribute`. So a stretch is of one colour exactly when its
answer is a number and its last cell, read alone, answers the same: four
attribute reads, whatever the stretch's length.

The read (`terminal_colours.rs`) takes rows as wide as the screen's top
row as the provider expands it to a line (`row_width`), since a screen
written to its last column on every row reads in the console host as one
line. Each row is checked whole; a row of several colours is checked in
chunks of `COLOUR_CHUNK` (16) cells; a chunk of several colours is read
cell by cell. A check's last cell is made as the conformance runner makes
a cell. On the buffer's last row moving by characters stops a cell short
of the width, which the move to the row's end reveals, and that row is
then read cell by cell. The rows stop at the earlier of the document's
end and the screen's, and at most at the screen's height at that width
(`screen_height`).

The snapshot's own program reads the first rows after everything else. A
program reads rows while a worst-case row still fits in `COLOUR_BUDGET`
(4,600) instructions, so it stays under half the platform's limit, and
the next program, built once a width and kept, goes on from the row where
the last stopped. A screen at another width than the query's is read by
these programs alone. A row wider than about 300 cells is read alone, and
one wider than about 1,300 is answered classically. The classic
implementation makes the same checks call by call. On a 120 by 30 screen
a plain screen adds 1.6 to 2.6 milliseconds to the read, and the worst
case, every cell's colours differing from its neighbours', is six
programs and 31 to 72 milliseconds (`docs/performance.md`, "A terminal's
colours").

### A command mark's line

`terminal_find(uia, query, remote)` looks for a command mark's line by its
first row's text (`FindQuery`): outward from the row `from` rows above
where the next read starts, alternately the next match at or below it and
the next above it, at most `FIND_MATCHES` (20) matches that start a row
over at most `FIND_CALLS` (100) `FindText` calls, so that a needle found
mostly within rows ends rather than walking the whole history
(`docs/performance.md`, "A terminal's upward search"). Each match that
starts a row gives a `FindMatch`: its rows above that start, and the text
of `rows` rows from it, for the caller to check by its line's hash. A
failing call means nothing more is found in that direction. The program
is built for each search, about 30 instructions a `FindText` call.

## Layer 3: the caret read

`caret_read(query, remote)` (`CaretReadFn`) is one round trip remotely,
the formatting the theme asks for included. A `CaretQuery` gives:

- the element (a program starts from it), its text pattern and
  `TextPattern2` (the classic reads use them);
- `since`, where the caret was known to be, a `RangeEnd`: a range and
  which of its ends; the selection's known ends; and `previous_collapsed`,
  whether that selection held nothing (a caret report sets it; a caret
  key's watch leaves it false, a moved caret being its evidence);
- a unit to read besides the line, and whose formatting to read
  (`FormatSpan`: the character at the caret, that unit, or the line);
- `attributes`, an `Attributes` set of `TextAttribute`s: the annotation
  types for spelling and grammar errors, the font's name, size, and
  weight, italic, the underline and strikethrough styles, the foreground
  and background colors, the bullet style, and the link attribute; and
  `learning`, those whose support the caller does not know yet;
- `recheck`, to read the caret again at the end (a caret key's watch asks
  for it);
- `read_selection_text` and `read_previous_text`, for the texts the
  outpost compares to tell whether the text changed under the selection,
  each up to `MAX_COMPARED_TEXT`, 513 UTF-16 code units, one more than
  the 512 characters from which a selection is spoken as a count; and
  `max_change_text`, the limit for each stretch of a selection change.

The `CaretAnswer`:

- `caret`, a range whose start is the caret, and whether it is known to
  be collapsed; `selection`, the selected range when text is selected.
  The caret is the selection's first range, collapsed, when nothing is
  selected; with text selected, `TextPattern2`'s caret range where the
  provider has it, else the selection's start.
- `moved`: the caret is not where `since` says; `selection_moved`: the
  selection's ends are not the known ones.
- `line` and `unit`: each a `UnitRead`, the range, its text, and how many
  UTF-16 code units of it come before the caret.
- `runs`: the formatting, stretch by stretch, each a `Run`, a length in
  UTF-16 code units and its `RunAttributes`: spelling or grammar error
  (`ANNOTATION_SPELLING_ERROR`, 60001, `ANNOTATION_GRAMMAR_ERROR`,
  60002), font name, size, weight, italic, underline style,
  strikethrough style, color, background color, bullet style, and link,
  each `None` when not read, not supported, mixed, or of another type.
- `unsupported`: the `learning` attributes the span answered "not
  supported" for; `None` when the span said too little to learn from.
- `moved_while_read`: with `recheck`, the caret read last is not where it
  was first read, since a program is not atomic and an application can
  write between its calls.
- `selection_text` and `previous_text`: the selection's text when its
  ends changed, and the old ends' text now, each only when asked.
- `selection_start`, `selection_line` and `start_first`: where the
  selection starts, since `TextPattern2`'s caret can be its other end, so
  a Backspace over the selection can say what comes before it, as NVDA's
  caret, the selection's start, reads it.
- `changes`: with a previous selection that moved, the
  `SelectionTextChange`s by the text protocol's rule, read inside the
  provider, so a selecting key's answer is one round trip with the text
  of what changed.

The program gets its text pattern with `GetTextPattern` (opcode 10014),
reads the selection with `GetSelection` and, only when text is selected,
the caret with `GetCaretRange`. It compares the caret and the selection
with the imported known ones, expands copies of the caret to the line and
the unit, and reads their text and the text before the caret, whose
length is the offset. Formatting is read by UIA's format unit, as NVDA
reads it, until the span's end or `MAX_RUNS` (64) stretches. A stretch
with an attribute that answers "mixed" (`IsMixedAttribute` in the
program, `verbatim_uia::is_mixed` classically) is walked again by words,
and a mixed word by characters, as NVDA walks a mixed stretch by finer
units, so a provider whose format unit does not end where an attribute
changes, such as both terminals' for italics, still has each part read
with its own value. Values are appended as the provider gave them and
typed by the caller, so the program spends no instructions on testing
types. The annotation types are asked of the stretches only when the
span has some.

The classic implementation makes the same reads one call at a time, a
stretch's attributes together in one
`IUIAutomationTextRange3::GetAttributeValues` call as NVDA reads a
range's formatting (`verbatim_uia::text::TextRangeExt::attributes`). An
attribute whose read fails is not supported on both paths. Both read the
whole answer on every call, so each check of a caret key's watch costs
one round trip remotely (`docs/performance.md`, "A caret move, UIA").

## Layer 3: the text protocol's other reads

`text_units`, `text_range`, and `text_location` (`TextUnitsFn`,
`TextRangeFn`, `TextLocationFn`) answer the rest of the text protocol.
Each takes a `TextTarget`, the element and its text patterns, and starts
from a `TextFrom`:

- `Caret`, `SelectionStart`, `SelectionEnd`: read as the caret read reads
  them, once however many points use them.
- `Start` and `End`: the document range collapsed to one end.
- `At(Position)`: a range and the end of it the caller holds.
- `After { from, prefix, counts }`: a point some text after a held one,
  the text between them being `prefix`. Each of `counts` (UTF-16 code
  units, code points, grapheme clusters) is tried in turn, moving a copy's
  end by that many characters and comparing the text passed with `prefix`
  inside the provider; the last tried is kept when none matches. This is
  how a position Core found inside a chunk is resolved in one round trip.

Every point is returned (`FoundPoint`) so the caller can remember it.

- `text_units` (`UnitsQuery`, `UnitsAnswer`, each unit a `UnitText`):
  from the point, an optional `Movement` (`By(unit, count)` from the
  unit's start, its count negative for a backward move even from a
  provider that answers a positive one, as NVDA corrects it; or
  `Document(count)`, to an end), then the unit containing the point
  reached: its range, its text up to `max_text`, the point's offset, and
  its language (`Culture`, as a BCP 47 tag from
  `verbatim_uia::text::locale_name`). With a `count` above one it reads
  on until it has `count`, the text reaches `max_total`, or a move does
  not move (`ended`): say-all's batch, twenty lines in one round trip. A
  batch's language is read once over the whole batch, and unit by unit
  only when that answers "mixed" (`verbatim_uia::text::Language`). A unit
  is expanded from a copy whose start is the point, never collapsed
  first, since `ExpandToEnclosingUnit` normalizes a range from its start.
- `text_range` (`RangeQuery`, `RangeAnswer`, `RangeAction`): two points
  (or one, for moving the caret), ordered by comparing them, and the text
  between them read up to a limit or selected; with `RangeQuery::through`,
  through the character at the later. A refused selection is caught
  inside the program and answered as not selected.
  `RangeAction::SelectReporting` also reads the selection before and
  after selecting and answers how it changed (`RangeAnswer::changes`) by
  the caret read's rule: the review copy's first press in one round trip.
  When a read after the select fails, the program answers with the
  selection it read before (`RangeAnswer::unread`, `ChangesUnread`), and
  `text_range` reads the selection again classically and compares, rather
  than selecting a second time. With the changes read, the answer carries
  the selection after (`RangeAnswer::selection_after`, `SelectionAfter`):
  its range, whether something is selected, and its text up to
  `MAX_COMPARED_TEXT`.
- `text_location` (`LocationQuery`, `LocationAnswer`): the character at
  the point and its bounding rectangles.

## Layer 3: an object-navigation step

`navigation_step(uia, query, remote)` (`NavigationStepFn`) takes one
raw-view step (parent, next or previous sibling, first child) from
`StepQuery::element`, a `Cached<Kept>` the outpost's node registry keeps
(decision 2 of `docs/design/uia-shapes.md`), and finds that element's
nearest window, as the outpost's navigation needs to correct the
neighbor's backend. The answer, a `Step`, holds the neighbor as a
`Cached<Node>` filled by `StepQuery::fill`, with its absent record, and
the window. The classic implementation is `nearest_window_handle` and a
tree walker step, one call each; a step from a gone element is an error,
any other failure no neighbor. A program reports a top-level window's
parent as null where a tree walker reports the desktop, so the outpost
takes a step from a top-level window classically.

## Layer 3: an element found by its path

`element_by_path(uia, query, remote)` (`ElementByPathFn`) finds an
element again from one the caller has, `PathQuery::root` (a
`verbatim_uia::Live`), by `PathQuery::path`, the runtime ids of the
raw-view elements below it, outermost first: at each level the children
are looked at in order until one has the next runtime id. The element
found is a `Cached<Node>` filled by `PathQuery::fill`. The program does
the whole path in one round trip; the classic walk takes one call per
child it looks at (`verbatim_uia::Live::step`), one more for the fill,
and checks `out_of_time` between calls. Both compare children by runtime
id alone and fill no other cache, because a top-level window's first
child is its title bar, whose location UIA asks of the window's thread
(`WM_GETTITLEBARINFOEX`), which Windows answers for one window at a time
across the desktop. The outpost uses it to find its own copy of an
element the focus listener read (`docs/design/focus-pipeline.md`, section
12.5).

## Layer 3: the tree dump

`tree_dump(uia, query, remote)` (`TreeDumpFn`) answers the control
plane's tree dump: the raw-view subtree under `TreeQuery::root`, every
other element filled by `TreeQuery::fill` (without the place unless
locations are asked for), each node's id minted by `TreeQuery::registry`,
capped at `max_depth` levels and `max_nodes` nodes, with whether it is
incomplete. Each element's children are read in one call, as NVDA reads
an object's children (a cache request scoped to the children, then
`GetCachedChildren`), breadth first.

The classic implementation makes one call per element above the deepest
level (`verbatim_uia::Uia::children`), and one uncached step at the
deepest level to learn whether the dump is incomplete. An element gone
during its read is left childless and marks the dump incomplete; any
other failure fails the dump. The program reads up to
`CHILDREN_PER_RUN` (60) children a run, from up to sixteen waiting
elements, each read in a try block, and at the run's end reads each
element it found live for its name, giving back one gone since as none,
because a gone element among a run's results fails the whole run. An
element with more children than the run has room for is read
classically. A run that fails as a whole puts back what it took, and the
rest of the dump is read classically (`Path::Fallback`).

## Layer 3: a selection the focus controls

`controlled_selection(uia, query, remote)` (`ControlledSelectionFn`)
answers NVDA's test for a selection inside an element the focus controls,
whose behavior is `docs/parity/changes-and-selection.md`, "Selection in a
list the focus controls". The `Controlled` answer holds `controlled`, the
runtime ids (as `runtime_id_key` writes them) of the elements the focus's
`ControllerFor` relation names, which the outpost logs, and `selected`, a
`Cached<Node>`: the element the selection event named
(`ControlledQuery::selected`) when it is a descendant of one of them, or,
when it is in none and `ControlledQuery::current_instead` asks, the first
controlled element's first selected element unless that is the named
element itself. The program walks each controlled element's raw-view
descendants depth first comparing runtime ids, since there is no
`FindFirst` instruction; at about 35 instructions an element, a list of
more than about 280 is answered classically. The classic implementation
is `Uia::controlled`, a `FindFirstBuildCache` under each controlled
element.

## Fallback rules

How the outpost chooses, per UIA focus (`uia_remote_enrichment` in its
`read.rs`), and the same way for every other named operation:

- A window without a native UIA provider is read through MSAA, so its
  focus never reaches this choice.
- An import that fails (`Error::Import`, a client-side proxy) marks the
  window for its lifetime, and the outpost then reads it classically
  without trying; the mark is forgotten when the window is destroyed. The
  window marked is the one the operation already holds: the window with
  the keyboard focus for a focus-now query, the path's window for an
  element found by its path, the top-level window of the step's answer
  for a navigation step, the top-level window of the tracked focus's
  window for a controlled selection, and the tracked focus's window for
  the caret read, the text reads and a terminal's screen.
- A run that fails otherwise is answered classically for that call
  (`Path::Fallback`) and logged as a warning with the failing instruction
  and its source line.
- A gone provider and one that times out (`UIA_E_ELEMENTNOTAVAILABLE`,
  `UIA_E_TIMEOUT`) are returned by every entry point without the classic
  calls, which would fail the same way after a second wait; the outpost
  answers them as it answers a gone or unresponsive element.
- A developer setting (`uia.remote_operations` in `settings.toml`, on by
  default) forces the classic walk; the supervisor passes it to each
  outpost as `--classic-uia`.

Not implemented: marking a window after repeated run failures, and
retrying a run that exceeds the instruction limit with a smaller bound.

NVDA declines remote operations before Windows 11, which Verbatim always
runs on (and a Windows without the API fails per call,
`Error::Unavailable`), and guards its uses of Word's provider extensions,
which Verbatim does not call.

## The instruction limit and counting

UIA lets one run execute 10,000 instructions and stops it past that with
`Status::InstructionLimitExceeded`. `counting` measures programs against
it: `counting::start()` turns it on for the thread, and
`counting::stop()` returns how many of each program's own instructions
its run executed. The counting form puts an `Add` of one to a register of
its own before each instruction and doubles each jump's offset less one
so the jump lands on the `Add` before its target; it executes twice as
many instructions and two more, so it counts only programs under half the
limit. It is never on in Verbatim itself.

Programs are not resumable: their work is bounded by their queries. Every
program's worst case stays under half the limit (`docs/performance.md`,
"The instruction limit") except the caret read's walk of a span whose
every stretch is mixed, about 7,700, which still runs under it. A run
that exceeded the limit would be answered classically for that call.

## Where remote operations are not used

- Reading the focused element (`GetFocusedElementBuildCache`): one call,
  and the element is what a program would start from.
- A node's text patterns, fetched once per node the first time its text
  is read: the classic reads need the pattern objects. Making the fetch
  lazy, so a remote read never makes it, is a possible follow-up.
- Activation (`Uia::activate`): the pattern methods (`Invoke`, `Toggle`,
  `Select`) have no instructions.
- The menu item's legacy checked state for the focus-now answer and a
  navigation step's neighbor: one call.
- A dialog's own text through UIA: already one call.
- The ancestors query (`Query::Ancestors`): nothing sends it.
- The arbitration probe and the checks of a console's and a Windows Forms
  window's provider: window messages and one-time checks made before the
  window's backend is known.

A terminal's reads are each one program, and the outpost's terminal code
makes no UIA call beyond them. The conformance runner (`xtask`) reads
classically, as an independent reference for the program's colours.

## A stalled or exited provider

`Execute` blocks while the provider is stalled, as a classic call does.
UIA's connection timeout does not bound it; its transaction timeout
(`IUIAutomation2`'s `TransactionTimeout`, process-wide, five seconds in
every Verbatim process, `verbatim_uia::TRANSACTION_TIMEOUT`) does, with
`ExecutionFailure` and extended error `UIA_E_TIMEOUT`, as a classic call
fails in the same time. So the outpost's worker calls `Execute` directly,
and its watchdog covers both the same way; a program is one wait where a
classic walk against a provider that stalls partway is one wait per
remaining hop. When the provider's process has exited, `Execute` returns
at once with `UIA_E_ELEMENTNOTAVAILABLE`.

## Costs

In a release build against mockapp: five ancestors to the top-level
window take 3.4 ms remotely and 25 ms classically, most of the classic
time being the last hop to the desktop root; a list with its selected
child and one ancestor 3.4 ms and 17 ms; one live property read 0.31 ms.
The text reads and the navigation step are in `docs/performance.md`,
"Wall-clock gain".

## Tests

Unit tests cover each instruction's bytes and the builder's offsets,
constants, and source locations. The mockapp tests run both
implementations of each operation against real providers and assert that
they agree: `remote_ops.rs` (the focus ancestry, the navigation step,
cached properties, a stalled and an exited provider), `call_counts.rs`
(calls and provider hits of each read), `text.rs` (the caret read and the
text reads), `terminal.rs` and `terminal_snapshot.rs`,
`controller_for.rs`, `uia_tree.rs` and `time_limits.rs` (the tree dump),
and `instruction_limit.rs` (the platform's limit and every program's
count), all under `crates/mockapp/tests/`. `cargo xtask
regenerate-terminal-conformance-expectations` checks the colour read
against both real terminals, failing when its runs differ from the cells'
read one by one.
