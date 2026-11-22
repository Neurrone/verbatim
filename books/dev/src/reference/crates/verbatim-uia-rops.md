# verbatim-uia-rops

UIA remote operations: small programs that run inside a UIA provider's
process, so work that would take one cross-process round trip per step
takes one in all. Why Verbatim uses them is in `books/dev/src/explanation/architecture.md`,
section 4, "UIA"; how their caches are filled is in
`books/dev/src/explanation/design/uia-shapes.md`, section 7. The crate calls Windows' own API,
the WinRT class
`Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, as NVDA
does, not Microsoft's `microsoft-ui-uiautomation` library. It is
Windows-specific and GPL, so it ports NVDA's framework
(`source/UIAHandler/_remoteOps/`) freely, with attribution in each
ported file; where it follows Microsoft's MIT-licensed
`RemoteOperationInstructions.h`, that file carries Microsoft's notice.

The crate has three layers: the instruction set and a typed builder,
execution, and what runs on them. The request list's fusable operations
(`verbatim_model::batch`) each have an emitter, which writes the
operation's part of a program and declares its worst case; one program
builder puts a run of a batch's steps into programs from those emitters;
and one executor of ordinary calls runs the same operations call by call,
for the fallback rule, which is per operation. The reads that are not
operations of the list keep a remote program and a classic
implementation of the same signature behind one entry point that
chooses between them and answers with a `Path`: the focus ancestry, an
element found by its path, a focus's states after its registration, the
caret read, a terminal's snapshot, colours and search, and the shared
operation `TerminalRead`'s window read. Every UIA
path in the outpost that makes a sequence of calls to the application
runs through one or the other; "Where remote operations are not used"
lists the rest.

## The program builder and the ordinary-calls executor

`batch` runs the request list's operations on UI Automation subjects
(`verbatim_model::batch`): `Related`, `Descendants`, `Caret`,
`ReadText`, `ReadRange`, `CompareSelection`, `Select` and `Locate` with a
program arm and a call arm each, and `Read`, `Test`, `IsOneOf`,
`Activate` and `Follow` with a call arm only, each run alone by calls
(`batch/object.rs`): a `Read` adds reads a program would drop or cut its
segment for, a menu item's legacy state, a selected item's value and
refreshes of its own, and a `Test` reads only what its matcher asks next
(Claude, 2026-11-22). `Ancestors` is not among them: the outpost answers
it through its own reads. Its input is a `Segment`: a run of
consecutive steps of one parsed batch, the model's own `Step`s, with the
place of the first, the outcomes of the batch's steps before it, and a
resolver (`Resolve`) for what the steps name that the outpost holds: the
element an object step's subject names (`Element`, whose `calls_only`
sends the step to the calls whatever the mode, for a subject served only
by UI Automation's proxies inside the outpost or a step from a top-level
window), the text a text step's subject names, a position Core names or
a point of a step before the run (`HeldAt`, the position and any text
after it), a baseline, how a `ReadText`'s unit and details are read
(`UnitsRead`), the `Fill` each node reached takes, a walk's judgement
(the outpost's matchers evaluated in Rust on each node read), a
caret wait's starting point, and for the object arms the element filled
(`filled`, found again by its route when the outpost keeps none), the
node's id, the properties the outpost answers from what it holds
(`held`: its MSAA ids, whether its window is shown, its window's class,
a tree view item's children, and the text summary until the text reads
join the executor), a property of an element read in full (`value`, from
an `ElementRead`, the node minted by the outpost), and the focus
registration a `Follow` moves. A reference or condition on a step before
the run is decided from the earlier outcomes. Each resolution that
fails, a refusal or an anchor no longer held, is the step's outcome. The
executors read every other input from the step itself, so no second
description of the operations sits between the model and the programs
(Claude, 2026-11-22). The answer is a `Ran` for each step
(`Done(Found)`, `Unrun`, `Gone`, `TimedOut`, `Failed`) and whether the
window is to be read classically from now on.

`run_batch` cuts the segment into programs. A program ends after the
act, `Select`, so the act is its program's last step and runs only if
every earlier step of its program finished; after a walk down the tree,
whose later runs come before anything reads it; and before a step whose
declared worst case would take the program past the platform's limit of
10,000 executed instructions. A walk of the control view, since a
program's `Navigate` walks the raw view, runs by calls (Claude,
2026-11-21). Each step is lowered to a shape that holds no
object of UI Automation's (its imports by their place, a point's text
found by its prefix as a constant, since a program imports only
elements and ranges), and the shapes key the thread's `ProgramCache`, so
a caret key's starting read and each later one bind one built and
checked program to their own imports. Each emitted step runs in a try
block with its own status register, written "not reached" before any
step runs; a gate inside the program tests the statuses of the steps of
the same program it names, and a condition's found-or-not on one of
them is a register taken as it is or negated, never compared. Each node
a relation or a walk reaches fills its cache inside the provider with
the absent record of `RemoteCache`, so a property the element lacks
reads absent as it does by calls.

The fallback rule. After a run whose statuses are known (it ran, or
stopped partway with `InstructionLimitExceeded` or
`UnhandledException`, whose registers stay readable), the first step
that failed other than by its element being gone or the provider timing
out, or that the run did not reach, and every step after it in the
segment, run again by calls; a gone or timed-out step stands. A run
whose answer is lost (`ExecutionFailure`, or the call failing) leaves no
status: a program holding the act reruns its own steps, the act among
them, and the next program runs as usual; any other reruns the rest of
the segment. A program that could not run at all, an import refused or
an instruction unsupported, reruns the rest of the segment and marks the
window for classic reading. A run past the instruction limit is a
defect of an emitter's declared worst case, logged to
`verbatim_model::DEFECT_TARGET` and never run again. Every step run
again is logged through `remote_fell_back`, the one fallback log, to
`verbatim_model::REMOTE_FALLBACK_TARGET`. A controller-for relation
longer than a program reads (16) runs by calls with what follows it, a
size limit and no fallback.

A walk down the tree has a section of its own, "The walk down the
tree", below.

`run_calls` runs each step through the classic calls, cut per operation
and shared with the reads outside the list where they make the same
calls (`text::classic_units`, `classic_range`, `classic_location`,
`classic_caret`, `watch_check_classic`, the walk's `Uia::children`). The developer setting with remote
operations off, a window marked for classic reading, and the fallback's
reruns use it. mockapp's `batch.rs` runs every converted shape through
both executors and requires the same answer, and `batch_faults.rs`
makes each case of the fallback rule happen at a fault point
(`ProgramStepFails`, `ProgramStops`, `ProgramOverruns`,
`ProgramAnswerLost`); `instruction_limit.rs` pins each shape's count.

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
  opcode is `(patternId << 10) | vtableIndex`, `pattern_method`). The
  builder keeps `GetTextPattern2` and `TextPattern2GetCaretRange`, but no
  program emits them: the caret is the selection's start, as NVDA's is.
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
for those a typing user runs most: a batch's programs, keyed by their
shape, a terminal's screen and colours, and the caret read, whose
building costs 22 to 78 microseconds against 0.4 to 1.1 milliseconds for
`Execute`. Those are kept per thread (`ProgramCache`, at most 16 of each
kind, all forgotten when full) by a key naming everything the program was
built from but its imports, and a query with the same key runs the kept
program bound to its own imports (`Program::bind`, which checks they are
as many and of the same kinds). The cache is per thread
because a remote operation and its imports are per client and thread. A
program whose run on the thread found every opcode supported skips the
support check on later runs; any failed run has the next check again.
`work::programs_built` and `work::support_checks` count both per thread
for tests.

A failed run is an `Error`:

- `Unavailable`: Windows lacks the API.
- `Import`: an import failed. An element served by a client-side proxy
  (UIA's MSAA proxy inside Verbatim's own process) fails with
  `E_UNEXPECTED`, since there is no provider process to run in. A
  window's title bar is such an element inside a window whose provider is
  the application's, so a step whose subject a tree-walker step fetched
  and whose cached provider description names only UIA's proxies
  (`Cached::served_only_by_uia_proxies`) is marked by the outpost for the
  ordinary calls (`Element::calls_only`), rather than fail the import and
  fall back (Claude, 2026-11-21).
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

- `element`, a `verbatim_uia::Cached<FocusInput>`, whose control type,
  class and window are in its cache (decision 8 of `books/dev/src/explanation/design/uia-shapes.md`);
  `known`, the runtime ids the caller already has; and `depth_limit`;
- `fill`, the `verbatim_uia::Fill<Ancestor>` every ancestor is filled by,
  and `element_fill`, the element's and its value's selected item's, which adds
  the node's place since each is read alone (decision 9 of
  `books/dev/src/explanation/design/uia-shapes.md`);
- `require_focus`, for a live read of the element's `HasKeyboardFocus`
  (NVDA's check that a focus event is not stale), which stops the walk
  when false and lets it go on when the read fails, by UIA's timeout or
  any other error, a gone element's among them, as NVDA's
  `shouldAllowUIAFocusEvent` answers true on a COM error
  ([ADR 0001: Focus](../adr/0001-focus.md), "The listener's checks, ported
  from NVDA's focus handler"). The read is the remote program's first, and
  the classic walk's first call;
- for the classic walk only, `refresh`, for the focused element's own
  cache filled afresh, so the focus listener, the one caller that sets it,
  reports its value, states, and details as they are when the focus is
  read; `before_call`, told each call's name as it begins so
  the caller can time it; `nearest_window` and `value_available`,
  answers the caller already has, each saving a call; and `plain`, which
  says, by local calls, whether a window is a ghost's or a hung
  application's: a step that reaches such a window, other than the
  nearest window, ends the walk there with the parent not kept
  (`Ancestry::plain_window`), as NVDA makes that one step and reads the
  window as a plain window. The remote program cannot judge a window
  between its steps; a caller with such a window above the focus runs the
  classic walk instead. And `progress`, told each fact as the walk reads
  it (`Progress`: the element read
  afresh, the value's selected item, and each ancestor kept), so a caller keeps
  what was read should a later call fail. Nothing of the query's ends the
  classic walk early: each call waits for the provider, up to UIA's
  transaction timeout.

It answers `FocusAncestry::NotFocused` when `require_focus` is set and
the element does not have the focus, and otherwise an `Ancestry`:

- `ancestors`: the raw-view parents, nearest first, each a
  `Cached<Ancestor>`, ending at the top-level window of the element's
  process or below the process boundary
  (`books/dev/src/explanation/design/listener-without-connections.md` defines it;
  `verbatim_win32::provider::parent_across_process_boundary` judges it by local
  calls).
- `met_known`: which known runtime id the last ancestor has, when the
  walk stopped there; `depth_limited`: the walk stopped at the depth limit
  with ancestors left.
- `failed_hop`: how a step to a parent failed below the top, by UIA's
  transaction timeout or any other error, a `FailedHop`
  (`verbatim_uia::FailedHop`, re-exported): `Error` with the failing code,
  `UIA_E_TIMEOUT` among them, or `NoParent`. `Ancestry::ancestors_unknown`
  is true for a walk with a failed step, whose chain is not the whole
  chain read. Callers go on from the parent window of the outermost
  element's nearest window after a failed step, as NVDA's failed parent
  read goes on by `Window._get_parent`, and complete a walk cut by its
  depth limit by plain windows from that nearest window.
- `plain_window`: the window, read as plain by the query's `plain`, at
  whose step the classic walk stopped; callers go on from it as a plain
  window and by the window hierarchy.
- `selected_child`: for a combo box without the `Value` pattern or a
  looping selector, its first selected item, which is its value
  (`verbatim_uia::map::value_from_selection`); a list's or tab control's
  is never read, since a focused list's selected item is not spoken
  (Dickson, 2026-11-17). It is optional: when its
  read fails, as on an item gone, or in the classic walk times out, it is
  `None` and the rest is read without it, as NVDA's walk reads no
  selection.
- `window`: the native window handle of the element or of its nearest
  raw-view ancestor that has one (NVDA's `getNearestWindowHandle`).
- `element`: with `refresh`, the classic walk's fresh cache of the
  focused element.
- `legacy_state`: for a menu item, its legacy MSAA state bits, read by
  the remote program, `Some(None)` when unsupported; always `None` from
  the classic walk, whose caller makes one live read. A menu item that no
  pattern makes checkable, as a Windows Forms one, says it is checked only
  there.
- `element_table`: the table of the element, when it is a cell read with
  the theme's cell coordinates, a step of the program (`read_table`) and
  one call of the classic walk. `FocusQuery::is_cell` says whether the element is a cell
  when the caller's cache knows, so the program grows only for one; the
  program reads no table without it, and the classic walk takes it from
  the element read afresh.

Only reaching the top ends a walk as known. In the classic view a
top-level window's parent is the desktop root, so the classic walk stops
below the root, and any step below it that fails or answers none sets
`failed_hop`. Inside a program the parent of a top-level window, and of a
window below the process boundary, is null, so the program's walk ends at
any null parent and then checks, with local calls
(`verbatim_win32::provider::is_top_level_window`,
`verbatim_win32::provider::parent_across_process_boundary`), that the last element
read has as its window one of those two; otherwise it sets `NoParent`. An
element's window is the one its runtime id `[42, handle]` names, since a
read inside the provider can leave the handle out (`books/dev/src/reference/uia-bugs.md`,
"Inside a remote operation, conhost's window has no handle or title"), or
the one its cached `NativeWindowHandle` reports. A provider whose
`Navigate` fails reaches the client as no parent.

The remote program reads `HasKeyboardFocus` and halts when it is false.
For a looping selector or a combo box the program reads
`Selection2FirstSelectedItem` (`SelectionPattern2`), else the `Selection`
property; for a combo box it asks `IsValuePatternAvailable` first, all
inside a try block whose catch leaves the selected item out. Then
it walks with `Navigate` to the parent, filling each ancestor's cache
inside the provider and stopping at the first whose runtime id, turned
into a string with `Stringify` (`[42,14681214,4,5]`), is in a string map
of the known ids; `runtime_id_key` makes the same string on Verbatim's
side. While the window is unknown each ancestor's `NativeWindowHandle` is
read as the walk passes it, and a walk that stopped before finding one
goes on up, returning nothing more, until it does.

The classic walk is the focus listener's read, which makes no remote
operation (`books/dev/src/explanation/design/listener-without-connections.md`, section 2), and
the outposts' fallback: the same live reads, the value's selected item through
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
its error is returned. In the classic walk a keyboard focus read that
times out lets the focus through, and a hop that times out ends the walk at a failed step
(`Ancestry::failed_hop`), so a walk that hit the timeout is never
reported as a short but complete ancestry; a selected item that times
out or is gone is left out and the walk goes on. It fails only when the
element's fresh cache, a cache request, the desktop root or the tree
walker fails. NVDA makes each call site check `remote.isSupported()`
and run its own classic code instead; here the choice and the fallback
live in one function, so a failure is handled the same way everywhere.

### Cached properties filled remotely

A cache filled by a remote program stores a property's default where a
locally built cache stores "not supported", so a read that ignores
defaults cannot tell the two apart (`books/dev/src/explanation/design/uia-shapes.md`, section
7). `ValueIsReadOnly` and `RangeValueValue` are therefore read only when
`IsValuePatternAvailable` and `IsRangeValuePatternAvailable` say the
pattern is there. `IsDataValidForForm` and `IsEnabled` have no pattern,
so the program leaves each out of the cache of an element that does not
support it (the console host's own provider for its window supplies no
`IsEnabled`, which a filled cache would hold as false, disabled): since
`PopulateCache` replaces an element's cache, it builds one request per
combination of such properties (`verbatim_uia::LEFT_OUT_WHEN_UNSUPPORTED`,
two properties, so four requests), picks one per element after an
`IsNotSupported` test of each, and returns which were left out
(`verbatim_uia::Absent`), so the element, wrapped as a `Cached<S>` with
that record (`Fill::claim`), answers the property as "not supported"
without asking UI Automation (decision 3 of `books/dev/src/explanation/design/uia-shapes.md`).
Every program here that fills a cache builds its requests from the
`Fill`'s properties and patterns (`RemoteCache`), and the classic
fallbacks fill with its request, so both come from one shape.

## Layer 3: a terminal's snapshot

`terminal_snapshot(uia, query, remote)` (`TerminalSnapshotFn`) is the
read of `books/dev/src/explanation/design/terminal-model.md`, "The read", which gives the
reasons for its fields: the outpost reads every focused terminal with
it, and `cargo xtask regenerate-terminal-conformance-expectations` reads
the real terminals with it. A `SnapshotQuery` names the element and its
text pattern; `start`, where the last read said to start (none for a
first read); `screen_rows`, the screen's height as last found, 0 when not
known; `whole`, to read the whole text instead; `above`, how many rows
above the start to begin (`books/dev/src/explanation/design/terminal-model.md`, "Positions");
`caret`, a `CaretLineQuery`, for the caret and its line; and `colours`,
for the screen's colours, with the screen's width as last found.

The unit of reading is one `GetText`: both terminals answer one from one
locked state of their buffer, while separate calls are not atomic with
each other. So the snapshot's text is the only thing anything spoken or
counted comes from, and every other field is a hint. The `Snapshot` gives
text as the provider gave it, padding and line breaks included (the
conformance expectations record each provider's,
`books/dev/src/reference/crates/verbatim-term-sim.md`):

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
  (`books/dev/src/explanation/design/terminal-model.md`, "Lists and colours").

Without colours the program has no loop: 50 instructions for a first read
and 90 with a start and the caret.

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
programs and 31 to 72 milliseconds (`books/dev/src/reference/performance.md`, "A terminal's
colours").

### A command mark's line

`terminal_find(uia, query, remote)` looks for a command mark's line by its
first row's text (`FindQuery`): outward from the row `from` rows above
where the next read starts, alternately the next match at or below it and
the next above it, at most `FIND_MATCHES` (20) matches that start a row
over at most `FIND_CALLS` (100) `FindText` calls, so that a needle found
mostly within rows ends rather than walking the whole history
(`books/dev/src/reference/performance.md`, "A terminal's upward search"). Each match that
starts a row gives a `FindMatch`: its rows above that start, and the text
of `rows` rows from it, for the caller to check by its line's hash. A
failing call means nothing more is found in that direction. The program
is built for each search, about 30 instructions a `FindText` call.
Windows Terminal answers a search that finds nothing with a range on
which some calls fail with `RPC_E_SERVERFAULT`, which ends a remote
operation whatever try block holds the call, so the program makes none
of those calls: it first searches the whole document and compares the hit
with the range a search for text no terminal holds answers, ending with
nothing found when they are equal, and compares each later hit with the
span searched before calling it, which fails, closing that direction,
when the hit is the not-found range (`terminal_find.rs`'s module
documentation).

### The window read

`terminal_read(uia, query, remote)` (`TerminalReadFn`) is the shared
operation `TerminalRead`'s read (`verbatim_model::TerminalWindow`). A
`ReadQuery` names the element and pattern, the screen's height as last
read (0 at first), up to two end marks as held ranges, "Lines spoken in
full" as `opening`, and the caret's `CaretLineQuery`. One program, with
no loop, answers a `Window`: the rows from twice the height above the
document's end to its end in one `GetText` (from the screen's top when
the height is 0), how many rows above the end the window and the
screen's top start, the screen's text and top, its top row as the
provider expands the top to a line (for the width, a wrapped line's
first row alone), whether the screen starts
at the document's start, the document's end collapsed, its first row and
its row count (one move by line from the end to the start), and for each
mark a `MarkRows`: refused when it lies past the document's end or the
provider will not compare it, otherwise the rows from it to the end, with
the `opening` rows after it when more than those and the window follow,
and the caret's rows to the end, counted from the selection's first
range as the snapshot program counts them. The classic implementation
makes the same calls. The program executes 78 instructions naming no
mark and 128 at its cap, both marks with their opening rows
(`instruction_limit.rs`).

Two reads sit beside it, outside the program. `terminal_row_colours`
reads the colours of screen rows, counted from the screen's top by the
line unit, so a wrapped line's rows are each read as a row, with the
colour read's emitter and classic checks (`emit_rows`,
`classic_rows`). And `terminal_read_find` searches for a mark's line by the
classic search alone (`classic_search`, taking the nearest match):
output that trims the history between a search's calls makes a miss
Windows Terminal's nowhere range, whose calls end a remote run, while the
classic calls answer it as not found.

## Layer 3: the caret's emitters

`caret` holds what the program builder's `Caret`, `ReadText` and
`CompareSelection` emit and read classically (`emit_caret`, `emit_runs`,
`emit_changes`, `classic_runs`, `classic_changes`), and a terminal
snapshot's caret and line (`CaretLineQuery`, `CaretAnswer`). Every caret
read is a batch's steps, a focus's among them: the focus's caret read and
report focus's text summary are a `Caret`, a `ReadText` of the line and
a `ReadRange` of the selection run by the program builder, so no caret
program is written by hand (Claude, 2026-11-22).

The caret is the start of the selection's first range, as NVDA's caret
is (`NVDAObjects/UIA/__init__.py` 488 to 498), whichever end of the
selection the provider's own caret is at. A program gets the text
pattern with `GetTextPattern` (opcode 10014) and reads the selection
with `GetSelection`. Formatting is read by UIA's format unit, as NVDA
reads it, until the span's end or `MAX_RUNS` (64) stretches. A stretch
with an attribute that answers "mixed" (`IsMixedAttribute` in the
program, `verbatim_uia::is_mixed` classically) is walked again by words,
and a mixed word by characters, as NVDA walks a mixed stretch by finer
units, so a provider whose format unit does not end where an attribute
changes, such as both terminals' for italics, still has each part read
with its own value. Values are appended as the provider gave them and
typed by the caller, so the program spends no instructions on testing
types. The annotation types are asked of the stretches only when the
span has some. A selection that moved is compared inside the provider by
the text protocol's rule, each change with its text up to its limit, and
the texts compared are read up to `MAX_COMPARED_TEXT` (513 UTF-16 code
units, one more than the 512 characters from which a selection is
spoken as a count).

Classically the same reads are made one call at a time, a stretch's
attributes together in one `IUIAutomationTextRange3::GetAttributeValues`
call as NVDA reads a range's formatting
(`verbatim_uia::text::TextRangeExt::attributes`). An attribute whose read
fails is not supported on both paths.

## Layer 3: a caret wait's check

A caret wait's check (`src/watch.rs`; the request list's
`WaitForCaret`) is the first step of each poll's program,
`batch::Input::WaitCheck`, emitted by `watch::emit_check`, with
`watch_check_classic` the same reads call by call for `run_calls`. A
`WatchQuery` names the text element, the caret and the selection's ends
at the key's starting point, and whether to read the word at the caret
(a deleting key's check, 30 ms after its pass). A `WatchAnswer` says
whether the caret and the selection's ends moved from the starting
point, a compare the provider refuses counting as a move, gives the
caret's line with the caret's offset, the word when asked, and whether
the caret read again at the end moved while the check read. The check's
result (`batch::Checked`) is found when the caret or the selection
moved or at the bound, so the batch's later reads, gated on it in the
same program, run then and cost the poll no round trip of its own. The
comparisons of text (the characters beside the caret, the line, the
word) are the caller's, made on the texts returned. The poll's
instructions are pinned by `crates/mockapp/tests/instruction_limit.rs`
(`wait_polls_execute_exactly`).

## The text emitters

The text operations' emitters and their classic calls live in
`src/text.rs`, shared by the two executors. A text step starts from a
`TextFrom`, which the outpost resolves from the step's point:

- `Caret`, `SelectionStart`, `SelectionEnd`: read as the caret read
  reads them, once however many points of a program use them.
- `Start` and `End`: the document range collapsed to one end.
- `At(Position)`: a range and the end of it the outpost holds.
- `After { from, prefix, counts }`: a point some text after a held one,
  the text between them being `prefix`. Each of `counts` (UTF-16 code
  units, code points, grapheme clusters) is tried in turn, moving a
  copy's end by that many characters and comparing the text passed with
  `prefix` inside the provider; the last tried is kept when none
  matches. This is how a position Core found inside a chunk is resolved
  in one round trip.

Every point is returned (`FoundPoint`) so the outpost can hold it.

- `ReadText` (`emit_units`, `classic_units`): from the point, an
  optional `Movement` (`By(unit, count)` from the unit's start, its count
  negative for a backward move even from a provider that answers a
  positive one, as NVDA corrects it; or `Document(count)`, to an end),
  then the unit containing the point reached: its range, its text up to
  the step's limit, the point's offset, and its language (`Culture`, as a
  BCP 47 tag from `verbatim_uia::text::locale_name`). With a count above
  one it reads on until it has the count, the text reaches its total
  limit, or a move does not move: say-all's twenty lines in one round
  trip. The language is read once over the whole read, and unit by unit
  only when that answers "mixed" (`verbatim_uia::text::Language`). A unit
  is expanded from a copy whose start is the point, never collapsed
  first, since `ExpandToEnclosingUnit` normalizes a range from its start.
- `ReadRange` and `Select` (`emit_between`, `classic_range`): two
  points, ordered by comparing them, and the text between them read up
  to a limit, or selected; with `Ends::through`, through the character at
  the later. Two equal points selected move the caret. A selection the
  provider refuses is caught inside the program and answered refused.
- `Locate` (emitted in the program, `classic_location` by calls): the
  character at the point and its bounding rectangles.

## Layer 3: an element found by its path

`element_by_path(uia, query, remote)` (`ElementByPathFn`) finds an
element again from one the caller has, `PathQuery::root` (a
`verbatim_uia::Live`), by `PathQuery::path`, the runtime ids of the
raw-view elements below it, outermost first: at each level the children
are looked at in order until one has the next runtime id. The element
found is a `Cached<Node>` filled by `PathQuery::fill`. The program does
the whole path in one round trip; the classic walk takes one call per
child it looks at (`verbatim_uia::Live::step`), one more for the fill,
and checks the caller's `out_of_time`, when it gives one, between calls. Both compare children by runtime
id alone and fill no other cache, because a top-level window's first
child is its title bar, whose location UIA asks of the window's thread
(`WM_GETTITLEBARINFOEX`), which Windows answers for one window at a time
across the desktop. The outpost uses it to find its own copy of an
element the focus listener read ([ADR 0001: Focus](../adr/0001-focus.md),
"The outpost's own element").

## Layer 3: a focus's states after its registration

`focus_states(uia, query, remote)` (`FocusStatesFn`) answers the outpost's
read of a UI Automation focus once its event registration is in, which
NVDA's `event_gainFocus` makes before it speaks the focus
(`NVDAObjects/UIA/__init__.py` 1623 to 1626): `StatesQuery::focus`'s cache
filled anew by `StatesQuery::fill`, and the children of each of
`StatesQuery::dialogs`, the dialogs the focus enters, filled by
`StatesQuery::children` as `verbatim_uia::DialogText`, from which their
text is gathered. The program reads them all in one round trip, up to
`DIALOG_CHILDREN_PER_RUN` (60) children in all, each dialog's children in
a try block; a dialog whose children fail or do not fit is answered
unread (`None`), for the caller to read classically. The classic
implementation refreshes the focus and reads each dialog's children, one
call each.

## The walk down the tree

A `Descendants` step (`batch::Input::Descendants`, `batch/walk.rs`)
walks breadth first from its starts, capped at its depth and node count,
answering its nodes in tree order, each with its parent's place and its
start's, and whether the walk was complete; the tree dump, the tray
list and the controlled selection's search are each one on a UI
Automation subject (an MSAA subject's walk is the outpost's own). The outpost's matchers (`Descend::judge`) judge each node
between runs: a pruned subtree is not read, and a walk that prunes reads
one level a run, so that each node is judged before its children are
read. The walk stops once nothing waiting could come before the last
node its limit answers. The walk's first run is fused into its program,
which ends after it. By calls, each element's
children are read in one call for the raw view, as NVDA reads an
object's children (a cache request scoped to the children, then
`GetCachedChildren`), or a walker step a child for the control view; an
element whose read answers gone is left childless, and a child gone as
the walk steps to it ends its parent's children there, those before it
kept (in the raw view, whose one call fails for it, the children are
read again a walker step a child to find them), each leaving the walk
incomplete. A program's run reads up to `CHILDREN_PER_RUN` (60) children
from up to 64 waiting elements, each element's step to its first child
and each child's read and step on in a try block of its own, and at the
run's end reads every element it answers live for its name, because a
gone element among a run's results fails the whole run: an element whose
step to its first child failed, or found gone at the end, is given back
as none and left out with its subtree; a child whose read or step on
failed ends its parent's children there, as by calls, those before it
kept; each leaves the walk incomplete, its later siblings kept. A read
that failed other than by its element being gone falls back to calls,
logged as any remote failure is. An element with more children than a run has room
for is read by calls, and a later run that fails as a whole leaves the
rest of the walk to calls.

## Fallback rules

A batch's steps fall back by the rule of "The program builder and the
ordinary-calls executor" above, step by step. The reads outside the
list choose by their entry point, the same way for each:

- A window without a native UIA provider is read through MSAA, so its
  focus never reaches this choice.
- An import that fails (`Error::Import`, a client-side proxy) marks the
  window for its lifetime, and the outpost then reads it classically
  without trying, a batch's segments by `run_calls` included; the mark
  is forgotten when the window is destroyed. The window marked is the
  one the read already holds: the window with the keyboard focus for a
  focus-now read, the path's window for an element found by its path,
  the top-level window of a batch segment's subjects, and the tracked
  focus's window for the caret read and a terminal's screen.
- A run that fails otherwise is answered classically for that call
  (`Path::Fallback`) and logged as a warning with the failing
  instruction and its source line.
- A gone provider and one that times out (`UIA_E_ELEMENTNOTAVAILABLE`,
  `UIA_E_TIMEOUT`) are returned by every entry point without the
  classic calls, which would fail the same way after a second wait; the
  outpost answers them as it answers a gone or unresponsive element.
- A developer setting (`uia.remote_operations` in `settings.toml`, on by
  default) forces the classic calls everywhere; the supervisor passes it
  to each outpost as `--classic-uia`.

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

Programs are not resumable: their work is bounded by their queries. The
program builder sums its emitters' declared worst cases and ends a
program where the next step would pass the limit, so a batch's program
that overruns is a defect, as "The program builder and the
ordinary-calls executor" says. Every program's worst case stays under
half the limit (`books/dev/src/reference/performance.md`, "The
instruction limit") except the caret read's walk of a span whose every
stretch is mixed, about 8,000, which still runs under it. A read outside
the list that exceeded the limit would be answered classically for that
call.

## Where remote operations are not used

- Reading the focused element (`GetFocusedElementBuildCache`): one call,
  and the element is what a program would start from.
- A node's text patterns, fetched once per node the first time its text
  is read: the classic reads need the pattern objects. Making the fetch
  lazy, so a remote read never makes it, is a possible follow-up.
- Activation (`Uia::activate`): the pattern methods (`Invoke`, `Toggle`,
  `Select`) have no instructions.
- The menu item's legacy checked state for the focus-now answer and an
  object reached by navigation: one call.
- A dialog's own text through UIA: already one call.
- A `Descendants` walk of the control view, since a program's
  `Navigate` walks the raw view.
- The arbitration probe and the checks of a console's and a Windows Forms
  window's provider: window messages and one-time checks made before the
  window's backend is known.

A terminal's reads are each one program, and the outpost's terminal code
makes no UIA call beyond them. The conformance runner (`xtask`) reads
classically, as an independent reference for the program's colours.

## A stalled or exited provider

`Execute` blocks while the provider is stalled, as a classic call does.
UIA's connection timeout does not bound it; its transaction timeout
(`IUIAutomation2`'s `TransactionTimeout`, process-wide, ten seconds in
every Verbatim process, `verbatim_uia::UIA_TRANSACTION_TIMEOUT`) does, with
`ExecutionFailure` and extended error `UIA_E_TIMEOUT`, as a classic call
fails in the same time. So the outpost's worker calls `Execute` directly,
and an abandonment for the foreground covers both the same way; a program is one wait where a
classic walk against a provider that stalls partway is one wait per
remaining hop. When the provider's process has exited, `Execute` returns
at once with `UIA_E_ELEMENTNOTAVAILABLE`.

## Costs

In a release build against mockapp: five ancestors to the top-level
window take 3.4 ms remotely and 25 ms classically, most of the classic
time being the last hop to the desktop root; a list with its selected
child and one ancestor 3.4 ms and 17 ms; one live property read 0.31 ms.
The caret and say-all reads are in `books/dev/src/reference/performance.md`,
"Wall-clock gain".

## Tests

Unit tests cover each instruction's bytes and the builder's offsets,
constants, and source locations. The mockapp tests run both executors,
or both implementations of a read outside the list, against real
providers and assert that they agree: `batch.rs` (every converted batch
shape, a shape's program bound again, absent properties),
`batch_faults.rs` (each case of the fallback rule at a fault point),
`remote_ops.rs` (the focus ancestry, a navigation step, cached
properties, a stalled and an exited provider), `call_counts.rs` (the
calls of each read), `text.rs` (the caret read and the text steps),
`watch_checks.rs` (a wait's polls), `terminal.rs`,
`terminal_snapshot.rs` and `terminal_window.rs` (the window read, its
marks, a wrapped line's rows and colours, and the classic search against Windows Terminal's
two kinds of miss), `controller_for.rs`, `uia_tree.rs` and
`time_limits.rs` (the tree dump), and `instruction_limit.rs` (the
platform's limit and every program's count, a batch's shapes at their
caps among them), all under `crates/mockapp/tests/`. `cargo xtask
regenerate-terminal-conformance-expectations` checks the colour read
against both real terminals, failing when its runs differ from the cells'
read one by one.
