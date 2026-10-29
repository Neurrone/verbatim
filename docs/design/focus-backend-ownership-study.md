# Who decides which backend speaks a focus: the outpost's one verdict and its gap

Status: study for Dickson's decision, at main 280a4b47. It works out candidate B, which Dickson chose to have worked out on 2026-10-29 after the adversarial review of the study's first recommendation. Nothing in the product is changed. The text it proposes for [the focus pipeline rework](focus-pipeline-rework.md) is under "Proposed text for the rework design" and is not applied.

## In short

B holds. Each application's outpost keeps the one verdict per window that both routes are judged by, comparing a kept verdict with each event's observation stamp rather than the moment it is handled, and it keeps the gap record (the wait for a UIA focus that never comes) beside the verdict that dropped the `WinEvent` copy; the listener's probe only decides whether the listener spends calls reading. With three further rules (the outpost's reads in place of a missing UIA focus go through UIA only, the listener makes NVDA's checks that need no call before it drops a focus on its own verdict, and the intake keeps the newest answer per top-level window), every counterexample of the first study and every finding of its review is answered, and Verbatim speaks a focus twice only in the two cases where NVDA's single cache does, re-probing a window no more often than NVDA.

The rules of B:

- One verdict per window, the outpost's. Every event of an application is judged for its backend by its outpost's verdict for the window NVDA judges it on: an MSAA event on its own window, a UIA event on its sender's nearest window, each window of a chain on itself. A kept verdict judges every event observed before it expired, whenever the worker handles the event.
- The listener's verdict saves reads and decides nothing. It reads a focus it judges UIA, and the outpost drops the read when its own verdict for the window is not UIA. For a focus it judges not UIA or unknown, it makes NVDA's checks that come after the native check and need no call, and then sends a drop naming the window; the outpost reads the focus in the drop's place when its own verdict for that window is UIA.
- The gap record is the outpost's, in its intake. It opens only for a copy the worker dropped because its verdict said UIA. Each UIA focus's outcome answers it by the class the listener sends with the outcome, and the answer is final. Its one-second bound is a threadpool timer in the outpost.
- The outpost's two reads in place of a missing UIA focus, the gap's read at its bound and the read after the listener's drop, go through UIA only, and each is made once.

Decisions for Dickson, each answerable in a word:

- Adopt B: the outpost's verdict as the one verdict per window for both routes, the listener's verdict only saving reads, and the gap record back in the outpost beside it? Recommended: yes.
- Compare a kept verdict with each event's observation rather than with when it is handled, removing the renewal of a "no" at an entry's end? Recommended: yes.
- Keep D15's UIA for a window's life and the unknown's keep from the probe's end, so that Verbatim re-probes a window no more often than NVDA's half-second cache, rather than copying that cache exactly? Recommended: yes.
- Make both recovery reads through UIA only, so a focus that has moved into an MSAA window is spoken by its own copy, at the cost of the transient UIA focus NVDA speaks before it? Recommended: yes.
- Let another application's events no longer answer a record, the gap's read instead refusing another application's focus window with no call? Recommended: yes.
- Have the listener make NVDA's checks that follow the native check and need no call before it sends a drop on its verdict? Recommended: yes.
- Build B as step 5's fix, one step reviewed as `REVIEW.md` says, with the four fix items as placed below? Recommended: yes.

## Terms

- A copy is one focus `WinEvent` of an application. Both the listener's desktop-wide hook and the outpost's own hook receive it.
- The record is the gap record: the oldest copy whose UIA focus has not come. At main it is the listener's (`crates/verbatim-outpost/src/gap_record.rs`); under B it is the outpost's.
- An outcome is what the listener sends for one UIA focus event: its read, or its drop with a reason.
- An answer is what an outcome says of the record (`gap_answer`, `crates/verbatim-outpost/src/focus_read.rs` 638): it answers a copy in one top-level window, it answers a copy in any window, it answers one that is then read at once, or it leaves the record open.
- A verdict drop is the listener's drop of a UIA focus because it judged the focus's nearest window not UIA or unknown (`Judged::DroppedOnVerdict`).
- A hint is the listener's verdict under B: it decides only whether the listener reads.
- A keep is how long a verdict is kept: UIA for the window's life (decision D15), not UIA for 500 ms, unknown for 500 ms from the probe's end.
- A recovery read is a read of the application's focus by the outpost through `read::focused_control_in`, made in place of a UIA focus that was not read: the gap's read at its bound and the read after a verdict drop.
- T and C are the two windows of the review's sequence. T is the top-level window a copy names, such as Windows Terminal's `CASCADIA_HOSTING_WINDOW_CLASS` window, on the good class list. C is the child window holding the UIA focus, which no class list names, so it is judged by the probe.

## The invariant, and what was dropped

The invariant (Dickson, 2026-10-29) is one verdict per window, as NVDA's, so that Verbatim speaks a focus twice only where NVDA does. It replaces "a focus is never spoken through both backends". NVDA keeps one cache per window in `isUIAWindow` (`nvda/source/UIAHandler/__init__.py` 1368 to 1380), stamped before the probe and kept half a second for either answer, and takes a cancelled probe as not UIA (1283 to 1286). Its MSAA path judges the event's own window (`nvda/source/IAccessibleHandler/__init__.py` 577). Its UIA path judges the sender's nearest window (`UIAHandler/__init__.py` 887, through `isNativeUIAElement`, 1516 to 1545), and so do its property and automation event handlers (1020 and 819). Its walk up a chain chooses each window's API by the same cache when the window changes (`nvda/source/NVDAObjects/window/__init__.py` 319 to 330, through 115). Only NVDA's main thread can cancel a probe (`nvda/source/watchdog.py` 417 to 420): the UIA handler's thread waits for `UiaHasServerSideProvider` itself, so NVDA's two paths can probe one window at once and keep different answers.

Narrowed A, the first recommendation, gave the listener's verdict the UIA route and the outpost's the MSAA route. Its review found eight Important findings, listed with B's answers under "Each counterexample and finding". Their common cause is that A had two verdicts per window, one per route, so one focus could be judged differently on its two routes in either order. C, the outpost reading after it drops a listener read, was a fourth round of fixes on one mechanism and an undecided use of `GetFocusedElementBuildCache`.

## Candidate B

### The one verdict

- The outpost judges a window with the class lists, then the raw probe, then the post-probe class checks (`crates/verbatim-outpost/src/arbitration.rs`, `decide`, 519), as today. Reason: the code both processes share is unchanged; only who decides changes.
- It keeps UIA for the window's life and not UIA for 500 ms; an unknown is taken as not UIA, as NVDA takes a cancelled probe, and kept for 500 ms from the probe's end, as decided on 2026-10-29. Reason: D15 spares a probe per focus change in every UIA window not on the class lists, and the keep from the end spares a second three-second probe of a busy window within half a second of the first. Both keep a verdict at least as long as NVDA's cache does, so Verbatim re-probes a window, and so changes a verdict between two events, no more often than NVDA.
- A kept verdict is compared with the event's observation stamp: it judges every event observed before its keep ended, however late the worker handles the event. Reason: NVDA judges each event as it arrives, so its two events of one focus, at most 25.5 ms apart, meet one cache entry; the outpost handles the UIA focus only after the listener's read and the relay, possibly seconds later, and comparing with that moment let a keep lapse between the two judgements of one focus (review finding 7). The renewal of a "no" at an entry's end (`Arbitrator::renew_probes_since`, 276) existed for the same reason and goes.
- Each event is judged on NVDA's window: the outpost's own MSAA events on the event's window (`worker.rs`, `msaa_focus`, 3438), the listener's read and the outpost's own UIA events on the sender's nearest window (`uia_focus`, 3585; the UIA event path, 2869 to 2872), and each window of a chain as the chain is built or converted (`read.rs`, `uia_chain_from_read`, 438 to 448). Reason: NVDA judges each of these windows with its one cache, and a chain that crosses into MSAA above a window judged not UIA is NVDA's chain.
- The outpost probes a window it has not judged before it reports a listener read there, as decided on 2026-10-29. Reason: with one verdict per window there is no other verdict to report by.
- A backend override (`SupervisorToOutpost::SetBackendOverride`) then needs to reach only the outpost, since the listener's hint decides nothing.

### The listener's hint

- The listener probes before it reads, in NVDA's order, with its own kept verdicts, and reads a focus whose nearest window it judges UIA. Reason: the probe saves a read through UIA's MSAA proxy for every focus in an MSAA application.
- For a window it judges not UIA or unknown, it goes on with NVDA's checks that need no call into the application: the acceptance test against the real foreground window, the console window's own focus, and Core's hidden frame (`focus_read.rs`, `judge_before_reading`, 1326). It sends the drop of the first that fails, and otherwise a verdict drop naming the nearest window. Reason: NVDA makes those checks after the native check, so the outpost's read in the drop's place stands only for a focus NVDA would have read had the window been UIA; today a verdict drop skips them, and the read after it makes none. NVDA's repeat and keyboard-focus checks need calls; the outpost's read reads the element that has the keyboard focus, and Core's duplicate rule drops a repeat.
- Each drop carries its answer and the top-level window it is placed in: the window the listener judged, the sender's own from the event's cache or the nearest window it found, and none when it found neither. A read carries its window already, and the outpost places it.

### The gap record in the outpost

- The record lives in the intake's shared state, under its lock. The worker opens it when it drops a copy because it judges the copy's window UIA, after the copy has passed the acceptance test and the readability checks; the copy comes from the outpost's hook, or is forwarded by the listener before the hook is in. A copy handled through MSAA opens none. Reason: the record then says exactly that this process dropped the copy for UIA, so nothing judges its window again.
- The intake applies each outcome's answer as it resolves the outcome's marker, with no wake of the worker. A read answers a copy in its top-level window; the other answers are the listener's (`gap_answer`): a verdict drop and a verdict of NVDA's checks (hung, a repeat, a failed comparison, not related to the foreground, the console window's own focus, Core's hidden frame) answer a copy in their top-level window, or in any window when unplaced; the listener giving up on the application answers one in any window; a drop for no window above the sender answers one in any window, which is read at once; a drop because the element no longer has the keyboard focus, or because a newer event of its element replaced it, leaves the record open (Dickson, 2026-10-21).
- An answer is final, whatever the worker later does with the outcome. Reason: every way the worker drops a listener outcome is a refusal NVDA makes too: its verdict on the window, another process's window (538bdca3), Core's hidden frame, a hung window until step 13 removes the intake's retest, and from M5 a module's veto.
- An outcome observed up to 100 ms before a copy answers it, as today (`gap_record.rs`, 161 to 164). The intake keeps the newest answer for each top-level window, not one newest answer. Reason: an outcome's marker can resolve before the worker has judged an earlier copy of another window queued ahead of it, and one newest answer would then stand in for that copy's own.
- A focus or foreground event the worker accepts, observed after the record's copy in another top-level window of the application, answers it. A focus counts only once it passes NVDA's acceptance test, which the worker makes before it handles any focus. Other applications' events never reach the outpost and do not answer it.
- The bound is a second from the copy's observation, timed by one one-shot threadpool timer per outpost, set when the record opens and cancelled when it is answered. When it fires it pushes an entry into the intake. Reason: an answered record costs two local calls and no wake-up, and the entry waits behind every marker queued before it, as every entry does, so a UIA focus already being read decides first with no rule of its own.
- When the bound's entry, or a drop for no window, is handled with the record still unanswered, the outpost makes the gap's read, stamped with the copy's observation.
- The outpost's idle answer waits while its record is open. The listener's idle answer no longer waits on records.

### The recovery reads

- The read after a verdict drop is made in the drop's place in the outpost's ordered stream when its verdict for the drop's window, at the drop's observation, is UIA; otherwise nothing is done, since that window's MSAA events speak for it. It does not depend on the record, as NVDA's speech of a native focus depends on no copy.
- Both recovery reads go through UIA only. The read is refused, with local calls only, when the system's foreground thread's focus window (`GetGUIThreadInfo`) is not the application's, is Core's hidden frame or cannot be read (`read.rs`, `checked_focus_window`, 1054), or is a window the outpost judges not UIA. Reason: a focus that has moved into an MSAA window is spoken by that window's copy, and reading it through MSAA as well can speak it twice, Core's duplicate rule not being known to equate two MSAA reads of one object; a focus in another application is that application's. The foreground thread's check also replaces the evidence from other applications that the listener's record had: once the user is in another application, the gap's read makes no call.
- Each recovery read is made once and stamped with the observation of the event it stands for. One that fails or is refused is not made again, the decided limit (Dickson, 2026-10-29). A read that finds the focus Core holds is dropped by Core's duplicate rule, its node coming from the same runtime id.
- The triggers are those already decided for `focused_control_in`: the read after a listener's drop and the gap's read, which at main is focus-missing's read.

## What each process owns, and the facts B splits

- The window's verdict. The outpost owns the one that decides. The listener keeps its own as a hint, with the same code; no verdict passes between them.
- The UIA focus's outcome. The listener reads or drops it and computes its answer and placement. The outpost judges the outcome's window and applies the answer to its record.
- The copy. Both hooks receive it. Only the outpost's handling counts: the listener's copy is forwarded before the outpost's hook is in, and is otherwise used only for the listener's own stamps.
- The nearest window and its top-level window. The listener finds the nearest window with its lookup and sends it; each process finds a top-level window locally with `GetAncestor`.
- Time. Every observation is stamped by its observer from one microsecond clock (`verbatim_process::clock`), so the look-back and the keeps compare stamps from either process.
- Whether a UIA focus is being read. The listener knows it; the outpost sees its marker waiting in the intake, which holds every later entry, the bound's included.
- Whether the focus has gone to another application. The listener's hook saw it; under B the outpost learns it from the foreground thread's focus window at the read, with no message.
- NVDA's checks after the native check. The listener makes those needing no call; the outpost's read and Core make the repeat and keyboard-focus checks.

## The invariants, and the sequences tried against each

Each sequence runs across the listener, the outpost's intake and worker, and Core, from the code at main. "Heard" is under B, then under NVDA.

### One verdict decides both routes

Claim: the outcome of every event depends only on the outpost's verdict for NVDA's window at the event's observation.

- The listener judges C UIA and reads; the outpost kept "no" for C from a probe 200 ms earlier that timed out on another event in C. The worker drops the read, and the read answers the record of a copy in T the worker had dropped for UIA. Heard: nothing. NVDA, whose cache holds "no" for C, drops both events: nothing.
- The listener's probe of C times out and it sends a verdict drop naming C; the outpost keeps UIA for C. The worker reads in the drop's place. Heard: the focus once, a relay and a read late. NVDA: once.
- The outpost has a backend override forcing MSAA. Every listener read is dropped and every copy is spoken through MSAA, with no change in the listener. Heard: each focus once.
- A chain whose ancestor window A, above C, the outpost judges not UIA. The conversion crosses into MSAA at A, and the focus is reported as UIA from its window C. Heard: the focus and its containers once, with A's MSAA object. NVDA's walk crosses at A in the same way.

### One judgement for one focus in one window

Claim: two events of one focus in one window observed before the first one's verdict keep ended are judged by one verdict, in either order and after any delay.

- Copy first. A copy in W, a window on no list, is observed at 0 ms and its UIA focus at 10 ms. The worker handles the copy at 1 ms; its probe of W times out at 3 s, so W is unknown, kept for events observed until 3.5 s, and the copy is spoken through MSAA. The listener's probe of W answers UIA at 2.9 s; its read arrives at 3.9 s and is handled at 4.0 s. Judged at its observation, 10 ms, it meets the unknown and is dropped. Heard: once, the MSAA object. At main the worker probes again at 4.0 s, the keep having lapsed by the clock, and reports the read as well: twice. NVDA: its main thread's cancelled probe gives "no", its UIA thread's probe is not cancelled and answers UIA: twice.
- UIA first, by up to 25.5 ms (5 of 1,680 measured). The marker reaches the outpost first and holds the copy behind it (`intake.rs`, `next`, 735 to 760). The read resolves the marker, and its answer is applied at once; the worker judges the read's window and reports it, then judges the copy by the same kept verdict and drops it, and the look-back answer opens no record. Heard: once. NVDA: once.
- A new process's late UIA focus. A copy in W at 0 ms is judged "no", kept to about 0.5 s; UIA delivers the focus at 2.7 s, within the measured 222 to 2,694 ms. The read is judged by a fresh probe. If W now answers UIA, the read is reported. Heard: twice. NVDA, whose half-second entry has also expired, probes again: twice. If W answers "no" again: once in both.
- The keep's end between a focus's two events. A "no" kept from an earlier focus ends at X; a copy observed 5 ms before X meets it and is spoken through MSAA; its UIA focus, observed 5 ms after X, meets a fresh probe, which answers UIA only if W has gained a provider at that moment. Heard: twice. NVDA has the same case at the end of its own entry: twice.

### The record judges nothing again

Claim: a record exists only for a copy the outpost dropped as UIA, and its read never judges that copy's window again.

- A copy in W is judged unknown and spoken through MSAA, and UIA never delivers the focus. No record opens. Heard: once. At main the listener's record sends focus-missing at the bound, and the outpost judges W again: if a fresh probe answers UIA, it reads the focus as well, twice. NVDA: once.
- A copy in T is dropped for UIA and UIA never delivers its focus. The bound's entry is handled at 1 s; the focus window is C, kept UIA, and the gap's read reports the focus. Heard: once, a second late. NVDA: nothing, the deliberate departure of decision 11.

### Every answer is final

Claim: no judgement made after an answer revises it in favour of the other backend.

- The worker drops a read whose window it judges not UIA, a read on Core's hidden frame, or a read whose window another process serves; each read answered the record as its marker resolved. Heard: nothing from this outpost. NVDA: nothing, from the native check, or its own GUI not being watched, or the other process speaking for its window.
- A verdict drop that the outpost answers with a read that fails. Heard: nothing, the decided limit. NVDA: the focus once, if its read of the sender succeeds.
- A drop for no keyboard focus leaves the record open, and nothing follows. The gap's read is made at the bound. Heard: the focus once. NVDA: nothing.
- A drop of another top-level window resolves before the worker judges an earlier copy of the first window queued ahead of its marker. That copy's own answer is still the newest for its top-level window, so it opens no record. Heard: once, with no read. With one newest answer, the copy would open a record read at the bound, which Core drops as a duplicate: one read for nothing.

### The recovery reads never speak a focus through MSAA

Claim: a recovery read reads only the application's UIA focus, and never one in another application.

- Review finding 4's fast Tab: the focus moves from C, a UIA window, into M, an MSAA window of the same top-level window, within milliseconds. The listener's hint drops C's focus as not UIA, and the outpost judges C UIA; when the drop is handled, the focus window is M, which the outpost judges not UIA, so nothing is read, and M's copy is spoken. Heard: M once. NVDA: C, then M, which cuts it. At main the read after the drop reads M through MSAA before M's copy: twice, unless Core equates the two MSAA nodes.
- The user goes to another application before the bound. The foreground thread's focus window is that application's, so the gap's read is refused with local calls. Heard: nothing for the record. NVDA: nothing. The listener's record at main is closed instead by the other application's accepted focus.
- The user presses Alt+Tab at the moment of the bound, before the foreground changes. The gap's read finds the focus still in the application and reports it, stamped with the copy's observation; the other application's focus follows, and Core's stale rule and its background drop at a foreground change order the two. Heard: the record's focus and then the other application's, or the other alone.

### Each recovery read is made once

Claim: one read per record or per verdict drop.

- The bound's entry and a drop for no window both reach the worker for one record. The first takes the record; the second finds it closed and reads nothing. Heard: once.

### A focus is spoken twice only where NVDA's single cache speaks it twice

Claim: B doubles a focus in two cases only, both NVDA's, and re-probes a window no more often than NVDA.

- Two windows judged differently: the copy's window judged not UIA and the UIA focus's window judged UIA, as T and C can be. Heard: twice, the MSAA object and then the UIA focus. NVDA: twice, its MSAA path judging T and its UIA path C.
- One window's verdict changing between a focus's two observations: the third and fourth sequences under "One judgement for one focus in one window". NVDA has this case too, at the end of its own entry, which never outlasts B's keep from the same probe; its UIA answers also expire after half a second, which B's never do, and its two threads can probe one window at once with different answers, which B's one worker cannot.

### The process rules

- Core makes no call: Core is not touched, and relies only on its duplicate and stale rules, as now.
- The listener calls only on held elements and connects to nothing: it loses its records and gains three local checks after its hint.
- The outpost connects only to its own application: both recovery reads are `focused_control_in` with its process-id and served-by checks (`read.rs`, 951 to 992), and refuse another application's focus window before any call.
- No `GetFocusedElement` use beyond those decided: the triggers are the read after a listener's drop and the gap's read, both decided in the rework's "Decisions".

## Each counterexample and finding, heard under B and under NVDA

From the first study:

- Sequence 1, the copy in T judged UIA and the listener's read in C judged not UIA: B nothing, the read answering the record; NVDA nothing.
- Sequence 2, a verdict drop whose read after it fails: B nothing, the decided limit; NVDA the focus once if its read succeeds.
- Sequence 3, a listener read the outpost drops: B nothing, as NVDA's native check; NVDA nothing.
- Sequence 3, a read after a verdict drop refused by its own checks: B nothing; NVDA nothing, the refusals being its own, except a focus moved into an MSAA window, which B leaves to that window's copy and NVDA speaks before it.
- Sequence 4, the copy in T and the UIA focus in C, which the outpost judges not UIA by a timed-out probe or within its keep: B nothing; NVDA nothing with that verdict, but its UIA thread does not cancel its probe, so where C answers between 3 and about 5 seconds NVDA speaks the focus late (difference 15, deliberate).
- The same windows with C judged not UIA by both: B nothing, the outcome answering the record; NVDA nothing.
- Counterexample 2, a background application's focus `WinEvent` while a record is open: B the record's focus once at its bound, as no other application reaches the outpost; NVDA nothing for that focus, which UIA did not deliver.
- Counterexample 3, a new process's first focus forwarded before its outpost's hook is in: B once, by its UIA read or by the starting outpost's gap read; NVDA once if UIA delivers it, else nothing.
- Counterexample 4, the outpost's probe before it reports a listener read: B keeps it, so the Windows Terminal launch's first focus is spoken once after the outpost's probe of C, which follows the listener's; NVDA once after its one probe.
- Counterexample 5, a UIA focus 25.5 ms before its copy: B once, the copy opening no record; NVDA once.
- Counterexample 6, a windowless sender: B once, the read placed by its nearest window's top-level window; NVDA once.
- Counterexample 7, a drop for no keyboard focus or for being replaced: B once, by the next focus or the gap's read; NVDA once, by the next focus.
- Counterexample 8, the idle barrier: B's outpost answers idle only once its record is closed, so the round waits for the gap's read; nothing is heard of it.
- Counterexample 9, NVDA's own structure: B has its one verdict per window and its windows, with D15, the unknown's keep from the probe's end, the three-second probe bound and the recovery reads as Verbatim's differences.
- A probe disagreement after the outpost spoke its MSAA copy, the listener then reading the focus as UIA: B once, the read judged by the outpost's kept verdict; NVDA once, or twice when its two threads' probes disagree.

From the review of the first recommendation:

- Finding 1, the double in both orders and with late delivery: B once in either order for one window, and twice only for two windows judged differently or a verdict changed past its keep, as NVDA, so Core absorbing neither double does not matter; NVDA twice in those cases.
- Finding 2, the chain's conversion probing every ancestor with a window: B keeps those probes on purpose, as NVDA judges each window of its walk, and the focus's own window is judged first, so the report is UIA only when C is; NVDA the same chain.
- Finding 3, the outpost's own UIA events dropped in a window it does not judge UIA: B judges them by the verdict that let the focus be reported, kept for the window's life, so a reported focus does not go mute; NVDA drops such events by the same check.
- Finding 4, the read after a verdict drop judging the copy's window late: B judges the drop's own window at the drop's observation, which no keep can lapse under, and reads through UIA only, so a focus that moved into an MSAA window is spoken once by its copy; NVDA speaks the UIA focus and then the MSAA one.
- Finding 5, a copy within 100 ms after an earlier UIA focus opening no record: B reads after a verdict drop with no record, so the next focus's drop loses nothing; a next focus that UIA never delivers stays a limit, heard as nothing for it, and NVDA hears nothing for it either.
- Finding 6, a focus both processes refuse must not be read: B reads nothing when the outpost's verdict refuses the focus's window, the outcome answering the record; NVDA nothing.
- Finding 7, B doubling once its keep lapses: B compares a keep with the event's observation, so it doubles only in NVDA's two cases, as the invariants above show, and its keeps are never shorter than NVDA's; NVDA doubles in the same two cases, and also when its two threads probe one window at once.
- Finding 8, the contradictions in the rework's text at its lines 10, 102 and 231: replaced in the proposed text below, with lines 25, 28, 38, 101, 111, 113, 164, 186, 188, 196, 207, 208, 227, 265 and 288.

## The old outpost gap's faults

The outpost's gap was removed in ad78f2a8 (its last state is in ad78f2a8's parent). With one microsecond clock and the record beside the verdict, each fault stands as follows.

- Matching across two clocks: does not return. The copy and the outcome are stamped from one clock.
- `DropEvidence`'s classes: return as data, the listener's `Answer` from `gap_answer` sent with each drop, and with the drop's top-level placement, which the old classes lacked.
- The look-back: returns unchanged in meaning, 100 ms on one clock, now with the newest answer per top-level window.
- `MarkerNews`: does not return. The intake applies each answer itself as it resolves the marker, under the lock it already holds, and the worker opens and closes the record through the same state; the bound's entry waiting in the queue replaces `next_with_gap`'s wake and `marker_waiting`.
- A drop in another window closing a gap with nothing read: does not return. Each answer is placed by its top-level window; only the listener giving up and an unplaced drop answer any window, as decided on 2026-10-21, and a drop for no window reads at once.
- The reads that answered nothing: the fault was that an outcome the outpost then dropped had answered the gap and nothing spoke. Under B every such drop is a refusal NVDA makes, so its silence is NVDA's; and a recovery read that fails or is refused answers it, the decided limit.

## Limits

- A focus spoken twice where NVDA speaks it twice, the two cases above.
- A UIA focus whose window's probe times out at three seconds is not spoken, where NVDA's UIA thread, waiting for `UiaHasServerSideProvider`, speaks it if the window answers in about five (difference 15).
- A focus that moves on before the read in a verdict drop's place: into an MSAA window or another application, only the focus it moved to is spoken, where NVDA speaks both; within UIA windows, the read speaks the focus as it is then. `GetFocusedElement` also does not always name the sender, as Start's search results raise focus while it stays on the search box (018e62d5), so the read in such a drop's place says the search box, which Core drops as a repeat.
- A next focus whose copy comes within 100 ms after the previous focus's UIA outcome in the same top-level window, and whose own UIA focus never comes, is not recovered.
- The failed read after a drop is made once (decided).
- An application whose window is accepted by NVDA's test while not in the foreground thread, such as a topmost window of a background application, has no recovery read, as `focused_control_in` reads only the foreground thread's focus window.

## Costs

The figures are per focus change, against main.

- Steady path in a UIA window, a copy, its UIA focus and a read:
  - Messages: the marker and the read to the outpost, and one report, as now; no focus-missing.
  - The listener's `WinEvent` callback no longer takes the readers' lock or calls `GetAncestor` for the records.
  - The outpost opens and answers the record and sets and cancels the timer: the intake's lock, `GetAncestor` on the read's window and two `SetThreadpoolTimer` calls, which the listener makes in the design as it stands.
  - Wake-ups: none added; the timer does not fire.
  - Probes: none, both verdicts being kept.
- Steady path in an MSAA window not on the class lists:
  - Messages: the marker and the drop, the drop about 10 bytes larger for its answer and placement.
  - The listener adds three local checks after its hint: the acceptance test's window reads, a class name and the hidden frame's property.
  - Probes: two per focus change more than 500 ms after the last, the listener's and the outpost's, as decided.
  - No record and no timer.
- First focus in a UIA window not on the class lists: the listener's probe and the outpost's probe before its report, as decided.
- Worst case, the gap's read: one threadpool callback, one intake entry that wakes the worker, the local checks of the focus window, one `GetFocusedElementBuildCache` read with its enrichment, and one report. Against main, one message and its relay hop fewer, so two wake-ups fewer, the supervisor's and the outpost's pipe thread's. When the focus is in another application, the read is refused with local calls only.
- Worst case, the read after a verdict drop: the same read, at once, with no message.
- A copy the look-back misses costs one read at the bound that Core drops as a duplicate, as now.

## What B undoes and keeps

- Undone from ad78f2a8:
  - the listener's records in the readers' shared state and `Readers::observed`;
  - the arming of the bound by the callback and the reading threads, and its timing on the readers' watchdog;
  - `DeliveredFact::FocusMissing` and its handler as a message, the handler's read becoming the gap's read;
  - `GapRecords::open_through` in the listener's idle answer;
  - the stand-in listener's records and bound in mockapp's harness (`crates/mockapp/tests/common/outpost.rs`);
  - decision 11's amendment of 2026-10-29 that the wait is the listener's.
- Kept from ad78f2a8: the removal of `outpost/gap.rs` and its cross-clock matching, `MarkerNews`, `next_with_gap`'s wake and `marker_waiting`, `Held.elsewhere`, `kept_read_answers` and `DropEvidence`; `gap_answer` and its classes; the record's logic of the look-back, the read at once after a drop for no window, and the drops that leave it open, moved into the outpost.
- Changed from 094a8dc8: the keeps compared with an event's observation, the renewal at an entry's end removed with them; and the read after a verdict drop, which goes through UIA only and comes after the listener's local checks.
- Kept from 094a8dc8: the unknown verdict and its keep from the probe's end, the listener's drop naming its window, the read after it in a window the outpost judges UIA, the outpost's drop of a read in a window it judges not UIA, its probe before it reports a read, `focused_control_in`'s process-id check, and `call_counts`' pins.
- Tests that go:
  - listener unit test `a_record_of_an_application_with_no_read_is_armed_by_the_callback`, the bound being armed when the record opens;
  - the stand-in listener's records, through which mockapp's gap tests run today.
- Tests that move into the intake's unit tests: `a_drop_for_no_keyboard_focus_leaves_the_gap_record_open` and `a_windowless_sender_answers_the_record_through_its_read`.
- Tests that change meaning: `a_focus_queued_from_another_application_after_the_copy_answers_the_wait`, since another application's focus no longer answers the record.

## What remains of step 5 and its four fix items

Step 5, "The gap in the listener", is built (ad78f2a8). Under B it becomes "The gap", and its fix moves the record into the outpost. Its four agreed fix items become:

- The records for every application: met with nothing built. The starting outpost handles the forwarded copy and opens its own record, and the supervisor already holds forwarded facts for it; no focus-missing needs holding.
- The threadpool timer: built in the outpost, one per outpost, pushing the bound's entry into the intake.
- The acceptance filter: met by the worker's acceptance test, which every focus passes before the worker handles it; the listener's filter is not built.
- The idle-barrier test: `the_idle_answer_waits_for_the_wait_for_a_uia_focus` on the real outpost, whose idle answer waits while its record is open.

## Proposed text for the rework design

Each item names the passage of `docs/design/focus-pipeline-rework.md` it replaces, by its line at main. None is applied. "(Dickson, the date of his decision)" stands for the attribution added once he decides.

### In short and the shape

- Line 10: "the wait for a UIA focus that never comes moves into the listener, on one clock; the listener and the outpost judge a window's backend with the same code, and the outpost's verdict decides which focus counts" becomes "the wait for a UIA focus that never comes stays in the outpost, beside the verdict that dropped the copy, on one clock; the outpost's verdict is the one verdict per window, as NVDA's, and the listener's, made with the same code, only saves reads".
- Line 12: "the gap in the listener" becomes "the gap".
- Line 101, the listener's bullet: "each application's gap record (below), " is removed; "the gap's bound, " is removed; "its own backend verdicts" becomes "its own backend verdicts, which only decide whether it reads"; after "the sender's keyboard focus)" is added "; after a verdict of not UIA or unknown, the checks among these that need no call, before it sends the drop"; "focus-missing; " is removed; and "reads, unread repeats and drops" becomes "reads, unread repeats and drops, each drop with its answer to the gap record and its top-level window".
- Line 102, the outpost's bullet: "its intake with the marker wait and the hand-over's hold" becomes "its intake with the marker wait, the gap record and its bound, and the hand-over's hold"; and "each window's backend with the listener's code; and, from what it receives, a disagreement with the listener's verdict, reading a focus the listener dropped as not UIA or unknown in a window it judges UIA, and dropping a listener's report in a window it judges MSAA" becomes "each window's backend, the one verdict for both routes, each event judged by the verdict kept at its observation: reading a focus the listener dropped on its verdict in a window it judges UIA, through UIA only, and dropping a listener's report in a window it judges not UIA".
- Line 231: "two independent verdicts need settling (by the outpost, from what it receives)" becomes "two processes' verdicts need one owner (the outpost's, the listener's only saving reads)", and "a missed UIA focus needs a recovery (the gap in the listener)" becomes "a missed UIA focus needs a recovery (the gap, beside the verdict that dropped its copy)".

### Decisions

Line 25, "Backend verdicts (2026-10-29)", becomes:

"Backend verdicts (2026-10-29; amended, Dickson, the date of his decision, from [the backend ownership study](focus-backend-ownership-study.md)). One verdict per window, as NVDA's (`UIAHandler/__init__.py` 1368 to 1380), so that Verbatim speaks a focus twice only where NVDA does. The outpost's verdict decides, for both routes, which backend's events count in each of its application's windows: it judges each event on the window NVDA judges it on (an MSAA event's own window, `IAccessibleHandler/__init__.py` 577; a UIA event's sender's nearest window, `UIAHandler/__init__.py` 887 and 1516 to 1545; each window of a chain, `NVDAObjects/window/__init__.py` 319 to 330) by the verdict it kept at the event's observation. The listener judges with the same code (the class lists, the raw probe, the post-probe class checks) only to decide whether it reads; it probes before it reads, in NVDA's order. No verdict message passes either way. A timed-out probe is unknown, which the outpost takes as not UIA, as NVDA takes a cancelled probe. Two deliberate differences: in an MSAA window not on the class lists, each focus change more than 500 ms after the last makes two raw probes, the listener's and the outpost's, where NVDA makes one; and the outpost probes a UIA window not on the class lists that it has not judged before it reports the listener's read there (Dickson, 2026-10-29), so a first focus waits for a busy window's second probe, as on a Windows Terminal launch, whose UI thread is blocked for 0.3 to 1.5 seconds (measured in step 3's scenarios). A focus is spoken twice only where NVDA's single cache speaks it twice: when its copy's window is judged not UIA and its UIA focus's window UIA, or when a window's verdict changes between the observations of its two events. Every window the outpost judges or acts on is first checked as served by its application, and another application's window is not probed, read or reported (Dickson, 2026-10-29); a window above the process boundary is a plain window, with no call (below)."

Line 28, "The gap", becomes:

"The gap (decision 11 of the listener design, 2026-10-18; amended 2026-10-21, 2026-10-25, 2026-10-29 and, Dickson, the date of his decision). The outpost's own read of a focus UIA did not deliver stays. The wait for it is the outpost's, beside the verdict that dropped the copy, on one clock: a record opens only for a focus copy the outpost dropped because it judged its window UIA, and each UIA focus's outcome answers it by the class the listener sends with it; a drop for no window above the sender answers it at once, a drop for no keyboard focus or for being replaced does not (2026-10-21); at its one-second bound, timed by a threadpool timer, the outpost reads its application's focus through UIA, in its ordered stream."

Line 38, the outpost's read of its application's focus: "this design's read after a listener's drop, focus-missing and the desktop's return" becomes "this design's read after a listener's drop and the desktop's return"; and "the read after a listener's drop on a verdict disagreement, the desktop's return and focus-missing in place of the gap read" becomes "the read after a listener's drop on its verdict in a window the outpost judges UIA, through UIA only, and the desktop's return".

### Who owns each duplicated fact: the verdicts

Line 111 becomes:

"The verdicts. One verdict per window, each outpost's. The outpost judges a window's backend with the class lists, then the raw probe, then the post-probe class checks, keeping UIA for the window's life (D15, a deliberate difference from NVDA's half-second for both answers in `isUIAWindow`, `UIAHandler/__init__.py` 1368 to 1380) and not UIA for 500 ms. A probe that runs out of time is unknown, which the outpost takes as not UIA, as NVDA takes a cancelled probe (1283 to 1286), kept for 500 ms timed from the probe's end (Dickson, 2026-10-29), a deliberate difference from NVDA, which times its cache from before the probe, so that a busy window is not probed again within half a second of a probe that waited three seconds. A kept verdict is compared with each event's observation, not with when the event is handled: it judges every event observed before its keep ended, so a focus's `WinEvent` copy and its UIA focus, observed within 25.5 ms of each other, are judged by one verdict whatever order and delay they reach the worker in, as NVDA judges each as it arrives; the renewal of a 'no' at an entry's end goes. Each event is judged on the window NVDA judges it on: the outpost's own MSAA events on the event's window; the listener's report of a UIA focus, and the outpost's own UIA events, on the sender's nearest window; each window of a chain the outpost builds or converts, as NVDA chooses each window's API on its walk (`NVDAObjects/window/__init__.py` 319 to 330, through 115). In a window it judges UIA, the listener's report counts and the outpost drops its own MSAA copy; in one it judges not UIA, it handles its MSAA events and drops the listener's report, by window handle. The listener judges with the same code and its own kept verdicts only to save reads: it probes before it reads, in NVDA's order, never reads a focus whose window it has not judged, and keeps nothing of an unknown. For a window it judges not UIA or unknown, it makes NVDA's checks that follow the native check and need no call (the acceptance test against the real foreground window, the console window's own focus, Core's hidden frame) and sends the drop of the first that fails, or else a drop on its verdict naming the nearest window and its reason. The outpost then reads its application's focus at once in that drop's place, stamped with the dropped event's observation, when its own verdict for that window is UIA, and otherwise does nothing; the read goes through UIA only, and is refused with local calls when the system's foreground thread's focus window is not the application's, is Core's hidden frame, cannot be read, or is one the outpost judges not UIA, since an MSAA window's own events or another application speak the focus there. No verdict message goes either way; `record_probe`'s copy of the listener's verdict goes. So a window the two processes judge differently costs a read or a wasted one, and changes nothing that is spoken. A focus is spoken twice only where NVDA's single cache speaks it twice: when its copy's window is judged not UIA and its UIA focus's window UIA, as NVDA's two paths judge two windows; or when one window's verdict changes between a focus's two observations, the later past the earlier's keep, as a new process's UIA focus delivered 222 to 2,694 ms late can be. Verbatim's keeps are never shorter than NVDA's, so it re-probes a window no more often, and NVDA also doubles when its UIA thread and main thread probe one window at once and keep different answers (`watchdog.py` 417 to 420). The probe has one bound in both processes, `RAW_PROBE_TIMEOUT_MS`, 3 seconds (difference 15). A deliberate difference: in an MSAA window not on the class lists, each focus change more than 500 ms after the last makes two raw probes, the listener's and the outpost's, where NVDA makes one. The first step's experiment on the cached provider description may later let the listener skip its probe for such a window; nothing here depends on it."

### Who owns each duplicated fact: the gap's evidence

Line 113 becomes:

"The gap's evidence. In the outpost, beside the verdict that dropped the copy, on one clock, with no message of its own; it stays as designed, by the first step's rule: a gap fired after its application's first delivered UIA focus ([the step's measurements](../../experiments/focus-pipeline-rework-measurements/readme.md)). The record is in the intake's shared state, under its lock: the oldest focus copy the worker dropped because it judged its window UIA, from the outpost's hook or forwarded by the listener before the hook is in, so a new process's first focus is recovered by its starting outpost (Dickson, 2026-10-29); a copy handled through MSAA opens none, so nothing judges its window again. Each UIA focus's outcome carries its answer, which the listener computes from the outcome (`gap_answer`) and sends with each drop, placed by the top-level window of the window it judged; the outpost places a read by the read's window. The intake applies the answer as it resolves the outcome's marker, and the answer is final whatever the worker then does with the outcome, since each of the worker's drops of a listener outcome (its verdict on the window, another process's window, Core's hidden frame, a hung window until step 13, from M5 a module's veto) is a refusal NVDA makes too. A read, a drop on the listener's verdict and a verdict of NVDA's checks (hung, a repeat, a failed comparison, not related to the foreground, the console window's own focus, the hidden frame) answer a copy in their top-level window, or in any window when unplaced; the listener giving up on the application (a timed-out nearest-window lookup included, difference 14) answers one in any window; a drop for no window above the sender answers one in any window, which is read at once in the drop's place; a drop because the element no longer has the keyboard focus, or because a newer event of its element replaced it, leaves it open, as decided on 2026-10-21 (`focus_read.rs` 451 to 470, 525 to 540), since in fast navigation such a drop says nothing of the copy's focus. An outcome observed up to 100 ms before the copy answers it too, since a UIA focus can reach the listener up to 25.5 ms before its `WinEvent` (5 of 1,680 measured); the intake keeps the newest answer for each top-level window, so an answer resolved before an earlier copy of another window is judged cannot stand in for that copy's own. A focus or foreground event the worker accepts, observed later in another top-level window of the application, answers it, a focus only once it passes NVDA's acceptance test (Dickson, 2026-10-29), which the worker makes before it handles any focus; another application's events do not reach the outpost, and the gap's read refuses another application's focus window with no call. The record's bound, a second from the copy's observation, is timed by a one-shot threadpool timer, set when the record opens and cancelled when it is answered (Dickson, 2026-10-29), so a record answered first costs two local calls and no wake-up; when it fires it pushes an entry into the intake, which waits behind every marker queued before it, so a UIA focus already being read decides first. Unanswered when that entry is handled, the outpost reads its application's focus through UIA, with the checks of the read after a listener's drop (the verdicts, above), stamped with the copy's observation; a read that finds the focus Core holds is dropped by Core's duplicate rule. The outpost's idle answer waits while its record is open. Removed: the listener's records with `Readers::observed`, the arming of their bound and its timing, `open_through` in the listener's idle answer, and focus-missing; and, as before, `gap.rs`, `MarkerNews`, the wake of `next_with_gap`, `DropEvidence` and its classes, `Held.elsewhere`, the look-back across two clocks, and any matching of the two processes' copies."

### The audit's differences and the mechanisms

- Line 164, difference 11, becomes: "Two verdicts for one window (11): one verdict per window, the outpost's, each event judged by the verdict kept at its observation, and the listener's verdict only deciding whether it reads. Heard: the focus once, late by a read at worst; twice only where NVDA's single cache speaks it twice."
- Line 186, "Added": "focus-missing; " is removed, and "the unknown verdict" becomes "the unknown verdict, kept verdicts compared with each event's observation".
- Line 188: "A gap record costs no wake-up when answered before its application's reads go idle, and one at its bound otherwise." becomes "A gap record costs two local calls and no wake-up when it is answered, and at its bound one threadpool callback and one wake-up of the outpost's worker, with no message."
- Line 227, M5's rules: "(focus-missing, the read after a listener's drop, focus-now)" becomes "(the gap's read, the read after a listener's drop, focus-now)".

### Edge cases

- Line 196, "Verdicts", becomes: "Verdicts. The listener's verdict decides nothing, so a window the two processes judge differently costs a read or a wasted one: when the listener drops a focus on its verdict and the outpost judges the window UIA, the outpost reads the focus at once, so it is spoken once, late by a relay and a read; when the listener reads a focus whose window the outpost judges not UIA, the read is dropped, as NVDA drops a focus that is not native. The read in the drop's place reads the application's focus as it is then, which `GetFocusedElement` does not always name as the sender (018e62d5), and finds nothing once the focus has gone into an MSAA window or another application, a limit: that focus is not spoken, where NVDA speaks it and then the next. A focus is spoken twice where its copy's window is judged not UIA and its UIA focus's window UIA, as in NVDA, and where one window's verdict changes between a focus's two observations, past its keep. A UIA focus whose window's probe times out at 3 seconds is not spoken, where NVDA's UIA handler, whose probe is not cancelled, speaks it if the window answers before `UiaHasServerSideProvider` gives up, about 5 seconds (difference 15). Measured: probes per focus in `call_counts` against mockapp; and, in `conhost_two_windows`, `windows_terminal_two_windows` and `explorer_folder_window` ten times each, no focus lost or doubled, with the windows the two processes judged differently, from both processes' verdict logs, and the reads after a drop, counted."
- Line 207, "The read after a listener's drop is made once", becomes: "The read after a listener's drop is made once (Dickson, 2026-10-29). When the listener drops a focus on its verdict and the outpost judges the window UIA, the outpost reads its application's focus at once in its place; if that read fails or its checks refuse it, the focus is not read again and nothing is spoken for it, because a read that has just failed in a busy application would very likely fail again, and NVDA has no recovery read there at all."
- Line 208: "The gap in the listener and the hold: measured as the order of work says." becomes "The gap and the hold: measured as the order of work says. A next focus whose copy comes within 100 ms after the previous focus's UIA outcome in the same top-level window, and whose own UIA focus never comes, is not recovered, a limit."

### Order of work

Line 265, step 5, becomes:

"5. The gap, as step 1's rule or Dickson decided: built first in the listener (ad78f2a8). Its fix, from the step's review and the backend ownership study, is one step: the record moved into the outpost's intake, opened by the worker's drop of a copy as UIA, from its hook or the listener's forwarding, answered by each read and by the answer each drop carries as the intake resolves its marker, with the newest answer per top-level window, and answered elsewhere only by an event the worker accepts; its bound a one-shot threadpool timer in the outpost that pushes an entry into the intake; the gap's read and the read after a listener's drop through UIA only, with their checks of the foreground thread's focus window; the listener's drop on its verdict sent after NVDA's checks that need no call; kept verdicts compared with each event's observation, the renewal of a 'no' at an entry's end removed; the outpost's idle answer waiting on its record; and the listener's records, their bound, `open_through` and focus-missing removed, with mockapp's stand-in listener's records."

### Tests

Line 288, step 5's tests, becomes:

"5. The gap. Removed at its build: `gap.rs`'s ten tests, and the intake's eight gap tests (`a_gap_is_not_woken_for_while_a_marker_waits_for_its_read` to `a_drop_resolved_before_a_copy_is_news_handed_out_with_the_copy`). Removed at its fix: the listener unit test `a_record_of_an_application_with_no_read_is_armed_by_the_callback`. Moved at its fix into the intake's unit tests: `a_drop_for_no_keyboard_focus_leaves_the_gap_record_open`, and `a_windowless_sender_answers_the_record_through_its_read`, which becomes `a_windowless_senders_read_answers_the_record_in_its_nearest_windows_top_level_window`. Updated at its fix: mockapp's eight `a_winevent_focus_` tests and `the_idle_answer_waits_for_the_wait_for_a_uia_focus`, through the real outpost's record and timer, the time injected and not waited for; `a_focus_queued_from_another_application_after_the_copy_answers_the_wait`, which becomes `a_record_whose_focus_went_to_another_application_reads_nothing_at_its_bound`; and the comment of `a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read`, the drop answering the outpost's record. Added at its fix: intake unit tests `an_answer_resolved_before_its_copy_is_judged_opens_no_record`, `an_answer_in_another_top_level_window_leaves_a_copys_own_answer_standing`, `a_copy_handled_through_msaa_opens_no_record`, `an_accepted_focus_in_another_top_level_window_answers_the_record`, `the_bounds_entry_waits_behind_a_marker` and `an_answered_record_cancels_its_timer`; arbitration unit test `a_kept_verdict_judges_an_event_observed_before_its_keep_ended`; listener unit tests `a_verdict_drop_is_sent_only_after_nvdas_checks_that_need_no_call` and `a_drop_carries_its_answer_and_its_top_level_window`; mockapp tests `a_copy_and_its_uia_focus_handled_after_the_keep_are_judged_by_one_verdict`, `a_verdict_drop_whose_focus_went_into_an_msaa_window_reads_nothing` and `a_forwarded_copy_opens_the_starting_outposts_record`. Scenarios: `conhost_two_windows`, `settings_system_page`, `explorer_folder_window`, `measure_focus_explorer`, `windows_terminal_two_windows`."

### Related records

`docs/design/listener-without-connections.md`, decision 11, is brought back to the outpost's wait, with the answers carried on each drop. The verbatim-outpost crate guide (`docs/crates/verbatim-outpost.md`, the verdicts and the gap record) and `docs/performance.md`'s ledger are brought up to date with the build, as the order of work requires.

## Evidence

No experiment was run. Every claim follows from the code at main 280a4b47 (`crates/verbatim-outpost/src/gap_record.rs`, `crates/verbatim-outpost/src/focus_read.rs`, `crates/verbatim-outpost/src/listener.rs`, `crates/verbatim-outpost/src/outpost/worker.rs`, `crates/verbatim-outpost/src/outpost/intake.rs`, `crates/verbatim-outpost/src/outpost/read.rs`, `crates/verbatim-outpost/src/arbitration.rs`, `crates/verbatim-core/src/reduce.rs`), from the commits 094a8dc8, 538bdca3, 01cba97e and ad78f2a8 and ad78f2a8's parent, and from the pinned NVDA source at 732d1ce2 (`UIAHandler/__init__.py`, `IAccessibleHandler/__init__.py`, `NVDAObjects/window/__init__.py`, `NVDAObjects/UIA/__init__.py`, `watchdog.py`). Three figures are not known and are measured in the step's runs: how often the two processes judge one window differently; how often a read in a verdict drop's place finds the focus gone into an MSAA window; and how often a focus is spoken twice, from both processes' verdict logs.
