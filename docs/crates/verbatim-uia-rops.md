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
terminals `terminal_snapshot` and `terminal_rows`, for caret reports `caret_read`,
for the text protocol's other requests `text_units`, `text_range`, and
`text_location`, for object navigation `navigation_step`, and for a
selection inside an element the focus controls `controlled_selection`
(layer 3 below). Every UIA path in the outpost that makes a sequence of calls to
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

A finished program is an `Operation`: its imports, and a shared
`Program`, the instructions, bytes, opcodes, requested results, and the
kind of each import. The two programs a typing user runs most, a
terminal's screen and the caret read, are kept per thread
(`ProgramCache`, at most 16 of each, all forgotten when full) by a key
naming everything the program was built from but its imports: the
anchor's rows, the counts, the caret's shape and the ends compared with
for the screen; the unit, formatting, attributes, limits, and ends for
the caret. A query with the same key runs the kept program bound to its
own imports (`Program::bind`, which checks they are as many and of the
same kinds), so the program is not built or encoded again; both
programs import everything first, in the order their `imports` function
gives. A program holds no UI Automation object, but the cache is per
thread all the same, as a remote operation and its imports are per
client and thread. A program whose run on the thread found every opcode
supported and ran skips the support check on later runs, since the
provider is the thread's application; any failed run has the next check
again. `work::programs_built` and `work::support_checks` count both per
thread for tests. Measured on 2026-10-13 against mockapp, release, for
each terminal read: building the program took 22 to 78 microseconds,
the support check 2 to 7, importing 1 to 12, against 0.4 to 1.1
milliseconds for `Execute`. Other programs are built per call, since
their constants change with nearly every call (below).

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
`Outcome::get_utf16` gives a `Str` register as the provider's UTF-16,
never decoded, empty for a null one, for text the caller trims before it
decodes it.

## Layer 3: the focus ancestry

`focus_ancestry_remote` and `focus_ancestry_classic` share one signature
(`FocusAncestryFn`): a `&Uia` and a `FocusQuery`, which holds the focused
element as a `verbatim_uia::Cached<FocusInput>` (its control type and
window, read from its cache; the focus listener narrows the focus event's
own element to it, and the outpost the focused element a focus-now query
read), the runtime ids the caller already knows, a depth limit, the
`verbatim_uia::Fill<Ancestor>` every ancestor is filled by (`fill`, the
shape's properties for the gates its request was built with: every gate
on for the listener, the theme's for the outpost, but the node's place,
its rectangle and whether it is off the screen, which a read of many
elements never holds, decision 9 of `docs/design/uia-shapes.md`), the
fill of the element itself and its selected child (`element_fill`, the
same with the place, since each is read alone; the program builds its
requests only when it reads one of them), an out-of-time check
(`out_of_time`), which only the classic walk makes, before each of its
calls after the first, what a skipped call would have read then unknown,
and `nearest_window`, the element's nearest window when the caller has
already found it, which the classic walk then does not look up, and
`before_call`, told the name of each of the classic walk's calls as it
begins, so that the caller can time them (the focus listener logs one
slower than a second), and `value_available`, whether the element has
the `Value` pattern when the caller's cache already says, as the
outposts' node does, which the classic walk then asks nothing for. Since the input is typed, its
control type is always in its cache, and the live read of it the query
once fell back to is gone (decision 8 of `docs/design/uia-shapes.md`).
Both answer `FocusAncestry::NotFocused` when a
live read of the element's `HasKeyboardFocus` is false and the query's
`require_focus` asks for it (NVDA's check that a focus event is not
stale), and otherwise an `Ancestry`; a query without `require_focus` is
for an element whose focus event said it had the focus, which has moved
on since, and reads its ancestry all the same. A query
may also name `previous`, the element the caller holds under the focused
element's runtime id, whose `HasKeyboardFocus` is read live too: an
application can give a dead element's runtime id to a new one (File
Explorer does), and NVDA treats a focus as a duplicate only while the
element it compares equal to still has the focus. The `Ancestry`:

- `ancestors`: the raw-view parents, nearest first, each a
  `Cached<Ancestor>`. The walk ends at the top-level window of the
  element's process: in a program, the top-level window's parent is null
  (verified), so the classic walk stops below the desktop root as well,
  and below the process boundary, at an element whose own window's
  parent window, or visible owner for a top-level window, another
  process serves (`verbatim_uia::parent_across_process_boundary`, local
  calls, made for each element with a window of its own), as the
  Settings page's content is inside `ApplicationFrameHost`'s frame.
- `met_known`: which of the known runtime ids the last ancestor has, when
  the walk stopped there.
- `depth_limited`: whether the walk stopped at the depth limit with
  ancestors left.
- `out_of_time`: whether the classic walk stopped when the query's time
  was up, with ancestors left; never set by the program.
- `failed_hop`: how the walk's step to a parent failed below the top, a
  `FailedHop` (`verbatim_uia::FailedHop`, re-exported): `Error` with the
  failing code, or `NoParent` for a step that answered none; `None` when
  no step failed. Only reaching the top ends a walk as known. For the
  classic walk the top is the desktop root, since in the classic view a
  top-level window's parent is the root element, and any step below it
  that fails or answers none sets this. Inside a program a top-level
  window's parent is null, so the program's walk ends at any null
  parent, and sets `NoParent` when it ended there, short of a known
  ancestor and the depth limit, anywhere but at the top: the last element
  read, or the focus itself with none, must have as its window a
  top-level window or one below the process boundary
  (`verbatim_uia::is_top_level_window`,
  `verbatim_uia::parent_across_process_boundary`, local calls made once
  after the program), the rule the classic walk stops by. Its window is
  the one its runtime id `[42, handle]` names, since a read inside the
  provider can leave the handle out (`docs/uia-bugs.md`, "Inside a remote
  operation, conhost's window has no handle or title"), or the one whose
  handle its cached `NativeWindowHandle` reports, as the classic walk
  identifies an element's window by its handle first. Inside a program
  the parent of a window below the process boundary is null, as a top-level window's is
  (verified: mockapp's window made the child of another mockapp's by
  `SetParent`, as the Settings page's content is of
  `ApplicationFrameHost`'s frame, ends the walk at an element that is
  not a top-level window's, so that without the boundary check the
  focus-now answer's chain is not continued through the parent and the
  test fails; `uia_focus_chains_stop_below_the_process_boundary` in
  `crates/mockapp/tests/call_counts.rs`, its "child, remote" case). Any
  other window handle reported below the top does not make the walk
  complete: UIA reads a fragment element reporting a handle as that
  window's element, and one reporting a window that no longer exists has
  no parent, and no top-level window
  (`a_focus_now_whose_program_ends_below_the_window_at_an_element_reporting_a_handle_has_its_ancestors_unknown`).
  Since UIA gives every element reporting a handle that window's runtime
  id, the handle check agrees with the runtime id's for every provider
  mockapp can make; it keeps the two walks' rules the same.
  A provider whose `Navigate` fails reaches the client as no
  parent (verified against mockapp's `answer-parent`).
  `Ancestry::ancestors_unknown` is true for a walk out of time or with a
  failed step, and callers report such a focus with its ancestors
  unknown, as NVDA falls back to the window hierarchy when its parent
  read fails.
- `selected_child`: for a list or tab control (by the element's cached
  control type), the first selected child, a `Cached<Ancestor>` too,
  which the outpost reports as its node; and for a combo box without the
  `Value` pattern, its first selected item, which is its value
  (`verbatim_uia::map::value_from_selection`). The program asks the
  combo box for `IsValuePatternAvailable` itself and reads the selection
  only without the pattern; the classic walk takes the answer from the
  query's `value_available`, else from the element read afresh, or,
  without either, asks it live, one call.
- `window`: the native window handle of the element or of its nearest
  raw-view ancestor that has one, which `verbatim_uia::nearest_window_handle`
  would find (NVDA's `getNearestWindowHandle`). The outpost needs it to
  arbitrate and report a focus whose event names no window, and reading
  it here saves that separate call.
- `previous_focused`: whether the query's `previous` element has the
  keyboard focus, false when it does not or its read failed, `None` when
  the query named none.
- `legacy_state`: for an element whose cached control type is a menu
  item, its legacy MSAA state bits (`LegacyIAccessibleState`, ignoring
  its default), read by the remote program, `Some(None)` when the element
  does not support it; `None` when it was not read, for any other element
  and always from the classic walk, which leaves it to its caller's one
  live read. A menu item that no pattern makes checkable, as a Windows
  Forms one, says it is checked only there.

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

The classic implementation is the focus listener's read, which makes no
remote operation (`docs/design/listener-without-connections.md`, section
2), and the outposts' fallback: the same live read, the selected child through `SelectionPattern2` or else the
Selection pattern
(`Uia::selected`, which `Uia::selected_node` also
uses), and one `GetParentElementBuildCache` round trip per ancestor over
the raw view, the walk `Uia::ancestors` makes, each filled by the fill's
request (`Fill::request`). `ancestors`
itself is not called, because it returns filtered snapshots, not
elements: the presentable-ancestor filter, the switch to MSAA, and the
splice with the previous focus's chain stay with the outpost, which
applies them to either implementation's elements with
`Uia::ancestors_from`. It stops below the process boundary, judged with
local calls only: before each step, at an element whose window, its own
or, for the nearest window's own element held without its handle (its
runtime id `[42, window]`), the nearest window, has a parent window or
visible owner that another process serves
(`verbatim_uia::parent_across_process_boundary`); and after each step, a
parent whose own window is not the nearest window and is served by
another process, as an owner shown between the check and the step makes
it, is not kept and ends the walk. Its window is the query's
`nearest_window` when given; else it comes from the walked ancestors'
caches, or, when it stopped before reaching one, from
`nearest_window_handle`, one more call. Every call it makes goes through
`verbatim-uia`'s safe wrappers, so it is counted, and the module has no
`unsafe` code.

With `FocusQuery::refresh`, the program also fills the focused element's
own cache afresh by the query's fill, in the same round trip, and
answers it as `Ancestry::element` (the classic walk makes it one
`BuildUpdatedCache`): the focus listener reports a focus's value, states,
and details from it, as NVDA reads those afresh when it handles a focus
event, without a second call. Every other caller leaves it off.

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
complete ancestry; any other failed hop, or one that finds no parent,
below the root ends the walk with `failed_hop` set, its ancestors
unknown. Pinned against mockapp with `slow` and `stall`
(`crates/mockapp/tests/remote_ops.rs`, `a_focus_walk_that_times_out_fails`)
and with `answer-parent`
(`a_focus_whose_parent_step_fails_is_reported_with_its_containers_unknown`).
The program's walk ends at a null parent, which is how it reaches the top
of a program's tree; one that ends there anywhere but at an element whose
window, named by its runtime id or by the handle it reports, is a
top-level window or one below the process boundary, ended below the top, and is reported with `failed_hop` set, its
ancestors unknown (`a_focus_now_whose_program_finds_no_parent_below_the_window_has_its_ancestors_unknown`).

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
  are in the `ValueSet` and `States` shapes. Without that, every container read
  remotely was read-only with a value of "0".
- `IsDataValidForForm` (whose default reads as false) has no pattern, so
  the program leaves it out of the cache of an element that does not
  support it. `PopulateCache` replaces an element's cache rather than
  adding to it (verified), so the program builds one request per
  combination of these properties
  (`verbatim_uia::LEFT_OUT_WHEN_UNSUPPORTED`, one property, so two
  requests) and picks one per element after a `GetPropertyValue` with
  `ignoreDefault` and an `IsNotSupported` test. It returns, with each
  element it filled, which of them it left out, one integer per element
  (`verbatim_uia::Absent`), and wraps the element as a `Cached<S>` with
  that record (`Fill::claim`), so the element's read answers the
  property as "not supported" without asking UI Automation, which would
  otherwise fail the read and report an error inside itself (decision 3
  of `docs/design/uia-shapes.md`). Pinned against mockapp
  (`crates/mockapp/tests/remote_ops.rs`,
  `a_remotely_read_element_without_validity_reads_valid_with_no_failed_read`:
  without the record, six failed reads).

The programs build their cache requests from the `Fill`'s properties and
patterns (`RemoteCache`, shared by every program here that fills a
cache), and their classic fallbacks fill with its request, so both come
from one shape. Against mockapp, every other cached property reads the
same both ways, and what the two implementations' elements map to is
equal.

## Layer 3: a terminal's snapshot

`terminal_snapshot_remote` and `terminal_snapshot_classic` share one
signature (`TerminalSnapshotFn`), and `terminal_snapshot(uia, query,
remote)` chooses between them as `focus_ancestry` does, answering with a
`Path`. It is the read of `docs/design/terminal-model.md`, section 4.2:
the outpost reads every focused terminal with it, and
`cargo xtask regenerate-terminal-conformance-expectations` reads the real
terminals with it. A `SnapshotQuery` names the element and its
text pattern, where the last read said to start (`start`, a range; none
for a first read), the screen's height in rows as the last read found it
(`screen_rows`, 0 when not known), whether to read the whole text instead
(`whole`), how many rows above the start to begin the read (`above`, the
history rows whose place shows how far a full history rotated,
`docs/design/terminal-model.md` section 4.3 item 5; ignored with `whole`),
and, with `caret` (a `CaretLineQuery`), the caret and its line,
read in the same program or, classically, after the text, and back as
`Snapshot::caret`; and, with `colours`, the screen's colours (below),
given the screen's width in cells as the last read found it, 0 when not
known.

The unit of reading is one `GetText`. Both terminals answer one from one
locked state of their buffer (measured on 2026-10-23 in both, with a
writer printing numbered, checksummed lines while snapshots were taken
back to back: no snapshot of 3,561 held a gap or a torn line), while
separate calls are not atomic with each other. So the snapshot's text is
the only thing anything spoken or counted comes from, and every other
field is a hint. The answer, a `Snapshot`, gives text as the provider
gave it, padding and line breaks included; how each provider pads, breaks
and ends its text is recorded by the conformance expectations
(`docs/crates/verbatim-term-sim.md`, "How it works"):

- `text`: from the start to the document's end, in one call. The start is
  the query's `start` when it lies no lower than where the next read
  starts (`from_start`). Otherwise it is refused (`shrank`): a start below
  that, or one the provider cannot compare (a range on the other screen,
  whose row lies beyond its height), means the text was cleared or
  switched, and the snapshot starts where the next read will. A first read
  starts at the screen's top; a `whole` read at the document's start. The
  snapshot then starts `above` rows higher, as far as the document's start
  allows, moving by lines, which both terminals take as rows.
- `above`: how many rows the snapshot holds above its start, fewer than
  asked when the document's start came first.
- `screen`: the first visible range's text, for the screen's height in
  rows.
- `next_start`: the document's end moved back by `screen_rows` lines, the
  top of the screen output is written to, wherever the user has scrolled
  the view; the screen's top when the height is not known. A range stays
  on its coordinate: once a terminal's history is full, newer text moves
  up beneath it.
- `at_document_start` and `alternate`: whether the snapshot, and the
  screen, start at the document's start.
- `first_row`: the document's first row; when it reads otherwise, the
  history rotated.
- `caret_rows_to_end`: how many lines the caret moves to reach the
  document's end.
- `colours`: when asked, each screen row's runs of one foreground and
  background (`ColourRun`: its first column, its length in cells, and
  its two colours as `0x00BBGGRR`), from the screen's top to the
  earlier of the document's end and the screen's, the form the terminal
  reading rules take (`verbatim-text`'s `terminal_reading::ColourRun`,
  `docs/design/terminal-model.md` section 5.7). Read after the text, so a
  hint like the rest.

Without colours the program has no loop, so its cost does not grow with
the text: 50 instructions for a first read and 90 with a start and the
caret, each with four rows above its start, and 5 fewer without
(`crates/mockapp/tests/instruction_limit.rs`).

### Rows by their place

`terminal_rows(uia, query, remote)`, with `terminal_rows_remote` and
`terminal_rows_classic` sharing one signature (`TerminalRowsFn`), reads
runs of a terminal's history rows by their place. The terminal reading
rules no longer ask for it, since a read a screen above the last screen
settles the rotation (`docs/design/terminal-model.md`, "The read"); it is
kept for reading history rows by their place. A `RowsQuery` gives the element and its text pattern, the
start the snapshot was asked for (the last read's `next_start`), and the
runs, each how many rows above that start it begins and how many rows it
holds, at most `MOST_RUNS` (16). For each run the program collapses a copy
of the start, moves it up by the run's rows and, when it moved that far,
moves its end down by the run's rows and reads its text; a run whose first
row the document's start came before is read as nothing. It then reads the
document's first row, so the caller can tell whether the buffer moved
between the snapshot and these rows. `RowsAnswer` holds each run's text,
UTF-16 as given, and the first row. The console host's text of a run
leaves out its last row's line break (measured 2026-10-24), which the
reading rules allow for. The program has no loop: ten instructions a run
and twelve besides, 172 at 16 runs, the program kept per thread for each
list of runs.

### The colours

What the providers answer, measured in both terminals on 2026-10-24: a
cell's colour attribute is its own colour; a stretch of cells answers its
first cell's colour when every cell but its last has it, and UIA's
"mixed" otherwise, since neither provider compares a stretch's last cell
(a stretch of two cells always answers its first cell's colour); their
format unit stops only at rows; and Windows Terminal does not implement
`FindAttribute` (`E_NOTIMPL` inside a program). So a stretch is of one
colour exactly when its answer is a number and its last cell, read on its
own, answers the same: a check of four attribute reads, whatever the
stretch's length. A check compares only numbers, since comparing
"mixed" fails a program.

The read (`terminal_colours.rs`) takes rows as wide as the screen's top
row as the provider expands it to a line (`row_width`): a screen whose
every row is written to its last column reads in the console host as one
line, every row marked as wrapping, so the narrowest line of the screen's
text can be the whole screen. Each row is checked whole; a row of
several colours is checked in chunks of `COLOUR_CHUNK` (16) cells; a
chunk of several colours is read cell by cell, a cell a `Move` by one
character and two attribute reads, but for its last cell, which its check
read. A check's last cell is made as the conformance runner makes a cell,
its end moved from the row's start by characters and then its start. On
the buffer's last row (the alternate screen's, and the main screen's once
the history is full) moving by characters stops a cell short of the
width, so the cell made at the last column is empty and reads in the
colours the next character would be written in, as the runner records
it, and a stretch to the row's end leaves out its last two cells: the
move to the row's end says so by moving a cell fewer, and that row and
its last chunk are then read cell by cell. The rows stop at the earlier of the
document's end and the screen's, or when a move by a line moves nowhere,
and at most at the screen's height in rows at that width
(`screen_height`).

The snapshot's own program reads the first rows, at the width the query
gives, after everything else, and returns the screen's top, the limit
and the top row's text. A program reads rows while a worst-case row still
fits in `COLOUR_BUDGET` (4,600) instructions, each part's cost measured
from the code it emitted, so a program stays under half the platform's
limit whatever the screen holds, and the next program, built once a
width and kept, imports the row where the last stopped and the limit and
goes on. A screen at another width than the query's (a first read, or
one after a resize) is read from its top by these programs alone. A row
wider than about 300 cells is read alone, one row a program, and one
wider than about 1,300 cells exceeds the limit and is answered
classically. The classic implementation makes the same checks call by
call, both colours of a stretch in one `GetAttributeValues`.

Measured on 2026-10-24 on 120 by 30 screens (`docs/performance.md`, "A
terminal's colours"): a plain screen is one program of 1,349
instructions, 1.6 to 2.6 milliseconds added to the read; Pi's colourful
screen two programs, 5 to 8 milliseconds; the worst case, every cell's
colours differing from its neighbours' with reverse video and true
colour, six programs of about 4,100 instructions, 72 milliseconds in the
console host and 31 in Windows Terminal, where reading every cell
regardless costs as much and an ordinary screen would cost it too.

## Layer 3: the caret read

`caret_read_remote` and `caret_read_classic` share one signature
(`CaretReadFn`), and `caret_read(query, remote)` chooses between them as
`focus_ancestry` does, answering with a `Path` (milestone M4 items 3 and
7; `docs/design/phase6-design.md`, "Caret responsiveness"). One difference: a run
that failed because the provider's process has gone or did not answer in
time (`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`) is returned as the
error without running the classic reads, which would meet the same
failure, a second wait for a stalled application. A `CaretQuery` gives
the element with the text (a program starts from it), its text pattern
and `TextPattern2` (the classic reads use them), where the caret was
known to be (`since`, a `RangeEnd`: a range and which of its ends), the
selection's ends as they were known, whether that selection held nothing
(`previous_collapsed`, which a caret report sets so that with nothing
selected now nothing is compared or read, and a caret key's watch leaves
false, a moved caret being its evidence), a unit to read at the caret besides
the line (a word, a paragraph, a page), whose formatting to read
(`FormatSpan`: the character at the caret, that unit, or the line),
which attributes to read (`attributes`, an `Attributes` set of
`TextAttribute`s: the annotation types for spelling and grammar errors,
the font's name, size, and weight, italic, the underline and
strikethrough styles, the foreground and background colors, the bullet
style, and the link attribute), and which of them are being learned
(`learning`: those whose support the caller does not know yet), and
whether to read the caret again at the end (`recheck`, which a caret
key's watch asks for), and whether to read the selection's text
(`read_selection_text`) and the text between the old ends as they now
are, when that selection held something (`read_previous_text`), which
the outpost keeps and compares to decide whether the text changed under
the selection. Both are read up to `MAX_COMPARED_TEXT`, 513 UTF-16 code
units, one more than the 512 characters from which a selection is
spoken as a count, since they serve only that comparison. The `CaretAnswer`:

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
- `moved_while_read`: with `recheck`, the caret read again once
  everything else was read is not where it was first read. A program is
  not atomic: the provider takes each of its calls on its own (the
  console host takes its lock for each), so an application can write
  between them, and the line then shows a character whose write moved the
  caret past where the caret was read (2026-10-14, `conhost_up_typing`).
  False without `recheck`.
- `selection_text` and `previous_text`: the selection's text when
  something is selected and its ends are not the old ones (or no old
  ones were given), and the old ends' text now, each only when asked and
  up to `MAX_COMPARED_TEXT`. With the ends unchanged the selection's text
  is not read: the old ends' text is the same text.
- `selection_start`, `selection_line` and `start_first`: where the
  selection starts, since `TextPattern2`'s caret can be its other end
  (Windows 11 Notepad's after Shift with Right Arrow or End): its offset
  in the caret's line when it starts there (the caret's own offset
  without `TextPattern2`), else the earlier line it starts in, a
  `UnitRead` whose `offset` is the start; and, when it starts at that
  line's start, whether that is the text's start (the document range
  compared), so a Backspace over the selection can say what comes before
  it, as NVDA's caret, the selection's start, reads it.

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
that neither overlap nor touch, or either of which holds nothing, are the
old text unselected and the new selected, each whole (a side holding
nothing read not at all; whether the old held nothing is the query's
`previous_collapsed`, so no comparison is made for it); otherwise the text between the new and the old start, then
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
  read up to a limit or selected. With `RangeQuery::through`, the points
  just past the two points' characters, the range runs from the earlier
  through the character at the later. A selection the provider refuses is
  caught inside the program (`try_catch`), answered as not selected, as
  the classic `Select` failure is. `RangeAction::SelectReporting` also
  reads the selection before selecting, with the caret read's
  instructions, and again after, and answers how it changed
  (`RangeAnswer::changes`) by the caret read's rule, with the same
  instructions (`emit_changes`), all in the one program: the review
  copy's first press, one round trip (`docs/performance.md`, "The review
  copy's selection, UIA"). The reads after the select are in a try block
  of their own: when one fails, the selection is made, and the program
  still answers with the selection it read before
  (`RangeAnswer::unread`, `ChangesUnread`). `text_range` then reads the
  selection again the classic way and compares it with that one, which
  is all of the classic implementation's work that remains, rather than
  running the classic implementation, which would select again and
  compare the new selection with itself, so the first press would say
  nothing; the answer's path is a fallback. With the changes read, the
  answer carries the selection read after the select
  (`RangeAnswer::selection_after`, `SelectionAfter`): its range, whether
  something is selected, and its text up to `MAX_COMPARED_TEXT`, read in
  the program (one `GetText` more), which the outpost keeps as it keeps a
  caret report's selection.
- `text_location` (`LocationQuery`, `LocationAnswer`): the character at
  the point and its bounding rectangles, the first rectangle's left and
  top. The program returns them as an array of doubles (verified against
  mockapp, which now reports one rectangle per character on a fixed grid).

Against mockapp (`crates/mockapp/tests/text.rs`), the two implementations
of each agree: a line at the caret, the next line, the end of the text,
every line ahead with the end found, a word from a point found by its
text, a range read in either order, a selection made and read back, a
selection made with its change reported, and locations.

## Layer 3: an object-navigation step

`navigation_step(uia, query, remote)`, with `navigation_step_remote` and
`navigation_step_classic` behind it (`NavigationStepFn`), takes one
raw-view step (parent, next or previous sibling, first child) from an
element and finds the element's nearest window, as the outpost's
navigation needs to correct the neighbor's backend. The program reads the
element's own window handle from its cache, else walks raw-view parents
reading `NativeWindowHandle` until one is set, then navigates and fills
the neighbor's cache inside the provider by the query's
`Fill<Node>` (`StepQuery::fill`), choosing a cache request per
`LEFT_OUT_WHEN_UNSUPPORTED` combination and returning the absent record
as the focus ancestry does; the neighbor is a `Cached<Node>`. The
classic implementation is `nearest_window_handle` and a tree walker step
filled by the fill's request, one call each; a step that fails because the
element is gone is an error, any other failure no neighbor. The step's
own element (`StepQuery::element`) is a `Cached<Kept>`, the element the
outpost's node registry keeps, whose window handle is read from it as the
`Kept` shape, which every kept element's cache holds whichever request
filled it (decision 2 of `docs/design/uia-shapes.md`).

A program's walk ends at the process's top-level window, whose parent it
reports as null where a tree walker reports the desktop, so the outpost
takes a step from a top-level window classically. Against mockapp's
ancestry fixture (`crates/mockapp/tests/remote_ops.rs`), both give the
same neighbor with the same cached properties and snapshot, and the same
window, for every direction and at an edge.

## Layer 3: an element found by its path

`element_by_path(uia, query, remote)`, with `element_by_path_remote` and
`element_by_path_classic` behind it (`ElementByPathFn`), finds an element
again from one the caller has, such as a window's element from its
handle, by the runtime ids of the raw-view elements below it, outermost
first (`PathQuery::path`, from `PathQuery::root`, a `verbatim_uia::Live`;
the element found is a `Cached<Node>`, filled by `PathQuery::fill`): at
each level, the current element's children
are looked at in order until one has the path's next runtime id, and the
walk goes down to it; a level with no such child answers that nothing was
found. The program does the whole path in one round trip and fills the
found element's cache inside the provider; the classic walk takes one
call per child it looks at (`GetFirstChildElement`, then
`GetNextSiblingElement`, through `verbatim_uia::Live::step`) and one more
that fills the element found (`BuildUpdatedCache`), and makes the query's
out-of-time check (`out_of_time`) between calls. Both compare a child by
its runtime id alone and fill no other element's cache: filling every
child passed would read each of its properties, and a top-level window's
first child is its title bar, whose location UIA asks of the window's
thread (`WM_GETTITLEBARINFOEX`). Windows answers that request for one
window at a time across the desktop, so on a busy machine it held
mockapp's window thread for seconds. The outpost uses it, since stage A step 6 of phase 6, to find its
own copy of an element the focus listener read
(`docs/design/focus-pipeline.md`, section 12.5): the focus, from the
nearest element of its chain with a window of its own, after the focus is
reported; and an element of that chain it needs later. Its instruction
count grows with the children looked at on the way: 1,134 down mockapp's
deepest path, sixty-two levels, 148 one level down, and 141 for a path
that ends at once (`crates/mockapp/tests/instruction_limit.rs`).

## Layer 3: the tree dump

`tree_dump(uia, query, remote)`, with `tree_dump_remote` and
`tree_dump_classic` behind it (`TreeDumpFn`), answers the control plane's
tree dump: the raw-view subtree under `TreeQuery::root`, a
`verbatim_uia::Cached<Node>`, every other element filled by
`TreeQuery::fill`, which leaves out the place unless the dump is asked
for locations, each node's id minted by `TreeQuery::registry`, capped at
`max_depth` levels and `max_nodes` nodes; it returns the tree and whether
it is incomplete, and, from `tree_dump`, which path answered. Each
element's children are read in one call, as NVDA reads an object's
children (`_get_UIAChildren`: a cache request scoped to the children,
then `GetCachedChildren`), breadth first, so a child gone before its
parent's read is absent and one gone after it keeps its later siblings.

The classic implementation makes one call per listed element above the
deepest level (`verbatim_uia::Uia::children`), and for an element at the
deepest level one uncached step to its first child (`Live::step`), which
marks the dump incomplete when there is one. A read that fails because
its element is gone (`UIA_E_ELEMENTNOTAVAILABLE`) leaves that element
childless and marks the dump incomplete; any other failure, UIA's
transaction timeout among them, fails the dump. UIA itself answers a
read whose element dies when asked for its first child as no children,
not as a failure.

The program starts from up to sixteen waiting elements and reads, for
each in turn and then each child it finds, the children, filling each
inside the provider, until it has read `CHILDREN_PER_RUN` (60) children
or the node cap's room. An element's read is in a try block: one that
fails is reported with its status, gone or otherwise, as the classic
read's would be, and, when the run found that element, it is given back
as none. At the run's end every element it found is read live for its
name inside a try block, and one gone since is given back as none and
marks the dump incomplete: a gone element among a run's results fails
the whole run (`ExecutionFailure` with `UIA_E_ELEMENTNOTAVAILABLE`,
verified against mockapp). An element whose children are more than the
run has room for ends the run, and is read classically, one call. A run
that fails as a whole puts back what it took, and `tree_dump` reads the
rest of the dump classically (`Path::Fallback`), failing instead on UIA's
transaction timeout. A remote program walks the same raw view as the
classic read, a top-level window's title bar included (verified). The
worst case, sixty children read one each down a chain, executes 4,097
instructions (`crates/mockapp/tests/instruction_limit.rs`).

## Layer 3: a selection the focus controls

`controlled_selection(uia, query, remote)`, with
`controlled_selection_remote` and `controlled_selection_classic` behind it
(`ControlledSelectionFn`), answers NVDA's test for a selection inside an
element the focus controls (its base `event_selection` reads the focus's
`controllerFor` live for each selection event) with a `Controlled`: the
runtime ids of the elements the relation names (`controlled`, as
`runtime_id_key` writes them, up to the one the element was found in,
all of them when it was found in none), which the outpost logs when the
element is in none of them, and
`selected`, a `Cached<Node>` filled by the query's fill, when the element a selection event
named by its runtime id (`ControlledQuery::selected`) is a descendant of
one of the elements the focus names in its `ControllerFor` relation.
When it is in none, and the query asks for it
(`ControlledQuery::current_instead`, false for a selection the outpost
reads as the focus's own, which then reads nothing more), `selected` is
the first controlled element's first
selected element instead, read through its `Selection` property in the
same program and filled the same way, unless the event named that
element itself: Settings raises its search suggestions' first selection
for an item it has already replaced, and the list's current selection
is the new item (`docs/parity/changes-and-selection.md`, "Selection in a
list the focus controls"). A caller tells the two answers apart by the
runtime id. The
program reads the relation, then walks each controlled element's
raw-view descendants depth first (first child, else next sibling, else
the nearest ancestor's next sibling, counting depth so the walk ends back
at the controlled element, which is not its own descendant), comparing
each one's stringified runtime id with the selected one's
(`runtime_id_key`), and fills the found element's cache inside the
provider. The classic implementation is `Uia::controlled`: the
relation, then a `FindFirstBuildCache` under each controlled element,
and, found in none, the first controlled element's Selection pattern,
its current selection, and the first selected element's cache, when the
query asks for it.
There is no `FindFirst` instruction, so the program walks; each element
it passes costs about 35 instructions, so a run reaches the instruction
limit after about 280, and a list that large is answered classically for
that call. Against mockapp's controller fixture
(`crates/mockapp/tests/controller_for.rs`) both answer the same, the
program in one call, for a result one level down and one inside a group
of the list, and neither finds the controlled list itself, an item
outside it while the list has nothing selected, or anything from an
element that controls nothing; both answer the list's current
selection for a runtime id the list does not hold, and for an item
outside it while the list has a selection.

## Fallback rules

How the outpost chooses, per UIA focus (`uia_remote_enrichment` in its
`read.rs`), and the same way for every other named operation (the caret
read and the text reads through `UiaText`, a terminal's screen, a
navigation step):

- A window without a native UIA provider (arbitration's raw probe
  verdict) is read through MSAA, so its focus
  never reaches this choice.
- An import that fails (`Error::Import`, a client-side proxy) marks the
  window the focus is in for its lifetime, and the outpost then reads it
  with its own classic walk, without trying; the mark is forgotten when
  the window is destroyed, since its handle may be reused. The window
  marked is the one the operation already holds: for a focus-now query,
  the window with the keyboard focus; for an element found by the path
  the focus listener read for the focus, that path's window; for a
  navigation step, the top-level window of the window the step answered;
  for a controlled selection, the top-level window of the tracked focus's
  window; and for the caret read, the text reads and a terminal's screen,
  the tracked focus's window.
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
bounded by their queries. The terminal colour read is the one read made
of several programs, each bounded by its budget and the next going on from
the row the last stopped at (above, "The colours"), so no program of it
nears the limit.

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
- Activation (`Uia::activate`): each pattern is fetched live and its
  method called, two calls for an `Invoke`. The pattern methods
  (`Invoke`, `Toggle`, `Select`) have no instructions.
- The menu item's legacy checked state for the focus-now answer and a
  navigation step's neighbor: one call, read for that element alone. A
  focus's is read by its program.
- A dialog's own text through UIA: already one call
  (`BuildUpdatedCache` over the children).
- The ancestors query (`Query::Ancestors`), a full classic walk: nothing
  sends it today.
- The arbitration probe and the checks of a console's and a Windows Forms
  window's provider: window messages and one-time checks made before the
  window's backend is known.

The terminal reading, audited call by call on 2026-10-24:

- The read program (`terminal_snapshot`): one program with the screen,
  where the next read starts, the start's check, the rows above it, the
  text, the first row, the caret's distance to the end, the caret and its
  line, and the first rows' colours; the pattern is got from the element
  inside it. Only a screen of many colours takes more, each further
  program reading as many rows as stay under its budget. Classic only as
  the per-call fallback.
- Rows by their place (`terminal_rows`): one program for every run, and
  the document's first row. Classic
  only as the per-call fallback.
- The caret event's read in a terminal, and the caret read before a held
  key: the caret read's program (`caret_read`), one program each.
- The outpost's terminal code (`crates/verbatim-outpost/src/terminal.rs`
  and `terminal/`): no UIA call of its own; it runs the snapshot above,
  for every read, the read before a held key, and the history a review
  command reads above the memory's rows.
  Around it, the text patterns are fetched once a node and kept (above),
  the class name comes from the element's cache, the console window's
  class from `GetClassName`, and the caret's positions are kept as the
  ranges the program returned, all without a call.
- The conformance runner (`xtask`) reads the caret's line, and the colours
  cell by cell, classically, as an independent reference against which
  the program's colours are checked; it is a measuring tool, not
  Verbatim.

No sequence was found that should be one program and is not.

## A stalled or exited provider

Verified against mockapp's `stall` command, which blocks the window
thread its apartment-threaded providers run on:

- `Execute` blocks while the provider is stalled, exactly as a classic
  call does. The UIA connection timeout (`IUIAutomation2`'s
  `ConnectionTimeout`) does not bound it:
  with a one-second connection timeout, both a program and the classic
  walk waited out a four-second stall and then succeeded.
- UIA's transaction timeout (`IUIAutomation2`'s `TransactionTimeout`,
  20 seconds by default, five in every Verbatim process,
  `verbatim_uia::TRANSACTION_TIMEOUT`) does bound it. With it at one second, `Execute`
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
pins a terminal's reads; `crates/mockapp/tests/terminal_snapshot.rs` pins
the snapshot's fields both ways, runs of rows by their place both ways,
and its colours as exact runs at the
width the last read found, at none and at a stale one, from a provider
that leaves a range's last character out of its colour answer; and
`crates/mockapp/tests/instruction_limit.rs` pins every program's
instructions, the colour read's worst case among them;
`crates/mockapp/tests/uia_tree.rs` pins the tree dump's calls by both
paths, an element gone during it keeping its later sibling by both, and
a dump of mockapp's window reading nothing that has UIA ask the window's
thread (`WM_GETTITLEBARINFOEX`), and `time_limits.rs` its failure at the
transaction timeout. `cargo xtask
regenerate-terminal-conformance-expectations` checks the colour read
against both real terminals after every chunk of the corpus, failing when
its runs differ from the cells' read one by one.
