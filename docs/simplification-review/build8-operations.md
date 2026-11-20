# Build 8, the design study, part one: the closed list of operations and the batch

Draft of 2026-11-20 against main at 368d3b5b. Scope: only what Dickson rules now because it is a public contract (plan.md 83, builds.md 985): the closed list of generic operations on objects and text, and a request as a batch of them. The program builder and executor (question 3), the property table (question 4), the object abstraction (questions 6 and 7) and the second proof are later parts of the study. Line numbers are today's; the request, reply and text-op lines match the model record's count table (`docs/simplification-review/merged-model-protocol.md` 257-263).

Terms used below, defined once:

- Operation: one generic step a reader performs on one object or one object's text, such as reading properties or reading a unit of text.
- Batch: one request from Core to one reader, holding an ordered list of operations, sent together and answered together in one answer (Dickson's ruling, plan.md 45).
- Subject: how an operation names its object: a held node, a kept element, MSAA ids, or the result of an earlier operation in the same batch.
- Result reference: a later operation naming an earlier operation's result by its place in the batch, so that dependent steps need no second round trip (plan.md 73).
- Condition: an optional test on an earlier result that decides whether an operation runs (open point 1).

## What Dickson ruled on 2026-11-20 (section 6 has each with its example)

1. An operation may carry a condition on an earlier result, which keeps today's guarded reads (focused state, the identity check, the hidden dialog) in one round trip with no extra calls; dropped later only if the round-trip measurement shows guarded reads are rare.
2. The extension surface imports in M5 the reads the Extensions guide's imports page lists, each one call that Core turns into a batch; batches are not exposed to modules, and exposing them later adds to the contract without changing it; one matcher grammar serves `Find` and conditions.
3. The notification area's two reads are converted in build 8.
4. The batch's typing, in section 1: built through typed handles in Core and parsed, not validated, where it arrives on each side.

Everything else below is decided by the study with its reason, open to his objection; section 7 lists those decisions.

## 1. The batch

### Shape

A request stays an `Ask` (request.rs 54-67) with its number, its window for the hang check, Core's backend verdict and, replacing `Request`, a batch: an ordered list of operations, capped (section 7). A batch does not save round trips against today: today's purpose-built requests already run many calls in the reader and answer once. What it removes is the kind per behaviour; result references keep a generic list at one round trip where a dependent step would otherwise need a second. Two fields move from request kinds onto the `Ask`: whether it is the event queue's or from outside it, today derived from the kind by `outside_the_queue` (request.rs 994-1004, ending in a wildcard, model finding 14), and the nodes and anchors another request holds while this one is out, today `MsaaRun.spare` (request.rs 635).

The answer keeps ADR 0003's five outcomes for the batch as a whole (ADR 0003 line 30: "the request carries what it needs and gets one answer, with one of five outcomes"). Not responding, the reader ending, Core's stop and the cancel apply to the batch, since the batch is one call sequence on one worker. A result carries one outcome per operation, in order:

| Operation outcome | Meaning | Replaces |
| --- | --- | --- |
| Done(result) | the operation's result | each `Reply` and `TextReply` own variant |
| Refused(reason) | the object cannot do this: unsupported, no text, no such unit (with the unit); the backend refusals (`ReadThroughUia`, `ReadThroughMsaa`, request.rs 884, 887) stay outcomes of the whole batch, since the verdict is the `Ask`'s | `Refusal::Unsupported`, `TextReply::Unsupported`, `NoText`, `UnsupportedUnit` (text.rs 1229-1234) |
| NotThere(node or anchor) | its subject's node, or a text anchor it names, is no longer held or no longer exists | `Outcome::NotThere` per read, `TextReply::Gone`, `AnchorLost` (text.rs 1236-1238) |
| Failed | the call failed or timed out inside a window not reported hung | `TextReply::Unanswered` (text.rs 1240) |
| Skipped(why) | not run: an operation it names did not finish Done, or its condition did not hold | new; today encoded by `CandidateFacts`, `CandidateSame`, `HiddenDialog`, `Watched::BoundUnread` |

So `TextFailure` and `into_failure` (text.rs 1248-1297), the second failure vocabulary of model finding 7, go.

### How results are addressed (question 2)

- Each result is addressed by its operation's place in the batch. A subject may be `Result(i)`, the node operation i read, or `Item(i, j)`, the j-th node of a list operation i answered. A text point may be `Of(i, edge)`: the start, the end or the reported point of the chunk or caret operation i answered, beside today's `TextPoint` forms (text.rs 88-105).
- References point only backwards, and three layers make a wrong batch impossible or refused (Dickson agreed the shape on 2026-11-20, "as long as we parse, not validate at the boundary where the data comes in"):
  - In Core, by construction. A batch is built inside a closure, `Batch::build(|b| ...)`, whose builder methods return typed handles, `Ref<Caret>`, `Ref<Chunks>`, `Ref<Node>`, `Ref<Nodes>`, each branded with the closure's invariant lifetime, so a handle cannot leave its batch. An input that names an earlier result takes a handle of the one kind that result can be (`At::CaretOf` takes only a `Ref<Caret>`), and a handle exists only once its operation has been added, so a reference of the wrong kind, a forward reference and another batch's handle do not compile.
  - At the reader, by parsing. The batch arrives as data, so the reader parses it once at its boundary into the same typed form: every reference points to a smaller index and to an operation whose result is the kind it needs. A batch that does not parse is refused whole as a protocol error, logged as a bug, and none of it runs; nothing is defaulted (CLAUDE.md, "Code reaches no fallback that could only be a logic bug").
  - In Core, by parsing the reply. The reply is parsed against the batch that was sent, once, where it arrives: one outcome per operation, each `Done` of the kind its operation produces. A reply that does not parse is the reader's failure. Core then reads each result through the handle, `answer.get(word)` giving `Outcome<&Chunks>`; the accessor's one mismatch arm is unreachable by that parse, written once for every batch. A reply type generated per batch shape, which would remove that arm, is not built: its macro or code generation costs more than the one parse.
- A failed operation does not end the batch. Every operation that names it, by reference or condition, is Skipped; independent operations still run. Reason: a focus read's foreground read and its caret read do not depend on each other, and "complete or nothing" for a focus read stays Core's judgement over the outcomes (ADR 0003 line 200), not a rule in the reader.
- Node ids minted inside a batch are minted by the outpost unstamped and stamped by Core from the pipe the answer arrived on, as today (lib.rs 178-181: "Core attaches it to everything an outpost sends according to the pipe the message arrived on, never from the message body"), because a compromised outpost must not name another generation. With every node in a reply inside an operation outcome of one of three result types (node, list, ancestors), the four hand-written walks (`Ask::unstamp` request.rs 926, `Answer::named` 1020 with its clone at 1021, `Answer::visit_nodes` 1063, `Named::text_reply` protocol/mod.rs 721) become one walk each way over the batch.
- Anchors cross as today: the outpost mints a `TextAnchor` (text.rs 63) per node into each text result and keeps it until Core's held view releases it (ADR 0003 line 47). An anchor minted earlier in the same batch is used by reference, so a caret key's starting point and its first read need not wait for the anchor to reach Core.

### Is a wait an operation? (question 1)

The caret watch is an operation: `WaitForCaret`, the one operation in the list that waits, run from the outpost's table of waiting requests that ADR 0003 decided (line 45: "An outpost has a table of requests waiting on an event or a time, such as a caret key's watch, which Core can cancel"). The operations after it in its batch run once it ends, so the watch's final read is ordinary `TextRead`s, and today's `WatchFinalRead` and `Watched` variants (text.rs 792, 850) become the wait's result plus which later operations ran. At most one waiting operation per batch; a later key or a pending focus cancels the batch as it cancels the watch today.

It is not a registration that tells: that would send every caret check or event to Core, the option ADR 0003 rejected (line 215: "each check would be a round trip to Core and a wake of Core, up to ten for each key"). It is the mechanical work ADR 0003 line 36 names, "compare a caret with a starting point": the effect test is the model's shared `had_effect` (outpost text/check.rs 184, 203), chosen by the key's motion that Core names.

The show wait (`AwaitShown`) is not in the list: build 2 replaces it with an ordinary tell of the foreground window's show (builds.md 246-247). The one registration that tells stays `Follow`, as an operation whose result is "applied" and whose events arrive as tells.

## 2. The closed list

Subjects an operation may name (not operations themselves):

| Subject | What it names | Today |
| --- | --- | --- |
| `Node(id)` | a node the reader holds | `ObjectRef::Node` and every `node` field |
| `Kept(listener, token)` | an element the listener kept for a tell | `ObjectRef::Kept` (request.rs 107) |
| `Msaa(ids, redirect)` | an MSAA object by its event's ids, with or without the focus redirect | `ObjectRef::Msaa`; `RunCandidate` with `CandidateKind` (asked.rs 1237-1243 chooses the redirect) |
| `Window(handle)` or `WindowPath(classes)` | a window's own object, by handle or by a path of class names from a top-level window | the tree dump's main window; `Shell_TrayWnd` and `TrayNotifyWnd` (shell_items.rs 8-10); M5's `find-window` |
| `Focus`, `Foreground` | the application's current focus as `CurrentFocus` takes it (asked.rs 1345), the foreground object | `CurrentFocus`; `read::foreground_object` (asked.rs 1304, 1396) |
| `Route(route)` | the outpost's own copy of an element the listener read, found by runtime ids | `CompleteUia.route` (request.rs 516; `Route` at 486) |
| `Result(i)`, `Item(i, j)` | an earlier result | new |

A text subject is a node with the provider Core names (`TextProvider`, text.rs 579) and, for a caret tell, the told element the outpost adopts when the node's own has no text (`SelectionRead.adopt`, text.rs 926).

The operations:

| Operation | Inputs | Result | Replaces |
| --- | --- | --- | --- |
| `Read` | subject; a property set | the node: id (none from the listener), nearest window facts, values | `Properties`; `ReadObject`'s object; the node part of every focus read |
| `Step` | subject; parent, first child, last child, next or previous; set | the node reached, or none at an edge | `Navigate` |
| `Ancestors` | subject; until: ancestors Core knows, the nodes of an earlier result, a matcher, or the top; a depth limit; set | the ancestors nearest first, and how the walk ended: met (which), top, process boundary, continued from a window, depth limit | `ReadObject.walk`; the walks in `CurrentFocus`, `MsaaRun`, `CompleteUia`; `RouteEnd` (request.rs 461) as the ending; "is inside" of `RelationQuery` |
| `Find` | subject; children, or descendants with a depth and node cap; an optional matcher; a limit; set | the matching nodes in tree order, each with its parent's place in the list; complete or failed | `Find`, `Children`, `ReadObject.descendants`, `DumpTree`, `ShellItems`, `ShellItemPosition` |
| `Compare` | subject; a list of subjects | the place of the one it is, or none | `RelationQuery::OneOf` (request.rs 145) |
| `Activate` | subject | the action's name, if any | `Activate` |
| `Follow` | a node with its event group and extra node, or none | applied | `Follow` (request.rs 366, 381) |
| `Caret` | text subject | the caret and the selection's ends as held anchors, and the read's time | the point `StartingRead` mints; the baselines `ReadSelection`, `MoveCaret` and the focus caret mint |
| `TextRead` | text subject; start point; optional movement; unit; count (1 to `MAX_READ_AHEAD`); details | units moved, and the chunks | `Read`, `ReadAhead`; the line, unit and word of `StartingRead`; the watch's final read; the focus caret's line |
| `TextRange` | text subject; two points; optional "through" ends; details | the text, and whether it was cut | `ReadRange` |
| `SelectionChange` | text subject; the baseline Core names | the difference, the new baseline, an edit control's offsets | `ReadSelection`'s comparison; the changes of `Select` |
| `Select` | text subject; two points; optional "through" ends | done | `Select` |
| `MoveCaret` | text subject; a point | when the move began | `MoveCaret` |
| `Location` | text subject; a point | screen point if known, and offset and length where the text counts by offsets | `Location` (both of its replies) |
| `FindText` | text subject; start point; text sought; direction; most matches; rows read from each | the matches as anchors | `TerminalFind`, settled in build 9 |
| `WaitForCaret` | text subject; starting point; key's motion; when the key passed; poll; bound; whether UI Automation events count | effect seen, or bound passed | `Watch` (text.rs 818) |

Sixteen operations. Every reply variant that is a "facts" answer becomes properties: the focused state over an object and its ancestors (`has_focused_state`, asked.rs 1247-1251), a dialog's text (`describe_dialogs`, asked.rs 1293, 1398), a node's top-level visibility (build 2's hidden check), whether a runtime id was reused, and whether a value comes from the selection each become an entry of the property table, read as a property is read, some of them walks (the table's `cost` column, property.rs 184-194, has no reader today).

A condition, if Dickson allows it (open point 1), is one of three tests on an earlier result: it matches a matcher; it was found (a node, a non-empty list, a met ancestor, a seen effect); or the negation of either. No loops, no branches with bodies, no variables: the batch stays a straight list in which an operation may be skipped.

## 3. What each present kind becomes

### `Request`, 15 variants (request.rs 228-345)

| Kind | Becomes |
| --- | --- |
| `ReadObject` (231) | a batch: `Read`, optional `Ancestors` until the known ancestors, optional `Find` of descendants, optional relation. `RelationQuery::ControlledBy` is `Read` of the focus's controller-for property, then `Ancestors` of the object until that result's nodes, including the object itself; `Selection` adds a `Compare` with the nodes when that walk met nothing. `Purpose` goes: `Focus` becomes the kept element's ordinary "read" mark, `Change` a condition that the relation was found (request.rs 180-185), and `Selection` goes because every read answers the fresh name, which the listener's cache request already fetches (model record 331-332), and Core chooses |
| `Properties` (233) | `Read` |
| `Navigate` (240) | `Step` |
| `Find` (249) | `Find` with a matcher and limit 1 |
| `Children` (262) | `Find` with no matcher |
| `CompleteUia` (269) | a batch on subject `Route`: `Ancestors` from the route's end until the known ancestors; for a focus, `Follow` before `Read` of the states, as today's order (request.rs 498); `Caret` and `TextRead` of the line; for a selection's item, the controller-for walk as above. The listener's values (`CompleteUia.read`) stop crossing back: Core already holds them and merges |
| `MsaaRun` (271) | a batch on subject `Msaa(ids, redirect)`: `Read` of the facts (focused state, role); the full read (`Read`, `Ancestors`, the foreground when the walk met nothing, `Caret`, `TextRead`) under conditions: focused for `FullIfFocused`, not reading as the identity for `FullUnlessSame` (ADR 0004 line 28: "The outpost makes the comparison against the values Core names and answers 'the same' as a fact"), none for `Facts`. `Depth`, `CandidateKind` and `HiddenRoot` go |
| `CurrentFocus` (273) | the same batch on subject `Focus`; `FocusDepth` is the focused condition |
| `Text` (293) | the container goes; its ops are below |
| `Activate` (300) | `Activate` |
| `Follow` (305) | `Follow` |
| `DumpTree` (309) | `Find` of descendants of the main window with depth 64 and 4,096 nodes and every property, the locations being two properties of the set; the control protocol's dump request and reply are unchanged, built by the app from the flat list with parent places |
| `ShellItems` (319) | open point 3: `Find` on `WindowPath` with the shell's caps (`MAX_DEPTH` 16, `MAX_NODES` 1,024, shell_items.rs 48-54), matching named on-screen buttons, the taskbar's excluding nodes whose nearest window class is `TrayNotifyWnd` |
| `ShellItemPosition` (328) | open point 3: the same `Find` matching the item's runtime id or name, reading the place |
| `AwaitShown` (339) | gone in build 2 |

### `TextOp`, 13 variants (text.rs 975-1061)

| Op | Becomes |
| --- | --- |
| `StartingRead` (975) | a batch: `Caret`, `TextRead` of the line at the caret, `TextRead` of the key's unit (for Backspace and Control+Backspace at the caret after a movement of one character back; for Delete also the word at the caret). `StartingUnit` and `for_motion` (text.rs 737, 754) move to Core, as builds.md 964 proposes |
| `Watch` (977) | `WaitForCaret`, then the final `TextRead`s, under the condition "effect seen" where today's final read is `UnitAtEffect`, and none for a terminal |
| `MoveBySentence` (980) | a batch: `TextRead` from the starting point moved one sentence, `MoveCaret` to its start by reference, `Caret`, `TextRead` of the line; Core tells "moved", "end" and "stayed at start" (text.rs 879) from the units moved. Still one request the outpost runs whole (ADR 0003 line 86) |
| `ReadSelection` (983) | a batch: `Caret`, `TextRead` of the line, `SelectionChange` against the baseline |
| `Read` (985) | `TextRead` with count 1 |
| `ReadRange` (988) | `TextRange` |
| `Select` (1001) | `Select`, then `SelectionChange` against the selection read before, so the outpost-internal `Selected` and the queue's `SelectionPlaced` are one shape |
| `MoveCaret` (1013) | `MoveCaret`, then `Caret` where the queue's request needs the baseline (`CaretPlaced`) |
| `Location` (1018) | `Location` |
| `ReadAhead` (1021) | `TextRead` with a count |
| `TerminalHistory` (1026) | build 9: `TextRange` from a point a number of lines above the start anchor |
| `TerminalFind` (1047) | build 9: `FindText`, then `TextRange` of the rows from each match |
| `ReadTerminal` (1061) | build 9: `Caret`, `TextRange` from a screen above the start anchor with colours as details, and the start anchor minted by `Caret`; the raw section for terminal text (raw.rs 40-110, `docs/design/communication.md` 62) stays a matter of the transport |

### `Reply`, 16 variants (request.rs 833-875)

| Reply | Becomes |
| --- | --- |
| `Object` (835) | the outcomes of the `ReadObject` batch |
| `Properties` (837) | `Read`'s result |
| `Focus` (839) | the outcomes of the completion batch; `FocusRead`'s fields are results (`ancestors`, `met`, `foreground`, `caret`) or properties (`value_from_selection`, `runtime_id_reused`, `class`) |
| `Candidate` (841) | the outcomes of the run batch; `CandidateFacts::Skipped` is the verdict refusal at the `Ask`, `NotThere` the first `Read`'s outcome, `Found` its properties |
| `CandidateSame` (845) | the full read's operations Skipped by the identity condition, with the facts read |
| `Navigated` (848) | `Step`'s result |
| `Found` (851), `Children` (853) | `Find`'s result |
| `Text` (855) | the container goes |
| `Activated` (858) | `Activate`'s result |
| `Followed` (860) | `Follow`'s result |
| `Tree` (862) | `Find`'s result, turned into `DumpedTree` in the app |
| `ShellItems` (864), `ShellItemPosition` (867) | `Find`'s result, or kept with them (open point 3) |
| `HiddenDialog` (872) | after build 2, the read Skipped by the condition on the top-level window's visibility and role, with those properties read |
| `Shown` (874) | gone in build 2 |

### `TextReply`, 22 variants (text.rs 1115-1241)

| Reply | Becomes |
| --- | --- |
| `Started` (1117) | the starting read batch's results |
| `Watched` (1119) | `WaitForCaret`'s result and the final reads that ran |
| `Sentence` (1121) | the sentence batch's results |
| `SelectionRead` (1123) | the selection batch's results |
| `Read` (1128), `Chunks` (1139) | `TextRead`'s result |
| `Range` (1146) | `TextRange`'s result |
| `Moved` (1153), `CaretPlaced` (1167) | `MoveCaret`'s result, with `Caret`'s for the baseline |
| `Selected` (1162), `SelectionPlaced` (1177) | `SelectionChange`'s result after `Select` |
| `TerminalRead` (1184), `TerminalHistory` (1188), `TerminalFound` (1199) | build 9 |
| `Location` (1209), `OffsetLocation` (1219) | one `Location` result |
| `UnsupportedUnit` (1229), `Unsupported` (1231), `NoText` (1234) | `Refused` with the reason |
| `AnchorLost` (1236), `Gone` (1238) | `NotThere`, naming the anchor or the node |
| `Unanswered` (1240) | `Failed` |

### Kinds that might stay specialized

None is kept a priori. Two could, each only after a measurement, and each would then be one generic operation shared by every caret key or every terminal read, not one per behaviour, brought to Dickson for sign-off (plan.md 45):

- The caret read. Today one program reads the line, the key's unit, its formatting runs and the selection comparison, worst 4,453 and typical 91 executed instructions (mockapp `instruction_limit.rs` 524). If a builder cannot fuse `Caret`, `TextRead` and `SelectionChange` on one node into a program near that size (measurement 2), the list keeps one `CaretRead` operation with those parts as inputs.
- The terminal read, decided in build 9 by its own measurement of the generated program against `terminal_snapshot` (50 and 90 instructions, `instruction_limit.rs` 733-737) and the size limits builds.md 979 names.

The batch's wire shape does not depend on either: the executor may recognise a run of operations on one node and run a program written for it, so these decide the builder (question 3), not the contract Dickson rules now.

## 4. Which reader runs which operation (question 10)

- The listener: `Read`, `Ancestors`, `Find` and `Compare` on `Kept` and `Msaa` subjects; its focus walk stays call-by-call by decision (builds.md 986). It answers every other operation `Refused`, as it refuses every kind but `ReadObject` today (listener asked.rs 142-144).
- An outpost: every operation.
- Core's plain reader: answers each operation from the shell's facts, as it answers each kind today (plain.rs 159-175): `Read` from the facts, `Step` and `Find` with nothing, text operations `Refused(no text)`, `Activate` refused. Its classifier `subject` (plain.rs 278-310) classes operations instead of the 15 kinds, so a new behaviour adds nothing to it (question 8).

## 5. The extension surface, and decisions of record the list touches

### M5

The Extensions guide's imports (`books/extensions/src/reference/imports.md` 13-21) each map to a batch of one or two operations: `parent()`, `first-child()`, `last-child()`, `next-sibling()`, `previous-sibling()` to `Step`; `children()` to `Find` with no matcher; `find-descendant(matcher, depth-limit)` to `Find` with limit 1; `find-ancestor(matcher)` to `Ancestors` until the matcher; `window()` to `Read` on the `Window` subject; `text()` to `TextRange` from start to end; `find-window(application, class-path, backend)` to `Read` on `WindowPath`; `click(node, button)` to a `Read` of the place before Core clicks. `properties()`, `process()`, `focus()` and `foreground()` read what Verbatim holds and make no request. So every read M5 designs is in the list, and the list needs nothing M5 adds beyond `WindowPath`.

ADR 0003 line 123 stands: "An extension's reads travel through Core as ordinary requests from the shared list, outside the event queue; ... an extension has one read outstanding at a time." One read is one batch. The M5 entries written before ADR 0003 say otherwise (`docs/roadmap.md` M5, "its reads are requests straight to the claiming outpost"; `docs/design/extensions.md` 63 and 66); the later decision governs, by the session's default (plan.md 134), and the list does not change that.

### Decisions of record

- ADR 0003 line 53, "One request list is shared by Core's own features and the extension surface: properties of an object, navigation, find a descendant by property, and text reads. A new command adds no kind of request." Carried out, not changed. The list adds operations the same ADR already names elsewhere (line 36: "read, compare a caret with a starting point, report how a selection differs from one Core names, report whether an object reads as one Core names, ... move the caret"; line 146: "actions, focus reads ..., the caret watch, terminal reads, follow these objects").
- ADR 0003 line 146 names "focus reads including the completion of a UI Automation focus read", "the caret watch" and "terminal reads" as members of the list. Under the batch they are batches of operations, not kinds; the ADR's wording changes, its decision does not. The fact that changed: the counts in builds.md 953-959 show that kinds per read became kinds per behaviour.
- ADR 0003 line 30, one answer with one of five outcomes: kept for the batch; per-operation outcomes live inside the result.
- ADR 0003 line 86, "A move by sentence is one request the outpost runs whole": kept, as one batch.
- ADR 0003 line 114, a UI Automation focus "read in two ordinary requests chained through Core": kept, as two batches, since the two readers differ.
- ADR 0004 line 28, the identity check answered as a fact: kept, as a condition's outcome reported with the facts read.
- `docs/design/extensions.md` 61, "One matcher grammar, one type, parser and evaluator in `verbatim-ext-api`, serving rules, hook registrations and the find reads": `Find` uses it; conditions would too (open point 2).
- The remote-operations crate's shape, "Each algorithm is a named operation with a remote program, a classic implementation of the same signature ... and an entry point that chooses between them" (`books/dev/src/reference/crates/verbatim-uia-rops.md` 17-20, quoted in merged-text.md 57): untouched by the list; question 3 and measurement 2 decide it.
- uia-shapes decision 1 (2026-10-13), "the macro, ... so a shape's request and read come from one field list" (`books/dev/src/explanation/design/uia-shapes.md` 391-394): untouched by the list; question 4 decides how a `Read`'s set produces the cache request.
- builds.md 995, "`ShellItems`, `ShellItemPosition`: stay until M5 makes the tray list a global module", against plan rule 5 (plan.md 58) and the test of done (plan.md 114): open point 3.

## 6. Open points for Dickson

### Open point 1: conditions in a batch

Example: File Explorer raises a second MSAA focus event for the item just spoken. Today Core sends `MsaaRun` at `FullUnlessSame`, and the outpost answers `CandidateSame` with nothing more read (asked.rs 1270-1278). As a batch, the first `Read` reads the item's facts and identity (focused state, states, role, position, set size, name, value, ids after the redirect); the full read's operations follow.

- Option A, conditions allowed: the full read's operations carry "not reading as the identity Core names". Same item: one round trip, the full read Skipped, the facts answered; Core makes it the duplicate and says nothing more. A different item: one round trip with the full read.
- Option B, no conditions, always read in full: one round trip, plus the full read's calls (ancestors to a known one, the caret, the line) for every same item, every unfocused candidate and every unrelated changed object. A guess that costs only calls, which the design principles allow.
- Option C, no conditions, ask again: the facts in one batch, the full read in a second only when needed, so every new focus, the common case, pays a second round trip.

Speech is the same under all three; they differ in round trips and calls. Recommendation: A, because today's `Depth`, `FocusDepth`, `HiddenRoot` and `Purpose::Change` already make these reads conditional in the reader, so A removes four modes for one rule and adds no call or round trip; the condition is a Core-written test the reader evaluates, the mechanical comparison ADR 0004 line 28 already places in the outpost. If measurement 4 shows the guarded reads rare, B is simpler and A's conditions can go.

### Open point 2: what the extension surface imports, and one matcher grammar

Example: a module calls `node.find-descendant(matcher, 3)`; Core sends one batch, `Find` on the node with descendants to depth 3, the module's matcher and limit 1, and returns the node or none.

Recommendation: the WIT imports keep the reads `imports.md` names, each a single call that Core turns into a batch of one or two operations; `Caret`, `TextRead`, `SelectionChange`, `Select`, `MoveCaret`, `Location`, `FindText`, `Compare`, `Activate`, `Follow` and `WaitForCaret` stay Core's until a module needs one, when importing it is an addition to the contract, not a change. Batches are not exposed to modules in M5, since `one read outstanding at a time` already bounds them. The matcher in `Find` and in conditions is the one grammar `extensions.md` 61 decided, its node subset, so a module's matcher and Core's are the same type.

### Open point 3: the notification area's two reads

Example: the user opens the notification area list. Today Core sends `ShellItems` to File Explorer's outpost, which walks `Shell_TrayWnd`'s notification area and the overflow flyout (shell_items.rs 1-34). Converted, Core sends one batch of two `Find`s, one per window found by `WindowPath`, matching named on-screen buttons, each complete or failed as the walk is today; choosing an item sends a `Find` matching its runtime id or name and reading its place. The list shown and the click are the same.

Recommendation: convert in build 8. Plan rule 5 asks every user of the old form converted, the test of done asks no one-behaviour kind kept without his sign-off, and the conversion needs only `WindowPath`, which M5's `find-window` needs anyway. The alternative keeps two request kinds, two replies and their outpost arms until M5 moves the list to a global module, with his sign-off.

## 7. Decided by the study, open to objection

- A failed operation skips only the operations that name it (section 1), so independent reads are not lost and Core keeps the "complete or nothing" judgement.
- Node ids keep being stamped by Core from the pipe, for the security reason lib.rs 178-181 gives, in one walk over the batch.
- References are backward only, typed in Core's builder and parsed once at the reader's boundary.
- One waiting operation, at most one per batch; no generic "wait for an event" operation, since after build 2 only the caret watch waits.
- Queue placement and held spares move onto the `Ask`.
- `Purpose` goes, the listener always answering the fresh name, which costs no call since it comes in the same cache request (listener asked.rs 867-872).
- Derived facts (focused state over the ancestors, dialog text, top-level visibility, runtime id reused, value from selection) become properties of the table, so readers report them and Core judges.
- `Location` and `OffsetLocation` become one result; the six text failures become operation outcomes; `TextFailure` goes.
- The terminal operations are settled in build 9, from its corrected study.
- A batch holds a capped number of operations, so a runaway batch is refused as a bug rather than run. The cap is not yet set: the study sets it from the largest batch the conversions actually build (the largest estimated above, an MSAA focus run in full with its caret, is about 8), not from a round number. On UI Automation the program's limit of 10,000 executed instructions binds first.
- `Find` with no matcher reads as a search, which children are not. The study either keeps `Children` as its own operation or names the merged one for what it does (`Descendants`, with an optional matcher and a depth), so that Core's code says what it reads. Modules see neither name: they call `node.children()`.

## 8. The measurements the list still depends on

builds.md 968-973 names four before the design; the list's shape survives every outcome below, and each says what it changes.

1. One round trip, Core to an outpost and back with an empty batch, and Core to the listener's reading thread and back, median and worst, x64 and the ARM64 runner, with the wake-ups on each side, from `crates/verbatim-process/examples/crossing.rs`; the build notes record 0.65 ms for a reading thread's read and 0.45 ms for a reading thread's start (builds.md 970). It changes open point 1: a round trip small against the calls of a full MSAA read favours B or C over A.
2. Control+Backspace's starting read as a generated program from `Caret`, `TextRead` of the line and `TextRead` one character back expanded to a word, against today's caret program (worst 4,453, typical 91, `instruction_limit.rs` 524): executed instructions with every program under 10,000, processor time in the outpost, calls; and the same batch run as ordinary calls, counting calls. Also the focus ancestry as `Read` and `Ancestors`, against today's worst 3,078 and typical 1,278 (`instruction_limit.rs` 329-333), as plan.md 142 asks for one hard read. A generated caret read well above today's keeps one `CaretRead` operation (section 3); a generated ancestry that fails keeps its program written by hand behind the same operations. Neither changes the wire list.
3. The added round trips of each one-behaviour kind decomposed: by the tables above, none with result references and conditions. The measurement that remains is for options B and C of open point 1: from the recordings replayed through Core, how often a candidate is not focused, how often the identity check holds, how often a focus read meets no known ancestor (the foreground read), and how often a changed object is unrelated. If those are rare, B costs few calls and conditions can go.
4. Build 5's count of fallbacks to the classic twins, which decides whether the ordinary-call executor replaces every twin (question 3); it does not change the list.

A result that would change the list itself: any present read that cannot be written as these operations without a second round trip. The tables in section 3 found none, but the focus completion's order (`Follow` before the states, request.rs 498) and the MSAA redirect (asked.rs 1237-1243, 1256-1265) are the two places where a batch must reproduce an order or a resolution the hand-written code holds today, and the adversarial review should derive both sequences against the code.
