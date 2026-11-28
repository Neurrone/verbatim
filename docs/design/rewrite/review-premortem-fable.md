# Adversarial review of the rewrite's pre-mortem

Status: review, Claude, 2026-11-28, of `premortem.md` in this folder against `design.md` and `extensions-design.md` at commit 970fed5f. Nothing was built or run. Findings are numbered M (must fix before the pre-mortem's recommendations are acted on) and S (should fix). Line numbers are `design.md`'s unless a file is named.

## Assessment

- The pre-mortem's main answer holds: the design moves several classes from compile time to simulator runs, and the verification can agree with itself while being wrong.
- Four of its recommended mitigations do not do what it says, and two of them work against the design's own rules (M1 to M4).
- Its largest risk is aimed at the wrong component: the design already ties the world server to Windows by two checks the pre-mortem never mentions, while the recorder, which shares Verbatim's own reading code, is the unlisted common-mode source (M5).
- Its expectation-file contradiction is real but its fix does not stop blessing, and it missed that the design already stores the same differences a second time in the allowlist (M6).
- Eleven bug classes are missing (the section "Missing bug classes").
- Six evidence claims do not hold as written; none changes a conclusion alone, but three of the cited commits do not show the class they are cited for.

## Must fix

### M1. The drop bomb on `Ticket` contradicts the design's ending by drop

- The pre-mortem (line 76, recommendation 5) makes a `Ticket` panic on drop unless `resolve` or `cancel` consumed it.
- The design uses a drop as a legitimate ending in three places: "dropping the ticket cancels the request" (line 48), "Dropping the ticket answers `Ended`" (line 321), and "A record dropped with its owner, such as an `Application` at its process's exit, drops its tickets" (line 367). CLAUDE.md's principle is "best a token whose drop answers it".
- With the bomb, an owner's end must walk every nested record and consume each ticket by hand. That is the hand-written clean-up function the design says has no counterpart (lines 188 and 897).
- A panic in `Drop` while a test is already unwinding aborts the process, so the bomb hides the first failure unless it checks `std::thread::panicking()`.
- A version that fits: the bomb applies only to a replacement by assignment, which is the pre-mortem's actual case. The ticket field is reachable only through a `replace(new) -> Cancel` method on a wrapper type with no public setter, so assignment does not compile, and the owner's drop stays the ending. That is mechanical and needs no bomb.

### M2. `Fresh<T>` with `keep()` does not move the class to compile time, and adds a run-time outcome that can change speech

- The pre-mortem (line 37, recommendation 4) claims crossing a wait "becomes explicit, greppable and checked on every run".
- A `Fresh<bool>` must give its `bool` out to be used in an `if`. Once out, the plain value is `Send` and crosses the wait as before. The type narrows the class to values someone unwrapped; it does not close it. The design's own statement (line 309) still stands.
- `kept.get(cx)` returning `Stale` "in every build" moves the per-accessor version counters from the simulator (line 311, under `cfg(feature = "sim")`) into the product's records. That is new long-lived state on every transition, of the stamp kind CLAUDE.md's one-state-value rule argues against.
- The counters are per accessor, not per argument (line 311). A kept answer about one node goes `Stale` when any other node's transition bumps the counter. Each caller then needs a branch for `Stale`, and a branch that drops or repeats speech on a spurious `Stale` is a timing-dependent speech change in the product, which is the class the mitigation was meant to remove.
- Async rule 8's stamps (lines 339 to 345) are values deliberately carried across waits; the pre-mortem does not say how they pass `Fresh`.
- What survives: the `!Send` wrapper as a lint-like narrowing, with the version compare kept in the simulator only.

### M3. The partial-commit check cannot see its own example

- The pre-mortem (line 45) proposes each sealed record's own invariant, checked at every wait.
- Its example is "a key's stroke passed but the terminal not told". Those are two records (`Keys` and the terminal's reading state in `Application`). Each is valid alone after each transition, which sealing already guarantees (line 188). The broken fact is a relation between records, which no per-record check reads.
- What would catch it: an obligation value. The first transition returns a `#[must_use]` token that only the second transition consumes, held in the entry's ids record or the queue so the entry's end (line 336, `ended_waiting`) is forced to discharge it. That is the design's ticket pattern applied to a two-step change.

### M4. `#[derive(NamesIds)]` is worth doing, but none of the cited history is the defect it prevents

- 48431c2f reverts ce3305a1, which did not release anything. It left told-only elements out of the listener's duplicate-check read (`KeptElements::with_runtime_id`), so "the check report[ed] none". The pre-mortem's "elements only told were let go before Core had read the tell" (line 90) is wrong. The class is a reader judging what Core has read, which CLAUDE.md already names.
- 7c507787 is retention by count and time (64 tells or 60 seconds), which held views replace by construction.
- 17adee32 sent flat offsets as anchors: an id of the wrong kind named, the opposite of an omission.
- So "closes the history's commonest held-view defect" (recommendation 3) has no support. The history's held-view defects are in position accounting (`handled`, and which messages count toward a position, 7c507787's second paragraph), and the pre-mortem lists no class for that. See "Missing bug classes", item 4.

### M5. The world server's risk is overstated where it is covered and missed where it is not

- The pre-mortem (line 201) says the conformance test "holds the two sides to each other, not to Windows", and rates the class high and high.
- The design has two checks that do hold each side to Windows, neither mentioned: live Verbatim's speech must equal replayed Verbatim's for one scenario per family (line 816), and headless NVDA must equal live NVDA's capture, the copy check (line 817, and line 609).
- The pre-mortem's two "instances" are not common-mode failures. The `GetRuntimeId` bug (line 727) and the Run dialog's children (line 504) were defects in NVDA-only stand-ins, before Verbatim read any world, and the Run dialog's was found by exactly the copy check.
- The real gaps the tie checks leave, none of which the pre-mortem lists:
  - The recorder is Verbatim's own readers (line 484), reading through Verbatim's own platform layer. A misreading there, such as 9e517cad's missing property read as false, is recorded into the world and then replayed to both screen readers. The replay-honesty check passes, because live and replayed Verbatim share the misreading. Only the copy check can see it, and only for worlds that have a live NVDA capture.
  - Generated worlds (lines 795 to 803) have no live capture at all. Their application model is written once and judged by nothing: both screen readers can agree on behaviour no application has.
  - One scenario per family is a thin tie.
- The recommended semantic-coverage check (line 205) is a presence check, the same weakness the pre-mortem finds in the `NamesIds` check: one live test per operation does not pin cached against current, ignore-default, view and condition in combination. Its cost is not "cheap": the stand-in methods are the R2 class, about 37,600 NVDA lines waiting on them (line 568), every test needs the desktop, and the design budgets 1,500 lines for all live API tests (line 1117).
- Better aimed: the recorder reads through a path independent of `verbatim-uia`'s parsing (raw calls, raw variants written as they came), and every generated-world rule cites the recording that shows it, as the reaction model list already must (line 542).

### M6. Expectation files: the contradiction is real, the fix as argued is not

- Confirmed: lines 965 to 969 store NVDA's output and Verbatim's expected output whole, and the second is derived from the first plus rulings.
- Missed: the design holds the ruled differences a third time. The allowlist (lines 769 to 786) stores, per difference, the world, step, count, NVDA's text and Verbatim's text with the ruling. "Comparison" and "Expectations" are two representations of one fact inside `design.md`, and nothing says which is the source.
- The fix does not remove blessing:
  - Verbatim's side cannot be blessed alone under Dickson's form either, since check 3 (line 969) fails any differing step without a ruling.
  - The blessable side is NVDA's stored output, in both forms. Under the pre-mortem's form a harness regression changes Verbatim's computed expectation with no diff in any Verbatim file, so there is less to review, not more.
  - "Nothing to bless" (line 219) is false: a `status = "pending"` difference (line 774) passes until ruled, in either form. That is the blessing route, and the pre-mortem does not list it.
  - The rule that actually guards NVDA's side, "changes only with the harness or the world and a passing copy check", is independent of the file's form. The pre-mortem offers it as the lesser alternative (line 276); it is the effective part.
- Whether it contradicts Dickson's decision: his three checks (lines 967 to 969) all survive. Verbatim's expectation is still checkable on Linux with no NVDA, because NVDA's output is stored. Only the literal "stores two outputs" changes.
- A version that satisfies both: the file stores NVDA's output whole, and for Verbatim only the steps that differ, each whole and citing its ruling; a step with no override is equal by definition. Those overrides are the allowlist's `[[difference]]` entries, so the separate allowlist file goes and the fact is held once. A command prints Verbatim's full expected output for reading, generated and never stored. A pending override is counted, and the count only falls without Dickson's ruling. This reads as an implementation of his sentence "equals NVDA's except where a ruling differs", not a reversal, but the wording is his.

## Should fix

### S1. The speech root as a second asker: partly confirmed, not new, and "certain" is too strong

- Confirmed in the text: "Request numbers are minted by Core only" (line 58) against the stage's asker being the speech root (`extensions-design.md` lines 98 and 325).
- Not new: `design.md` already has a second asker, tentatively ruled, for the synthesizer host (lines 1012 to 1017), and the extensions design lists the stage's asker as Dickson's, provisional (question 4, line 643). The pre-mortem presents a known open decision as a discovery.
- The new part is real: a host is "the process that runs one extension's modules" (`extensions-design.md` line 42), and nothing gives a stage a host of its own as synthesizers have (line 97). An extension with an app module and a stage has one peer with two askers and two `Out` records.
- Number collision is speculation: answers are matched by peer first (line 124), so separate pipes do not collide.
- Unlisted consequences of the same root: `transform(ctx, utterance)` takes the context, which is "read from Core's records when the call is sent" (`extensions-design.md` line 399), so the speech root would hold a copy of Core's facts; the `Instance` request and the breaker's failure counts are Core's (line 218), but a stage's instance would end at the speech root; and nothing says how the speech root learns of the host's end, which is Core's `Serve` request.

### S2. Records owning requests are not a new risk

- The pre-mortem calls them "the second largest" new risk and says they "reintroduce" the interleaving (line 25).
- The design's table (line 355) shows these reads are outside the queue today, by ruling. The flaky entry "A key pressed as a terminal flood ends" is today's code, and its recorded fix is a planned step on how a key takes ownership of reads. The rewrite carries the behaviour over; it does not add it. The entry lists one failure, so "flakiest" is not supported.
- The interleaving mode is still worth having, with one caveat: for Verbatim-only behaviour such as the terminal's row memory there is no NVDA answer, so exploring both orders needs the reference model (line 787) to say which is right.

### S3. Mitigations that add their own costs or bugs

- The credit-only message (line 128) is a pipe write and a reader wake per step under a flood, and each carries a full held view (line 139), which worsens the pre-mortem's own first performance item. A threshold grant (send when half the credit is used) bounds it. The design does mention a credit message in passing (line 170), so "states neither" is half right.
- `not_after` (line 141) checks the clock and then calls; a call that enters the hung application after the check still runs late. It covers only a later step queued behind a hung one. "Never queued behind a read that has passed half its due" is a heuristic that needs a waiting place for the act, a new state.
- The starvation invariant (line 185), "a command under a bounded event rate reaches its `Final`", fails by design: the order rule cancels it (`extensions-design.md` line 179).
- The generational `IdRef` (line 99): if `let_go` consumes a non-`Clone` value there is no stale one, and the `Stale` answer is a fallback that can only be a logic bug, which CLAUDE.md's coding standards forbid. Keep the move, drop the outcome.
- The `props!` projection (line 157) cannot be "built only from an answer" for the commonest read, a feature reading the committed focus from `Versions`, where the stored element is the three-state struct. The check moves to the projection's constructor at run time, which is a good single place but not "cannot be read". It also changes the surface of a decision in "The cost of a feature" (line 951), which the pre-mortem does not flag.
- A ratchet on waits behind other requests "over the desktop scenarios" (line 83) is a timing figure from a shared desktop and will flake; report it, or take it from the simulator.
- Simulator-only behaviour (drop bombs, the `sim` accessor log, fixtures) lives behind features. `cargo xtask ci` builds the whole workspace into `target/debug`, the output the end-to-end suite uses, and cargo unifies a member's features across one build. Not verified here for the `verbatim-next-*` crates, which do not exist yet; the pre-mortem should name the hazard.

### S4. Likelihood and severity

- Two askers of one host: "certain" only for an extension that mixes kinds.
- One world server: high and high is right for the recorder and generated worlds, not for the server's semantics, which the tie checks cover per family.
- Brittle worlds: severity should be high, not medium. Every behaviour change that shifts when Verbatim acts needs a desktop re-recording by the one agent that may hold the desktop, and the comparison gates `cargo xtask ci` (line 788).
- Stale derived values: "high" has no direct history, since today's Core is not async; 5ce4f65f is about copies in records, which the design closes.

## Missing bug classes

1. A retry loop on a level-triggered end cause. A cause that already holds ends the wait "at once" (line 383), in the same step. An entry that asks again under the same stage, as the M5 loop's shape does (line 995), spins inside one poll and freezes Core. Nothing in the driver bounds re-asks within a step.
2. `Cancel(Late)` amplification. Core answers each item for a number it does not hold with a `Cancel(Late)` (lines 124 and 269). A flooding standing request sends many items before the cancel lands; each costs a message with a held view. If "once" means once per number, Core needs a set of numbers already answered, a new table with its own growth and forgetting rule. The pre-mortem rates the area "Low" (line 132).
3. A record's rule keyed to another feature's stage name. A record-owned answer judges from the front view's kind and stage (line 368). Stages are strings (line 302). Renaming or adding a stage in the review press silently changes the terminal record's judgement.
4. Position accounting. `handled` and which messages count toward a position decide every release (lines 139 to 142). Coalesced items, credit-held items, refused messages and `Holding` reports each need a rule; this is where 7c507787 was.
5. A withdrawn batch that still acts. A batch cancelled for any reason but the foreground stop "runs on" (line 274). If it holds a `Select` or a click, the act happens after Core resolved the ticket `Ended` and went on as if it had not. This is commoner than the abandoned-worker case the pre-mortem lists.
6. An under-explored NVDA outcome set. With one outcome found, Verbatim "must equal it" (line 758), and the set depends on the seed budget (line 762). A point where the budget found only NVDA's silent order makes silence the required output; the guard at line 763 applies only to sets with several members. The plausible-boundary definition also rests on a one-time desktop measurement of NVDA's handler times (line 601), a machine-dependent constant inside the oracle.
7. Trace replay across processes. Each process keeps its own ring and checkpoint (line 863). A message is both the sender's replayed output and the receiver's recorded input, one fact twice, and rings of different lengths under a flood give no common window. A reader that hangs or is killed loses its ring, in the case the trace exists for. The no-time-bound holding hook (`extensions-design.md` line 388) makes "the oldest unfinished entry's checkpoint" unbounded in age, which the pre-mortem's checkpoint item does not name.
8. The user's content in the corpus. The trace records applications' answers (line 856), world capture records text, messages and the clipboard (lines 521 to 527), a dump "becomes a permanent replay test at once" (line 865), and the real-use loop runs on Dickson's machine (line 813). Nothing scrubs passwords or documents before a file joins the repository.
9. Scope and lock-screen filters applied by hand. The lock-screen rule runs "at every send and arrival" over contexts, reads, events and effects (`extensions-design.md` line 145), and scope fills each context field (line 143). Each new field, effect or step needs the filter, and an omission is a trust-boundary bug that no comparison with NVDA sees. A read whose step navigates out of scope must be filtered on the answer, after the outpost has read it.
10. Cancel-safety exported to extension authors. Cooperative cancellation returns `cancelled` from any import and keeps the guest's state (`extensions-design.md` line 383). A module cancelled between two updates of its own state is left half-updated, the class the pre-mortem cites Tokio for, now in third-party code.
11. Host imports that outlive their call. A blocked import "finishes on its own thread and its result is dropped" (`extensions-design.md` line 211). A cancelled translation still sends the user's speech to the network, and with one cancel per key press the in-flight operations and blocking-pool threads pile up without bound.

Smaller: the guest's determinism across architectures (a trace from ARM64 replayed on x64 Linux needs NaN canonicalisation in Wasmtime); a foreground stop for a record's stuck request (line 364) loses the focus line's elements on that worker, so a background terminal read can make the next focus repeat the window's title.

## Evidence that does not hold as written

- 48431c2f: not a release (M4).
- 17adee32 and 7c507787: not omissions from a view (M4).
- 8141d0fa and `patterns.md`: cited for partial commits (line 42). The commit merged six queue hooks into one judge; `patterns.md` line 34 lists the clean-up functions as owner state outside its owner. Neither shows a half-finished lifecycle.
- 8ce48d3e: "came from one such fact" (line 155) reads as a not-asked confusion; the commit adds a property. The 29 files are right.
- 7948f852: a frame too large, not an answer that fails to fit its batch.
- Tokio: the page says "when an asynchronous method is cancelled, that always happens at an `.await`", not the quoted words, and lists `write_all` as not cancellation safe without the stated reason.
- FoundationDB: the page says performance tests run "in addition to simulation"; it gives no "for this reason".
- "Orphan messages" (line 132): the term is not in Torres Lopez and others, and no source is cited.
- Knight and Leveson: the link is a summary of the paper, not the paper; the 27 versions and the conclusion are in it.
- "No dump round trip" (line 59): line 865 already makes every kept dump a permanent replay test, and build step 3 proves replay from a checkpoint (line 1136).

Held as claimed: 5ce4f65f, 671aa993, e0092292, 5c4f5d16, 015e80dc, 8f2093e3, 9e517cad (as an analogy), d08b85c3, 5cfa65e9 (quoted exactly), the memory note on generators, `inbox.rs` 601, both flaky-test entries, the roadmap's two quotations, Torres Lopez's three definitions, Tu and others' 58 percent (from the summary), Kleppmann, Fluent, Dropbox, Jest and Just and others' 73 and 17 percent.

## Not verified

- Moreau, Dickman and Jones, and Barr and others: both pages refused the fetch (HTTP 403), and two other routes failed.
- Tu and others' paper itself; only the cited summary was read.
- Whether workspace feature unification would reach the new crates (S3).
- No claim about NVDA's source was checked against `nvda/`; the review read the designs' citations as given.
