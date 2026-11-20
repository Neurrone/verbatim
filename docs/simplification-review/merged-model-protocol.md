# Merged review: the model, the protocol and the transport

Area: `crates/verbatim-model`, `crates/verbatim-control`, `crates/verbatim-process`. Merged from reviewer A's and reviewer B's notes, every claim checked by me against the tree at ba89dca7 (2026-11-20). "Grep" below means `grep -rnE '<pattern>' --include=*.rs crates xtask uia-facts`, with the file the item is declared in left out, unless another scope is named. Test code means files named `tests.rs`, `*_tests.rs` or under `tests/`, and anything after a file's first `#[cfg(test)]`.

No finding changes speech. Four groups need Dickson because they change a decision of record or a public contract (the control protocol), not because of speech.

## 1. Size

Recounted with one script: a file's product lines are those before its first `#[cfg(test)]`; test files and the rest are test lines. Both reviewers' totals were right (19,712 product, 4,593 test); their per-part figures for the transport were not (A: 7,152, B: 5,525 for `src`). The settled figures:

| Part | Product lines | Test lines | Tests | NVDA counterpart (whole files) | Ratio | What the extra buys |
|---|---|---|---|---|---|---|
| Model vocabulary: tree.rs 733, speech.rs 522, text.rs 1,435, terminal.rs 195, event.rs 1,327, lib.rs 372, the small files 587 | 5,171 | about 550 | | `controlTypes/role.py` 576, `state.py` 231, `processAndLabelStates.py` 207, `formatFields.py` 183, `speech/commands.py` 485, `speech/types.py` 88, `textInfos/__init__.py` 1,091: 2,861 | 1.8 (whole lines, doc comments included) | event.rs's `Input` and `Effect` (Core's reducer contract) have no counterpart in those files; about 400 lines of event.rs are the interim key-press shape (finding 9) and about 200 are never-sent stream variants (finding 4). Code-only ratios were not recounted; A gave 0.5 to 0.9 per part and B 1.2 overall, both in proportion |
| Themes (theme.rs) | 912 | 193 | | none | | a ruled feature, not judged |
| Protocol: request.rs, property.rs, event_kind.rs, tell.rs, protocol/ | 4,873 | 956 | | none: NVDA calls in its own process | all hand-off | the process boundary of D9 and D13; about 1,050 lines of it are the property and event-kind tables' unread parts (finding 1) |
| Transport: verbatim-process `src` | 5,105 | 2,021 | 63 | none | | the pipes |
| Measurement tool: verbatim-process `examples/crossing.rs` | 1,091 | 0 | | none | | evidence cited by `docs/design/readers-tell-core-asks.md`; stays |
| Control plane: verbatim-control | 2,560 | 874 | 23 | none | | development tooling and the harness's contract |
| Totals | 19,712 | 4,593 | model 75, control 23, process 63 | | | |

Deletion target, in product lines and test lines:

- Remove now: about 500 product lines in the area (findings 3, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15) and about 300 outside it (the dead stage computations in `crates/verbatim-app/src/latency.rs`, about 200; Core's four copies of the typed properties request, about 90; the outpost's never-sent `Entry.timing`, about 15). Tests: 24 tests and about 630 lines in the area, about 40 added back; about 250 lines of latency.rs tests outside it.
- If Dickson rules for the removal: about 1,050 product lines of the property and event-kind tables (question 1), and about 330 lines of control-protocol vocabulary (question 3: 110 of stage kinds, 200 of never-sent stream variants and fields, 20 of `live_re_reads` and `Restarting`). Tests: 14 tests and about 265 lines.
- Planned phase: about 400 model lines and 100 Core lines of the interim key-press and focus shapes ("Readers tell, Core asks", its clearing-up step; finding 9); about 100 lines of raw.rs's mirror types (M6.4; finding 17).

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| 1 | The property table's interface columns and the compile-time sets are read by no product code; readers map by hand | Question for Dickson | both | `crates/verbatim-model/src/property.rs` 188-324, 425-463, 1025-1340 | the uia, msaa, ia2 and cost columns and their types, the `mod uia` ids, the set machinery | about 650 | about 170 | no |
| 2 | The event-kind table is a second copy of the WinEvent mapping; only `cached` is read | Question for Dickson | both | `crates/verbatim-model/src/event_kind.rs` 94-558 | the entry table, `Teller`, `Source`, the constant ids, `EVENT_KINDS` | about 400 | about 95 | no |
| 3 | 12 of `EventTiming`'s 16 fields are never written outside tests; 19 of the 30 latency stage kinds can never be produced | Remove now (model, app); the stage kinds are question 3 | both | `crates/verbatim-model/src/protocol/mod.rs` 25-90; `crates/verbatim-app/src/latency.rs` 140-330; `crates/verbatim-control/src/protocol.rs` 378-550 | the 12 fields, the outpost's `Entry.timing`, the dead stage computations | about 45 model, about 200 app, about 15 outpost | about 250 (latency.rs) and 6 | no |
| 4 | `NormalizedEvent` keeps variants never constructed and fields always constant | Question for Dickson (control protocol) | both | `crates/verbatim-model/src/event.rs` 278-520; `text.rs` 389-517 | 5 variants, 3 always-constant fields, `ReadFacts`, `Change`, `FocusGroup`, `AlertOrigin`, `CaretChanges` | about 200 | a few | no |
| 5 | Dead message parts: `NotResponding`, `Kind` and four `kind()`, `parked_on_hung`, two always-zero listener fields | Remove now | both (A: all; B: all but `parked_on_hung`) | `protocol/mod.rs` 232, 275-285, 386-397, 418-425, 441-444, 488, 578, 600-611, 822-834, 863-882, 895 | the variants, fields and classification | about 110 | about 180 (2 tests, 1 test, 2 sample chains) | no |
| 6 | Four hand-written walks over a message's node ids and anchors; one clones every answer | Remove now | A only, confirmed | `request.rs` 926-982, 1020-1060 (clone at 1021), 1063-1122; `protocol/mod.rs` 721-781 | three of the four walks and the clone | about 80 | 0 | no |
| 7 | A request's failure has two vocabularies and "gone" two encodings | Remove now | A only, confirmed (B noted `OutsideOutcome`) | `text.rs` 1229-1297; `request.rs` 894-905, 1562-1578; outpost `asked.rs` 227, 243-248 | `TextFailure`, the text failures that duplicate outcomes, `OutsideOutcome` | about 90 | 0 | no, provided each Core text caller maps the outcome as it maps the reply today |
| 8 | Three classifications of the same outpost variants (`Urgent`, `Peeked`, `carries_nodes`) | Remove now | both | `protocol/mod.rs` 620, 888-935; `crates/verbatim-process/src/message.rs` 204-233 | `Urgent` as a copy, the string peek | about 50 | about 60 (1 test) | no |
| 9 | Two shapes of a key press and of a focus answer, translated for "today's" code | Planned phase ("Readers tell, Core asks", clearing up) | both | `request.rs` 1467-1540; `event.rs` 596-770; Core `keys/entry.rs` 266-323, `entries.rs` 1258-1280, `focus/execute.rs` 79-120 | the interim `Input` variants and the translations | about 500 (400 model, 100 Core) | not counted | no |
| 10 | Protocol fields defaulted for an "older peer" that cannot exist | Remove now | both (B the list, A the test) | `protocol/mod.rs` 24, 181, 198, 304, 424, 428, 507, 528, 532, 536, 581; `protocol/tests.rs` 353-358 | the `#[serde(default)]` attributes on the private protocol and the test | about 15 | 6 | no |
| 11 | The control plane's startup speech history never closes for a user with no client | Defect | A only, confirmed | `crates/verbatim-control/src/server.rs` 1073-1081, 1186-1191; app `latency.rs` 734-745 | a fixed close point for the history; the false comment | a few added | 0 | no |
| 12 | Element-fact types in the model that no message carries | Remove now (two of the three) | B only, confirmed in part | `protocol/mod.rs` 110-167; outpost `asked.rs` 1437 | `UiaAncestorFact` and `AncestorsEnd` moved into the outpost, `AncestorsEnd` replaced by `RouteEnd` | about 40 | 0 | no |
| 13 | Smaller dead code in the transport and control plane | Remove now | A: `BlockingWriter`, `read_message`; B: `send_all`, `MAX_MESSAGE_BYTES` | `transport.rs` 441-470, 1211; `message.rs` 33; control `protocol.rs` 577-585 | four unused or duplicated items | about 80 | about 40 (2 tests) | no |
| 14 | `Request::outside_the_queue` ends in a wildcard | Remove now | A only, confirmed | `request.rs` 1003 | `_ => false,` made exhaustive | 0 | 0 | no |
| 15 | Small model items: `Input::MarkReached` never constructed; `TextReply::Moved` and `Selected` never cross the pipe; three requests repeat one focus-read spec | Remove now | B only, confirmed | `event.rs` 767; `text.rs` 1153-1162; `request.rs` 273-292, 514-534, 627-645 | the dead variant; a note on the two outpost-internal replies; one shared spec type | about 10 | 0 | no |
| 16 | Core wraps `Request::Properties` four times | Remove now (outside the area) | B only, confirmed | Core `navigator.rs` 142, `report_focus.rs` 62, `announcements/selection.rs` 97, `changes.rs` 278 | three of the four wrappers | about 90 (Core) | 0 | no |
| 17 | raw.rs mirrors `Answer`, `Reply`, `TextReply` and `TerminalSnapshot` | Planned phase (M6.4) | both | `crates/verbatim-process/src/message/raw.rs` 40-110 | the `Wire*` mirror | about 100 | 0 | no |
| 18 | `ObjectClass` has one variant | Planned phase (the classes its doc names join with their milestones) | both | `request.rs` 731-741 | nothing now | 0 | 0 | no |
| 19 | `Request::Find` and `Request::Children` have no product constructor | Question for Dickson (one line) | both | `request.rs` 249-267 | nothing unless he rules | 0 | 0 | no |
| 20 | Stale figures and headings in the model's crate guide | Remove now (documentation) | B only, confirmed | `books/dev/src/reference/crates/verbatim-model.md` 250-252, 474-475, 733-734 | the figures, replaced by a pointer to the test | 0 | 0 | no |

### 1. The property table: what of ADR 0003's decision is built (both)

The decision of record, ADR 0003, "Properties, event kinds and requests (Dickson, 2026-11-13)", line 52: "Properties and event kinds are each declared once. A property's entry gives its name, its type and how each interface reads it. A read names a set of properties fixed at compile time; the cached request and the remote program are built from that set, so every field a read needs is cached and costs nothing extra, and the compiler checks that a rule uses only what its read asked for. Extensions name entries of the same table at run time." Line 53: "One request list is shared by Core's own features and the extension surface: properties of an object, navigation, find a descendant by property, and text reads. A new command adds no kind of request."

The entry, `property.rs` 306-324, verified character for character as A quotes it: `pub struct PropertyEntry { pub id: PropertyId, pub name: &'static str, pub kind: ValueKind, pub uia: Option<UiaRead>, pub msaa: Option<MsaaRead>, pub ia2: Option<Ia2Read>, pub cost: Cost, pub detail: Option<NodeDetail>, }` (doc lines omitted).

Each column, macro and type, with the grep that shows its readers outside `property.rs`:

| Item | Lines | Grep | Count | Readers |
|---|---|---|---|---|
| `uia` column, `UiaRead`, `UiaConversion` | 196-264 | `(entry\(\)\|entry)\s*\.\s*uia\b`; `UiaRead\b`; `UiaConversion` | 0; 0; 0 | none |
| `msaa` column, `MsaaRead` | 266-297 | `\.\s*msaa\b` on an entry; `MsaaRead` | 0; 0 | none |
| `ia2` column, `Ia2Read` | 299-302 | `\.\s*ia2\b` on an entry; `Ia2Read` | 0; 0 | none |
| `cost` column, `Cost` | 184-194 | `\.\s*cost\b` on an entry; `\bCost::` | 0; 0 (the 21 hits for `\bCost\b` are mockapp's own `struct Cost` in `tests/call_counts.rs`) | none |
| `mod uia` property ids | 425-463 | `win_event::\|uia_event::` and `uia::` outside the table | 0 | the table's own entries only |
| `kind` column | | `entry().kind` outside the model | 0 (the 9 `entry.kind` hits are `crates/verbatim-e2e/src/timeline.rs`'s own entries, another type: B's claim that `.kind` is read there is wrong) | inside the model, only `SetMismatch`'s `Display` (1051) and `answers_in_order` (1110), both unused |
| `name` column, `PropertyId::named` | 145-149 | `PropertyId::named` | 0 outside the model's tests | M5's run-time naming, not built |
| `detail` column | | read through `id.entry().detail` | 2 sites | Core `details.rs` 32-33 and 54-55, and `NodeDetailSet::named_by` and `choose` (property.rs 407, 418), which Core `details.rs` 16 calls |
| `id` and `PROPERTIES` | 882 | `PROPERTIES\b` | 1 product use | `crates/verbatim-outpost/src/driver.rs` 106, `PROPERTIES.iter().map(\|entry\| entry.id).collect()`, for the attach mode's read of every property |
| `property_set!` macro | 1269-1340 | `verbatim_model::property_set\|model::property_set` | 0 (the 4 `property_set!` hits are `crates/verbatim-uia/src/shape/mod.rs`'s own macro, `$crate::shape::property_set!`) | none |
| `PropertySet` trait | 1076-1097 | `PropertySet\b` | 1, a doc link at `request.rs` 205 | none |
| `Property` trait and the `props` markers | 1061-1268 | `property::props`; `impl Property for` | 0; 0 (the 48 `props::` hits are mockapp's own `props` module) | none; 22 markers for the 28 properties (counted) |
| `answers_in_order`, `SetMismatch` | 1025-1132 | `answers_in_order`; `SetMismatch` | 0; 0 | none |

The hand-written mappers that do the work instead, verified:

- From a reader's snapshot to `PropertyValues`:
  - outpost `crates/verbatim-outpost/src/outpost/asked.rs` 361-410, `values`, with a list of 11 properties answered `None` ("Not in a snapshot") at 397-407;
  - outpost `asked.rs` 469-525, `read_apart`, ending `_ => None,` at 522;
  - outpost `asked.rs` 789-800, an MSAA relation read's own match, ending `_ => None,` at 800;
  - listener `crates/verbatim-listener/src/listener/asked.rs` 480-535, `values`;
  - listener `crates/verbatim-listener/src/listener/checking.rs` 149-180, `cached_of`, ending `// No kind the listener hears caches another.` and `_ => None,` at 176-177;
  - outpost `crates/verbatim-outpost/src/outpost/worker.rs` 392-419, `told_property`, from UI Automation property ids;
  - Core `crates/verbatim-core/src/focus/plain.rs` 126, the plain reader's `values`.
- From `PropertyValues` back to a `NodeSnapshot`, with defaults:
  - Core `focus/execute.rs` 79-120, `snapshot`: `_ => Role::Unknown,` at 86 and `_ => StateSet::default(),` at 92;
  - outpost `asked.rs` 531-560, `snapshot_of`: the same two defaults at 546 and 552;
  - Core `focus/validity.rs` 981-998, `text` and `role`.

What is built of the decision: one `PropertyId` enum used by every reader and Core; the `detail` column, which drives the settings' filtering of a read's set; the `id` list; the property's `name`, ready for M5. What is not built: "how each interface reads it" (no reader reads the columns; `MsaaRead::TreeViewChildCount`, added for 8ec23de0, did not drive that read, which is the hand-written arm at outpost `asked.rs` 515-521); "a read names a set of properties fixed at compile time" (every set travels as `Vec<PropertyId>` chosen at run time, Core `details.rs` 25-37); "the cached request ... built from that set" (the UI Automation cache requests are built from `verbatim-uia`'s own shapes, `crates/verbatim-uia/src/shape/mod.rs` 1-14, keyed by UI Automation ids: a separate compile-time declaration, per purpose, not per `PropertyId`); "the compiler checks that a rule uses only what its read asked for" (no rule reads a typed set).

The consequence (A's item 9 sequence): a focus answer without `Role`, or with a value of another type, can only come from a reader bug, and Core's `snapshot` then makes `Role::Unknown` and speaks the focus with no role, logging nothing; `answers_in_order`, which would refuse it as `Missing` or `WrongKind`, is never called. No live sequence produces it today.

The fact that has changed since 2026-11-13: Build 1, which the roadmap lists with "each property and event kind declared once" (`docs/roadmap.md` 128-137), is built and pushed, and neither the readers nor Core took up the columns or the sets. The two shapes, both from the reviewers, are question 1.

### 2. The event-kind table (both)

`event_kind.rs` writes each of its 36 kinds in five places, verified: the enum (13-92), `entry()` (94-136), a constant per kind (293-520), `EVENT_KINDS` (521-558) and the test's `ALL` (566-603). A's line numbers are right; B's drifted. Of `EventKindEntry` (180-192), the greps `EVENT_KINDS` 0, `\bTeller\b` 0, `\.teller\b` 0, `win_event::\|uia_event::` 0, `event_kind::Source\|Source::(Hook\|Uia\|Console)` 0 relevant (the 2 hits are verbatim-audio's and verbatim-config's own `Source`). `entry()` is called at `crates/verbatim-listener/src/listener/checking.rs` 162 for `.cached`, and at mockapp `tests/common/core_driver.rs` 1394, test code. The mapping in force is `crates/verbatim-ia2/src/hook.rs`: `WinEventKind` at 61, `SUBSCRIPTIONS` at 174, `tell_kind` at 851 (B's line; A's 857 drifted), with `WinEventKind::MenuEnd if event.event == EVENT_SYSTEM_MENUPOPUPEND => EventKind::PopupEnd,` at 869. The table's `teller` and `source` restate what hook.rs and the outposts subscribe to, and the tests `each_interface_event_has_one_teller` and `a_win_event_carries_no_cached_property` check the table, not the subscriptions. Removal changes the same ADR sentence as finding 1 ("Properties and event kinds are each declared once"), so it is part of question 1.

### 3. EventTiming and the latency ledger (both)

`EventTiming`, `protocol/mod.rs` 25-90, has 16 fields (A right; B said 15). Field by field, the grep `\b<field>\s*(:[^:]\|=[^=>])\|\.<field>\s*=[^=]` plus a search of every `EventTiming {` literal:

| Field | Written in product code | Read in product code |
|---|---|---|
| `raised_ms_ago` | outpost `outpost/mod.rs` 1236, into the reducer's `Entry.timing` | latency.rs 227, 400, but the ledger's timing never carries it |
| `observed_at_us` | listener `listener.rs` 614; app `main.rs` 1144; outpost `outpost/mod.rs` 1237, 1445, 1470 | yes |
| `listener_handled_at_us` | listener `listener.rs` 561 | supervisor `owner.rs` 726 (a trace line) |
| `listener_written_at_us` | `protocol/mod.rs` 843 (`written`) | supervisor `owner.rs` 726-727 |
| `relayed_at_us`, `dequeued_at_us`, `awaited_at_us`, `published_at_us`, `written_at_us`, `calls`, `awaited_calls`, `core_received_at_us`, `core_relay_us`, `core_held_us`, `core_sent_at_us`, `core_read_at_us` (12) | none: every write is in latency.rs's tests, from line 1046 on (its `#[cfg(test)]` is at 872) | latency.rs 149-303 |

So the 12 fields both reviewers name are confirmed. A said only three are written; B's four (with `raised_ms_ago`) is closer, and `observed_at_us` and `raised_ms_ago` are also written by the outpost into `Entry.timing` (`crates/verbatim-outpost-logic/src/entry.rs` 171), which nothing reads but `observed_us` at outpost `mod.rs` 623 and which no message carries (B: "outpost-internal intake"; A: "written but never sent").

The ledger's only `event_received` call is app `main.rs` 1141-1147, `EventTiming { observed_at_us: tell.at_us, ..EventTiming::default() }`, and the listener's `Progress` timing goes only to the supervisor's trace line (`crates/verbatim-supervisor/src/owner.rs` 723-730); the app's `OutpostMessage::Progress(change)` (main.rs 1107) drops it. Walking `to_outpost_read` and `after_outpost_read` (latency.rs 200-330) with that input, these 19 of `LatencyStageKind`'s 30 variants (`crates/verbatim-control/src/protocol.rs` 378-494) can never be produced: Windows, ListenerToOutpost, CoreToOutpost, ListenerToRelay, ListenerHandling, ListenerQueue, ListenerPipe, RequestToRelay, OutpostStart, Relay, RelayToOutpost, OutpostQueue, CaretWait, OutpostRead, ToCore, OutpostToRelay, OutpostOutbound, OutpostPipe, ReducerWait. A listed 13 and B 16; both missed Windows, ListenerHandling and ListenerQueue, and A also ListenerToRelay, ListenerPipe and ReducerWait. B's gap holds: no stage kind names a request Core sends and its answer, the round trip ADR 0003 added. Removing the fields, the outpost's `Entry.timing` and latency.rs's dead computations changes no message anyone reads; removing the stage kinds and their labels (`protocol.rs` 496-550) changes the control protocol, question 3. Phase: the roadmap's "Clearing up: the messages and code nothing uses are deleted" (`docs/roadmap.md` 150-152) is where it lands. The ledger's comment cites `docs/design/communication.md`, section 10.1, which describes the old relay.

### 4. NormalizedEvent's never-sent variants and constant fields (both)

`event.rs` 291-470 is now Core's event stream to the control plane (`Effect::Observed`). Product constructors (grep `NormalizedEvent::<variant>` outside test code): `FocusChanged` at Core `focus/execute.rs` 540, `CaretMoved` at `caret_tell.rs` 286, `TextChanged` at `changes.rs` 367, `TerminalSnapshot` at `terminal/reading.rs` 610. `SelectionChanged`, `ControlledSelection`, `Notification`, `ActiveTextPositionChanged` and `Alert`: 0 each. `PropertyChanged` and `ValueChanged`: only verbatim-inspect's matches and control `server.rs` 1410 and 1558, after its `#[cfg(test)]` at 1272. Always constant: `group: None,` (execute.rs 546), `content_changed: false,`, `new_selection: None,`, `facts: None,` (caret_tell.rs 294-296), `facts: None,` (execute.rs 459) and `changes: verbatim_model::CaretChanges::default(),` (execute.rs 458). So `ReadFacts` (text.rs 505), `Change` (483), `FocusGroup` (event.rs 278) and `AlertOrigin` (506) have no product reader or writer (grep counts 0, 0, 0, 0 outside the model). A's claim that `ToastId` (520) is dead is wrong: Core `announcements/toast.rs` 11, 92-101 and `state.rs` 488 use it. `ReadFacts`'s doc cites docs/design/caret-selection-rework.md, which does not exist. Because `NormalizedEvent` travels in the control protocol's event frames, removing variants changes a public contract even though no client ever received them: question 3.

### 5. Dead message parts (both)

- `OutpostToCore::NotResponding { window }` (`protocol/mod.rs` 441-444): grep `NotResponding {` outside `Outcome::` finds only matches (app `main.rs` 1259, process `message.rs` 283, the protocol's own arms) and `samples.rs` 36; no reader constructs it. ADR 0003, "Messages that remain" (line 148): "What goes, about 40 names: ... the not-responding message". The model guide's lines 474-475 still say Core plays the sound for it.
- `protocol::Kind` (863-882) and the four `kind()` (275, 386, 600, 822): read only by `protocol/tests.rs` 189-273 (`every_variant_has_its_reviewed_kind`, `core_sends_no_report`), whose sample chains `next_core_to_outpost` (31) and `next_core_to_listener` (66) nothing else calls.
- `Pong.parked_on_hung` (424-425) and `Urgent::Pong`'s (895): every writer writes 0 (outpost `mod.rs` 544-545: `// The outpost judges no hang.` then `parked_on_hung: 0,`); the one reader, outpost `shell.rs` 769, is after that file's `#[cfg(test)]` at 622.
- `ListenerToCore::Ready.target_pid` ("A sentinel, 0", 486-488) and `ListenerToCore::Pong.parked_count` ("Always 0", 577-578) (B).
- A's claim that nothing calls `stamp_sent` and Ping's stamp is dead is wrong and dropped: `crates/verbatim-supervisor/src/writer.rs` 131 calls `command.stamp_sent(now)`, and the listener reads the stamp at `listener.rs` 869. Only the outpost ignores its Ping's stamp (outpost `mod.rs` 1190); that field is a few lines.

All removals implement ADR 0003 or remove values with one possible state. About 110 product lines; tests: the two `Kind` tests and their chains, about 175 lines.

### 6. Four walks over node ids and anchors (A only, confirmed)

`Ask::unstamp` (`request.rs` 926), `Answer::named` (1020), `Answer::visit_nodes` (1063), `Named::text_reply` (`protocol/mod.rs` 721), and `Event::assign_outpost` (`tell.rs` 244). `named` begins `let mut copy = self.clone();` (1021) to run the `&mut` walk over a borrowed answer, and its doc says "but for a text reply's anchors, which the protocol's own walk of a text reply names" (1017-1018). It runs for every outpost message: outpost `store/mod.rs` 470, `let named = message.named();`, through `OutpostToCore::named` (`protocol/mod.rs` 665-696). bc99bc40 added arms to three of them (its diff, below). B's shape for the root cause: the reader's own numbers are one type in two states (`OutpostId::UNASSIGNED`, `lib.rs` 188-191, "the placeholder an outpost mints its node ids with, before Core stamps the real generation"). One walk taking `&self` and `&mut self`, or a distinct type for an unstamped id, removes the clone and about 80 lines.

### 7. Two failure vocabularies (A only, confirmed; B found `OutsideOutcome`)

`TextReply`'s six failures (text.rs 1229-1240), copied as `TextFailure` (1248-1262) with `into_failure` (1264-1297), beside `Outcome`'s `NotThere`, `NotResponding`, `Refused(Unsupported)` (request.rs 894-905). The outpost's terminal path maps `TextReply::Gone => Outcome::NotThere,` (outpost `asked.rs` 246) and answers a missing client `return Outcome::Result(Reply::Text(TextReply::Unanswered));` (227) and an unknown op `_ => TextReply::Unsupported,` (243); its watch path also maps `Err(TextReply::Gone) ... => Some(Outcome::NotThere)` (`text_reads.rs` 674); its caret and review reads answer `TextReply::Gone` as a result (`text_reads.rs` 279, 307, 316, 341). Core's review code turns `Outcome::NotThere => TextReply::Gone` back (`review_text.rs` 220-221). `OutsideOutcome` (request.rs 1562-1578) is Core's outcome flattened, converted variant by variant at Core `entries.rs` 1380-1388. Speech is unchanged if each Core text caller maps the outcome as it maps the reply today, which the change must check caller by caller.

### 8. One classification of the outpost variants (both)

`Urgent` (`protocol/mod.rs` 888-935) repeats four `OutpostToCore` variants field for field; process `message.rs` 204-233 `Peeked`/`peek` names outpost frames by their JSON variant name; `carries_nodes` (620) is a third. Each has its own change-detector test (`a_frames_kind_is_its_variants_name`, `each_urgent_message_converts_to_the_variant_of_its_name`, both in process `message.rs`). With `Kind` gone (finding 5), one classification remains to choose; about 50 product and 60 test lines.

### 9. Interim shapes (both; planned phase)

`today` (Core `keys/entry.rs` 266-323, doc "The input today's handling of `press` takes") turns `PressAction` (`request.rs` 1467-1529) into `Input::{Command, Script, HeldKey, CaretKey, CharacterTyped, ClearingKey, EditingChord, ControlTyped}`; its only product constructors are there and in `keys/mod.rs` (the greps' other hits are in test code: verbatim-replay after 372, Core `recorder.rs` after 362, e2e `artifacts.rs` after 694). Core `entries.rs` 1258-1280 does the same for `UtteranceCancelled`, `UtteranceFailed`, `TerminalDue` into `TerminalTimer`, and `LockKeyRead` into `LockKeyToggled` (B). `FocusRead` and `FocusCaret` become `NodeSnapshot` and `FocusText` for "today's announcement code" (execute.rs 75-120, 452-462). This is code in use, so not "nothing uses"; it belongs to the clearing-up step ("Readers tell, Core asks"), and B's caution stands that the next phase, "Exact replay of a paused entry" (`docs/roadmap.md` 171), touches entries.

### 10. Defaults for a missing field (both)

`#[serde(default)]` on `EventTiming` (24), `Ping.sent_at_us` (181, 304), `NodesHeld.anchors` (198), `Pong.parked_on_hung` (424) and `written_at_us` (428, 581), `Progress.timing` (507), `Foreground.root_owner` (528), `process` (532) and `observed_at_us` (536). verbatim.exe starts every child from its own build, so a missing field is a bug, and `Foreground` without `process` decodes as process 0. The test `a_timing_from_an_older_peer_reads_with_no_calls` (`protocol/tests.rs` 353-358) pins the fallback. The control protocol is a public contract and keeps its defaults (B).

### 11. The startup speech history (A only, confirmed; defect)

`server.rs` 1073-1081, as A quotes it: `has_speech_subscribers` is true while `self.history` is `Some`. The history is created `Some(Vec::new())` (1021) and taken only by the first `SubscribeSpeech` (259); app `main.rs` 417 starts the control server in every run. Sequence: start Verbatim with no client and speak anything; app `latency.rs` 736-745 sees `has_speech_subscribers()` true, copies the text and calls `broadcast_speech`, which takes the history's lock and the registry's and pushes a frame until 1024 (`STARTUP_HISTORY_LIMIT`, 154; push at 1186-1191), then takes both locks for every later utterance for the life of the process. The comment at latency.rs 734-735, "The text is copied for the control plane only when some connection is subscribed to speech", is false. The fix closes the history at a fixed point; whether its cost on the speech path matters needs a measurement. Speech unchanged.

### 12. Element-fact types no message carries (B only, confirmed in part)

`UiaSnapshotFact` (110), `UiaAncestorFact` (132) and `AncestorsEnd` (147) appear in no message variant. B's proposal to convert straight to `verbatim_uia`'s `CachedUiaParts` fails for `UiaSnapshotFact`: `crates/verbatim-outpost-logic/src/entry.rs` 88 holds it, and that logic crate depends on no Windows crate (CLAUDE.md, "NVDA provenance"), and the listener uses it too (`listener.rs` 1213), so a shared Windows-free home is needed and the model is it. `UiaAncestorFact` and `AncestorsEnd` are used only by the outpost (`completion.rs`, `read.rs`, `asked.rs` 1437); `AncestorsEnd` is a near-copy of `RouteEnd` (request.rs 461-479: `Top` stands for its `Known`, `ProcessBoundary` and `Root`, and it holds `isize` where `RouteEnd` holds `WindowHandle`). About 40 lines.

### 13. Smaller dead code in the transport and control plane

- `BlockingWriter` (`transport.rs` 441-470): only `crates/verbatim-outpost/tests/target_exit.rs` uses it (A).
- `Sender::send_all` (1211): only its two tests use it, `frames_sent_together_are_written_with_one_write_and_taken_by_one_read` and `a_batch_larger_than_the_pipe_pends_and_is_finished_whole_before_what_follows` (B).
- `MAX_MESSAGE_BYTES` (`message.rs` 33) is `crate::transport::MAX_FRAME_BYTES` again (B).
- Control `read_message` (`protocol.rs` 577-585) does a subset of `MessageReader::read` (591-640), as its own doc says ("A reader with a timeout uses [`MessageReader`] instead"); its remaining users are verbatim-agent's tests (A).

### 14. A wildcard in `outside_the_queue` (A only, confirmed)

`request.rs` 994-1004 ends `_ => false,`: a new request kind made outside the queue would be treated as a queue request with no compile error. Making the match exhaustive costs nothing.

### 15. Small model items (B only, confirmed)

- `Input::MarkReached` (`event.rs` 767): no constructor; `QueueInput::MarkReached` becomes `Entry::Mark` (Core `entries.rs` 1199). The one match is a test helper.
- `TextReply::Moved` and `TextReply::Selected` never cross the pipe: the outpost answers every text op but the watch through `held::perform_placed` (`text_reads.rs` 623), which turns them into `CaretPlaced` and `SelectionPlaced` (`text/held.rs` 454-467); `Selected` is rebuilt inside Core (`review_text.rs` 219). They are outpost-internal values in the shared enum; Core must still accept `TextReply`'s full set, so nothing goes but a split of the type, and no lines are saved.
- `CurrentFocus` (290), `CompleteUia` (514-534) and `MsaaRun` (627-645) each repeat `set`, `known`, `details` and, since bc99bc40, `hidden`. One shared spec type would make the next focus-read option one field.

### 16. Four copies of the typed properties request in Core (B only, confirmed)

Core `navigator.rs` 142 `Properties`, `report_focus.rs` 62 `FocusProperties`, `announcements/selection.rs` 97 `NodeProperties` and `changes.rs` 278 `ChildCountRead` each implement `Request` with the same `reader`, `window` and `wire` (`Wire::Properties { node, set }`); three decode `Reply::Properties(values) => Some(values)`, and `ChildCountRead` picks one count out. About 90 Core lines; outside this area.

### 17. raw.rs's mirror (both; M6.4)

`raw.rs` 40-110 mirrors `Answer`, `Outcome`, `Reply`, `TextReply` and `TerminalSnapshot` as `Wire*` types to put terminal text in the raw section, decided by Dickson on 2026-10-28 (`docs/design/communication.md` 62: "a terminal's text and colours, which made a whole-document read cost the reducer 0.5 to 1.6 ms to parse"). Both directions destructure exhaustively, so drift fails to compile. M6.4 rewrites these reads.

### 18. ObjectClass (both; planned)

`request.rs` 737-741, `pub enum ObjectClass { Plain }`, matched in Core `focus/duplicate.rs` 32, 60, 107. Its doc (731-736) says "Word's, Outlook's and Internet Explorer's join with the milestones that read them", so deleting it would be reversed; kept.

### 19. Find and Children (both)

`Request::Find` and `Request::Children` (249-267): grep `Request::(Find|Children)\b` outside test code finds only the outpost's dispatch (`asked.rs` 96, 108, 201-202), outpost-logic `waiting.rs` 44-45 and Core's plain reader (`plain.rs` 285-286); no product code constructs them. They are ADR 0003's "find a descendant by property" for M5, so they stay unless Dickson rules otherwise.

### 20. Stale guide text (B only, confirmed)

`books/dev/src/reference/crates/verbatim-model.md` 733-734: "`Indication::catalogue()` lists all 96 in display order: the 52 roles (`role-link`), the 13 spoken states". `Role::ALL` has 71 (`tree.rs` 187) and `SPOKEN_STATES` 16 (`theme.rs` 67), and the test at theme.rs 967-970 pins the catalogue's length as `Role::ALL.len() + SPOKEN_STATES.len() + NEGATED_STATES.len() + 28`. Lines 250-252 say the ADR 0003 types are "built beside the protocol below; no reader or Core uses them yet"; lines 474-475 still describe Core's sound for the not-responding message. The guide should point at the test.

## 3. Test findings

### verbatim-model (75 tests, 1,698 lines)

- Remove now: `every_variant_has_its_reviewed_kind`, `core_sends_no_report` and their two sample chains (`protocol/tests.rs` 31-100, 177-280); `a_timing_from_an_older_peer_reads_with_no_calls` (353-358). 3 tests, about 180 lines.
- Replace now: `a_generic_read_and_its_answer_travel_as_json_and_back` (48 lines), `a_cancel_travels_with_its_reason` (8), `a_uia_focus_tell_travels_as_json_and_back` (50), `a_reply_round_trips_through_json` (19), `tree_node_round_trips_through_json` (30) test serde's derive; one round trip per message direction over the existing samples replaces them. 5 tests and about 155 lines out, 2 tests and about 20 lines in (both reviewers). The hand-written `NodeId` codec's tests stay.
- Remove now: one of `a_pongs_trip_back_runs_from_its_write_to_verbatim_exes_read` and `a_bar_changes_trip_runs_from_the_listeners_write_to_verbatim_exes_read`, both of which test `hop_us` through a message (A). 1 test, about 15 lines.
- If Dickson rules (question 1): the 10 property tests of the unread columns and sets (`every_property_has_one_entry_in_the_table_in_order`, `every_property_is_read_by_some_interface`, `an_extension_names_a_property_by_its_entrys_name` unless the name stays for M5, `a_set_lists_its_properties_in_field_order`, `a_sets_answer_parses_into_its_fields_in_any_order`, `a_set_goes_out_and_back_unchanged`, and the four "does not parse" tests), about 170 lines; the 4 event-kind tests, about 95 lines. B's alternative if the table stays: replace `each_interface_event_has_one_teller` with a test over ia2's `SUBSCRIPTIONS` and the outposts' subscriptions.

### verbatim-control (23 tests, 874 lines)

- Replace now: the 11 send-keys parse tests (`a_gesture_is_pressed_with_the_verbatim_modifier_as_insert` through `parse_all_resolves_every_entry_in_order`, 134 lines) cover one parser; 2 table tests of about 40 lines replace them (A). 9 tests and about 90 lines fewer.
- Remove now: of `requests_and_frames_round_trip`, `dump_tree_request_and_reply_round_trip` and `dump_recorder_request_and_reply_round_trip`, keep one (A; B named the two dump tests). 2 tests, about 60 to 100 lines.

### verbatim-process (63 tests, 2,021 lines)

- Remove now: of `messages_round_trip_over_a_byte_stream`, `ping_and_pong_round_trip` and `the_idle_barriers_query_and_answer_round_trip`, keep one (A). 2 tests, about 50 lines.
- Remove now: `send_all`'s two tests with it (B). 2 tests, about 40 lines.
- With one classification (finding 8): one of `a_frames_kind_is_its_variants_name` and `each_urgent_message_converts_to_the_variant_of_its_name` goes. 1 test, about 30 lines.
- The framing and pipe tests (`transport.rs` from 1537, `raw/tests.rs`) test real behaviour and stay (both).

### Outside the area

- `crates/verbatim-app/src/latency.rs`: the tests of the 19 dead stages, about 250 of its 727 test lines, go with the computations (both).

### To add

- Finding 11: one control-server unit test that `has_speech_subscribers` is false once the history has closed with no subscriber.
- Question 1, if Core parses answers at the boundary: one test feeding Core's focus execution a `FocusNode` whose values lack `Role`, asserting the answer is refused as undecodable (A).
- Question 3, if a request stage is added: one test of the request-to-answer stage (B).

Totals, remove and replace now in the area: 24 tests fewer and about 630 lines, about 40 lines added. If Dickson rules: 14 tests and about 265 lines more.

## 4. Questions for Dickson

### Question 1: ADR 0003's "Properties, event kinds and requests" as built (findings 1 and 2)

The decision is quoted in finding 1. The fact that has changed: its Build 1 is pushed, and the readers and Core map every property by hand (eight mappers, listed in finding 1), the UI Automation cache is built from `verbatim-uia`'s shapes, no read names a compile-time set, and of the event-kind table only `cached` is read. No speech changes under either shape; one concrete sequence is A's: a reader answers a focus without `Role`, and today Core speaks it with no role and logs nothing. The shapes:

- (a) Cut the property table to what is read (`id`, `name` for M5's run-time naming, `kind` if answers are checked, `detail`), and the event-kind table to one `cached` match: about 1,050 model lines and 14 tests go. A also proposes that a node read answer a typed `NodeSnapshot` plus the few extra properties, removing the mappers back to `NodeSnapshot` and their silent `Role::Unknown` (about 150 lines in the outpost, the listener and Core). This changes the ADR's wording ("how each interface reads it", "a set of properties fixed at compile time").
- (b) Keep the decision and build it: the readers' matches, the `read_apart` dispatch and ia2's subscription table derive from the tables, and Core parses answers with `property_set!` at the boundary, deleting the readers' copies instead.
- Doing neither keeps two declarations of each property and event kind.

The ADR's reason, line 55: "today a new property touches about twenty files in five crates ... declared once, a property is one entry". 8ec23de0 (section 5) touched four files and three places in the outpost's `asked.rs`.

### Question 2: request kinds per behaviour against "A new command adds no kind of request" (section 5)

Of 15 `Request` variants, 6 serve one behaviour each, 2 serve none yet, and of 13 text ops 9 serve one behaviour each. The shapes the reviewers would have needed for the three traced behaviours are laid out in section 5, without a recommendation. One touches a dated ruling: ADR 0003 line 46, "The listener has one kind of request waiting on an event, which Core can cancel: 'tell me when this window is shown', kept on its WinEvent thread from the request to its one answer (Dickson, 2026-11-19)". B names as the changed fact that `Follow` is already a generic registration that tells. No speech changes.

### Question 3: the control protocol, a public contract (findings 3 and 4)

May the never-produced vocabulary go from the control protocol, at version 5: the 19 `LatencyStageKind` variants (about 110 lines with labels), `StatusInfo::live_re_reads` and version 4, which exists only for it (`protocol.rs` 1-7, 27-33; every producer writes 0, app `main.rs` 2288 and agent `server.rs` 1245; verbatim-inspect prints it at `main.rs` 153), `OutpostState::Restarting` (336-337, constructed nowhere, matched by verbatim-inspect 165), and `NormalizedEvent`'s 5 never-constructed variants, its 4 always-constant fields and the 4 types only they use (about 200 model lines)? No client ever received any of them. No speech change. B also proposes adding one stage for a request's round trip, which is a new contract item.

### Question 4: Find and Children before M5 (finding 19)

Keep `Request::Find` and `Request::Children` and the outpost's code for them until M5 uses them, as ADR 0003's shared list names them, or remove them until then?

## 5. Cost of one behaviour

### The counts, settled from the code

| Kind | Count | Variants with file and line | How each reviewer counted |
|---|---|---|---|
| `Request` | 15 | `request.rs`: ReadObject 231, Properties 233, Navigate 240, Find 249, Children 262, CompleteUia 269, MsaaRun 271, CurrentFocus 273, Text 293, Activate 300, Follow 305, DumpTree 309, ShellItems 319, ShellItemPosition 328, AwaitShown 339 | A: 15, the enum. B: 27 "leaf" kinds, the 14 variants other than `Text` plus the 13 text ops. Both right for their unit |
| `TextOp` | 13 | `text.rs`: StartingRead 975, Watch 977, MoveBySentence 980, ReadSelection 983, Read 985, ReadRange 988, Select 1001, MoveCaret 1013, Location 1018, ReadAhead 1021, TerminalHistory 1026, TerminalFind 1047, ReadTerminal 1061 | both 13 |
| `Reply` | 16 | `request.rs`: Object 835, Properties 837, Focus 839, Candidate 841, CandidateSame 845, Navigated 848, Found 851, Children 853, Text 855, Activated 858, Followed 860, Tree 862, ShellItems 864, ShellItemPosition 867, HiddenDialog 872, Shown 874 | A: 16, the enum. B: 38, the 16 plus `TextReply`'s 22, which counts the `Text` container and its contents both; leaf replies are 37 |
| `TextReply` | 22 | `text.rs`: Started 1117, Watched 1119, Sentence 1121, SelectionRead 1123, Read 1128, Chunks 1139, Range 1146, Moved 1153, Selected 1162, CaretPlaced 1167, SelectionPlaced 1177, TerminalRead 1184, TerminalHistory 1188, TerminalFound 1199, Location 1209, OffsetLocation 1219; failures UnsupportedUnit 1229, Unsupported 1231, NoText 1234, AnchorLost 1236, Gone 1238, Unanswered 1240 | both 22 (16 own, 6 failures) |
| Messages | 35 variants: `CoreToOutpost` 8 (176), `CoreToListener` 8 (290), `OutpostToCore` 9 (402), `ListenerToCore` 10 (480) | | Counted as ADR 0003 counts, by distinct kind per direction: 13 to Core (with the dead `NotResponding`; the ADR's "shutdown done" has no variant), 8 to the listener (`Configuration` added by ccdb2a7a, 2026-11-19), 8 to an outpost: 29. A said 29, B 28; A is right. The ADR's own list adds to 28 (13, 7, 8), not the 27 it states (line 148), as A noted |

Who uses each kind: every product constructor in `crates/verbatim-core` (grep `(Wire\|Request)::<variant>\b` and `TextOp::<variant>\b` outside test code; Core names the wire enum `Wire`), and in verbatim-app for the three requests from outside the queue. A behaviour family counts as one.

| Kind | Constructed at | Behaviours | One behaviour? |
|---|---|---|---|
| ReadObject | Core `changes.rs` 258 (`ChangeRead`, built by changes.rs 427, `announcements/alert.rs` 60, `show.rs` 33), `focus/uia_focus.rs` 46 (`ListenerRead`, built by uia_focus.rs 128, `desktop_name.rs` 27, `selection.rs` 191, `toast.rs` 49) | UIA focus, property changes, alerts, shows, the desktop's name, toasts, selections | no |
| Properties | `selection.rs` 109 (`NodeProperties`, built at selection.rs 295, 322, 365 and changes.rs 703), `changes.rs` 290 (`ChildCountRead`), `navigator.rs` 154, `report_focus.rs` 74 | selections, tree view child count, the navigator, report focus | no |
| Navigate | `navigator.rs` 88 | object navigation | yes, a generic kind the ADR names |
| Find, Children | none | none | none yet (M5) |
| CompleteUia | `uia_focus.rs` 79 (`Completion`), `selection.rs` 75 (`ItemCompletion`) | UIA focus; a selection's item | no (A said focus alone) |
| MsaaRun | `focus/msaa_run.rs` 192, built at msaa_run.rs 282, 289, 296, 318 and `keys/caret_key.rs` 424 (`FactsOnly`) | MSAA focus, menu start, a caret key's confirmation | no (A said focus alone) |
| CurrentFocus | `focus/retake.rs` 134, built at retake.rs 259, `menus.rs` 198, `uia_focus.rs` 256 | the re-take, the check after a menu end, a UIA focus's fallback | focus family |
| Text | see text ops | | container |
| Activate | `navigator.rs` 122 | the navigator's activate | yes |
| Follow | `focus/follow.rs` 77, `focus/execute.rs` 607, `entries.rs` 805 and 1303, `terminal/reading.rs` 199 | focus changes, terminals | no (A said focus alone) |
| DumpTree | app `main.rs` 1807 | the control plane's tree dump | yes |
| ShellItems | app `main.rs` 1832 | the notification area list | yes |
| ShellItemPosition | app `main.rs` 1836 | the notification area list's click | yes |
| AwaitShown | Core `focus/shown.rs` 80 | the dialog read once shown | yes |
| StartingRead | `caret_key.rs` 557 | caret keys | yes |
| Watch | `caret_key.rs` 727, 851 | caret keys | yes |
| MoveBySentence | `caret_key.rs` 893 | sentence keys | yes |
| ReadSelection | `caret_tell.rs` 138 | the caret tell's selection | yes |
| Read | `editing.rs` 791 (navigator line), `review_text.rs` 341, 397 | the navigator, the review cursor | no |
| ReadRange | `editing.rs` 781, `review_text.rs` 689 | the navigator's selection read, review copy | no |
| Select | `review_text.rs` 678 | review's select-then-copy | yes, generic in shape |
| MoveCaret | `say_all.rs` 588 | say-all | yes, generic in shape |
| Location | `caret_location.rs` 37, `review_text.rs` 701 | report caret location, report review location | no (both reviewers said one) |
| ReadAhead | `say_all.rs` 306 | say-all | yes |
| TerminalHistory | `terminal/commands.rs` 178 | terminal review commands | yes |
| TerminalFind | `terminal_model/review.rs` 902 | terminal command marks | yes |
| ReadTerminal | `terminal/reading.rs` 280, 304, 1163 | terminal output, baseline, caret keys and review reads in a terminal | terminal family |

Replies, by the Core code that decodes them (grep `Reply::<variant>\b` outside test code): Object (`selection.rs`, `changes.rs`, `uia_focus.rs`), Properties (four files), Focus (`uia_focus.rs`), Candidate (`msaa_run.rs`, `retake.rs`), Followed (`follow.rs`, `terminal/commands.rs`) and Text serve several; CandidateSame (`msaa_run.rs`, the MSAA duplicate-focus identity check), Navigated and Activated (`navigator.rs`), Tree, ShellItems and ShellItemPosition (decoded in the app), HiddenDialog and Shown (`focus/shown.rs`) serve one each; Found and Children none. `TextReply`'s own replies follow their ops; `Moved` and `Selected` never cross the pipe (finding 15); the six failures are shared.

So "serves exactly one behaviour" holds for 6 requests (Navigate, Activate, DumpTree, ShellItems, ShellItemPosition, AwaitShown), 9 text ops (StartingRead, Watch, MoveBySentence, ReadSelection, Select, MoveCaret, ReadAhead, TerminalHistory, TerminalFind: 7 behaviours, since StartingRead and Watch are both the caret keys' and MoveCaret and ReadAhead both say-all's) and 8 replies; A's list was too wide for CompleteUia, MsaaRun, Follow and Location, and both lists omitted Navigate and Select. B's "about 16 of 27 request kinds" includes HiddenRoot, Purpose and Depth modes, which are fields, not kinds.

Bookkeeping a new variant needs, by hand (both, verified): a node- or anchor-carrying variant gets arms in `Ask::unstamp`, `Answer::named`, `Answer::visit_nodes` and, for text, `Named::text_reply` and `TextReply::into_failure`; a new `OutpostToCore` variant also in `kind`, `carries_nodes`, `written`, `named`, `Urgent`, `Peeked`, `samples.rs`, the process test's match and app `main.rs` 1250-1262; a new request in outpost-logic's `waiting.rs` (44-53), the outpost's dispatch and set arm (`asked.rs` 96-211), Core's plain reader (`plain.rs` 283-309) and a typed `impl Request` in Core (21 today; B's 19 drifted).

### The three behaviours traced

**The dialog read once shown, bc99bc40** (one commit: 51 files, 3,718 lines added, 310 removed, A's figures; with its six follow-ups to 870bda79: 67 files, 4,931 added, B's figures; both verified with `git show --stat` and `git diff --stat bc99bc40~1 870bda79`).

- In the model (`request.rs` +52, `protocol/tests.rs` +1): `Request::AwaitShown { window: WindowHandle }`; a new enum `HiddenRoot { Check, ReadAnyway }`; a `hidden` field on `CurrentFocus`, `CompleteUia` and `MsaaRun`; `Reply::HiddenDialog(NodeWindow)` and `Reply::Shown`; `CancelReason::LeftHiddenDialog`; arms in `unstamp`, `named` and `visit_nodes`; the protocol sample's `hidden: HiddenRoot::Check`. No hand-written encode or decode (serde derives), no table row, no model test.
- verbatim-process and verbatim-control: no change.
- On either side: Core's event queue (`driver.rs` +76, `inbox.rs` +34, `limiter.rs` +47, `mod.rs` +31, `outposts.rs` +19, `request.rs` +14, `table.rs` +44), its focus code (`shown.rs` 388, `msaa_run.rs` 153, `uia_focus.rs` 150, `retake.rs` 41, `menus.rs` 27, `plain.rs` 3, `entries.rs` 26) and `shown_tests.rs` 1,077; the listener (`listener.rs` 276, `listener/shown.rs` 182, `asked.rs` 2) and listener-logic (`shown.rs` 239); ia2 (`event_thread.rs` 148, `hook.rs` 57, `acquire.rs` 12); the outpost (`asked.rs` 89, `hidden_root_tests.rs` 155, `mod.rs` 5); outpost-logic (`waiting.rs` 4, tests 4); mockapp's `late_dialog.rs` 345, and every test that builds one of the three changed requests (mockapp `call_counts.rs` 90, `focus_reports.rs` 145, `msaa_events.rs` 22 and others).
- Shapes that would have needed no change in this area:
  - Facts, not a mode: every focus read reports, on `NodeWindow`, its top-level window's visibility and, when hidden, its client object's role, and Core decides to wait. `HiddenRoot` and `Reply::HiddenDialog` go (both).
  - A generic wait: A's `Request::Await { kind: EventKind, window }`, answered when the listener hears that event on that window and "not there" when it is destroyed; B's registration like `Follow`, with the show arriving as an ordinary tell of `EventKind::Show`. `AwaitShown`, `Reply::Shown` and `LeftHiddenDialog` go. Either changes the ruling quoted in question 2. ADR 0003 line 46 gives the reason the wait is the listener's: "only the listener's desktop-wide hook hears that show, since an outpost holds no WinEvent hook".
  - A cancel carrying only whether to abandon the call (A): readers read nothing else of a reason, as `abandons_call()` (`request.rs` 1158-1168) is the only reader in outpost-logic `waiting.rs` 185, listener `listener.rs` 898 and listener-logic `schedule.rs` 801; then `LeftHiddenDialog` stays inside Core.

**Control+Backspace says the whole word, e380a0ce** (28 files, 633 added, 159 removed, verified).

- In the model (`text.rs` +52, `lib.rs` 8 changed): a new enum `StartingUnit { At(TextUnit), AtWithWord(TextUnit), BeforeCaret(TextUnit) }` replaces `StartingRead`'s `unit` and `word: bool`, with `for_motion`, the rule for which key reads where, and `unit()`; `Started.unit`'s doc. No encode or decode, no table row, no model test.
- verbatim-process and verbatim-control: no change.
- On either side: the outpost (`text/held.rs` 131, `text/mod.rs` 18, `uia.rs` 15, `text_reads.rs` 10, `check.rs` 8), verbatim-uia-rops (`caret.rs` 99, a `BeforeCaret` mode in the remote program), Core (`editing.rs` 54, `caret_key.rs` 10), and tests (mockapp `call_counts.rs` 112, `text.rs` 55, `watch_checks.rs` 27; Core `caret_key_tests.rs` 47, `tests/text.rs` 40; outpost-logic `caret_watch/seam.rs` 27).
- Shape that would have needed no change in this area (both): `StartingRead` carries a list of the generic `TextRead { at, movement, unit }` (`text.rs` 533-540), made in its one program, and `for_motion` lives in Core; Backspace's word is then `TextRead { at: Caret, movement: Some(TextMovement { unit: Character, count: -1 }), unit: Word }`, and `StartingUnit` goes. B notes the cost: the uia-rops program must then run generic reads, which the project's limit of 10,000 executed instructions per remote program applies to.

**A's third: an expanded tree view item's child count, 8ec23de0** (4 files, 56 added, verified).

- In the model (`property.rs` +24): `PropertyId::ChildCount` and its `entry()` arm; `MsaaRead::TreeViewChildCount`; a `CHILD_COUNT` entry whose `uia`, `msaa`, `ia2` and `cost` nothing reads; its `PROPERTIES` row; the test's count, 27 to 28.
- On either side: listener `asked.rs` one "not read here" arm; outpost `asked.rs` three places (the snapshot's `None` list, `is_read_apart`, the reading arm in `read_apart` calling `verbatim_ia2::acquire::tree_view_child_count`); Core `changes.rs` 27.
- Shape: with question 1's shape (a), the model change is the variant alone; with `read_apart` keyed by the property, the outpost's three places are one (A).

**B's third: a selection's item name read afresh, 7eb9035a** (7 files, 78 added, verified).

- In the model (`request.rs` +7): `Purpose::Selection`. No other model, process or control change.
- On either side: listener `asked.rs` +31 (870-872: `if purpose != Purpose::Selection { read.parts.name.clone_from(&event.name); }`), `listener.rs` 2, Core `announcements/selection.rs` 2, mockapp `controller_for.rs` 44.
- Shape that would have needed no change in this area (B): every read answers the fresh name, which, as the listener's own comment at 867-869 says, "came in the same cache request as its role and position", and Core, which holds the tell's cached name, chooses; `Purpose::Selection` goes and the reader judges nothing.

### Shared mechanisms the reviewers propose, merged

- A request names facts to read, never a mode for one feature (`HiddenRoot`, `StartingUnit`, `Purpose::Selection`, `Depth::FullUnlessSame`); a composite request takes a list of generic reads, and Core decides from the facts (both).
- One generic wait, "tell me when this kind of event happens on this object" (A), or a registration that tells (B), in place of a request kind per wait.
- Readers learn only whether to abandon a call (A).
- One walk over node ids and anchors, or a distinct type for an unstamped id parsed once at the boundary (A, B), and one classification of a message's variants (both).
- The property table drives the readers or is cut to what is read (both; question 1).
- One shared typed Core request per wire kind (B, finding 16).

## 6. Dropped

- A: "nothing calls `stamp_sent`, and every Ping is built with 0", dead Ping stamp. Contradicted: `crates/verbatim-supervisor/src/writer.rs` 131 stamps pings and listener commands, and `crates/verbatim-listener/src/listener.rs` 869 reads the stamp.
- A: `ToastId` (event.rs 520) dead. Contradicted by grep: Core `announcements/toast.rs` 11, 92-101 and `state.rs` 488.
- B 2.8: replace `port_queue` with crossbeam's channel. Contradicted by a decision of record: `books/dev/src/explanation/design/ownership.md` line 75 records "the hook's cancel took a channel's waker mutex" as a defect, and line 80 rejects "a standard channel" for the hook's route; crossbeam-channel 0.5.16's `SyncWaker::notify` takes a `Mutex` when a receiver waits (`src/waker.rs` 178-184, 221-224), and the hook thread posts to this port (`docs/design/communication.md` 178). No changed fact is named.
- B 2.12: the control server's own overlapped pipe code. B itself notes it is recorded as kept (communication.md section 16); no proposal.
- B: "`.kind` is read only in `verbatim-e2e/src/timeline.rs`". That is the timeline's own entry type, not `PropertyEntry`.
- Counts corrected rather than dropped: `EventTiming`'s fields (16, not B's 15); dead stage kinds (19, not A's 13 or B's 16); the transport's size (6,196 with the example, not A's 7,152 or B's 5,525 for `src` alone); the "one behaviour" lists (finding tables above); B's 2.5 proposal for `UiaSnapshotFact` (finding 12).
