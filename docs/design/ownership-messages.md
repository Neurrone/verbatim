# The messages build

Status: design, written on 2026-10-30 against main at 50370e74. It applies [the ownership model](ownership.md)'s rules "Messages are of declared kinds", "Queries have exactly one answer", "Merging only for kinds that declare it" and "Lifecycles as typestates" to the protocol between verbatim.exe and its children, the supervisor and the app shell; its brief, starting draft and known traps are "The messages build" in [the ownership builds](ownership-builds.md). It is roadmap milestone M4.4. It runs beside [the input build](ownership-input.md); "Where it meets the input build" names the shared code. The refactor preserves what is heard, and this build changes no speech; it undoes no recently finished work.

## Feasibility

The goal is achievable against the current code. Every wire type can move into `verbatim-model` with no new crate edge, every direction's messages already fall into one of the six kinds, and every lifecycle record's flags map onto states. Six facts of the code change the draft, and the design below takes each into account:

- The moved enums carry `#[non_exhaustive]` (`crates/verbatim-outpost/src/protocol.rs` lines 664, 865, 944, 966 and 1037). Inside their own crate that attribute is ignored; once they live in `verbatim-model`, every match in `verbatim-outpost` and `verbatim-app` would need a wildcard arm, which defeats the exhaustive matching this build exists for. The attribute is removed: the listener, the outposts and verbatim.exe are one build of these types and need no forward compatibility.
- `DeliveredFact::may_start_outpost` (lines 549 to 560) reads `verbatim_core::SNAP_RESULTS_ACTIVITY`, and `verbatim-model` cannot depend on `verbatim-core`, which depends on it. The function is the supervisor's routing rule, so it moves to `crates/verbatim-outpost/src/supervisor/policy.rs`, which may use `verbatim-core`. The protocol's re-export of `verbatim_core::focus_rules::HeldNode` (line 119) has no user left since the one judge build and is deleted.
- The draft's merge test compares Core's utterances with and without the older message. No merge passes that: a merge drops the older report's speech on purpose, as NVDA's limiter does (`nvda/source/IAccessibleHandler/orderedWinEventLimiter.py` line 79 gives a newer event of one key the newest place, and line 99 onward emits one per key). Core speaks a value change of its focus with `SpeechPriority::Queued` (`crates/verbatim-core/src/reduce.rs`, `reduce_value_changed`, lines 2148 to 2180), so "50" then "51" becomes "51" alone. The projection is redefined below ("The equivalence test").
- The relay's writers hold no judged message that merges. Their one replacement, a routed fact replacing a waiting fact of the same `FactKey` (`crates/verbatim-outpost/src/supervisor/writer.rs` lines 237 to 276), is NVDA's limiter applied to the listener's unjudged facts, the same rule as `HeldFacts::hold` (`policy.rs` lines 176 to 183). The release's slot, a newer `NodesHeld` replacing an unsent one (lines 300 to 304), is the release kind's own rule. So `Merge` is implemented only by `OutpostToCore`, for the outpost's outbound queue.
- An outpost has no reducer thread yet, so its reply tokens cannot live in that thread's shell as the model says. Today one slot holds the running query for every answer that can end it (`running` in the worker's watch, `crates/verbatim-outpost/src/outpost/worker.rs` lines 107, 456 to 463 and 697 to 703), and the reducer build replaces the intake, the worker and the watch. This is the second open question.
- The writer is shared by the listener and the outposts (`writer.rs`, `Outgoing` over `SupervisorToOutpost`). Splitting the enums by direction makes it generic over the child's message type.

The code has a defect that this build fixes. A wanted focus-now request is never ended when the application's last generation ends:

1. The attention application's ready outpost crashes with a backoff running.
2. The app records the application in `focus_now_wanted` (`crates/verbatim-app/src/main.rs` lines 2030 to 2036).
3. The application exits during the backoff.
4. The sweep removes the record with `OutpostId(0)` and tells the app nothing (`crates/verbatim-outpost/src/supervisor/owner.rs` lines 2165 to 2173 and 1755 to 1774). If the backoff ends first, the next generation is launched for the exited process, fails, and ends as `TargetExited`, and the app does not remove the entry then either (lines 2027 to 2045).
5. `idle_conditions_clear` (lines 1621 to 1627) stays false for the rest of the run, so every later `AwaitIdle` times out.

The same happens when the application exits while a replacement listener's focus-now request waits for its outpost to start.

No sequence was found that defeats the build's goal; "Sequences that would break it" lists those the design must prevent.

## Terms

- Wire types: the messages between verbatim.exe and the listener or an outpost, and the types they carry.
- A direction enum: one of `CoreToOutpost`, `OutpostToCore`, `CoreToListener` and `ListenerToCore`, replacing `SupervisorToOutpost` and `OutpostToSupervisor`.
- A kind: what `kind()` returns for a variant, `verbatim_model::protocol::Kind` (planned): `Ask` and `Answer`, the two halves of a query; `Report`; `Release`; `Lifecycle`; `Routed`; `Configuration`.
- A responder: a value, `#[must_use]`, that answers an asker once; answering consumes it, and its `Drop` answers "gone".
- A wanted request: a request the app holds for an application whose outpost is not ready yet, today `focus_now_wanted` and `ShellItemsWanted`.
- A generation's state: what the supervisor's per-application record knows of its current outpost generation, an enum replacing today's flags.

## The current code

- One enum per direction, shared by both kinds of child. `SupervisorToOutpost` (line 669) carries commands only an outpost takes (`DeliverFact`, `Query`, `Fetches`, `NodesHeld`, `Assign`) beside commands only the listener takes (`HookInstalled`, `HookRemoved`). `OutpostToSupervisor` (line 1042) carries `Event` and `Reply` beside `FocusFact` and `MenuOrSwitchEnded`. Each receiver drops what is not its own with a wildcard: the listener's `handle_command` (`crates/verbatim-outpost/src/listener.rs` line 1154), the owner's `listener_message` (`owner.rs` line 754, "The listener sends nothing else"), the outpost's `handle_command` by an explicit list (`crates/verbatim-outpost/src/outpost/mod.rs` lines 1548 to 1551), and the app's `on_outpost_input` (`main.rs` line 1478).
- `carries_nodes` (`protocol.rs` lines 1227 to 1236) is a `matches!`, so a new variant that carries node ids is not counted unless someone remembers it. Both the outpost's numbering (`crates/verbatim-outpost/src/outpost/outbound.rs`, `number`, line 208) and verbatim.exe's (`crates/verbatim-outpost/src/supervisor/frames.rs`, `take_frame`) call it.
- The outbound queue merges by a key of node, event kind and detail (`outbound.rs`, `key`, lines 63 to 84). A newer event replaces the older at the back. For a caret report the older report's selection changes are carried before the newer's (`merged`, lines 453 to 499). Nothing merges across a flush marker or an idle answer, and a caret report with selection changes does not merge across a reply or with a report in general terms (`waiting_with`, lines 187 to 205).
- The request table (`crates/verbatim-app/src/requests.rs`) gives each sent query one outcome: the first reply, "gone" at its generation's `Ended` (`outpost_ended`, lines 211 to 225), or "failed" at its deadline (`expire`, lines 228 to 247). Queries go only to ready generations, so a generation that fails before its `Ready` has none.
- Wanted requests have no single owner. `focus_now_wanted` is removed only at its outpost's `Ready` and at `NotWatched` (`main.rs` lines 1239 and 1452). `ShellItemsWanted` (`crates/verbatim-app/src/shell_requests.rs`) is ended at `Ready`, at `NotWatched`, at its 6-second deadline, and at an `Ended` with no replacement, which the app infers itself (`no_outpost_follows`, lines 2060 to 2064).
- A GUI request for the shell's items is a callback with no answer on drop (`crates/verbatim-gui/src/shell_items.rs` lines 24 to 60), so `ShellItemsHost::request` returns whether it was sent, and the GUI ends its enumeration itself when it was not (`crates/verbatim-gui/src/lib.rs` lines 394 to 404).
- The supervisor's `Record` (`owner.rs` lines 157 to 193) encodes its generation's life in `outpost` (with `OutpostId(0)` for none), `child: Option`, `spare: Option<Assigned>`, `ready: bool`, `fault`, `hang_check` and `unready`. `hooks_in` (line 1354) and `forwarding_ended` (lines 1384 to 1400) derive the hand-over from `ready`. Every transition is a sequence of field writes (`next_generation`, lines 1027 to 1066; `outpost_failed`, lines 1194 to 1270; `take_record`, lines 1785 to 1809).
- The app's `Live` (`crates/verbatim-app/src/live.rs`) keeps `ready: bool`.
- `OutpostMessage` (`crates/verbatim-outpost/src/supervisor/mod.rs` line 283) is public with public variants, so any code can build a `Started` or an `Ended`.

## The model this build applies

### The wire types in `verbatim_model::protocol`

A new module, `crates/verbatim-model/src/protocol` (planned), holds every wire type. That is all of `protocol.rs` except the framing: `EventTiming`, `hop_us`, the listener's facts (`UiaSnapshotFact`, `UiaAncestorFact`, `AncestorsEnd`, `UiaFocusRead`, `WinEventStamp` with `tick_is_after`, `ListenerFact`, `GapAnswer`, `DeliveredFact`, `FactKey`), the four direction enums, `Query`, `QueryOutcome`, `QueryResult`, `FocusNow`, `FocusedControl`, `DumpedTree` and `WALK_DEADLINE`. Each pure method goes with its type: `stamp_sent`, `sent_at_us`, `written`, `core_read`, `assign_outpost`, `carries_nodes`, `key`, `is_focus_read`, `is_win_event` and `ListenerFact::pid`. Reason: M4.4 and the communication design's decision 13 put the messages in a platform-neutral crate, beside Core's `Input` (`crates/verbatim-model/src/event.rs` line 584).

These stay in `crates/verbatim-outpost/src/protocol.rs`, since they use `verbatim-process` or `verbatim-ia2`, which use the Windows bindings: the `Message` trait and its implementations, the raw section (`protocol/raw.rs`), `write_message`, `encode_message`, `put_message`, `read_message`, `decode_message`, `message_kind`, `MAX_MESSAGE_BYTES` and `now_us`. `DeliveredFact::from_win_event` becomes the free function `fact_of_win_event` there. The trait being the outpost crate's own, its implementations for the model's types satisfy the orphan rule.

`verbatim-model` gains no dependency: every type derives only `serde`'s traits, which it has. The serde representation is unchanged, with the same variant names and fields, so every frame on the pipe is byte for byte what it is today, and `message_kind`'s peek still reads "Pong", "Ready", "TargetExited", "SpareReady" and "Fault". `verbatim_outpost::protocol` re-exports nothing from the model, so every import names the crate that owns the type. That means mechanical changes in `verbatim-app`, the outpost's own modules and tests, and 18 `mockapp` test files.

The mapping from an `OutpostToCore::Event` to `Input::Event`, today inline in `on_outpost_input` (`main.rs` lines 1424 to 1431), becomes one function of the module. The app and the merge test both call it, so the test reduces exactly what the app applies. `focus_inputs` (`requests.rs` line 461) stays in the app with the request table.

### Split by role and direction

- `CoreToOutpost`: `SetBackendOverride { backend_override }`, `DeliverFact`, `ForwardingEnded`, `Query`, `Cancel`, `Ping`, `Fetches`, `NodesHeld`, `IdleQuery`, `Shutdown` and `Assign`.
- `CoreToListener`: `SetBackendOverride { backend_override, pid }`, with `pid` no longer optional; `HookInstalled`; `HookRemoved`; `Ping`; `IdleQuery`; and `Shutdown`.
- `OutpostToCore`: `Ready`, `SpareReady`, `Event`, `Reply`, `Pong`, `Fault`, `TargetExited` and `IdleAnswer`.
- `ListenerToCore`: `Ready`, `FocusFact`, `ForwardingEnded`, `MenuOrSwitchEnded`, `ForegroundWindow`, `Pong`, `Fault` and `IdleAnswer`.

Reason: a message its receiver may not take cannot be decoded, so no receiver needs a wildcard to drop it (evidence: the owner's silent `_ => return` for listener messages). A frame of the wrong direction fails to decode and its child is ended as failed, as any frame that is not a message is today (`owner.rs` lines 533 to 562).

Each receiver's dispatch becomes an exhaustive match: the listener's and the outpost's `handle_command`, the owner's `listener_message` and `outpost_frame`, and the app's `on_outpost_input`. Each carries `#[deny(clippy::wildcard_enum_match_arm)]`, so a new variant fails the build at every receiver until it is handled.

The writer becomes `WriterHandle<M>` and `Queue<M>` over a trait `ToChild` (planned, in `writer.rs`). The trait gives what the overload policy needs: the ping, the shutdown message, the listener stamp and the relay stamp. The `Writers` map holds `WriterHandle<CoreToOutpost>` and `ListenerRecord` holds `WriterHandle<CoreToListener>`. The listener's writer never receives `Outgoing::Fact` or `FocusRead`, and its queue's policy is otherwise today's.

### Kinds and the reviewed list

Each direction enum has `kind(&self) -> Kind`, an exhaustive `match`, under `#![deny(clippy::wildcard_enum_match_arm)]` for the whole module:

- `Ask`: `Query`, `Cancel` (a query's withdrawal, answered by that query's own reply), `IdleQuery`.
- `Answer`: `Reply`, `IdleAnswer`.
- `Report`: `Event`, `Fault`, `MenuOrSwitchEnded`, `ForegroundWindow`.
- `Release`: `NodesHeld`.
- `Lifecycle`: `Assign`, `Ready`, `SpareReady`, `TargetExited`, `Ping`, `Pong`, `Shutdown`, `HookInstalled`, `HookRemoved`, both `ForwardingEnded`s. Each comes from one ordered path per generation: the owner thread's writer, or the child's own pipe.
- `Routed`: `DeliverFact`, `FocusFact`.
- `Configuration`: `Fetches`, both `SetBackendOverride`s.

A unit test in the module holds the list of every variant's name with its kind, built from one sample of each variant. The samples are made by a function whose `match` over the enum has no wildcard, so a new variant changes the function and the list, both reviewed. The same test asserts that no `CoreToOutpost` or `CoreToListener` variant is a `Report`, which is the back-channel's shape. That a variant carries no state of Core is a review rule, made visible by the list. A second test encodes each variant the relay peeks at and asserts `message_kind` names it as `outpost_frame` and `spare_frame` match it (`owner.rs` lines 586 to 640 and 644 to 674).

`carries_nodes` becomes an exhaustive `match` over `OutpostToCore`, with `Reply`'s outcome matched exhaustively over `QueryOutcome`: `Event` and `Reply` with `Done` are true, and every other variant is listed as false. Reason: the outpost and verbatim.exe number positions with this one function, so a variant forgotten here would shift both counts alike and never show, while releasing a node Core still reads from such a message.

### Merging in the outbound queue

`verbatim_model::protocol` defines `Merge`:

- `type Key: Eq`.
- `fn merge_key(&self) -> Option<Self::Key>`: `None` for a message never merged.
- `fn barrier(&self) -> Barrier`, declared once on the type: `Barrier::All`, which nothing merges across; `Barrier::Reply`, which only a key that refuses replies does not merge across; or `Barrier::None`.
- `fn may_merge(older: &Self, newer: &Self, crossed: Barrier) -> bool`.
- `fn merged(older: Self, newer: Self) -> Self`.

It also defines one generic function, `merge_into<M: Merge>`, which takes the waiting queue and the newer message and returns where the newer goes and what it replaced. `Pending::push_message` (`outbound.rs` line 159) calls it. `waiting_with`, `key`, `merged`, `carries_selection_changes` and `content_changed` become `Merge for OutpostToCore` and reproduce today's merge exactly:

- The key is today's node, event kind and detail.
- `IdleAnswer` is `Barrier::All`, `Reply` is `Barrier::Reply`, and the flush marker, which is the queue's own item and not a message, stays a full barrier in `Pending`.
- `may_merge` refuses a caret report with selection changes across a reply, and refuses either report in general terms.
- `merged` carries the older caret report's selection changes before the newer's, and otherwise keeps the newer.

Reason: a merge is a judgement made outside Core, and the type is where it is declared and tested once.

NVDA's limiter is not a merge and stays as it is in four places: the intake, the listener's schedule (`crates/verbatim-outpost/src/read_schedule.rs`, `push`), `HeldFacts`, and the writers' fact replacement.

### The equivalence test

`crates/verbatim-core/tests/merge.rs` (planned) is platform-neutral, since the trait, the messages and the mapping to inputs are all in the model. It has one test per merged event kind: a focus, a property change for each property, a selection, a controlled selection, an alert, a progress bar, a value change, a caret report, a text change and an active text position change.

Each test starts from one fixed state, with Core's focus in one outpost and its line of three nodes. It enumerates every sequence of the older message, zero to two others, and the newer. The others come from a fixed set of three: an event of a node outside both reports' lines, a reply to an unrelated query, and a fault. That makes 13 sequences per kind, and for the caret report the reply's placements also test the barrier. For each sequence the test reduces it in order on one side, and on the other side the others followed by `merged(older, newer)` at the newer's place.

It asserts equal, on both sides:

- Core's state: its focus and line, the nodes it holds, each node's states, name and value, the last announced selection, and the caret's node and position.
- The utterances caused by the others, as exact sequences.

It also asserts the merged message's utterances as an exact sequence fixed for each kind: what Core says today for the merged report. The older message's own utterances are left out on purpose, since dropping them is what the merge is for. Counters, the flight recorder, and the caret and text records of a focus the newer replaces are outside the projection, as the draft says.

A message of the older's node or its line between the two is not in the set. Such a sequence is the merge's reordering, a limitation stated below. One test each for the focus and the caret report pins today's outcome for it. The equivalence is exhaustive over a fixed set, not drawn at random, so each run checks the same sequences (`docs/testing.md`, "One target, one code path").

### Responders and the endings of a query

Across processes, the asking side's table answers. The request table's endings stay as built, and each gets a unit test: the reply, an outpost generation's `Ended`, and the deadline. A generation that fails while ready reaches the app as `Ended` before its backoff starts (`owner.rs` lines 1254 to 1263), so "gone" at a backoff is that ending, and one test sends a query, fails its generation with a backoff, and asserts "gone" at the `Ended`.

The wanted requests get one owner, `crates/verbatim-app/src/wanted.rs` (planned), which replaces `shell_requests.rs` and `focus_now_wanted`. It holds both kinds, keyed by the process whose outpost they wait for. Each entry ends exactly once:

- At its outpost's `Ready`, where it is sent and the request table owns it from then on.
- At the application's `NotWatched`, where it is answered "gone".
- At the application's `BackingOff`, for a shell items request only, which is answered "gone". A focus-now request waits for the replacement, as today, so the replacement reports the focus.
- At its deadline, for a shell items request only, as today.

`idle_conditions_clear` reads the table's count of focus-now requests in place of `focus_now_wanted.is_empty()`. The app keeps the processes whose replacement waits for a backoff, set at `BackingOff` and cleared at that process's `Started` or `NotWatched`, so a shell items request made during a backoff is answered "gone" at once. This is a lagging copy of the supervisor's lifecycle fact, whose staleness costs no word.

The supervisor sends the two notices these endings need, from the owner thread's one outbox:

- `NotWatched` after every `Ended` sent by `end_because` (`owner.rs` lines 1755 to 1774), that is at every retirement and target exit, including a record removed during its backoff with no generation. `no_outpost_follows` in the app is deleted, so "no generation follows" has one writer.
- `BackingOff { target_pid }` (new) when `outpost_failed` leaves a wait above zero (`owner.rs` lines 1266 to 1268), after the failed generation's `Ended`, if it was ready.

Within a process, the answering side holds a responder. `verbatim_model::protocol::Responder<T>` (planned) holds a boxed `FnOnce(Option<T>) + Send`. `answer(self, T)` consumes it, and its `Drop` calls the closure with `None` unless it was answered.

- `ShellAsk`'s two variants hold `Responder<Vec<ShellItem>>` and `Responder<ShellCandidates>`. `ShellItemsHost::request` returns nothing, and the GUI's branch for an unsent request (`crates/verbatim-gui/src/lib.rs` lines 400 to 404) is deleted. A request lost with the reducer's port at exit, or dropped anywhere, now answers "nothing" through the GUI's own channel. `answer_nothing` becomes `Drop`.
- The control plane's replies (`Asker::DumpTree`'s `Sender`, `ShellCommand::DumpFocus`) are channels with room for their one answer. A dropped `Sender` already tells its receiver the request is gone, so they stay as they are.
- The listener's marker. The schedule is pure and holds each event's `Meta`, which is `Copy`, so the responder lives in the readers' shell. `queue_for_reading` (`listener.rs` lines 1323 to 1366) makes a `MarkerOwed` with the marker and hands it to `Readers::push`, which keeps it in `ReadersState` by marker. Applying `ScheduleEffect::Outcome` takes it out and passes it to `Reading::outcome`, which consumes it with the fact. `MarkerOwed`'s `Drop` sends `UiaFocusDropped` with `GapAnswer::LeavesOpen`, the drop for an element that could not be kept (`listener.rs` lines 1353 to 1363). In debug builds the shell asserts after every step that the owed markers are exactly those the schedule still holds, which is the "transition that ends a request emitted its answer" assertion. The ownership check's pure rule on `read_schedule.rs` keeps the responder out of the schedule.
- The outposts' reply tokens: the second open question.

### Lifecycles as typestates

The supervisor's record keeps what belongs to the application: `held`, `idle_query`, `last_attention_at`, `since` and `backoff`. Its generation becomes `Generation<C>`, generic over the child so tests use `()`, with five states:

- `Launching { outpost }`: a launch for the application is at work.
- `SpareGiven { outpost }`: a spare was given whose launch has not reported. This is today's `Assigned::Pending`.
- `Starting { outpost, child, spare, live }`: running, not yet `Ready`. `spare` says whether a failure is a spare's, with its last fault; `live` holds the pong time, the abandoned count and the hang check.
- `Ready { outpost, child, live }`: its hook is in. `hooks_in` lists these, and `forwarding_ended` relays only to the `Ready` state that names the generation in the listener's message. That takes any number of them, since a replacement listener sends one for each outpost on its command line (`listener.rs` lines 963 to 967).
- `Waiting { unready }`: no generation while a failure's backoff runs, with the generation that failed before its `Ready`, if any. It replaces `OutpostId(0)`.

Each transition is a method that consumes the state and returns the next with the notices to send, in order: `launched`, `assigned`, `ready`, `failed`, `given` (from `Waiting`, at the backoff's end) and `removed`. The owner applies the notices: the listener's hand-over messages and the app's `OutpostMessage`s. Leaving `Ready` emits `HookRemoved` in the same step, before any later generation's `HookInstalled`, so no path out of `Ready` can forget it (difference 25). The owner thread itself stays impure, as Dickson decided. `ListenerRecord` and `SpareChild`, whose `child: Option` is the same launch pattern, take the shared `Launching` and `Running` pair.

The app's `Live` becomes `Starting` or `Ready`, and `mark_ready` consumes `Starting`.

`OutpostMessage`'s lifecycle variants are `Started`, `Ended`, `Regenerated`, `NotWatched`, `BackingOff`, `SpareReady` and `ListenerReady`. Each carries a `Sealed` field, a type whose only constructor is private to the supervisor module, so only the owner's path makes them. With the feature `lifecycle-for-tests` (planned) in `crates/verbatim-outpost/Cargo.toml`, a `Sealed::for_tests()` exists, which `verbatim-app`'s tests take as a dev-dependency feature. `Event` and `Frame` are not lifecycle and are not sealed. Patterns that destructure a sealed variant add `..`.

## Where each part runs

- `verbatim-model`, on whichever thread calls it: the types, `kind()`, `carries_nodes()`, `Merge` and `merge_into`, `Responder`, and the mapping of an event to Core's input.
- An outpost, on whichever thread sends a message: the outbound queue's merge, under the sender's backlog lock as today.
- The listener: its I/O thread decodes `CoreToListener`. The readers' shell, under the readers' lock it already takes, keeps the owed markers, and each reading thread or the watchdog answers one by applying an outcome.
- verbatim.exe's relay, the owner thread: the generation states and their notices, the writers' queues, and the peek at each outpost frame.
- verbatim.exe's reducer thread: `Frames`, `Live`, the request table, the wanted requests and the backoff view, and answering a `ShellAsk`'s responder.
- The GUI thread: makes each `ShellAsk` and takes its answer from its own channel.

## Edge cases and what is heard

- The attention application's ready outpost crashes, its replacement waits for a backoff, and the application exits meanwhile. Nothing is heard, as today. The focus-now request is answered "gone" at `NotWatched`, and the harness's `AwaitIdle` is answered, where today it never is.
- The same application exits while a replacement listener's focus-now request waits for its outpost to start. `Ended` with `TargetExited`, then `NotWatched`. Nothing is heard, and idle is answered.
- Verbatim+F11 while File Explorer's outpost waits for a backoff. No dialog opens and nothing is heard, at once, where today the request waits up to 6 seconds for the same outcome.
- Verbatim+F11 while File Explorer's outpost is starting normally. The request waits for `Ready` and the list opens, as today.
- File Explorer's outpost retires while a Verbatim+F11 request waits for it. The request is answered "nothing" at `NotWatched`, as `no_outpost_follows` answers it today, and the next press starts an outpost and lists the items.
- A given spare fails before its `Ready`. `Regenerated`, then the failed one's `Ended` if the app heard it started, then the next generation's `Started`, as today (df4c15d9). The held facts go to the next generation and are spoken once.
- A replacement listener starts while an outpost is ready. Its `ForwardingEnded` for that outpost is relayed, and the outpost's hand-over keeps the earlier tick (`crates/verbatim-outpost/src/outpost/handover.rs` lines 207 to 220), as today.
- A burst of value changes of the focused slider while the outpost's write is in flight. Only the newest is spoken, as today.
- A caret report with selection changes waiting while a caret key's reply is written. The newer report is not merged across the reply, as today (dd363ecc).

## Sequences that would break it

- A forged lifecycle notice. A module outside the supervisor builds `Ended` for a live generation, and the app answers that generation's queries "gone" while its replies are still coming. Prevented: the seal.
- A generation's message after its replacement's `Started`. Prevented as today: `Live::accept` drops it, and the owner's `outpost_frame` drops frames of an ended generation.
- `HookRemoved` forgotten on a new path out of `Ready`. The listener forwards nothing for an application whose outpost is gone, and its events are lost until the next outpost's hook. Prevented: `Ready` is consumed only by transitions that emit it.
- `ForwardingEnded` relayed to a later generation of the same application, which would then miscount its hand-over. Prevented: it is relayed only to the `Ready` state naming its generation.
- A new message variant that carries nodes, not counted. Core reads a node from it after the outpost released it on a `NodesHeld` acknowledging a position counted without it. Prevented: the exhaustive `carries_nodes`.
- A new message variant handled by no receiver. It falls through a wildcard and is lost, as `CoreFocus` was added without its kind being asked. Prevented: the exhaustive dispatches, the kind list and the decode refusing the wrong direction.
- A merge across an idle answer. A change written before the answer reaches Core after the harness heard "idle". Prevented: `Barrier::All` on `IdleAnswer`, which `merge_into` checks for every implementation.
- A responder held in a pure reducer. Replaying the schedule would answer markers as the test drops its state, unlike the live run. Prevented: the responder lives in the readers' shell, and the pure rule forbids it in `read_schedule.rs`.
- A marker answered twice, once by its outcome and again by `Drop`. Prevented: `Reading::outcome` consumes the responder, whose `Drop` then has nothing to answer.
- A shell items request answered twice. The GUI ends its enumeration on an unsent request, and the dropped responder would also post "nothing". Prevented: `request` returns nothing, so only the responder answers.
- A wanted request ended by `NotWatched` that came before a new generation's `Started` for the same process, since retirement and a new start are sequential in the owner. This is today's behaviour, and the user's next request starts an outpost; stated as a limitation.

## The facts and their owners

- Which generation serves an application, and whether one follows: the supervisor's owner thread, through the generation states and their notices. The app's `Live` and its backoff view are lagging copies, ordered with what they judge, since every notice comes through the one outbox ahead of the generation's messages.
- Which generation has its hook in: the owner's `Ready` states. The listener's map follows by `HookInstalled` and `HookRemoved`, a lagging copy whose window is difference 25's one relay hop.
- A sent query's outcome: the request table. A wanted request's outcome: the wanted table. A GUI request's outcome: its responder. A marker's outcome: the readers' shell.
- What carries nodes: `carries_nodes`, one function both counters call.
- What may merge: `Merge for OutpostToCore`.

Facts left with two writers:

- The backend override, sent as two messages to the outpost and the listener (`CoreToOutpost::SetBackendOverride` and `CoreToListener::SetBackendOverride`), so the two can differ while one is in flight. Only tests send it.
- The fetch configuration, which the app pushes through a writer that refuses it when full (`main.rs` lines 1795 to 1800). Both are configuration, whose version Dickson put in M5.

## Limitations

- The merge's reordering: an event of the merged report's node or line queued between the two reaches Core before the merged report, so Core judges it against an older focus than without the merge. It is NVDA's limiter's own effect, and one test per kind pins it.
- An event's merge still drops its older report's speech, as today and as NVDA does.
- A wanted request ended at a retirement just before a new start for the same process is answered "nothing", as today.
- A shell items request made as a backoff ends, before the app hears `Started`, is answered "nothing" by the lagging view; the next request is served.
- `kind()` exists for the wire enums only. `OutpostMessage` and Core's effects to other threads have no `kind()`. That every variant carries no state of Core stays a review rule.
- The outposts' queries are answered by today's watch slot until the second open question is settled.

## Cost

- The event path gains no wake-up and no call. The exhaustive matches and the generic merge compile to what the current code does.
- Per UIA event in the listener: one insert and one removal in the owed markers' map, under the lock its schedule step already takes. That is 16 bytes and a box per marker while its event is held. The schedule's own bound limits how many it holds (NVDA's limiter, `ABANDONED_LIMIT`).
- Per application lifecycle: one `NotWatched` per retirement or target exit, and one `BackingOff` per failure with a wait. Each is a post to the reducer's port, which the app handles in a few microseconds.
- Memory: the per-process wanted table, and the set of processes backing off.
- The builder records the listener's processor time per UIA event from five runs of `measure_focus_explorer` before and after, the worst case being a focus flood, in `docs/performance.md`.

## Tests

### Added

- The kind list for each direction enum, the check that Core sends no report, and the peek-name test (`verbatim-model` and `crates/verbatim-outpost/src/protocol.rs`).
- `carries_nodes`: one case per `OutpostToCore` variant and per `QueryOutcome` for `Reply`.
- The equivalence tests, one per merged kind, in `crates/verbatim-core/tests/merge.rs` (planned), and the two reordering tests.
- `merge_into` (`verbatim-model`): nothing merges across `Barrier::All`. A key that refuses replies does not merge across `Barrier::Reply`, and any other key does.
- The request table: a query answered "gone" at the `Ended` of a generation that failed with a backoff. The existing ending tests are kept.
- The wanted table (`wanted.rs`): a focus-now request answered "gone" at `NotWatched`, and kept through `BackingOff`. A shell items request answered "gone" at `BackingOff`, at `NotWatched`, and at once while its process backs off. Each is ended once, never again by a later `Ready` or deadline.
- `Responder`: answered once; dropped unanswered gives `None`; answered then dropped gives nothing more.
- The listener's readers (`focus_read.rs` tests): an event pushed and dropped by the schedule's `REPLACED` answers its marker once, and dropping the readers with an event waiting answers its marker with `UiaFocusDropped`.
- The generation states (`owner.rs` tests, with `Generation<()>`), the df4c15d9 sequences and difference 25:
  - A spare given and failed before `Ready` gives `Regenerated`, the failed one's `Ended`, then `Started`.
  - The same with its launch still pending gives no `Ended`.
  - A ready generation's failure with no wait gives `HookRemoved`, then `Ended`, then the next generation's `Started`, and its later `Ready` gives `HookInstalled`.
  - The same with a wait gives `HookRemoved`, `Ended` and `BackingOff`.
  - A target exit while `Waiting` gives `NotWatched`, with the unready generation's `Ended` first when the app heard it started.
  - A retirement gives `HookRemoved`, `Ended` with `Retired`, then `NotWatched`.
  - `ForwardingEnded` naming another generation is not relayed.
- The app: a sealed notice built with the test constructor reaches the reducer, which keeps `outpost_messages_and_commands_reach_the_reducer_in_arrival_order` (`main.rs` line 2684) compiling.

### Removed or made cheaper

- The process tests of `crates/verbatim-outpost/tests/spares.rs` that pin notice orders through real failures keep one test per real path (a pipe that closes, missed pings, a hook that fails). Their ordering assertions move to the generation-state tests above. Examples are `repeated_failures_are_spaced_by_the_backoff_until_a_foreground_change` (line 1080), whose backoff waits become `Instant`s given to `Backoff`, and the combination cases of `a_round_that_asked_a_failed_spare_waits_for_the_next_generations_answer` (line 906). The builder finds the rest and removes each only once its replacement passes.
- `shell_requests.rs`'s four tests become `wanted.rs`'s.
- The outbound queue's merge tests (`outbound.rs` lines 649 to 796) stay as tests of `Pending` over the generic function. Their merge cases are not duplicated in the model.

### Scenarios

The full suite once, then three runs each of `outpost_crash_recovery`, `return_after_retirement`, `explorer_folder_window`, `system_information_tree` and `settings_system_page`.

## The known traps, against the code

- Review 3, finding 4: a killed outpost runs no `Drop`, so across processes only the request table answers. In-process responders are used only where both sides share a process.
- Review 1, finding 4, and review 2, finding 2: configuration's version is M5's, so its relay path is not built here. Its two-writer residue is stated above.
- Review 1, finding 5: the listener's `Waiting` and schedule are NVDA's limiter and stay. So do the writers' fact replacement and `HeldFacts`.
- Review 1, finding 9: seals and tokens sit on types of the sending crate (`Sealed` in `verbatim-outpost`, `MarkerOwed` in the listener's shell), never on wire types. No responder sits in a pure reducer.
- Review 2, finding 5: the types move into `verbatim_model::protocol` with no new edge. `SNAP_RESULTS_ACTIVITY` leaves with `may_start_outpost`, and the framing, `now_us` and the `WinEventKind` conversion stay.
- Review 2, nits: the projection is named and redefined; the seal covers the lifecycle variants only, with a test constructor behind a feature.

The builder updates the crate guides of `verbatim-model`, `verbatim-outpost`, `verbatim-app` and `verbatim-gui`, and `docs/design/communication.md`'s references to `SupervisorToOutpost` and `OutpostToSupervisor`. The roadmap's M4.4 is closed by the documents commit.

## Where it meets the input build

The two builds share no file in the hook, the speech crate, the supervisor or the outposts. They meet in four places, and whichever lands second rebases them:

- `crates/verbatim-model/src/lib.rs`: each adds one module line, `protocol` here and `key_seq` there.
- `crates/verbatim-app/src/main.rs`: `idle_conditions_clear` (lines 1621 to 1627), where both change the conditions; `next_deadline` and `expire` (lines 2070 to 2096), where this build replaces the shell items deadline with the wanted table's and the input build adds the lock-key deadlines; and the `ShellCommand` enum (lines 1032 to 1090), where this build changes `ShellItems`' payload and the input build removes the router's commands.
- `docs/crates/verbatim-model.md` and `docs/crates/verbatim-app.md`.
- `Input::Event`, which the input build leaves without a key number and this build's mapping builds. The mapping takes the type as the input build leaves it.

## Open questions for Dickson

- Should a reply be a barrier to every merge now, or only to a caret report with selection changes, as today? The brief asks for a test that nothing merges across a reply. Today a value, name or state change of one node merges across a reply while the outpost's write is in flight, and the caret rework's rule says "the clause against merging a caret report across a reply becomes the barrier on replies", which places the general barrier there. A general barrier now changes speech only under a backlog, such as hearing "50" then "51" where today only "51" is heard. Recommendation: keep today's rule in this build, declared on the type as `Barrier::Reply` that only the caret report's key refuses, so behaviour is preserved, and generalise it in the caret rework as its rule says. NVDA has no replies to compare.
- Should the outposts' reply tokens be built now on today's intake, worker and watch, or in the outpost's reducer build? The model places them in the outpost reducer thread's shell, which that build creates. Today one slot, the watch's `running`, already holds the running query for every answer (the worker's reply, the watchdog's "abandoned", a panic's "failed"), and `Cancel` answers "not started" from the intake. No query is known to go unanswered. Recommendation: the reducer build, as part of its abandonment and claimed-request records, so no test is written against code the next build replaces; this build adds nothing on the outpost's query path.
