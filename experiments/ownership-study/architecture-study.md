# Races between Verbatim's processes: an architecture study

Status: read-only study, 2026-10-30, for Dickson's decision. Nothing here is built. Written from `docs/architecture.md`, `docs/design/focus-pipeline-rework.md`, the crate guides, `docs/performance.md`, the history from b9c02139 to 5797c075, the unbuilt designs `core-flow-design.md`, `core-flow-review.md` and `rest-design.md` in this scratchpad, the worktree `verbatim-wt-fix2`, and the two `explorer_folder_window` defects the coordinator relayed.

## In short

The defects share one cause. Some facts about one application are decided in two or three processes, and a decision often depends on a message from another process arriving first. The rework moved each fact to a better owner. It kept two kinds of message that let the defect class grow back. The first kind carries Core's state back to a reader for it to judge against: `CoreFocus` today, and the refusal notice and read requests the second build would add. The second kind lets a reader skip work because of a judgement only Core can make: the unread repeat and the unread foreground change.

Recommendation, called "one judge per scope" below:

- Every fact that spans applications, and every decision about words, is judged in Core, from complete facts: speech, attention, the foreground, repeats, chains, text and terminals.
- Every remembered fact that decides which calls to make for one application is judged in that application's outpost, the one place where both of the application's event routes meet in order: the order, verdicts, the gap, the batch, identity, the followed focus and the caret watch.
- That per-application judgement is built as a pure state machine, called the judge, which runs inside the outpost and is tested by replay as Core is (answers file, question 1, variant V1).
- The listener only observes and reads.
- Messages from Core to a process are only queries, each with exactly one answer, and node release.
- A reader may skip a read on its own cache, but always sends a complete fact. A repeat is sent as a full fact from the reader's kept record, with no extra call, and Core judges it.
- The application rules stay in Core until M5 moves them to modules, by D6, as one stated residual.

These facts then have one owner by construction, and the race classes that came with them cannot arise:

- the repeat test;
- the followed focus;
- the foreground tests;
- read requests;
- the refusal notice;
- MSAA equality's candidates.

The application rules are the exception until M5: they are still refused in Core, a stated residual.

Four classes remain, each confined to one owner and one rule:

- the order across applications, in Core's stale rule;
- the two routes of one application, in the outpost's intake;
- node lifetime, in one release rule;
- the threads inside an outpost, in the commit point.

None of the work built on main is reverted. The recommendation changes the unbuilt second build, and it reverses two decisions of record that are not yet built:

- repeats judged in Core through unread facts and read requests;
- the refusal notice.

The history supports this split. A reader judging repeats against its own observation failed once (43de3932). The terminal's rules moved into Core and no defect chain followed (ec7c88c8). Answers file, question 5.

## 1. The inventory

Each fact below is held or decided in more than one process, or in more than one thread of an outpost. For each: its owners, then the defects in this rework's history that came from it, with commits or review findings.

1. Core's focus. Owners: Core's `FocusContext`; the listener's `HeldElements`, marked by `CoreFocus`; the outpost's `Held`. Defects:
   - The repeat hole (audit difference 10): a focus that returns while Core's notice is in flight is never spoken.
   - A focus-now answer waits in the unjudged queue.
   - The second build's replacement added read requests, and with them review findings 1, 3 and 5 of `core-flow-review.md`. Finding 1: a stale requested read replaces a newer focus. Finding 3: a request is never answered when its outpost ends, so the idle round hangs. Finding 5: the followed focus stays on the node of an unread repeat.
2. The followed focus. Owners: the outpost's tracking or followed record, and Core's focus. Defects:
   - An outpost follows, and reads the caret of, a node Core refused (the rework's eleven facts).
   - The refusal notice was designed to correct this. It must carry Core's whole line, because MSAA equality takes its candidates from it (8826d457; core-flow, last question).
   - Handing the followed focus between the worker and the watchdog took four rounds: c47d57ae (six Important findings), 6dbcca25, 2bd69a85 (the commit point, reached under the three-rounds rule) and 8a087fe7.
3. Repeats and the currency of kept reads. Owners: the listener's kept read and its stamp; the outpost's kept read and its foreground rule; Core's duplicate rule. Defects:
   - The two readers can disagree for one application ("Two-owner facts left open" in the review).
   - Before the outpost's foreground rule, 5 of 24 MSAA unread repeats were on a node Core did not hold (rework, Terms).
4. The foreground. Owners:
   - in Core, four notions: attention, the foreground node, `foreground_changed_at`, and the newest focus's top-level window;
   - the keyboard hook's window;
   - the supervisor's attention;
   - the listener's test before a read;
   - the outpost's two tests, `in_front` and `report_foreground`'s second look;
   - Core's second test.

   Defects:
   - Audit difference 12: a focus accepted by its reader was refused after Alt+Tab.
   - The stamp that confirms a foreground report (b1917eed) is undone by the second build's observation stamp (review finding 2).
   - The stale rule's same-window exception is still open (06594d66, 9661f0d8).
   - The Settings frame's entry: 5f1c8e81, 3578a437, e60a7a58 and 38cab859. `settings_system_page` failed in 10 of 10 runs from c7c7c6dc until then.
   - The window title is lost when a foreground change and a focus share a batch (`text_box_editing`, 2 of 3 runs), now settled by Dickson's batch rule.
5. A window's backend verdict. Owners: the listener's arbitrators and the outpost's verdict (audit difference 11); after 22256896, the outpost's verdict, plus the class table in both processes. Defects:
   - 094a8dc8 and 07db29a8.
   - The ownership study, bef4d0e6 and 62c21062.
   - a3d6bc23 (four Important findings) and 248f85d3 (three fixes).
   - Remaining: a change to the class tables or an override can be in flight while one process has it and the other does not.
   - The coordinator's defect B in `explorer_folder_window`. The outpost's `uia_focus` drops the listener's read of the frame because its probe judged window 70059718 MSAA. The listener sent nothing for the list in DirectUIHWND 22480134, which probed as a provider. So Items View is never said. It is present since at least 392b393a. The listener's read-or-skip and the outpost's count-or-drop are still decided apart for windows off the class lists.
6. The gap record, the wait for a UIA focus after a `WinEvent` copy. Its evidence is split: the listener sees the UIA outcome and the outpost sees the copy. The record has moved between processes four times:
   - in the outpost before the rework, matched across two clocks;
   - 01cba97e, answered before the checks;
   - ad78f2a8, moved into the listener, then 9ba7625b, 5242658c and 28be3d08;
   - 22256896, moved back into the outpost, then f2f8209a (the timer and the record changed on two threads) and e02c7854 (a lookup that failed fast closed the record, and Settings was silent in run 5).

   The second build's redesign closes the record with the outpost's own reports. Its review found finding 4 (a give-up closes another window's record) and finding 6 (a popup is placed in the wrong window).
7. Hooks in and the hand-over. Owners: the listener's map; the supervisor's records; the outpost's counting multisets with `TWIN_BOUND`; `HeldFacts`. Defects:
   - Events are lost between an outpost's death and `HookRemoved` (difference 25).
   - The counts were never trimmed.
   - A released batch tried newest first lost a window title (core-flow, `second_application_and_verbatim_menu` run 1).
   - Review finding 7: the cost was unstated.
8. The order of one application's events across its two routes. Owners: the marker, the intake's reorder reference, and the subscription. Defects:
   - A menu opening was dropped behind a focus (difference 16).
   - The reorder reference lags.
   - An expand change was lost when the subscription moved: a5000841, 2 of 10 runs of `settings_combo_box_expanded`.
9. Node identity. Owners: the outposts, which give node ids; Core, which compares them; the listener, which holds the element. Defects:
   - One window reported by two outposts: b94d2ea2, `ObjectKey`.
   - Role-and-name matching in Core (9fd28a66), undone in part by MSAA equality in the outpost (8826d457).
   - A reused runtime id: 5797c075, and its half reversed a day later in `verbatim-wt-fix2`.
   - One window described as a UIA element by the listener and as a pane by the outpost (rest-design, first item, which undoes 9fd28a66 for windows judged UIA).
   - The coordinator's defect A: Core's line enters one window twice, as the client pane and as an ancestor pane with the same handle, both spoken. This is a regression from 3abda293 with 10c9e83a.
   - The outpost must find its own copy of the listener's element (fb7a8dae, f409ea69, 719b4ed4).
10. Node lifetime. Owners: Core's `NodesHeld` and the outpost's registries. Defects:
    - A spliced ancestor is released while Core holds it (rest-design, first possible defect).
    - Eviction compared by agile reference (the commit point).
11. Read requests and their answers, designed only. Review findings 1, 3 and 5, and the route of `ReadGone`.
12. The outbound merge against Core's view. Merged evidence is lost: a child count, the `earlier` evidence, an answer (rest-design, second possible defect).
13. Readiness. Three observers. The listener's `Ready` reversed twice in one day: 11f2d80a, 7e80b866, 5c64da12.
14. The order across outposts. Owners: every process's stamps, judged by Core's stale rule. Changes: microseconds (step 2); the same-window exception; the listener's kept chain stopped under a focus Core refused.
15. The threads inside an outpost: the worker, the watchdog and the pipe thread. This is the same class at a smaller scale. The publication at the bound (a9393c04, 45379c5c) took the four rounds of item 2, and the gap timer took f2f8209a.

Held events are part of these facts: the marker's hold, `HeldFacts`, and the hand-over's hold. So are caches: the listener's kept chain, the verdict keeps, and the registries' element and text caches. A cache is harmless while only its owner reads it. Each cache above became a defect when another process's decision depended on it.

## 2. Why the processes are split

Any alternative must meet the same needs, and these are the measured ones:

- A blocking call into a busy application cannot be cancelled. Only process exit reclaims the thread (D9). This is why there is one outpost per application, abandoned workers, and the kill-and-replace ladder. NVDA itself cancels its main thread's calls with `CoCancelCall` (`nvda/source/watchdog.py` 237). Whether that ends a UIA call is M6.1's open experiment.
- A UIA element cannot leave the process that received its event, so whoever receives a UIA focus must read it.
- The UIA focus registration is desktop-wide. Each registered process costs the focused application 47 to 90 provider calls per focus change (E-cost).
- A hung window named in a focus `WinEvent` holds every UIA event of any process with a focus registration, for about 4 seconds per event at the 2-second connection timeout (`docs/uia-bugs.md`). That process's explicit reads on other threads, and its MSAA hooks, are not held. This is why the listener is a process of its own, and why Dickson refused one UIA host on 2026-10-20: in a host, a stall would hold terminal output and every other application's UIA events.
- Two Verbatim processes connecting to one application collide inside UIA. So only an application's outpost connects to it.
- The first focus of a new application arrives before its outpost exists.

Latency, wake-up and memory figures:

- One relay hop: 0.05 to 0.15 ms at the median.
- A text request's round trip: a median of 0.8 ms, 2.6 ms at the 90th percentile, 62.4 ms at most.
- A key to its speech queued: a median of 0.6 ms and 6.1 ms at the 99th percentile (debug).
- D15's budget: 10 ms from the event to the utterance queued.
- A UIA steady-state focus: 5 UIA calls.
- An MSAA steady-state focus: 20 MSAA calls.
- Each cross-process call is a round trip through a possibly busy message loop. That is the cost the ledger exists to count.
- A spare outpost's start: a median of 1.4 to 3.6 ms. From nothing: 36 to 118 ms.
- Memory: about 23 MB private when idle. The listener takes 3.6 MB and each outpost 1.9 to 3.6 MB.
- The rule is that Core never wakes for a change event that is not on the followed focus or its chain. This is the 2026-10-29 targeting exception.

## 3. Prior art

- Chromium's accessibility. Each frame has its own accessibility tree, built in its renderer and sent one way to the browser process as atomic tree updates. The browser process alone decides which window has focus. A focus change inside one tree is only a cue to recompute global focus: start at the focused window, descend through focused frames, and fire a platform focus event only if the deepest node differs from the last one computed ([How Chrome Accessibility Works, Part 3](https://chromium.googlesource.com/chromium/src/+/798e59c5275d4131797e8ea2e96e6c6000256b58/docs/accessibility/browser/how_a11y_works_3.md); [one manager per frame](https://codereview.chromium.org/1761633002)). This is the closest analogue to Verbatim, and the split is the recommendation's: within a tree the renderer is authoritative, across trees the browser is, and nothing flows back for a renderer to judge against. Chromium differs in one useful way. A renderer does not ask the browser whether it may report a focus, so no renderer decision waits on browser state.
- Firefox's Fission synced fields. State shared between processes is declared field by field. Each field has a `CanSet` check that runs in the setting process and again in the parent process, and a failed check kills the misbehaving process. Racing sets are resolved in the order the parent process learns of them ([BrowsingContext.h](https://kdab.searchfox.org/firefox-main/source/docshell/base/BrowsingContext.h); [Project Fission/BrowsingContext](https://wiki.mozilla.org/Project_Fission/BrowsingContext); [BrowsingContext and WindowContext](https://firefox-source-docs.mozilla.org/dom/navigation/BrowsingContext.html)). The lesson is to declare each shared fact once, with who may write it, and to make one process the serializer of racing writes. Firefox checks this at run time in C++. Rust can check much of it at compile time (below).
- Erlang/OTP. Each piece of state lives inside one server process. Others only send it requests and receive replies. Supervisors restart failed workers ([OTP Design Principles](https://www.erlang.org/doc/system/design_principles)). Verbatim's supervisor and outposts already follow this. The rework drifted from it where a reader keeps a copy of Core's state.
- Servo's constellation. One coordinator holds the global browser state and talks to each script thread only by messages. Deadlock freedom comes from a declared relation saying which component may block on which ([constellation.rs](https://doc.servo.org/src/servo_constellation/constellation.rs.html)). Verbatim's equivalent rule is that Core never blocks on a reader and a reader never blocks on Core.
- VS Code's extension host. Extensions run in a separate process. They reach the editor's state only through typed RPC interfaces, one shape for each direction (`extHost.protocol.ts`; [summary](https://deepwiki.com/btwiuse/vscode/3.1-extension-host-protocol-and-api), a secondary source). The point here is a protocol typed per direction, which Verbatim's `SupervisorToOutpost` and `OutpostToSupervisor` already are.
- FoundationDB's deterministic simulation. All actors run on one thread against simulated I/O, so every interleaving can be replayed exactly ([CACM 2023](https://cacmb4.acm.org/magazines/2023/6/273229-foundationdb-a-distributed-key-value-store); [Flow](https://tm.durusau.net/?cat=1768)). Core's reducer already gives this for Core. The outposts' judgement does not have it yet, which is why their thread races are found only by review.

How far this applies to Verbatim: browsers own their renderers' code, and Verbatim does not own the applications. So the browser model holds between Verbatim's processes but not between Verbatim and an application. That boundary is still bounded only by timeouts and abandonment, and nothing in this prior art removes it.

What Rust can enforce at compile time, where C++ codebases such as Chromium's and Firefox's rely on run-time checkers and review:

- A fact writable only by its owner. Put the fact's mutators behind a module-private constructor or a capability type that only the owner's module can create, as `Watch::commit` with `Effects` does by convention today.
- Exactly one answer per request. Hand each query a responder type that must be consumed. Its `Drop` sends "gone", so a route that ends answers every open request. Review finding 3 becomes unrepresentable.
- No back-channel. Make the Core-to-process enum hold only query variants, each paired with its reply type, and `NodesHeld`. A new variant is then a visible, reviewable change. An `xtask` check can list them.
- A node leaves an outpost only through one function. If wire snapshots can be built only by the registry, which records the message sequence, the splice defect cannot recur.
- Lifecycle as typestate: an outpost Starting, then HookIn, then Forwarding Ended. Session types were applied to Servo's channels for the same purpose ([session-types](https://github.com/Munksgaard/session-types)).

## 4. Alternatives

Terms used below:

- A full fact is an event's complete read, as opposed to an unread fact, which carries only what the event itself carries.
- A back-channel is a message from Core that a reader judges events against.
- The scope of a fact is one application or several.

### A. One judge per scope (the recommendation)

The model:

- The outpost judges everything within its application:
  - the order of its two routes;
  - verdicts;
  - the gap;
  - the batch rule, including Dickson's 2026-10-30 rule that a foreground change into another top-level window is handled before a focus in its batch;
  - the menu opening's pending focus;
  - the hidden frame's refusal, which is already outside Core. The application rules (`applications.rs`) and the ignored-window rule stay where they are until M5's modules apply them at the outpost's emission, by D6. Until then, the switcher's frame refusal is a stated residual: File Explorer's outpost follows the frame during Alt+Tab (answers file, question 8);
  - node identity, with one node per window handle;
  - MSAA equality, whose candidates are its own reported line.
- The outpost follows its newest report. Core refuses a report only for a reason that spans applications, such as an older focus than another outpost's newer one. So the outpost's newest report is always either Core's focus or the focus of an application Core has left, which is what it should follow.
- Readers send full facts. The listener reads every UIA focus that passes its local checks: hung, the class list, keyboard focus, the acceptance test against the real foreground window. The outpost reads every MSAA focus and every foreground change it accepts. A repeat by NVDA's test against the reader's own kept read is sent as a full fact from that kept record, with no read and no request: the outpost's followed record, kept current by its own change events, or the listener's kept snapshot (answers file, question 7).
- Core judges the global facts from full facts:
  - repeats, from `earlier` (5797c075) and the MSAA address and simple-child mark;
  - NVDA's two foreground tests;
  - the order across outposts;
  - attention;
  - speech.

Made impossible by construction:

- The repeat hole.
- Every read-request finding: 1, 3 and 5, and the foreground half of 2. No requests exist.
- A followed focus on a node Core refused, and the refusal notice and its line.
- Currency disagreements between the two readers. Currency then only limits the listener's chain cache.
- Copies of Core's focus in a reader.
- MSAA candidates depending on Core.
- The foreground judged in three processes.
- Defect A, if the outpost's registry keys window nodes by handle and so cannot give two nodes for one handle in one chain.

What remains, and how it is confined:

- The order across applications. It cannot be removed while applications are read in separate processes. It stays in Core's stale rule, with the same-window exception and `ObjectKey`. Core is a deterministic reducer, so every ordering is a replayable unit test.
- The two routes of one application: the marker, the hold, the gap, the subscription's keep, and defect B. These are forced by the UIA constraints of section 2. They are confined to one process and one ordered intake. Defect B becomes a single-owner rule: the outpost alone decides which listener reads count, judged on the window NVDA judges. The open part is a parity fix on which window that is.
- Node lifetime. Core holds ids of objects an outpost owns. It is confined to one release rule: every node leaving the outpost goes through one function that records its message sequence, and a node is freed only when Core's `NodesHeld` names a later sequence.
- Threads inside an outpost. Calls cannot be cancelled. This stays confined to the commit point, and could go entirely if M6.1 finds `CoCancelCall` ends UIA and MSAA calls.
- Lifecycle. Processes start and die. It is confined to the supervisor's one writer, incarnation ids, and responders answered gone on drop.

Cost:

- A repeat costs NVDA's own calls (two for UIA, none for MSAA), plus one message of a full fact and one wake-up of Core.
- If repeats were read in full instead: 5 UIA calls where the drop makes 3, and about 20 MSAA calls.
- Step 1 counted 63 UIA and 6 MSAA repeats in 35 runs.
- The residual of the cached fact: a cached property changed with no event the reader saw is spoken stale, only where Core does not hold the node.
- Saved per focus: the two or three `CoreFocus` messages and their pipe-thread wake-ups.
- Saved in the rare case: a read request's round trip, 0.8 ms at the median and up to 62 ms.
- No call is added on the steady path.

Hung or crashing application: unchanged. Abandonment and kill-and-replace stay per application, and Core never blocks.

What it undoes:

- Nothing built is reverted.
- On main, the listener's and outposts' repeat drops against `CoreFocus` are removed, as step 11 planned anyway, but with no unread-fact replacement.
- MSAA equality's candidate source changes from `HeldFocus::for_outpost` to the outpost's reported line.
- Decisions reversed:
  - repeats judged in Core through unread facts and read requests (2026-10-25, amended 2026-10-29). This is replaced by full facts from the reader's cache;
  - the refusal notice (2026-10-29);
  - D14's "judged against Core's focus, outside Core only before a read", amended to: "words and everything across applications in Core; what to read for one application in its outpost, from its own observations; no state sent back".

Two variants are compared in the answers file, question 1.

- V1, the recommended way to build A: the outpost's per-application judgement as a pure state machine with calls as effects.
- V2: Core holds the per-application judge. It is rejected for its round trips at each decision point between calls, 3 to 5 ms at the median per focus.

A purer variant has the outpost also judge repeats by its own currency, so a repeat costs nothing and sends nothing. It is not chosen: this is what 43de3932 found failing on 2026-10-17. A return to an application through a window two applications share, such as Settings inside ApplicationFrameHost, raises no foreground change the outpost sees. That case would be silent, so its correctness is not by construction.

### B. The listener folded into the outposts, or the outposts into the listener

- Each outpost holds its own focus registration. This removes the two routes, the marker, the gap's split evidence, and finding the element's own copy.
- But every outpost would then receive every application's focus events. That is 47 to 90 provider calls in the focused application per outpost per focus change, and one wake-up per outpost.
- A hung window named in one focus event would hold every outpost's UIA events, terminal output included, for about 4 seconds per event. This defeats D9.
- A new application's first focus still needs a process that existed before its outpost, so a listener remains.
- Rejected on measured grounds.
- The reverse, one UIA host, makes UIA's one delivery thread the total order and removes classes 1, 2, 3, 6, 8 and most of 9. But it brings back the shared hold and the shared crash, and abandoned threads could be reclaimed only by restarting every application's reading. Dickson refused it on 2026-10-20. Nothing measured since changes that.

### C. One decision thread, NVDA's model

- Core receives every event unread and decides everything, including NVDA's checks and verdicts.
- The listener and the outposts only execute calls on command, keeping the live objects.
- Made impossible: every judgement split between processes, which covers classes 1 to 6 and 11.
- What remains:
  - The two routes still arrive with no common order, so Core needs the marker per application.
  - Each read is now asynchronous. To stay consistent within an application, Core must hold that application's events while a read is in flight, which reimplements the outpost's worker in Core across a pipe.
  - Node lifetime, lifecycle and abandonment are unchanged.
- Cost:
  - At least one round trip per check that needs a call, before every read: 0.8 ms at the median and up to 62 ms per trip, against D15's 10 ms.
  - A Core wake-up for every raw event, background events included, which breaks the targeting rule.
  - The verdicts, the walks, the plain windows, MSAA equality and window naming are ported NVDA code. They would have to be rewritten into the platform-neutral Core from the parity docs.
- It undoes most of the rework: the outpost verdict, the gap in the outpost, the commit point's follow state, and MSAA equality. Rejected.

## 5. Recommendation and the order of work

Choose alternative A. State the facts and their owners once, as a list in `docs/architecture.md` section 1, and require every design to say which owner each new fact has and that it adds no back-channel. The order of work:

1. Dickson decides the questions below. Amend D14 and section 1. One adversarial review of this model, breaking each owner with a cross-process sequence.
2. Finish `verbatim-wt-fix2`. It is a Core rule on a full fact, compatible with A, but it is the second round on that rule in a day, so its review checks the rule against NVDA's `CompareElements` before any third change.
3. The second build, redesigned from `core-flow-design.md`:
   - Remove `CoreFocus`, `HeldElements` and `Held`, with no unread facts, read requests or refusal notices.
   - Full facts for repeats and foreground changes, from the reader's cache where NVDA's test holds, judged in Core.
   - Core's one foreground record and one foreground test.
   - The application rules left in Core until M5.
   - The compile-time rules of section 3 for the code this build touches.
   - MSAA candidates from the outpost's reported line.
   - One node per window handle in an outpost (defect A).
   - The release rule by message sequence, moved here from the third build.
   - Responders that answer gone on drop.
   - The hold, with Dickson's batch rule.
   - The gap closed by the outpost's own reports, with review findings 4 and 6 fixed.
   - Defect B, as the outpost's one verdict rule.
4. The third build (`rest-design.md`), without its splice item and without "a report carrying `answers` is never merged".
5. Run M6.1's `CoCancelCall` experiment for UIA early: about half a day to a day of one subagent, with no desktop. Its answer decides the judge's threading. If calls can be cancelled, publication at the bound, owed work and most abandonment go, and the judge runs on the worker thread.
6. After the third build, make each outpost's judgement the pure judge (V1), in about two builds for the focus path and one later for the caret watch and the listener's checks. Then the two-route and thread orderings become replayable tests rather than review findings.

## Decisions for Dickson

- Adopt alternative A as V1:
  - words and everything across applications in Core;
  - what to read for one application in its outpost's pure judge;
  - no back-channel.
- Repeats sent as full facts from the reader's cache, at NVDA's call cost. The alternative is a full read at about 2 more UIA or 20 more MSAA calls per repeat.
- The application rules stay in Core until M5, accepting the switcher's frame residual, or move now.
- Amend D14 accordingly.
- Keep the gap as built in the outpost, or narrow it to the time before an application's first delivered UIA focus, its original reason. Narrowing removes most of its rules. The step 1 runs found firings after the first delivered focus, and e02c7854's case depends on one.
- Move the `CoCancelCall` experiment ahead of M6.
- The pure judge after the third build, about three builds, or as the second build's own shape, about one build more now.
