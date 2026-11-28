# Pre-mortem of the rewrite's design

Status: Claude, 2026-11-28, on Dickson's question of the same day: does the design structurally encourage other kinds of bugs? It reads `design.md` and `extensions-design.md` in this folder at commit 970fed5f, `patterns.md`, CLAUDE.md's "Design principles", `docs/flaky-tests.md` and the git history. It takes in the adversarial review `review-premortem-fable.md`, cited as "[review M1]" and so on. Nothing in the repository changes because of it. Line numbers are `design.md`'s unless a file is named.

Terms used here:

- Pre-mortem: a review that assumes the rewrite has shipped and failed, and asks which bugs it most likely failed by.
- Bug class: a family of defects that one mechanism produces, so that fixing instances leaves the class open.
- Common-mode failure: two implementations meant to check each other fail in the same way on the same input, so the check passes.
- Obligation token: a `#[must_use]` value, neither `Clone` nor `Copy`, that one transition returns and only a named second transition consumes, so a two-step change cannot stop after its first step.
- Blessing: regenerating a stored expectation from the current output, which records whatever the output now is, bug included.
- Interleaving: the order in which two independent inputs reach Core.
- Tie check: a check that holds one side of the replay to Windows itself, not to the other side.

Every likelihood and severity below is an estimate, Claude's judgement, not a measurement. A source marked "not fully read" was verified only through its abstract or a summary.

## Assessment

- The design closes the audit's classes (copies, flag lifecycles, owner state outside its owner, per-feature messages, test literals) mostly by construction, and the history shows those classes caused most of today's defects. That part holds.
- It moves several classes from compile time and review to the simulator, where only a seed or a world that reaches them finds them: a value derived from Core's facts carried across a wait, which the design itself admits (line 309); completeness kept by hand (a `NamesIds` implementation, a ticket replaced by assignment); protocol shape (generic batches, the element struct's "not asked"); and nested access to Core through the scoped thread-local, which becomes a run-time borrow panic.
- The largest risk is in the verification, but not where the first draft put it. The world server's semantics are tied to Windows by the design's two tie checks (lines 816 and 817) [review M5]. The unlisted common-mode sources are the recorder, which is Verbatim's own readers and platform layer, so a misreading is recorded into the world and served to both screen readers; and generated worlds, which no live capture judges.
- The design holds the ruled differences from NVDA three times: as Verbatim's whole expected output in each expectation file, beside NVDA's output, and in the allowlist [review M6]. That breaks "one owner per fact" inside the verification, and its blessing route is a pending difference, not the file's form.
- Records owning requests carry today's ruled behaviour over rather than adding a new risk [review S2]; the interleaving they bring is today's and still deserves a simulator mode.
- Eleven further classes, mostly at the edges (extensions, the trace, the corpus's content, trust filters), are listed in "Classes at the edges".
- Most mitigations are mechanical and cheap; where one adds a cost or a state, it says so.

## Async entries in Core

### Stale derived values across a wait

- How it arises: an entry runs `let committed = cx.read(|c| c.versions.is_committed(node))`, awaits a read, and then speaks on `committed`. A `bool`, a `Pid` or a `String` is `Send`, so the compiler passes it (line 309). The worked example "speak the focus's new name" reads such a `bool` before any wait; the next feature that adds a wait after it inherits the hazard silently.
- Evidence here: none direct, since today's Core is not async beyond the focus slice [review S4]. The nearest is 5ce4f65f, where the navigator's snapshot of the focus said a slider's value twice; that was a copy in a record, which the design closes, so it shows the cost of the class, not its frequency.
- Published experience: Tokio's `select!` documentation says that "when an asynchronous method is cancelled, that always happens at an `.await`" ([Tokio, `select!`](https://docs.rs/tokio/latest/tokio/macro.select.html)), so each await is a point where what was true before it may not be.
- Likely and bad: medium (no history yet), medium (wrong or doubled speech, intermittent).
- Mitigation, a narrowing, not a closure [review M2]: every `cx.read` result is a `Fresh<T>` that is `!Send`, so an unwrapped read cannot be held across a wait without an explicit `keep()`, which makes the crossing visible in review and greppable. Under `cfg(feature = "sim")` only, `keep()` records the accessor versions, and the simulator flags a kept value used after a change. This narrows the class to values someone unwrapped: a `bool` taken out of a `Fresh` with `*fresh` before the wait is plain and crosses as before, so async rule 4 stays the main guard. Async rule 8's stamps are values made at arrival, not reads, and need no `Fresh`. No version counter enters the product, and no `Stale` branch can change speech.

### Two-step changes that stop after the first step

- How it arises: an entry passes a held key's stroke (a `Keys` transition), awaits, and is to tell the terminal's reading state; it returns early on `Ended` through `?` in between. Each record is valid alone, which sealing guarantees; the broken fact is the relation between two records [review M3].
- Evidence here: none cited; the first draft's commits did not show the class [review, "Evidence"].
- Published experience: Tokio lists `write_all` among the methods that are not cancellation safe ([Tokio, `select!`](https://docs.rs/tokio/latest/tokio/macro.select.html)); a two-part write stopped between its parts is the same shape.
- Likely and bad: medium, medium.
- Mitigation, a type change: the first transition returns an obligation token that only the second consumes. The token is stored in the entry's ids record in `Queue`, beside the entry, so the entry's end and `ended_waiting` must discharge it, and a token left at an entry's end fails the simulator's run. This is the design's own ticket pattern applied to a two-step change; it adds one field kind to the ids record.

### Nested access to Core through the thread-local

- How it arises: `cx.apply` gets `&mut Core` from a scoped thread-local that holds a `Cell` (line 876). A helper called inside an `apply` closure that itself calls `cx.read` compiles and fails at run time as a borrow panic. Core panicking ends all speech.
- Evidence here: today's driver uses `shared.borrow_mut()` (`inbox.rs` 601), the same run-time borrow.
- Likely and bad: medium, high.
- Mitigation, a signature: `Cx::read` and `Cx::apply` take `&mut self`, and the closure receives `&Core` or `&mut Core` as its argument and never sees `cx`, so a nested call is a borrow-checker error. No cost.

### Checkpoints by replay

- How it arises: a dump's window starts at the oldest unfinished entry's checkpoint (line 322). Under a flood with no quiet moment, or a holding extension hook with no time bound (`extensions-design.md` line 388), that checkpoint can be older than the ring holds, so the dump grows past its bound or cannot replay the paused entry [review, missing class 7].
- Evidence here: the roadmap's "Exact replay of a paused entry" (Dickson, 2026-11-19): such dumps "report a mismatch where none happened live", and rebuilding a paused entry was judged too complex.
- Likely and bad: medium, medium (lost diagnosis rather than wrong speech).
- Mitigation, a simulator invariant: kept dumps already become permanent replay tests (line 865) and step 3 proves replay from a checkpoint (line 1136) [review, "Evidence"]. What neither checks is an arbitrary dump taken mid-run: the simulator takes one at random steps and requires its replay's decision log to equal the run's from the dump's start. The age of the oldest unfinished entry is a reported figure, so an unbounded hook shows.

## Records owning requests beside the front entry

### Interleaving of a record's answer with the front's wait

- How it arises: a review press is the front entry and waits on its read; a terminal output read, owned by the terminal's record, is answered in `step` and changes the model the press read before its wait. These reads are outside the queue today, by ruling (line 355), so the rewrite carries the interleaving over rather than adding it [review S2]. Each owner added to the closed list adds a source of changes to facts front entries read; the extensions design adds the instance record.
- Evidence here: `docs/flaky-tests.md`, "A key pressed as a terminal flood ends", one failure, whose fix is a planned step on how a key takes ownership of reads; 671aa993, the follow-caret ruling between the same two owners.
- Published experience: Torres Lopez and others class this as a message protocol violation, "bad message interleaving", a message processed between two meant to be consecutive ([Torres Lopez et al., 2018](https://arxiv.org/abs/1706.07372)). Tu and others report that message passing caused 58% of the blocking bugs they studied in six large Go systems ([Tu et al., ASPLOS 2019](https://songlh.github.io/paper/go-study.pdf), not fully read; figure from [the summary](https://blog.acolyer.org/2019/05/17/understanding-real-world-concurrency-bugs-in-go/)).
- Likely and bad: medium, medium.
- Mitigation, a simulator mode: at each front wait, every record-owned answer and arrival that could land during it is delivered both before and after the front's answer, within the seed budget. For Verbatim-only behaviour, such as the terminal's row memory, NVDA has no answer, so the reference model (line 787) must say which order is right.

### A ticket replaced by assignment

- How it arises: `self.reading = Reading::Asked { ticket: new, .. }` drops the old ticket. The design makes dropping a ticket a legitimate ending (lines 48, 321 and 367), so the old request is cancelled, but the record's transition returns no `Cancel` effect for it unless written to, and the peer runs it on.
- Evidence here: e0092292 ("`Settles::dispatched` overwrote the settle in flight without letting go of its waiter"); CLAUDE.md, "the settle waiters were lost twice that way".
- Likely and bad: medium, medium.
- Mitigation, a type [review M1]: the ticket field lives in a wrapper with no public setter, reachable only through `replace(new) -> Cancel`, so replacement by assignment does not compile and the cancel is a returned effect the transition must pass on. Dropping the wrapper with its owner stays the ending, as the design gives, so no owner walks its tickets by hand. The same wrapper fits any arena handle and `Transit`.

### Head-of-line blocking on first-in, first-out pipes

- How it arises: a tray walk or terminal baseline read, owned by a record, is ahead of a key's read on the same outpost pipe and worker, and the key's read may reach its due for a cause not its own.
- Evidence here: the design keeps a request class on `Ask` in reserve for exactly this; the extensions design adds one extension read per instance ahead of Core's.
- Likely and bad: medium, low to medium.
- Mitigation, a report from the simulator: each request's decision-log line carries its wait behind other requests on its pipe; the simulator reports the worst case per world, and the desktop figure is reported, not ratcheted, since a shared desktop's timing flakes [review S3].

## Held views, `Named` ids and the `NamesIds` check

### Position accounting

- How it arises: every release is decided by `handled` and by which messages count toward a reader's position (lines 139 to 142). Coalesced items, items held for credit, refused messages, `Holding` reports and items under `Cancel(Late)` each need a rule saying whether they advance the position and whether an id in them is "sent". A wrong rule releases an id Core has not read, or keeps one forever [review M4, missing class 4].
- Evidence here: 7c507787's second paragraph: "a tell naming an element by its token counts toward the view's positions", with "its rule of when a view is too early to judge", the rule that today's numbering needed.
- Published experience: Moreau, Dickman and Jones found that Birrell's reference listing, the collector behind Java RMI, had correctness cases its description did not account for ([TOPLAS 2005](https://eprints.soton.ac.uk/259448/), not fully read: abstract only); held views are a reference listing.
- Likely and bad: high, medium.
- Mitigation, a table and a generated test: the design states, per message kind in both directions, whether it counts toward a position and whether its ids count as sent, in one table in the model crate that both ends read; the simulator's fault profile coalesces, holds, refuses and cancels late in every combination and checks the quiet-world equality.

### A record that names its ids incompletely

- How it arises: the check fails the build when a ledger type holds a `Named<Id>` without implementing `NamesIds` (line 160). It cannot see an implementation that forgets one field or one enum variant.
- Evidence here: none in the history; the first draft's three commits show other classes [review M4]. 48431c2f reverted a change that left told-only elements out of the listener's duplicate-check read, so the check found none and a focus was said twice: a reader judging what Core has read, a class CLAUDE.md already names. 7c507787 was retention by count and time, which held views replace. 17adee32 named an id of the wrong kind.
- Likely and bad: low to medium, medium.
- Mitigation, mechanical and cheap: `#[derive(NamesIds)]` only, with hand-written implementations refused, and the constructor of a raw id private to each reader's intake, so an id cannot live outside a `Named` or an `IdRef`. For 17adee32's class, ids of each kind (node, anchor, token) are distinct types, so a flat offset cannot be named as an anchor.

### Use of an `IdRef` after `let_go`

- How it arises: `IdRef` is a place in the entry's ids record; if a copy outlives `let_go`, a reused place names another id.
- Mitigation, a type [review S3]: `IdRef` is neither `Copy` nor `Clone`, and `let_go` consumes it, so no stale one exists and no `Stale` answer is needed.

### Leaks that move from the reader into Core's records

- How it arises: the quiet-world invariant checks that a reader holds what the last view names; it does not ask whether Core should still name it. A navigator on a long-gone element, or an instance's `held` that only grows, keeps elements and their COM objects alive in the application, and every check passes.
- Evidence here: `patterns.md`'s `BackendCache.probed`, never pruned when a destroy is lost.
- Likely and bad: medium, low to medium.
- Mitigation, a simulator check: each record type declares a bound on the ids it may name, checked at every step, and a long generated world asserts every reader's element count stays under a fixed figure.

### Lost elements blur the strict invariant

- The invariant "no `NotThere` for an id Core's records name" carries an exception for elements lost with an abandoned worker (line 461), so a premature release can be explained as a loss. Mitigation: the reader logs every `NotThere` it answers for a `Lost` element, and the invariant checks the log, which adds no kind of reply. A distinct `Lost` reply is the other option and is Dickson's.

## Generic messages and standing requests

### Protocol errors moved to run time

- How it arises: with three message kinds and a shared step list, a batch a backend cannot run, or an answer that does not fit its batch, is found at run time, by a world that sends it.
- Evidence here: refusals added one at a time in the request-list builds: 5c4f5d16 ("Refuse a listener batch that reads an earlier step's node") and 015e80dc ("Refuse a batch read through the other backend before any step runs"). 7948f852 was a frame too large, a transport limit, not a misfit [review, "Evidence"].
- Likely and bad: medium, medium.
- Mitigation, kept and extended: Core's typed builder (8f2093e3) stays the one way to make a batch and returns a typed parser for its answer; a generated test runs every step against every backend and every outcome through the parse.

### Credit that is never granted

- How it arises: credit rides on Core's messages (line 90; the credit message at line 170 is one example). A follow whose items beyond its credit wait in the reader's coalescing while Core has nothing to send that reader waits until some unrelated message: Core waits for items and the reader for credit. Focus and foreground tells are exempt; terminal follows are not.
- Published experience: Torres Lopez and others' "behavioural deadlock": actors not blocked, each waiting for a message that is never sent ([Torres Lopez et al., 2018](https://arxiv.org/abs/1706.07372)).
- Likely and bad: medium under floods, high (silence).
- Mitigation, with its cost [review S3]: Core grants credit when half a reader's credit is used and it has no other message for it, so a grant costs at most one write and one reader wake per half-credit of items, each carrying a held view; the simulator's liveness invariant includes "no item held for credit while Core sends nothing".

### `Cancel(Late)` amplification

- How it arises: Core answers an item or `Final` for a number it does not hold with `Cancel(Late)` (lines 124 and 269). A flooding standing request sends many items before the cancel lands, each answered by a message with a held view; answering once per number needs a set of numbers already answered, a new table with its own growth and forgetting [review, missing class 2].
- Likely and bad: medium under floods, low to medium (wakes and writes, not speech).
- Mitigation: the reader stops sending under a number at the first `Cancel(Late)` and Core answers each late item, with no table, the cost bounded by the items in flight; the simulator's flood profile measures them.

### Orphan items

- Torres Lopez and others treat orphan messages, sent to a receiver that will never handle them, as "an observable property of an actor system" rather than a bug kind ([Torres Lopez et al., 2018](https://arxiv.org/abs/1706.07372)); the review said the term was absent, but the paper uses it [review, "Evidence", judged wrong]. The design's late rule answers each such item. Low.

## The readers as reducers and their workers

### A withdrawn or abandoned batch that still acts

- How it arises: a batch cancelled for any reason but the foreground stop runs on as `Withdrawn` (line 274), and an abandoned worker's job runs when the application recovers. If the batch holds a `Select`, a click or a `SetFocus`, the act happens after Core resolved the ticket and went on as if it had not. The withdrawn case is the commoner [review, missing class 5]. The generation stamp protects Verbatim's records, not the application.
- Published experience: Kleppmann's lease example: a client paused past its lease writes anyway when it resumes, and a fencing token works only where the store checks it ([How to do distributed locking](https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html)). An application checks no token.
- Likely and bad: medium, high when it happens (an action on the wrong item).
- Mitigation: a `Cancel` that reaches the outpost before an act step starts skips that step and every later one, as a check between steps in the executor, with no state; a job's `not_after` time covers a step queued behind a slow one. Neither covers an act already inside the application's call, which is a limit [review S3]. An act is never fused into a batch after a read that may be slow.

### Objects that must be released on one thread

- A `Transit` dropped by the root because its event was filtered, or an element whose job reached the wrong arena, is released on the wrong thread or never (CLAUDE.md: "A COM object is released only on a thread allowed to block"; d08b85c3 moved kept elements' release to the reading thread alone). Mitigation: the `replace`-style wrapper above, whose only consuming path is a `Release` job, and an executor check that an element's `Backing` names the worker the job runs on.

## The element struct, the property table, `msg!` and typed reads

### "Not asked" read as "absent"

- How it arises: each field is given, absent or not asked. A feature reading a property its batch did not ask gets "not asked", and a `match` with a `_` arm treats it as absent: a state not spoken.
- Evidence here: 9e517cad ("Take a missing cached keyboard focus as no answer, not a no"), the same confusion in today's code, as an analogy. 8ce48d3e is the 29-file change the struct removes; it added a property, it is not an instance of this class [review, "Evidence"].
- Likely and bad: high, medium.
- Mitigation, with its limit [review S3]: each feature declares the properties it reads as a generated projection type, `props!(Name, Role, ToggleState)`. Its constructor, the one place the check runs, refuses at run time a struct in which any of them is "not asked", whether the struct came from an answer or from `Versions`. That moves the check to one constructor, not to compile time. It changes the surface of Dickson's decision of 2026-11-28 in "The cost of a feature" (one struct, one constructor), so it is his to accept.

### Cost that grows with the table

- One field per property makes every element as large as the table, and table-driven reads tempt "read the whole table". Mitigation: the element's size and each kind's cache-request property count are checked figures, as the call-count ratchet is.

### Phrases extracted from code

- How it arises: `msg!` lets two modules define one key with different English, and the planned check (missing and unused keys) misses a translation whose placeholders differ.
- Published experience: Fluent's guidance warns that building messages from shared parts "obfuscates the translations" ([Fluent, Good Practices for Developers](https://github.com/projectfluent/fluent/wiki/Good-Practices-for-Developers)).
- Likely and bad: medium, low.
- Mitigation, checks: a key defined twice with different English fails the build; each translation's placeholders equal the English message's; the English file is generated by an xtask step, not by a procedural macro writing files, which incremental builds skip.

## Extensions

### The speech root as a second asker

- Known and provisional: `design.md` already has a tentative second asker for the synthesizer host (lines 1012 to 1017), and the stage's asker is an open question of the extensions design (question 4) [review S1].
- What is new: nothing gives a stage a host of its own, so an extension with an app module and a stage has one peer with two askers and two `Out` records, against "Request numbers are minted by Core only" (line 58). Answers are matched by peer first (line 124), so numbers on separate pipes do not collide; the cost is two owners of one peer's requests. `transform(ctx, utterance)` takes the context, read from Core's records, so the speech root would hold a copy of Core's facts; the breaker's counts are Core's while the stage's instance would end at the speech root; and nothing says how the speech root learns of the host's end.
- Likely and bad: certain for an extension that mixes kinds, otherwise none; medium.
- Mitigation, Dickson's: a stage runs in a host of its own whose only asker is the speech root, with a context the speech root owns (the utterance and its marks), not Core's.

### Commands cancelled by holding calls

- How it arises: a holding call that becomes due cancels the running command of the same instance (`extensions-design.md` line 179). A steady stream of matching events cancels every command of that module: Torres Lopez and others' livelock ([2018](https://arxiv.org/abs/1706.07372)).
- Likely and bad: low for the built-ins, medium for third-party modules registering `on-event` widely; the command says nothing.
- Mitigation: a command cancelled by the order rule is logged under its module and counted in the timeline; an invariant that commands complete would fail by design [review S3]. Whether to bound such cancellations is Dickson's.

### End causes declared wrongly, and re-asks on a held cause

- A stage that omits `LaterKey` holds the queue to its due; one that declares `PendingFocus` ends at once while focuses pend. The stage macro checks the name, not the meaning. Mitigation: the simulator runs each declared and undeclared cause against every stage, judged by the comparison, and the stage list records why each is declared.
- A cause that already holds ends the wait in the same step (line 383). An entry that asks again under the same stage, as the M5 loop's shape allows (line 995), spins inside one poll and freezes Core [review, missing class 1]. Mitigation: the driver refuses a second ask under one stage in one poll after a cause ended the first, as a panic in tests and an `Ended` in the product, and a stage may be re-entered only after an input.

### Cancellation exported to extension authors, and imports that outlive their call

- A module cancelled between two updates of its own state is left half-updated (`extensions-design.md` line 383): the cancel-safety class, now in third-party code [review, missing class 10]. Mitigation: the SDK documents it and offers a guard that defers `cancelled` to the end of a marked block, within the compute bound.
- A blocked import "finishes on its own thread and its result is dropped" (`extensions-design.md` line 211): a cancelled translation still sends the user's speech to the network, and with one cancel per key press the operations and pool threads pile up [review, missing class 11]. Mitigation: a bound on in-flight imports per instance, with a new import refused past it, and the network request aborted where the client allows.

### Scope and lock-screen filters applied by hand

- The lock-screen rule runs at every send and arrival over contexts, reads, events and effects, and scope fills each context field (`extensions-design.md`, section 5) [review, missing class 9]. Each new field, effect or step needs the filter, an omission is a trust-boundary defect no comparison with NVDA sees, and a read that navigates out of scope must be filtered on its answer.
- Mitigation, mechanical: the context and every answer pass through one filter function generated from the WIT types, so a new field cannot skip it; the simulator's invariant "nothing below the lock screen reaches a guest" runs over generated worlds that lock the screen at every step.

### The speech-text stage reorders speech

- The design states the limit: an untaken utterance can be spoken before an earlier taken one still being transformed. Mitigation: a decision-log line per overtaken utterance, and its count reported.

## The simulator, the recorder, the replayed Windows and the NVDA oracle

### Common-mode sources the tie checks leave

What the design already has [review M5]: the world server's semantics are tied to Windows by two checks. Live Verbatim's speech must equal replayed Verbatim's for one scenario per family (line 816), and headless NVDA on a recorded world must equal live NVDA's capture (line 817, the copy check). The first draft's two instances, the `GetRuntimeId` stand-in and the Run dialog's children, were NVDA-only stand-in defects, and the copy check found the second; they are not common-mode failures.

What the tie checks leave:

- The recorder is Verbatim's own readers reading through Verbatim's own platform layer (line 484). A misreading there, such as 9e517cad's missing property read as false, is recorded into the world and served to both screen readers. The replay-honesty check passes, since live and replayed Verbatim share the misreading; only the copy check can see it, and only for worlds with a live NVDA capture.
- Generated worlds (lines 795 to 803) have no live capture. Their application model is written once, and both screen readers can agree on behaviour no application has.
- One scenario per family is a thin tie.

Published experience: Knight and Leveson found that 27 versions written independently from one specification failed together on far more inputs than independence predicts ([Knight and Leveson, 1986](https://www.kth.se/social/files/564df871f2765419e306178d/KnightLeveson.pdf); the link is a summary, not the paper). Dropbox kept a slower suite against real servers because "there is high risk that our mock's behavior may drift from reality" ([Dropbox, Testing our new sync engine](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)). Barr and others' oracle survey names the human as the final fallback when automated oracles are inadequate ([Barr et al., TSE 2015](https://discovery-pp.ucl.ac.uk/id/eprint/1471263), not fully read: abstract only).

Likely and bad: high, high for the recorder and generated worlds; low for the server's semantics, which the tie checks cover per family.

Mitigations, none of them a presence check:

- The recorder reads through a path independent of `verbatim-uia`'s parsing: raw calls, raw variants written as they came, with Verbatim's parse applied only at replay. A misreading in the parse then cannot enter the world.
- Every generated-world rule cites the recording that shows it, as the reaction model list already must (line 542), and a rule with no recording fails the build.
- The copy check runs on every recorded world that has a live NVDA capture, and the report gives the share of corpus steps that rest on a world without one, so the gate's reach is read correctly.

The first draft's semantic-coverage check, one live test per world-server operation and stand-in method, is dropped: it pins no combination of cached, current, ignore-default, view and condition, and it would put desktop tests on the roughly 37,600 lines of the R2 class (line 568) against a 1,500-line budget for all live API tests (line 1117).

### Brittle worlds and the pressure to re-record

- How it arises: a reaction is released only when the reader makes the recorded action at the recorded time (line 541). Any change in when or whether Verbatim acts makes the rest of the world a miss, so behaviour changes invalidate worlds wholesale. Re-recording needs the desktop, held by one agent at a time, and the comparison gates `cargo xtask ci` (line 788).
- Likely and bad: high, high [review S4]: corpus churn on the critical path, and the habit of re-recording until it passes.
- Mitigation: the report counts, per change, the worlds it invalidated, and a world is re-recorded only with the reason in its commit; the measurement gate's corpus measures the churn rate before the gate is relied on.

### Expectation files, the allowlist and blessing

- How it arises: each world's expectation file stores NVDA's output and Verbatim's expected output whole (lines 965 to 969), and the allowlist stores the ruled differences again, with world, step, count and both texts (lines 769 to 786). One fact, Verbatim's departure from NVDA under a ruling, is held three times, and nothing says which is the source [review M6].
- The blessing routes: NVDA's stored output, regenerated from a harness regression, which then moves Verbatim's expectation with it; and a `status = "pending"` difference (line 774), which passes until ruled.
- Evidence here: the review of M4.6's generated tests found "models that recompute what the code computes, so a misreading made in both passes" (5cfa65e9); the memory note on generators, whose first model "modelled Verbatim's left mark as NVDA".
- Published experience: Jest's documentation asks reviewers to "fight against the habit of regenerating snapshots when test suites fail instead of examining the root causes" ([Jest, Snapshot Testing](https://jestjs.io/docs/snapshot-testing)).
- Likely and bad: high, high.
- Mitigation, the review's form [review M6]:
  - The file stores NVDA's output whole, and for Verbatim only the steps that differ, each whole and citing its ruling. A step with no override is equal to NVDA's by definition.
  - Those overrides replace the allowlist's per-world `[[difference]]` entries, so the fact is held once; the general `[[difference_rule]]` entries stay, since they span worlds.
  - A command prints Verbatim's full expected output for reading, generated and never stored.
  - NVDA's stored output changes only in a commit that changes the harness or the world and passes the copy check.
  - The count of pending overrides may only fall without Dickson's ruling, so a new pending difference is a visible request to him, not a pass.
- Dickson's three checks survive: Verbatim matches its expectation on any host, Linux included, since NVDA's output is stored; NVDA's stored output matches the oracle on Windows; and any differing step cites a ruling, which an override must. What changes in his decision of 2026-11-28 is its literal form, "stores two outputs", which becomes NVDA's output plus overrides, and the allowlist's per-world entries moving into the files. That is his to decide.

### Outcome sets and rules

- An under-explored NVDA set: where the seed budget found one outcome, Verbatim must equal it (line 758), so a point where only NVDA's silent order was found makes silence required; the guard at line 763 covers only sets with several members. The plausible-boundary definition rests on a one-time desktop measurement of NVDA's handler times (line 601), a machine-dependent constant in the oracle [review, missing class 6]. Mitigation: a one-member set found at a point the harness marks order-dependent (a thread or batch seam it enumerated) is reported with the orders tried, and silence at such a point needs a rule like any other; the handler-time measurement is stored with the machine it came from and re-taken on a runner change.
- Over-broad rules: a general rule such as "where NVDA's outcomes are silence and a text, Verbatim gives the text" matches every future world of that shape, a new defect included; the stale check fails a rule that matches nothing, never one that matches too much. Mitigation: each rule's match count over the corpus is stored and reported, and a rise lists the new matches for reading.

### Trace replay across processes

- Each process keeps its own ring and checkpoint (line 863). A message is both the sender's output and the receiver's recorded input, rings of different lengths under a flood share no common window, and a reader that hangs or is killed loses its ring in the case the trace exists for [review, missing class 7]. Mitigation: a dump takes the common window, from the latest of the processes' oldest checkpoints, and replays receivers from recorded inputs only; a reader's end flushes its ring to the supervisor, which keeps it for the dump.

### The user's content in the corpus

- The trace records applications' answers, world capture records text, window messages and the clipboard, a dump becomes a permanent replay test at once (line 865), and the real-use loop would run on Dickson's machine (line 813). Nothing scrubs passwords or documents before a file joins the repository [review, missing class 8]. Likely medium, severity high. Mitigation: a world or dump joins the corpus only through a scrub step that replaces text not produced by the scenario with generated text of the same shape, and a check refuses a committed recording from a password field or with clipboard text not typed by the scenario.

### What the simulator cannot see, and what the gate measures

- The shells' threads, their listed locks and atomics, real pipe timing and the always-on trace's cost run outside the simulator. FoundationDB runs performance tests "in addition to simulation" ([FoundationDB, Simulation and Testing](https://apple.github.io/foundationdb/testing.html)); Dropbox's post lists races between real system calls as what its serialised simulation misses. The design keeps end-to-end scenarios as a full layer; the gate's report should name these as outside it.
- Simulator-only code (the `sim` accessor log, fixtures, any test-only check) lives behind features, and `cargo xtask ci` builds the whole workspace into `target/debug`, the output the end-to-end suite uses, where cargo unifies a member's features across one build. If a product crate's `sim` feature is switched on by a test crate in the same build, the end-to-end suite runs simulator code [review S3]. Not verified for the `verbatim-next-*` crates, which do not exist yet. Mitigation: a check that the product binaries' feature set contains no `sim` or `fixtures` feature.
- Mutants written from bug descriptions measure the verifier on defects a mutant can express. Just and others found a coupling between mutants and real faults for 73% of real faults, with 17% coupled to none ([Just et al., FSE 2014](https://homes.cs.washington.edu/~mernst/pubs/mutation-effectiveness-fse2014-abstract.html)). The gate's report should class each historical bug that could not be expressed as a mutant.

## Performance

- Held views per message: each carries every id Core names for that reader, computed from the records; with browse documents, many outposts, instances' `held` sets and the credit grants above, the view and its computation grow on the flood path. Mitigation: a ratchet on view size and compute time per message at the worst case, taken in the simulator.
- The element struct and cache requests: as above.
- Wasm instantiation, unmeasured; the ledger check the extensions design proposes is enforced before the first built-in with a hook.
- End causes evaluated once per step while a stage waits, and the callback's extra hop: on the event path, with measurements the design names.

## Interactions between mechanisms

- Records owning requests, end causes and async rule 4: a record's answer can make an end cause true, change a fact the front read and push an entry in one step. Each is checked alone; the interleaving mode checks them together.
- A record's rule keyed to another feature's stage: a record-owned answer judges from the front view's kind and stage (line 368), and stages are strings (line 302), so renaming or adding a stage in the review press silently changes the terminal record's judgement [review, missing class 3]. Mitigation: the front view exposes stages as an enum generated from the stage lists, and a record matches it exhaustively, so a new stage fails to compile in each record that judges stages.
- A foreground stop for a record's stuck request (line 364) abandons the worker, losing the focus line's elements on it, so a background terminal read can make the next focus repeat the window's title. The simulator should combine abandonment during a record request with a focus change in one profile, and the reused-runtime-id fault with the join by runtime id.
- Held views and extensions: an instance's `held` comes from the host's report, Core's view from its records, and an abandoned lane loses its handles; the derive and record bounds cover Core's side, the extensions design's quiet-world invariant the host's.
- Generic batches and typed reads: an extension's `not-asked` context field makes the SDK read it live, two crossings per call; a ratchet on reads per hook call would make that visible.
- Guest determinism across architectures: a trace from ARM64 replayed on the Linux job needs Wasmtime's NaN canonicalisation switched on in both.

## Where the design makes a class harder to see

- From compile time to run time: generic batches, the three-state fields, the thread-local's borrow.
- From every run to some seeds: derived values across waits, interleavings of record answers, position accounting.
- From review to data: allowlist rules, outcome rules, expectation files and pending differences.
- From the reader to Core: leaks of named ids.
- From the product to the corpus: a recorder's misreading, which becomes a fact of the world.

## What the mitigations cost

- The obligation token adds one field kind to the ids record; the `replace` wrapper, `IdRef` moves and `&mut self` on `Cx` add none.
- `Fresh<T>` adds a wrapper type and, in the simulator only, the version log the design already has.
- Credit grants at half credit add at most one message per half-credit of items.
- The independent recorder path adds a raw writer beside the readers' reads, measured with world capture's cost (line 489).
- The scrub step and the trace's common window add tooling, not product state.
- The stage enum is generated from the existing stage lists.

## Recommended changes, ranked

1. Expectation files hold NVDA's output whole plus Verbatim's differing steps as overrides citing rulings, replacing the allowlist's per-world entries; Verbatim's full view printed on demand; NVDA's side changed only with the harness or world and a passing copy check; pending overrides only falling. Dickson's.
2. The recorder writes raw calls and raw variants through a path independent of Verbatim's parsing; every generated-world rule cites its recording; the report gives the share of steps with no live NVDA capture.
3. Position accounting stated once per message kind in a table both ends read, with a generated fault test over coalescing, credit holds, refusals and late cancels.
4. The ticket's `replace(new) -> Cancel` wrapper, consuming `let_go`, and `&mut self` on `Cx`.
5. Obligation tokens for two-step changes across records.
6. The interleaving mode: record answers and arrivals on both sides of each front wait, with the reference model judging Verbatim-only behaviour.
7. A scrub step before any recording joins the corpus.
8. Credit granted at half credit when Core has nothing else to send, with its liveness invariant.
9. The driver refusing a re-ask under a stage whose cause already ended it in the same poll; stages as a generated enum in the front view.
10. One filter function, generated from the WIT types, for scope and the lock screen.
11. `#[derive(NamesIds)]` only, private raw-id constructors, and a type per id kind.
12. `Fresh<T>` as a narrowing, with the version check in the simulator only.
13. A skipped act after a `Cancel` between steps, and `not_after` on jobs.
14. The mid-run dump round trip in the simulator, and the trace's common window.
15. Ratchets and reports: held-view size and time, element size, cache-request properties, outcome-rule match counts, overtaken utterances, invalidated worlds; a check that no `sim` feature reaches the product binaries.

## Decisions for Dickson

- The expectation file's form: NVDA's output plus overrides that replace the allowlist's per-world entries (recommended), keeping his three checks and changing only the literal "stores two outputs"; or his form as decided, with the rule on NVDA's side and the falling pending count added.
- The speech-text stage's asker: a stage host owned by the speech root alone, with a context the speech root owns (recommended), or through Core.
- The property projection: accepting `props!` per feature as a change to the surface of his "cost of a feature" decision.
- Commands of a module whose holding hooks keep cancelling them: accept and log (recommended for now), or bound the cancellations, which adds state to the instance record.
- Lost elements: the reader's log checked by the invariant (recommended, no new reply), or a distinct `Lost` reply.
