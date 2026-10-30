# The ownership refactor's builds

Starting drafts for the builds that [the ownership model](ownership.md) orders. Each build's section here is a draft, not a specification: just before that build it is redesigned against the code as it stands then and given its own adversarial review. The drafts were written before the model was restructured on 2026-10-30 and reviewed three times (`experiments/ownership-study/design-review-1.md`, `design-review-2.md` and `design-review-3.md` in that folder); each build lists the findings that concern it as known traps, saying whether its draft already answers one, which its own design must check against the code. The refactor preserves behaviour: the only speech changes it keeps are the three the model lists (Dickson, 2026-10-30); Core's second foreground test, one node per window and the merges' evidence fixes moved to the focus pipeline rework, and the drafts below mark where they were. The title rule left the refactor for the focus pipeline rework ([its remaining builds](focus-pipeline-rework-remaining.md)), and both reviews' first findings went with it.

## Every build

- Preserved: everything heard in every scenario except where a build names a change; the calls each mockapp `call_counts` test pins, except repeats read in full; latency within D15; wake-ups and processor time per event, except where a build states an increase.
- Builders end at unit tests and `cargo xtask ci`; the parent runs the scenarios three times each per build, ten only for one that failed.
- Test cost: each build removes tests or makes them cheaper where its changes allow. That means tests of mechanisms it removes, and end-to-end or mockapp tests whose ordering cases become replayed unit tests of a reducer, or whose real waits become injected timers. A test is removed only once the cheaper test covering the same behaviour exists and passes, and the replacements meet `docs/testing.md`. Each brief names examples; the builder finds the rest.

## The one judge build

Designed against the code in [the one judge build](ownership-one-judge.md), which replaces its draft and answers its known traps: an outpost reads changes for every node it has not released and Core targets, so no outpost follows a guess of Core's verdict.

## The input build

### Brief

Runs beside the messages build.

Scope:

- `KeySeq` on every user input and on `Speak`, with `KEY_ORIGINS` deleted. One minter, the hook thread: the control plane's injected gestures, today numbered on their own thread (`crates/verbatim-app/src/main.rs`) and posted separately, are posted to the hook thread's message loop, which numbers and forwards them in order with the keys. Test: the race of design review 3, finding 5 (the control plane takes n and is pre-empted; the hook takes n+1 and cancels through it; n's speech is fenced), which can no longer occur.
- Gestures posted by the hook to the reducer's port in key order. The router's work becomes reducer effects: its time read, the GUI commands, and the lock key's 30 ms timer, whose expiry reads the toggle state through the shell. Check with `lock_key_announcements` that `GetKeyState` gives the same state on the reducer's thread as on the router's, both with no message queue; use `GetAsyncKeyState` if it does not.
- `behind_toggles`' ordering (`crates/verbatim-app/src/main.rs`) moves into the reducer: the marks of handled input and the idle waiters behind a pending lock-key announcement wait for it, and the idle conditions gain "no lock-key announcement pending". A command pressed while a key is held waits behind it (`Effect::ForKey`).
- `SpeakToken` covers `speak`, `speak_for_key`, `play_earcon` and `play_earcon_to_end`. The reducer thread returns it when it ends, so speech at exit uses it. The settings window's sound preview keeps its own function (87b4f284).
- The ownership check's hook rule, skipping `#[cfg(test)]` modules.

Tests added: a review command pressed while a caret key is held runs after the key's caret read; an injected gesture fences speech as a key does; a lock key's announcement is still heard after a key pressed within its 30 ms, as NVDA; an idle round waits while a lock-key announcement is pending. Test cost: any app test of the router thread's relaying becomes a reducer unit test.

Scenarios: `lock_key_announcements`, `notepad_caret_keys`, `text_box_caret_keys`, `text_box_selection_keys`, `notepad_review_cursor`, `text_box_review_cursor`, `windows_terminal_typing`, `conhost_typing`, `measure_keys_notepad`.

### Mechanism draft

#### One route and one sequence for user input

Every user input (the hook, the control plane's injected gestures, later M5's module commands, M12's remote input, M15's braille keys) takes a key number from one counter and enters Core's port by one route in that order, carrying it; an utterance a key causes directly carries the number. Reasons:

- Review commands reach Core through the router while key reports go straight to the reducer, so they have no common order (Dickson, 2026-10-30, settles the deferred question). Past defects: ba7064e5 and its reversal e180f14a. Sequence: Down Arrow is held for its caret read; the review current-line command runs before Down Arrow reaches the application and reads the old line.
- The key number travels today through the global `KEY_ORIGINS` mutex, which the hook takes on every key, against `docs/design/communication.md` section 6.2. Sequence: the reducer holds it for a `Speak` lookup and is pre-empted at normal priority; the hook at `HIGHEST` waits on a thread that is not running, so every key and its speech cancel wait, a priority inversion; M12 and M15 would add writers.

The lock keys keep NVDA's behaviour (`nvda/source/keyboardHandler.py` 740 to 750): the announcement 30 ms after the key carries no key number, so a key pressed meanwhile does not silence it.

Enforcement: each user-input variant of `Input` has a `KeySeq` field and `Effect::Speak` an `Option<KeySeq>`, so the compiler requires them; that only the counter mints `KeySeq` is a review rule, the type being platform-neutral. The ownership check reads the hook's module against an allow-list with no lock or file type; that its channel is unbounded stays held by the run-time check in `crates/verbatim-input-windows/src/lib.rs`, since a path cannot tell a bounded sender from an unbounded one.

### Known traps

- Review 3, finding 5: two minters of the key number race; answered by the hook thread as the one minter.
- Review 1, finding 8: fencing the lock key's delayed announcement departs from NVDA; the draft keeps NVDA's behaviour.
- Review 1, finding 9 and nits: `KeySeq`'s minting cannot be restricted by type in a platform-neutral crate; `GetKeyState` on another thread; `behind_toggles`' ordering of marks and idle waiters; the `KEY_ORIGINS` sequence is priority inversion, not the hook's timeout.
- Review 2, nits: `SpeakToken` must cover `speak_for_key`, `play_earcon` and `play_earcon_to_end`, and speech at exit; the hook's allow-list check cannot tell a bounded sender from an unbounded one and must skip test modules.

## The messages build

### Brief

Scope:

- The wire types move into `verbatim_model::protocol`, which is M4.4 (`SNAP_RESULTS_ACTIVITY` removed). The framing, encode and decode, `now_us` and the `WinEventKind` conversion stay in `verbatim-outpost`.
- The enums are split by role and direction, each with `kind()` and the golden list.
- Responders for every query, for the wanted requests, for `ShellAsk` and `ShellItemsHost::request`, and reply tokens in the outposts' shell.
- `Merge` with its implementations and declared barriers in the outbound queue and the relay's writers.
- Configuration's version is not built here: Dickson put it in M5 (2026-10-30). The configuration part of the mechanism draft below is M5's starting point.
- The supervisor's `Record`, `Live` and hook records as typestates (the outpost's own hand-over moves in the reducer build), and the seal on `OutpostMessage`'s lifecycle variants. Its test constructor sits behind a feature, since `crates/verbatim-outpost/src/supervisor/frames.rs` builds `Event` and the app's tests build `NotWatched`.

Tests added: a shell-items request answered "gone" when its outpost ends or backs off; a wanted focus-now request answered "gone" when its application's outpost ends; each `Merge` implementation's equivalence test; a reply never merged across; the supervisor's df4c15d9 sequences and difference 25 as typestate tests.

Test cost, examples: the supervisor's lifecycle tests that pin combinations of flags become typestate tests. Responders: the request table's endings (an outpost generation's `Ended` or backoff, the deadline) each get a unit test, since a killed outpost runs no `Drop`; the listener's marker gets its responder here.

Scenarios: the full suite once, then `outpost_crash_recovery`, `return_after_retirement`, `explorer_folder_window`, `system_information_tree`, `settings_system_page`.

### Mechanism draft

#### Message kinds

Every message between processes, and every message a reducer sends to another thread, is one of six kinds:

- A query, with exactly one answer. The side that answers holds a responder, `#[must_use]`, consumed by answering, whose `Drop` answers "gone". Responders live only in a thread's shell, never in a pure reducer's state, since an answer sent by `Drop` inside a transition would break replay: Core's request table in the app, and each outpost's reply tokens in its reducer thread's shell, keyed by request id. The reducer emits an answer effect; the shell asserts in debug builds that a transition ending a request emitted one, and replay tests assert one answer per query. A query with several answers (the caret rework's key request) holds a stream responder that must end with a final answer. Evidence: 886d8e73, 76180512, 8d01bbd1, 0b5826cd.
- A one-way report.
- A release, `NodesHeld`, by message position (under "Identity and the release rule"). Its trigger is the one judge build's: written on its own when the nodes Core holds in an outpost change or after Core handles a focus report, every other change of its list or position riding on the next message to that outpost; the kind declares it a barrier and gives it that trigger.
- Lifecycle, from one ordered path per source generation (evidence: 1a3dc022, 8d01bbd1, a39f01f8, de7f481e).
- A routed report: another child's fact passed on unjudged.
- Configuration, a standing parameter Core sets in another process or thread: `Fetches`, the backend override, the class tables, M5's rule tables. Rules below.

A message carrying Core's state for a child to judge against fits none.

Configuration. Core is its one writer, and each value carries a generation. Mechanism: the app posts a new generation to the relay thread's mailbox, and the relay, in one step of its thread before it reads any later frame, appends it to every outpost's writer and then to the listener's. Its overflow policy is "always gets through, behind what was queued before it": it is never refused, and a newer generation still waiting in a backlog replaces the older at the older's place. A starting outpost receives the current generation with its assignment, before its held facts. Since every pipe keeps its order, a listener judgement made under a generation reaches an outpost after that generation does. Today `Fetches` goes from the app thread through `send_to_outpost`, refused and retried on a full queue, and the writer fails other messages at once, so a listener judgement under a new generation can reach an outpost still on the old one; this mechanism removes that. Each receiver applies a generation between two events: the outpost's reducer as an input; the listener's reading threads by loading the current generation once when they begin judging an event, so a read names the generation it began under. Every fact, drop or reply judged under configuration names its generation. A judgement under an older generation than the receiver's own: a read's facts are judged again under the current rules; a skip or veto closes no gap record, so an open record's read at its bound decides. Stated limit: with no record open, a focus skipped in the moment of a change is lost, and so is a focus whose listener read, made under the older generation, is judged again and dropped while the outpost's own copy of the event was skipped under the older one. The override changes only in tests and the rule tables only on M5's reloads. No receiver keeps older generations. Evidence: the verdict defects from one table in two processes (094a8dc8 and three more), defect B in `explorer_folder_window`.

Enforcement: the enums split by role and direction (`CoreToListener`, `ListenerToCore`, `CoreToOutpost`, `OutpostToCore`), so a message a role may not send cannot be decoded (evidence: the silent `_ => return` for listener messages in `crates/verbatim-outpost/src/supervisor/owner.rs`). Each has an exhaustive `kind()` with no wildcard arm, held by clippy's `wildcard_enum_match_arm` denied in that module; a unit test holds the list of variants with their kinds, so a new variant changes a reviewed file and a Core-to-child one-way report fails it. That a variant carries no state of Core is a review rule, made visible by that list.

Where the types live (Dickson, 2026-10-30): in a module of the platform-neutral `verbatim-model`, `verbatim_model::protocol` (planned), which already holds Core's `Input`, so the code turning messages into Core's inputs sits beside them, satisfying communication decision 13 and M4.4. `verbatim-model` gains no edge (its types need serde, which it has, and nothing of `verbatim-text`; if one ever does, the edge `verbatim-model` on `verbatim-text` makes no cycle, since `verbatim-text` depends on no Verbatim crate). The framing and its encode and decode (`verbatim_process::transport`), and `now_us`, stay in `verbatim-outpost`, since `verbatim-process` uses the Windows bindings; the `WinEventKind` conversion stays there too; `HeldFocus` goes with `CoreFocus`, and `SNAP_RESULTS_ACTIVITY` leaves the protocol. Seals and tokens sit on types of the crate that sends, never on wire types.

#### Merging only for kinds that declare it

- NVDA's limiter (the intake's limiter, the listener's `Waiting`, `HeldFacts`' one-per-object rule) drops an older event for a newer and may reorder, as NVDA does. It stays, with its own ordering rules and tests.
- Messages already judged, in the outpost's outbound queue and the relay's writers, may merge only when their type implements `Merge`: the newer replaces the older at the newer's place, carrying forward what the older said about its own change that the newer lacks. In the refactor each implementation reproduces today's merge exactly, its equivalence test written against today's outcome; keeping a tree item's child count and a focus report's stronger `earlier` moved to the focus pipeline rework's third build (Dickson, 2026-10-30). Its test: for every drawn sequence of the older, others, and the newer, a projection of Core's state after reducing (Core's focus and line, the nodes it holds, node states, names and values, and the utterances) equals that after reducing with the older dropped and its evidence kept; counters, the flight recorder, and the caret and text records of a focus the newer replaces are outside it. A barrier, declared once on the type (replies, answers, idle answers, hand-over messages, markers, `NodesHeld`), is never merged across. Evidence: dd363ecc, 913bb0e8, the losses in [the remaining focus builds](focus-pipeline-rework-remaining.md). Enforcement: the queues' merge function is generic over `Merge`.

#### Lifecycles as typestates

A lifecycle is an enum whose transitions consume the old state. Evidence: the supervisor's `Record` encodes its lifecycle in booleans, options and an `OutpostId(0)` sentinel (df4c15d9); the hand-over's states are implicit (difference 25, 89f94d33); M6.2's invariant.

### Known traps

- Review 3, finding 4: a killed outpost runs no `Drop`, so only the asking side's table answers across processes.
- Review 1, finding 4: what a receiver does with a judgement under an older generation, and what "between two events" means for the listener's reading threads; answered in the draft.
- Review 1, finding 5: the listener's `Waiting` reorders and drops, so it is NVDA's limiter, not a lossless merge; answered by separating the two.
- Review 1, finding 9: a token on a wire type cannot be private to a module of another crate; responders held in a pure reducer would break replay.
- Review 2, finding 2: no production path writes configuration in one relay pass, and the writer refuses a message on a full queue; the draft names a relay path and its overflow policy, to be checked against the code. Dickson put configuration's version in M5, so this trap is M5's.
- Review 2, finding 5: the wire types' edges; resolved by `verbatim_model::protocol`, with the framing, encode and decode, and `now_us` staying in `verbatim-outpost`.
- Review 2, nits: the `Merge` equivalence needs a named projection; the `OutpostMessage` seal must cover only lifecycle variants, with a test constructor; the older-generation limit also covers a read judged again and dropped.

## The outpost's reducer build

### Brief

Scope:

- The reducer on its own thread, owning everything the design lists.
- Workers run call programs on copies, including the yielding console poll and releases under a deadline. The watchdog only posts timers.
- The commit point's lock and its lock ordering (the commit's locks nested inside the watch lock; [prior art](../../experiments/ownership-study/prior-art.md), "Who may block on whom") are removed.
- The object store in `outpost::store`, owning both registries and the handle map. Walks intern through its trait, every mint tagged by generation and request.
- `Recorded` snapshots, which make the release rule a property of the store's types; the rule itself, the splice fix and text anchors released by message position are built in the one judge build. One node per window is not built here: it moved to the focus pipeline rework's third build (Dickson, 2026-10-30).
- The outpost's MSAA hand-over (`crates/verbatim-outpost/src/outpost/handover.rs`) moves into the reducer once, as the hold Dickson decided on 2026-10-25 (hook copies held until forwarding's end, in place of counting with `TWIN_BOUND`), with released events tried as today's batch, newest first, so nothing heard changes; the remaining doc's hold section and its review's cost finding are its draft. `handover.rs`'s ten tests become replay tests of the hold.
- The ownership check's reducer rule.
- Identity and the store change only in this build.

Every existing intake, worker, gap, commit and mockapp test keeps what it asserts; the unit tests move onto the reducer's inputs.

Replay tests added:

- the marker against the copy;
- the copy against the gap's timer (f2f8209a), and the gap's fast failure (e02c7854);
- a foreground change against a focus in one batch;
- a call returning before and after its bound and its abandonment;
- the four rounds on the publication, one test each:
  - c47d57ae: a publication when the worker is abandoned before adopting, and the worker's sends after it;
  - 6dbcca25: abandonment in an owed terminal caret read;
  - 2bd69a85: the worker and the bound's publication racing to write follow state;
  - 8a087fe7: a release into a hung terminal while the bound is published;
- 463f2ef9's three sequences;
- the poll yielding to a waiting caret read, ending on its check, and not yielding to a release;
- an abandoned worker inside a text call followed by a caret read that no longer waits;
- a focus group's older entries kept across an abandoned worker;
- a walk in flight crossing a `NodesHeld` that keeps its unrecorded nodes, and a dead generation's nodes freed;

Test cost, examples. These mockapp tests of `crates/mockapp/tests/focus_reports.rs` wait on the real one-second gap bound or race real orderings: `a_winevent_focus_no_uia_focus_follows_is_read_once_its_bound_has_passed`, `an_answered_record_cancels_its_timer`, `a_return_within_the_bound_is_read_with_the_returns_stamp`, `a_change_raised_after_a_focus_event_waits_for_the_focus_read` and `a_change_of_the_old_focus_queued_behind_a_focus_inside_it_is_handled_first`. They become replay tests with injected timers, keeping one mockapp test per path into a real provider. 6dbcca25's counterexample needed two 10-second deadlines and was pinned by a hand-driven unit test; it becomes a plain replay test. The gap record's unit tests in `crates/verbatim-outpost/src/outpost/gap_record.rs` move onto the reducer's inputs.

Measured, at most 20 runs each, for wake-ups, processor time and latency: `measure_focus_explorer`, `measure_focus_text_box`, `measure_keys_windows_terminal`, `measure_keys_conhost`, and the terminal baseline of 2026-10-23. `docs/performance.md` states the reducer's hop and wake-up cost.

Scenarios: the full suite once, then the `windows_terminal_` and `conhost_` scenarios, `text_box_editing`, `notepad_editing`.

### Mechanism draft

#### Pure reducers, with calls as effects

Every owner whose decisions depend on order is a pure state machine: inputs in, effects out, no clock, no call, no blocking. Calls into an application, timers and messages are effects; their results come back as inputs. Local reads of Windows' own state that never wait on an application (`GetForegroundWindow`, `GetAncestor`, `InternalGetWindowText`, `IsHungAppWindow`) are reached through an injected query trait, as `WindowQueries` in `crates/verbatim-outpost/src/outpost/acceptance.rs` is today, implemented live by the thread's shell and by a recording in tests, since the acceptance test interleaves 5 to 10 such reads. A timer belongs to the reducer that owns its record, set and cancelled as effects in transition order. Reason: a pure owner's orderings are enumerated by replay tests; an impure one's are found by review (the publication's four rounds, f2f8209a, 463f2ef9, the listener scheduler's three rounds).

The pure owners: Core's reducer (built), the outpost's reducer (new), and the listener's read scheduler as a pure transition under its existing lock (new).

Enforcement: a new ownership check in `cargo xtask ci` reads the reducer modules and fails on any `use` or path outside an allow-list (the module's own items, `verbatim_model`, `verbatim_core::focus_rules`, `std`'s collections, `cmp`, `mem`, `sync::Arc`, `time::Duration`), skipping `#[cfg(test)]` modules. It reads text, so a re-export could hide a call; the allow-list holds none, and adding to it is a reviewed change. Purity's real test is replay.

#### No state lent across a call into an application

No thread holds a lock or a borrow across a call into an application on state another thread may need; it works on a copy, and only the result's transition records what changed. Reason: a thread abandoned inside the call keeps what it holds, and its replacement blocks on it in turn. Evidence: d0ec3a89; the text anchors' locks held across text calls (an open follow-up); 5797c075's workaround. Enforcement: the outpost's reducer holds no COM object and a worker never sees its state; the object store's API returns owned clones and owned removed objects and has no method returning its guard or taking a closure, which Rust's privacy enforces outside its module; inside it, that nothing calls or drops a removed object under the lock is a review rule held by its tests.

#### The outpost's reducer

Each outpost holds one pure state machine for its application, in `crates/verbatim-outpost/src/outpost/reducer` (planned), with no Windows types. It contains ported NVDA code, so it lives in this Windows-specific crate; platform-neutral pieces stay in `crates/verbatim-core/src/focus_rules.rs`. It owns every remembered fact of the application: the intake's order (the marker's hold, the limiter, the newest-first batch); the gap record and its timer; the verdict keeps; the unreleased reports and follow state, today's commit point; the line a walk may stop at and MSAA equality's candidates, from the unreleased reports; the hand-over's hold; owed work and abandonment; text records, the caret watch and terminals.

Inputs: the intake's ordered entries; call-program results tagged by worker generation and request; timer expiries; Core's queries, configuration and `NodesHeld`. Effects: call programs, local reads, messages to Core, timers, releases.

What the outpost reads for changes is not a decision of the reducer: it reads for every node the store has not released, and Core targets ([the one judge build](ownership-one-judge.md)).

Threading (Dickson, 2026-10-30): the reducer runs on a thread of its own, since UIA calls cannot be cancelled; workers run call programs and post results; the watchdog only posts timer inputs (the bound, the deadline, the moved-on grace); objects the reducer releases are handed to a worker. The I/O thread, the event thread, UIA's callbacks and timer callbacks only append to its mailbox, today's intake; the intake's push-time drops stay, on copies written only by the reducer under the intake lock. A focus makes one to three call programs, so its latency grows by under 0.2 ms. What it removes: the commit point's lock and lock order, the watchdog publishing, the gap record written by three threads.

Abandonment is the reducer's decision on the deadline input: it marks the generation dead, starts a replacement and answers "abandoned". Publication at the bound is its transition on the bound input. Owed work is a record handed to the next worker as its first program. A late result of a dead generation or a claimed request is discarded. The sequences of c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7 and 463f2ef9 become replay tests named in the briefs; none can recur, because the worker writes no follow state and sends nothing to Core.

#### Workers and call programs

A worker takes one call program at a time with copies of what it needs, makes the calls, and posts the result. A program is a whole sequence with pure stopping rules: the focus's walk, the caret read, the terminal snapshot, a text operation, the `WinEvent` sync, a probe, the console caret poll, a release. Each makes exactly the calls today's code makes, so every `call_counts` test passes unchanged. A release runs under an entry's deadline like any program; a final `Release` that hangs is abandoned like any call, and the programs queued behind it go to the replacement.

The console caret poll (463f2ef9) loops on the worker's own wait, through the high-resolution waitable timer of the worker's alarm (`crates/verbatim-outpost/src/outpost/caret_poll.rs`), one wake per 5 ms tick as today. Before each read it looks at the worker's inbox: a waiting call program, such as the next key's caret read or a terminal read, makes it return at once with a yield result carrying its state, and the reducer posts it again after that program; a waiting release does not, and waits at most the poll's 50 ms bound. After each read it applies the evidence check, a pure function given a copy of the pinned baseline, and returns only when the check answers the key, at the bound, or on a yield. A console caret event during the poll makes no read of its own; the poll's next read is its check. A newer key's request ends the old watch, whose request is answered through its stream responder's final answer as the watch's end answers it today, and the old poll is not posted again.

#### Identity and the release rule

The object store is `crates/verbatim-outpost/src/outpost/store` (planned): a monitor that owns, as private fields, the UIA and MSAA node registries (`NodeIdRegistry` of `verbatim-uia` and `verbatim-ia2`), the window-handle map, and the release records. No other module holds a registry or a clone of it; its operations are the store's. The walks in `verbatim-uia` (`client.rs`, `shape/shapes.rs`) and `verbatim-ia2`, which mint today through `id_for_element`, take an interning trait those crates define and the store's handle implements, since those crates cannot depend on `verbatim-outpost`. The registries' `retain` with a closure is called only by the store's own code.

- Identity is decided where it is today, in the walk on the worker: the program carries copies of the outpost's own reported line (ids, window handles, MSAA identity keys) and the stop windows; as the walk reads each object it looks it up or mints it through the store, so the id exists when the object is read, and it stops at an entry of the reported line, judging MSAA equality with the live reads it makes. Every mint is tagged with its worker generation and request.
- The walk's progress is the slot it is today (`FocusProgress` in `crates/verbatim-outpost/src/outpost/bound.rs`), written by the worker between calls and read by the reducer at the bound under a lock held for one copy. At the bound the reducer copies it, describes the plain windows from the outermost element's nearest window through its local reads, minting them in the store by handle, publishes, and marks the request claimed.
- Not in the refactor (Dickson, 2026-10-30): one node per window moved to the focus pipeline rework's third build, and this item is its draft. In one chain at most one entry stands for each window handle. The store's handle map gives a window-standing entry (a UIA element hosting the window, an MSAA window or client object with `top_level_client`, a plain window) the node already minted for that handle. The node keeps the object of the backend the window's verdict reads it by; a plain window's node, which holds no object, takes that object when a walk first reaches it, keeping its id; an object of the other backend for the same handle is not kept. Before coding, the build reads NVDA's MSAA parent step (`nvda/source/NVDAObjects/IAccessible/__init__.py` 1229 to 1261) and stops if NVDA speaks two entries for one handle. Past defect: defect A in `explorer_folder_window`, one window entered twice and spoken twice.
- The release rule: a snapshot is sent only as a `Recorded` value, a type whose constructor is private to the store's module, handed out by the store; the outbound queue records each carried node at the message's position when it numbers the message at write. A node carried by a message is freed when Core's `NodesHeld` acknowledges a position at or after the last message that carried it and does not hold it. A node never carried is freed only when its mint tag names a dead worker generation or a claimed or ended request, so a walk still running keeps its nodes, as `take_releasable` in `crates/verbatim-outpost/src/outpost/outbound.rs` keeps never-reported nodes today. Every node a message carries, spliced and looked-up entries included, is recorded as carried, as the one judge build records it. Release is an effect naming ids, executed by the worker in charge outside the store's lock.

#### Text, terminals and the caret watch

Terminals comply already (Core judges every read; `terminal::reading` is pure; a read works on a copy since 2bd69a85) and join the reducer unchanged. The text records (`uia_anchors`, `edit_anchors`, `patterns`, `text_support`), the caret watch and the evidence judgement become reducer state, their provider ranges held in the store under keys the reducer keeps; a text call works on clones, and its result's transition records what changed. This ends the anchors' locks held across calls and 5797c075's workaround.

### Known traps

- Review 3, finding 6: the outpost's hand-over was changed in three builds; it is placed once, here.
- Review 1, finding 6: identity is decided mid-walk, and the bound publishes nodes a stuck worker read; the draft keeps identity in the walk through the store.
- Review 1, finding 7: the console poll as one program would block other work, and as reducer ticks would triple its wake-ups; the draft's poll yields.
- Review 1, finding 9: a `CallToken` borrow rule needs wrappers on every call; the draft instead gives the store no guard or closure in its API.
- Review 1, nit: "one node per window handle" was undefined and sat in the wrong build.
- Review 2, finding 3: freeing never-recorded nodes would free a running walk's nodes; the draft tags mints by generation and request.
- Review 2, finding 4: the store's crate and module, the registries' direct minting and `retain` closure, and which object a handle-keyed node keeps; answered in the draft, to be checked against `verbatim-uia`'s `client.rs` and `shape/shapes.rs`.
- Review 2, nits: positions are numbered at write, so `Recorded` is marked then; a release runs under a deadline; the poll's high-resolution wait, releases not making it yield, and the old key's answer; local reads through an injected query trait.

## The output build

Runs beside the reducer build; it shares no code with it.

- The settings store. `ConfigStore` (`crates/verbatim-config/src/lib.rs`, an `Arc<Mutex>`) has three writers, each writing the whole file under the lock: the GUI thread (the speech commit, the theme's `persist`), the synth thread (`persist_fn`), and the reducer (`save_reader_settings`).
  - Sequence: the GUI holds the lock while a slow disk or an antivirus scan delays the write; the user presses a toggle key; the reducer waits on the lock, and every key's speech waits behind a file write, against `docs/design/communication.md` section 6.6. M4.3 adds a writer per live setting.
  - Change: one saver thread off the path owns the file; each section has one writer (the settings host for speech, the reducer for reader settings, the theme's owner for the theme), which hands the saver its changed section. The saver's constructor returns one non-`Clone` handle per section, moved to each owner at start.
- The mirror. When a driver refuses a value, `SynthCommand::SetSetting` only logs it, and the dialog's mirror keeps the refused value: the dialog shows it, `commit` saves it, and the next start skips it. Change: update the mirror from the synth thread's answer, as `HostedSynth` updates its copy.
- Real-time audio. The audio thread, at MMCSS priority, holds the mixer lock across WASAPI calls, which the synth thread, the queue thread, the reducer's `play_earcon` and the GUI's `play_sound` also take. Its listeners take the latency ledger's mutex, and it allocates `notes` every pass.
  - Sequences: a normal-priority holder of the mixer lock is pre-empted and the audio thread misses its period; a WASAPI call blocks during a device change while the lock is held, and the reducer's earcon waits, stalling all speech; a control-plane reader of the ledger holds its mutex while the audio thread waits. M6.3 adds awake silence and the earcon re-mix. Passing runs log 1 to 10 "the device ran dry" warnings, not traced to a cause yet.
  - Change: the audio thread owns the mixer's state with no lock. Other threads send requests through bounded single-producer queues that never block it. Ledger records leave through a ring it never waits on, and every per-pass buffer is allocated once.
  - Tests: a counting allocator asserts no allocation in the mix loop after warm-up; the ownership check reads the loop's module.
- The synthesizer's display name. `DriverState` (`crates/verbatim-speech/src/manager.rs`) takes the host's `driver.display_name()` after a switch, and the host never loads the user's locale. Sequence: in M8 a second locale ships, the list shows Core's localized names, and after a switch the active name reads in English. Change: take the name from Core's registry only.

Tests added: a reducer toggle not waiting on a GUI commit's file write; a refused value neither shown nor saved; the allocator test; the display name after a switch equal to Core's registry entry.

Scenarios: `menu_and_settings_dialog`, `settings_dialog_keys`, `switch_to_onecore`, `synth_host_crash_recovery`, `terminal_settings_page`, `theme_panel`, `measure_speech_real`, counting the "device ran dry" warnings against the build before.

### Known traps

- Review 3, nit: the WAV recorder's tap copies and sends its samples on every pass (`crates/verbatim-audio/src/wav.rs`); it joins the audio change.
- Review 2, length section: these parts were moved out of the design whole; each sequence is still to be checked against the code.

## The other parts, as drafted

Each change is specified, with its sequence, in the briefs; here only what changes.

- Input and the hook: `KeySeq` on every input and on `Speak`, `KEY_ORIGINS` deleted; gestures into the reducer's port in key order, the router's work as reducer effects (its time read, GUI commands, and the lock key's 30 ms timer, with `behind_toggles`' ordering of marks and idle waiters moved into the reducer); `SpeechManager`'s speaking functions (`speak`, `speak_for_key`, `play_earcon`, `play_earcon_to_end`) take a `SpeakToken` returned only with the manager and moved to the reducer thread (6e303a61). Kept: the decision machine, the three held-key records, the views, the gesture snapshot.
- The supervisor and app: `Record`, `Live` and the hand-over as typestates; wanted requests (`focus_now_wanted`, `shell_items_wanted`) as responders (886d8e73); `OutpostMessage`'s lifecycle variants sealed by a type whose constructor is private to the owner module, with a test constructor behind a feature. Kept: `HeldFacts`, the idle round, process identity, `provider_process`, ignored processes, the focus-now queries, the backoff.
- Messaging and the control tools: the kinds, enums, responders (with `ShellItemsHost::request`, 76180512), `Merge`, configuration and placement above. Kept: the transport, framing, overflow policies, the relay, pings, the control plane's queries.
- Speech: kept (lanes, `FocusNow`, D17, pause). The synthesizer's display name comes from Core's registry, not the host, which never loads the user's locale.
- Audio: the real-time rule, fixed (the mixer's state owned by the audio thread with no lock, requests through queues that never block it, ledger records through a ring, buffers allocated once).
- The synthesizer host: kept; M6 builds its owed state as a typestate.
- The GUI and settings: one saver thread owns `settings.toml`, one writer per section through non-`Clone` section handles; the dialog's mirror is updated from the synthesizer's answer. Kept: `GuiThread`, the hidden frame mark, the Terminal page's deltas.

## The documents commit

- D14 amended.
- `docs/architecture.md` section 1: the facts and their owners, and the threads that call into applications.
- Section 2's "Where the rules run" replaced.
- `docs/design/communication.md`: the six kinds and the configuration path.
- `docs/glossary.md`: the outpost's reducer, call program, responder, standing parameter, object store.
- `docs/design/focus-pipeline-rework.md` and `docs/design/caret-selection-rework.md` amended.
- The roadmap: M4.4 closed, and each milestone's rules below.

## Rules for the planned work

- The caret and selection rework.
  - The last read per node and the pinned baseline are reducer state, released by the release rule.
  - Caret and text events are read for every node the outpost has not released, not only its newest focus, and Core decides which is spoken.
  - A repeat is read in full and compared with the last read, and Core's choice between a repeat and a new focus decides what is spoken, which removes the Control+A loss.
  - The key request is a stream responder; the 100 ms and 1-second bounds are reducer timers.
  - The auto-repeat mark, the edge facts and the chords' gesture travel with the report.
  - The clause against merging a caret report across a reply becomes the barrier on replies.
- M4.3.
  - Each setting has one owner and reaches it as a delta, and every setting writes through the saver.
  - The hook's settings and gesture map are one snapshot, loaded once per key.
  - Cancelling expired focus speech is Core leaving out `DropExpiredSpeech`.
- M4.4: built in the messages build.
- M5.
  - The loader owns the rule tables, configuration with a generation.
  - A listener veto names its generation, and the outpost's rules apply to recovery reads under its current generation.
  - A module's `focus()` is the outpost's newest report of a control, its own fact; a module that reacts to changes is given every change of a node the outpost has not released, and Core decides what is spoken.
  - A hook runs on a worker in a call program, and its answer is a reducer input.
  - The wasmtime store is owned by the worker generation in charge, and abandonment while it is lent is the module's fault: the store is discarded and the module instantiated again.
  - Load, swap and `on-command` are queries with responders.
  - A module's speech is a report judged against attention.
  - Module commands enter by the one input route.
  - Sleep mode is a view keyed by the foreground window.
  - Application identity is one function of the held process.
  - The application rules move into modules once.
- M6.
  - One owner per host, with users holding leases.
  - A non-active synthesizer's voices are a query.
  - Adapter callbacks only write frames.
  - The host's owed state is a typestate.
- M6.1: MSAA and IA2 only, since UIA cannot be cancelled.
  - The worker calls `CoEnableCallCancellation` at its start.
  - The worker publishes its current call ticket under a ticket lock as it enters and leaves each call.
  - The canceller holds that lock across `CoGetCancelObject` and `ICancelMethodCalls::Cancel`, and does nothing for a ticket no longer current.
- M6.2: starting an outpost generation takes proof that the previous one exited; a marker's outcome names its generation.
- M6.3.
  - The queue thread owns each utterance's ending, and the mixer only each attempt's.
  - `Source::register` consumes an admission token (3ebb7be0).
  - The awake period is configuration, under the real-time rule.
- M6.4: replies name the start they read from; an owed read has a timer input.
- M6.5: the classic edit control's drop of a caret event equal to the last read is a skip on the reader's own record, which is allowed.
- M7.
  - The helper talks only to its outpost.
  - A buffer in the helper sends numbered one-way updates.
  - Calls to the helper can be cancelled with `CancelIoEx`.
  - The helper's UI-thread work is thread-affine.
  - The store keys IA2 nodes by window and unique id.
- M8.
  - Core decides the active profile.
  - A switch is one transition whose effects carry its generation, with the theme and synthesizer settings ordered before the `Speak` the trigger caused.
  - Triggers are suspended while the settings dialog is focused.
  - Overlays are written only by reducer effects.
  - An OCR read is a query.
  - Words stay in Core.
- M8.1: two instances share no writable fact; the integrity of the window in front stays the listener's report.
- M9: logs are one-way and sent in batches; the Verbatim+F1 report is a query.
- M10: the console reads Core only by queries; its node reads are requests with responders.
- M11: a theme switch is an input and an effect ordered with `Speak`.
- M12: remote input enters the one route; the protocol is versioned and validated.
- M13: Java objects are released by the release rule; establish first whether the bridge is thread-affine.
- M14: drawing threads only append; one helper thread owns the model, sending numbered snapshots that Core judges.
- M15.
  - Core alone decides content and tethering.
  - Braille keys enter the one route with the buffer generation and a key number.
  - Speech and braille stay in step through the existing marks.

## Evidence

- Facts with more than one owner, and their defects: `architecture-study.md`, section 1.
- The back-channel's chain: 43de3932, 8fe59b1b, d893e07f, c0bbf19b; and `unbuilt-focus-designs/core-flow-review.md`, findings 1, 3 and 5.
- The foreground: b1917eed; audit difference 12; 06594d66 and 9661f0d8; 5f1c8e81, 3578a437, e60a7a58, 38cab859.
- The gap's moves: 01cba97e, ad78f2a8, 9ba7625b, 5242658c, 28be3d08, 22256896, f2f8209a, e02c7854.
- The publication's rounds: c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7.
- The listener's scheduler: 5204f351, e46d4183, 0b5826cd, 073d63b0, 69c06239.
- Verdicts: 094a8dc8, 07db29a8, a3d6bc23, 248f85d3, defect B.
- Identity: b94d2ea2, 9fd28a66, 8826d457, 5797c075, and defect A from 3abda293 with 10c9e83a.
- Lifecycle: 1a3dc022, 8d01bbd1, a39f01f8, de7f481e, df4c15d9, ded1988a.
- Requests: 886d8e73, 76180512, 348262b8.
- Merges: dd363ecc, 913bb0e8, 89bbbc7c, 71322fae, 06256225.
- Lent state: d0ec3a89, the anchors' follow-up, 2bd69a85's terminal copy.
- Input order: bf4fbedb, ba7064e5, e180f14a, 04214e7f, 49fcca0e.
- Output: 3ebb7be0, 21fb0e51, 6e303a61, 0d791a13, 2bdfffb7.
