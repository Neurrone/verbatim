# Who decides which backend speaks a focus: a study

Status: study of 2026-10-29, at main 28be3d08, for Dickson's decision. Nothing in the product is changed. The changes it proposes to [the focus pipeline rework](focus-pipeline-rework.md) are listed under "Proposed changes to the rework design" and are not applied.

## In short

The question: which one process should decide, for each focus, which backend's report of it speaks and whether it has been spoken, so that the backend verdicts and the gap cannot lose or double a focus between Verbatim's processes.

The recommendation: each focus event is judged once, by the process that receives it, on the window that event names, as NVDA judges each event. The listener's verdict decides whether a UIA focus counts, and the outpost reports every listener read without judging its window again. The outpost's own verdict decides its own MSAA copy, at once. The listener keeps the gap record, built at ad78f2a8, and a record is then answered only by outcomes that no other process revises on the backend. A drop on the listener's verdict carries the record's copy back to the outpost, which reads the focus at once only if it dropped that copy. This is candidate A below, narrowed, because the outpost cannot wait on the listener before it speaks an MSAA focus.

It closes all four sequences that led to the reassessment. It keeps all of today's built gap work and the three fixes reviewed since. It removes code and one probe per new window, and adds no message. Its cost is one stated limit: if the two processes' probes of one window disagree after the outpost has spoken its MSAA copy, the focus is spoken twice, through MSAA and then UIA. Dickson's decision of 2026-10-29 avoided that double at the price of the losses this study is about.

Decisions for Dickson, each answerable in a word:

- Adopt the narrowed candidate A: the listener's verdict decides each UIA focus, and the outpost reports every listener read without a verdict of its own? Recommended: yes.
- Accept, as a stated limit, a focus spoken twice when the outpost's probe of a window said no or timed out and the listener's said UIA for the same focus? This replaces the outpost's probe before it reports a listener read, decided on 2026-10-29. Recommended: yes. If no, the alternative is candidate B, which moves the gap record back into the outpost and undoes ad78f2a8.
- Stop reading after a listener's verdict drop when the outpost dropped no copy of that focus, that is, a UIA focus with no `WinEvent` copy, as NVDA drops it? Recommended: yes.
- Build this change in the same fix commit as the reviewed fix work for the gap step: the records for every application, the threadpool timer, the acceptance filter and the idle-barrier test? Recommended: yes.

## Terms

- A copy is one focus `WinEvent` of an application. Both the listener's desktop-wide hook and the outpost's own hook receive it.
- The record is the listener's gap record (`crates/verbatim-outpost/src/gap_record.rs`). It holds the oldest copy that no UIA focus has answered.
- A verdict drop is the listener's drop of a UIA focus on its own verdict for the focus's nearest window, either not UIA or unknown (`Judged::DroppedOnVerdict`).
- An outcome is final for the backend when no other process later drops it in favour of the other backend.
- T and C are the two windows of the review's sequence. T is the top-level window a copy names, such as Windows Terminal's `CASCADIA_HOSTING_WINDOW_CLASS` window, which is on the good class list (`crates/verbatim-outpost/src/arbitration.rs`, `GOOD_UIA_CLASSES`). C is the child window that holds the UIA focus. Neither class list names C, so it is judged by the probe.

## The fault, stated once

Every loss in the four sequences has the same shape. One process drops an observation because it expects the other backend's observation of the same focus to speak. The process that records whether that happened then counts an outcome as speaking, and a third judgement later drops that outcome.

At 28be3d08 this happens as follows:

- The outpost drops its MSAA copy in a window it judges UIA (`outpost/worker.rs`, `msaa_focus`, around lines 3498 to 3506).
- The listener closes the record when it reports its read (`focus_read.rs`, `send_outcome` and `gap_answer`, 638 to 649; `gap_record.rs`, `reported`).
- The outpost then judges the read's nearest window with its own verdict and drops the read when that verdict is not UIA (`worker.rs`, `uia_focus`, around 3640 to 3656).

The listener learns nothing of that drop, so nothing speaks. The verdicts and the record are each owned once. The defect is that the outpost can still revise a decision after the listener has acted on it, because the outpost judges the UIA event a second time.

The same shape also appears without a probe disagreement. The copy names T and the UIA focus is in C. If both processes judge C not UIA, the listener's verdict drop answers the record, and the outpost reads only when it judges C UIA (`worker.rs`, `focus_dropped_on_verdict`, 1385 to 1443). It does not, so T's dropped copy is never recovered.

NVDA has one process, so it never judges one event twice. Its UIA focus handler judges the sender's nearest window (`nvda/source/UIAHandler/__init__.py`, 887, through `isNativeUIAElement`, 1516 to 1545). Its MSAA path judges the event's own window (`nvda/source/IAccessibleHandler/__init__.py`, 577). Both use one cache, `isUIAWindow` (`UIAHandler/__init__.py`, 1368 to 1380), which keeps both answers for half a second, timed from before the probe. NVDA has no recovery read. For sequence 4's windows it speaks the UIA focus once if its one probe of C says UIA. If a cancelled probe makes C not UIA (`UIAHandler/__init__.py`, 1283 to 1286), it says nothing, because T is UIA and C is not.

## The candidates

### A. The listener's verdict decides each UIA focus

As asked, candidate A has each listener read or drop carry the listener's verdict, and the outpost follows it both for that report and for its own MSAA copy of the same focus. The second half cannot be built as stated. The outpost would have to hold each MSAA copy until the listener's outcome for the paired UIA focus arrives. That outcome comes after the listener receives the UIA event (for an MSAA window, UIA's proxied focus event), probes, which can take up to `RAW_PROBE_TIMEOUT_MS` (3 seconds), and relays through the supervisor. When UIA delivers late or never, the outcome comes at the record's one-second bound. While a hung window holds the listener's UIA events (`docs/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events"), every MSAA focus would wait too. The outposts' MSAA hooks exist so that this stall stays out of them (`docs/design/focus-pipeline.md`, section 12.3). So the outpost must decide its own copy at once, by its own verdict, as it does today.

Narrowed to what can be built, A is three rules:

- The listener's verdict decides whether a UIA focus counts. The outpost reports every listener read whose window its application serves. It neither probes the read's window for it nor drops it on its own verdict. That is the drop and the probe in `uia_focus`, and the probe before the report, a deliberate difference Dickson decided on 2026-10-29.
- The outpost's verdict decides its own MSAA events at once, as now, with its 500 ms keep of an unknown verdict.
- A verdict drop that answers a record carries the record's copy, its window and observation, to the outpost. The outpost reads its application's focus at once, stamped with the dropped event's observation, when it judges the copy's window UIA, because it dropped that copy. Otherwise it does nothing, because it handled the copy. With no record answered, a verdict drop leads to no read. When a copy is observed up to 100 ms after a verdict drop, in its top-level window, the listener sends it as focus-missing at once, as it does after a drop for no window (`gap_record.rs`, `last_no_window_us`).

With these rules every outcome that answers a record is final for the backend:

- A read is always reported.
- NVDA's check verdicts and the listener giving up are drops NVDA makes too.
- A verdict drop hands the decision back to the outpost, which is the process that knows whether it dropped its copy.

The outpost still drops some listener reads after a record is answered. Each of these is a judgement that the focus should not be spoken at all, not a choice of the other backend:

- a read whose window another process serves (538bdca3);
- a read on Core's hidden frame, which the listener drops first;
- a read in a hung window, by the intake's retest until step 13 removes it, which NVDA drops on arrival;
- from M5, a module's veto.

The removed `record_probe` copied the listener's UIA verdict into the outpost's arbitrator, and only the UIA verdict, never "no". The rework recorded two faults of that design ("Where state is duplicated", the verdicts). Narrowed A avoids both, as follows.

- "A probe that times out in one process and answers UIA in the other leaves a focus a second late through the gap." With `record_probe`, the listener's timeout dropped the focus, the outpost held a UIA verdict and dropped its copy, and the outpost's gap read at its bound. Under A the verdict drop carries the copy, and the outpost reads at once.
- "Negative verdicts expire on two clocks." Under A no verdict crosses a process and nothing kept by one process is consulted by the other. The verdict travels with its own event and has no lifetime, so the shared microsecond clock (697ab4f4) does not matter here.

The reverse disagreement remains. If the outpost's probe timed out or said no, it spoke its copy through MSAA. If the listener's probe then said UIA, the listener reads, and A reports both. The rework's design prevents this double by letting the outpost's verdict decide the listener's report. That same rule is what loses sequence 4.

### B. The outpost owns the verdict and the gap

Under B, the record moves back into the outpost. The listener sends each outcome with the answer it computes today (`gap_answer`) and its top-level placement. The outpost opens a record when it drops its own copy as UIA, including a copy forwarded before its hook is in. It closes the record only on an outcome it keeps. When a listener read answers an open record in its top-level window, the outpost keeps it without judging the read's window, because the outpost already decided this focus is UIA's when it dropped the copy. A read that answers no record is judged by the outpost's verdict, as now. The bound is a threadpool timer in the outpost, and when it fires it pushes an entry into the intake.

The old outpost gap had five faults (git keeps it at ad78f2a8's parent). With one microsecond clock across processes (697ab4f4), they stand as follows:

- Matching across two clocks: gone, because the copy and the UIA focus are stamped from one clock.
- `DropEvidence`'s classes: B needs them back, as the listener's `Answer` sent with each outcome (answers a copy in a given top-level window, answers a copy in any window, read at once, or leaves the record open). The class is the listener's judgement of its own drop, sent as data. That is sound, but it brings back the wire field ad78f2a8 removed.
- The look-back: a UIA focus observed up to 100 ms before its copy still answers it. On one clock this is the same comparison in either process, so it is logic to carry over, not a fault.
- `MarkerNews`: not needed if the record sits in the intake's shared state and is updated as each marker is resolved, with the bound on a timer rather than the worker's wait in `next_with_gap`.
- A drop in another window closing a gap with nothing read: gone, if each outcome carries its top-level placement, which the listener already finds.

What B cannot recover is the listener's evidence from other applications. The outpost's hook sees only its own application, so a move to another application no longer closes the record. The record is read at its bound in the application the user left. Core's stale rule then drops the report, which is stamped with the copy's observation. Nothing is heard, but the outpost makes one read of an application the user has left.

### C. The design as it stands, plus the review's patch

Under C, the outpost reads its application's focus at once (`read::focused_control_in`) whenever it drops a listener read on its own verdict. It is included for comparison only. The patch would be a fourth round of fixes on one mechanism. It is a new `GetFocusedElementBuildCache` use that the rework's "Decisions" do not list. It also reads again in the case where the outpost dropped the read because it had already spoken the same focus through MSAA. There the second report is silent only if Core's duplicate rule equates the MSAA object read from the focus with the one read from the event address, and nothing has measured whether it does.

### Others considered

- The listener decides both events: rejected for the reason the literal form of A is rejected. MSAA speech would wait on the listener.
- The outpost asks the listener, or the listener asks the outpost, for a verdict per window: rejected. It adds a round trip before a focus is spoken. It is also the verdict message the 2026-10-29 decision removed, and it brings back the faults of `record_probe`.

## Each candidate against each counterexample

For each sequence: whether the focus is spoken once, twice or not at all, and what is heard. "The design as it stands" means 28be3d08 together with the three fixes designed since. The code at 28be3d08 has those fixes in the design text only: `gap_record.rs` still keeps records only for applications whose hook is in, times the bound on the readers' watchdog, and closes a record on any later focus elsewhere.

- Sequence 1, from 01cba97e. The outpost answered its gap before it dropped a listener read in a window it judges MSAA, and its copy was in a window it judges UIA.
  - The design as it stands: not at all.
  - A: once, the UIA read, at once.
  - B: once, the UIA read, because it answers the outpost's open record.
  - C: once, late by one read through the focus window's backend, or not at all if that read fails.
- Sequence 2: a failed read after a listener's drop. The listener drops the focus on its verdict, the outpost judges the window UIA, and its read fails.
  - The design as it stands, A, B and C: not at all. This is the decided limit that the read after a drop is made once, because a read that has just failed in a busy application would very likely fail again, and NVDA makes none.
  - B could keep its record open to the bound and read again, as the old gap did, but that reverses the decision.
- Sequence 3: the part of 01cba97e that ad78f2a8 undid.
  - A listener read that the outpost drops: under A and B, once; under C, once, late by a read.
  - A read after a verdict drop that the read's own checks refuse (another process's focus window, an unreadable or hung window, Core's hidden frame): not at all under A and C. That is intended, because each refusal means the focus is not this application's to speak or its window cannot be read, and NVDA drops those too. B could leave the record to its bound, as 01cba97e did.
- Sequence 4, the review's Important finding 1. The copy names T. The UIA focus is in C. The outpost judges C not UIA, because its probe timed out or C is within the 500 ms keep.
  - The design as it stands: not at all.
  - A: once, the listener's UIA read, at once, with no outpost probe of C.
  - B: once, the UIA read, because it answers the outpost's record in T, with no probe of C.
  - C: once, late by one read through the outpost's MSAA verdict for C, which speaks the MSAA object where the listener's read would have said "terminal". Or not at all, because the window that just timed out a probe is busy and the read fails.
- The same windows when both processes judge C not UIA.
  - The design as it stands: not at all, which matches NVDA.
  - A: once, because the verdict drop carries T's copy and the outpost, which dropped it, reads.
  - B: once, because the outpost's record in T is read at once after the drop.
  - C: not at all.
- A background application's focus `WinEvent` closing a record (counterexample 2).
  - With the acceptance filter designed in 28be3d08: once in the design as it stands, A and C, because a focus that NVDA's acceptance test refuses does not close the record.
  - Without the filter: not at all when the record's UIA focus never comes.
  - B: once, and the filter is not needed, because only the record's own application closes it. A legitimate move to another application costs a read at the bound that Core drops as stale.
- A new process's first focus, forwarded before its outpost's hook is in (counterexample 3).
  - With the records for every application (9ba7625b, design only): once in the design as it stands, A and C, by its UIA read or by focus-missing held by the supervisor for the starting outpost.
  - With the code at 28be3d08, which keeps records only for hooked applications: not at all when UIA never delivers the focus.
  - B: once, with no special case, because the starting outpost handles the forwarded copy and opens its own record.
- The outpost's own probe before a listener report (counterexample 4). This is the deliberate difference for the design as it stands: the Windows Terminal launch's blocked UI thread, 0.3 to 1.5 seconds, and "pane" from a timed-out probe.
  - A: not needed. The Terminal launch's read is reported as soon as it arrives, and the probe's share of the wait goes. The find before a text focus's report is unchanged. A timed-out outpost probe now gives "pane" only for the outpost's own MSAA copy, and then, if the listener read the same focus, the read follows: twice.
  - B: needed only for a read that answers no open record. On the Terminal launch the copy in T opens one, so C is not probed.
  - C: needed, as now.
- A UIA focus reaching the listener before its `WinEvent`, up to 25.5 ms, 5 of 1,680 (counterexample 5).
  - The design as it stands, A and C: once, because the listener's look-back answers the later copy, which opens no record.
  - Under A, if that UIA focus was a verdict drop, the copy is sent as focus-missing at once, and the outpost reads if it dropped the copy: once.
  - B: once, by the same look-back in the outpost, whichever of the copy and the outcome reaches it first.
- A windowless sender (counterexample 6).
  - The design as it stands, A and C: once, placed by its read's nearest window under the readers' lock.
  - B: once, placed by the top-level window the listener sends with the outcome.
- A drop for no keyboard focus, or for being replaced, which leaves the record open (counterexample 7).
  - All candidates: once, by the next focus's read or by the bound's read. Core's duplicate rule silences a bound read that finds the focus Core holds.
- The idle barrier (counterexample 8).
  - The design as it stands, A and C: the listener's idle answer waits for every record opened before the round (`Readers::when_idle`, `GapRecords::open_through`). The review found this untested.
  - Under A, the read after a verdict drop is made in the drop's place in the outpost's ordered stream, so it comes before the outpost's idle answer.
  - B: the outpost's idle answer waits while its record is open, as the old gap's did (`crates/mockapp/tests/focus_reports.rs`, `the_idle_answer_waits_for_the_wait_for_a_uia_focus`), and `open_through` goes.
- NVDA's own behaviour (counterexample 9). NVDA has one process and one verdict per window for both events (`UIAHandler/__init__.py`, 1368 to 1380), with each event judged on the window it names (887 and 1516 to 1545; `IAccessibleHandler/__init__.py`, 577), and no recovery read.
  - A reproduces that structure: each event is judged once, by the process that holds it, on its own window. It differs from NVDA in having two caches, so one window can be judged differently by the two processes, and in its recovery read.
  - B and the design as it stands judge the UIA event twice, in the listener and in the outpost, which NVDA never does.
  - Where NVDA says nothing (T UIA, C not UIA), A and B speak the focus once, which is the rework's decided departure for a focus UIA does not deliver.
- A probe disagreement in one window after the outpost spoke its MSAA copy. The outpost's probe said no or timed out, and the listener's said UIA.
  - The design as it stands: once, through MSAA, within the 500 ms keep. Twice once the keep has lapsed before the listener's read arrives.
  - A: twice, the MSAA object and then the UIA focus.
  - B: once, through MSAA, within the keep, as now.
  - C: once, or twice if Core does not equate the second MSAA read with the first.

## Costs

The figures are per focus change, against the code at 28be3d08 with the fixes designed since. The steady path in a UIA window is a copy, then the UIA focus, then a read.

### The design as it stands

- Steady path:
  - The listener receives the `WinEvent`, opens a record and sets a threadpool timer: two local calls.
  - It sends the marker, reads the focus (five UIA calls in steady state) and sends the read, which cancels the timer.
  - Two messages go from the listener through the supervisor to the outpost.
  - The outpost handles its own copy by its kept verdict and the read by its kept verdict for C, and sends one report.
- In an MSAA window not on the class lists, the listener's verdict drop takes the marker's place in the outpost's intake, and the outpost judges C with its kept verdict.
- Probes:
  - The listener makes at most one per window: once for the window's life when it answers UIA, and again more than 500 ms after a "no".
  - The outpost makes at most one for its copy's window.
  - The outpost makes at most one more, for C, before it reports the first listener read in a window it has not judged.
- Worst case: one focus-missing message and one `focused_control_in` read at the bound. Or, after a verdict drop, the same read at once with no message.

### A

- Steady path:
  - The same messages, calls and wake-ups.
  - One probe fewer for each first focus in a UIA window not on the class lists that the outpost has not judged. In mockapp that is one `WM_GETOBJECT`, two `ProviderOptions`, one `HostRawElementProvider` and one `Navigate`, the amount 094a8dc8 added to `crates/mockapp/tests/call_counts.rs`'s cold pins.
  - A verdict drop that answers a record grows by about 16 bytes, for the copy's window and observation.
  - In an MSAA window, the outpost judges the copy's window instead of C, usually from the same kept verdict, with no call.
- Worst case: unchanged. The read after a verdict drop is made only when the outpost dropped the copy, so the case with no copy now makes no read.
- A double costs one more report and its speech.
- Removed:
  - the verdict check in `uia_focus`;
  - `focus_dropped_on_verdict`'s read on the drop's own window, replaced by a read on the copy's window;
  - the outpost's probe before the report.
- Added:
  - the copy on `VerdictDrop`;
  - in `gap_answer`, a verdict drop that answers a copy in its top-level window and sends it on with the drop;
  - the listener's focus-missing for a copy observed up to 100 ms after a verdict drop.
- Undone:
  - from 094a8dc8, the outpost's drop of a listener read in a window it judges MSAA, the outpost's probe before the report, and the read keyed on the drop's window;
  - from the 2026-10-29 decision on backend verdicts, the sentences that give the outpost's verdict both routes;
  - the mockapp test `a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped`.
- Kept: ad78f2a8, the three fixes designed since it, the unknown verdict, the 500 ms keep, 538bdca3's served-by check, and the removal of `record_probe`.
- A consequence: a per-application backend override, which `SupervisorToOutpost::SetBackendOverride` names but nothing outside the tests sends today, would have to reach the listener's verdicts too, as M5's rule tables do (the rework, "Room for extensions").

### B

- Steady path:
  - The same messages, each outcome a byte larger for its answer class.
  - The listener's `WinEvent` callback no longer takes the readers' lock to record copies.
  - The outpost opens and answers the record and sets and cancels the timer: two local calls, as the listener does now.
  - Probes as now, except that a read answering an open record needs no probe of C.
- Worst case:
  - At the bound, one threadpool callback, one intake entry, which wakes the worker, and one read, with no focus-missing message.
  - Each record of an application the user has left is read at its bound and dropped by Core.
- Removed:
  - from the listener, the records in the readers' state, `Readers::observed`, the bound's arming and timer, `open_through` in the idle answer, and `DeliveredFact::FocusMissing`;
  - the acceptance filter, because no other application closes a record.
- Added:
  - `gap_record.rs` moved into the outpost's intake state, fed by the worker's dropped copies and by the outcomes as their markers are resolved;
  - the answer class and placement on each outcome;
  - the outpost's timer;
  - the rule that a read answering an open record is kept unjudged.
- Undone:
  - ad78f2a8 in full, with its three listener tests and the stand-in listener's records;
  - decision 11's amendment of 2026-10-29, that the wait is the listener's;
  - the location of the three fixes designed since. The records for every application become needless, the timer moves, and the acceptance filter goes.

### C

- Steady path: unchanged.
- Worst case: one more `focused_control_in` read for each listener read the outpost drops on its verdict, through the focus window's backend.
- Added: that read, a use of `GetFocusedElementBuildCache` that has not been decided.
- Undone: nothing.

None of the candidates adds a wake-up on the steady path.

## The process rules

Every candidate keeps the rules the rework and the cross-process calls study set:

- Core makes no call: none of the candidates touches Core.
- The listener calls only on held elements and connects to nothing: A changes none of the listener's calls, and B removes work from the listener.
- The outpost connects only to its own application: every read above is `focused_control_in`, with its process-id check and its served-by check of the focus window (`outpost/read.rs`, 951 to 992).
- No `GetFocusedElement` use beyond those decided:
  - A keeps the decided read after a listener's verdict drop, with a different trigger. The trigger is the dropped copy, not the outpost's verdict for the drop's window. That makes fewer reads than now, never more.
  - B moves the gap read and keeps the read after a drop.
  - C adds an undecided use.
- The cross-process calls study's rules on the process boundary, the plain windows and the checked results are not affected.

## Recommendation

Adopt A as narrowed. Each focus event is judged once, by the process that receives it, on the window it names. The listener's verdict decides a UIA focus, and the outpost reports every listener read it serves. The outpost's verdict decides its own MSAA copy at once. A verdict drop that answers a record carries the record's copy to the outpost, which reads only if it dropped that copy.

The reasons:

- It removes the one judgement that revises a decision after another process has acted on it, which is the common cause of the four sequences, rather than patching one more route around it.
- It follows NVDA's structure.
- It keeps the gap work built and reviewed today.
- It removes code and a probe, and adds no message and no wake-up.

Its one new limit, the double when the two processes' probes of one window disagree after the outpost has spoken through MSAA, needs a window that one probe times out on and the other does not, or one that gains a provider between the two probes. It is measured in the step's runs from both processes' verdict logs.

If Dickson keeps "never spoken through both backends" as a requirement, B is the candidate that meets it with no loss. It costs ad78f2a8 and the evidence from other applications.

The change needs the adversarial review of a design before it is built, which the standing rules require.

## Proposed changes to the rework design

Each change names the passage of `docs/design/focus-pipeline-rework.md` it replaces. None is applied. In the proposed text, "(Dickson, the date of his decision)" stands for the attribution that is added once he decides.

### Decisions

In "Backend verdicts (2026-10-29)", the bullet becomes:

"Backend verdicts (2026-10-29; amended, Dickson, the date of his decision, from [the backend ownership study](focus-backend-ownership-study.md)). The listener and the outposts each judge a window's backend with the same code (the class lists, the raw probe, the post-probe class checks) and no verdict message between them; the listener probes before it reads, in NVDA's order; a timed-out probe is unknown. Each focus event is judged once, by the process that receives it, on the window it names, as NVDA judges a UIA focus on its sender's nearest window and an MSAA focus on its event's own window (`UIAHandler/__init__.py` 887 and 1516 to 1545; `IAccessibleHandler/__init__.py` 577): the listener's verdict decides whether a UIA focus counts, and the outpost reports every listener read whose window its application serves without judging that window's backend again; the outpost's verdict decides its own MSAA events, at once, never waiting on the listener. The second probe this makes per focus change in an MSAA window not on the class lists, where NVDA makes one, is a deliberate difference. A focus whose MSAA copy the outpost spoke under its own "no" or unknown, and whose UIA focus the listener read under its "UIA", is spoken twice, a stated limit; it replaces the outpost's own probe before it reported the listener's read, which lost a focus whose `WinEvent` named a window the outpost judged UIA and whose UIA focus was in a child window it judged otherwise. Every window the outpost judges or acts on is first checked as served by its application, and another application's window is not probed, read or reported (Dickson, 2026-10-29); a window above the process boundary is a plain window, with no call (below)."

In "The outpost's read of its application's focus (`focused_control_in`)", "the read after a listener's drop on a verdict disagreement" becomes "the read after a listener's drop on its verdict that carries a copy the outpost dropped".

### Who owns each duplicated fact: the verdicts

In the verdicts bullet, "within it, a listener's UIA report for that window and focus-missing for it are settled by that kept verdict; after it, the outpost probes again. Timed from the end, the keep exists to settle the two routes, so that a slow window cannot have one focus spoken twice, through MSAA and then UIA after a fresh probe;" becomes:

"within it, the outpost's own events in that window and focus-missing for it are settled by that kept verdict; after it, the outpost probes again. Timed from the end, the keep stops focus-missing for a copy the outpost handled through MSAA from reading the window again through UIA after a fresh probe;"

In the same bullet, everything from "The outpost settles a disagreement from what it already receives." to "and doubles one only in the limit above." becomes:

"Each focus event is judged once, by the process that receives it. A drop of the listener's for not UIA or unknown names its nearest window and its reason, and, when it answers its application's gap record (below), the record's copy, with its window and observation; the outpost, which judged that window when it handled its own copy, reads its application's focus at once, stamped with the dropped event's observation, when it judges the copy's window UIA, having dropped that copy, and otherwise does nothing, having handled it. A drop that answers no record makes no read. The outpost reports every listener read whose window its application serves, as NVDA's UIA handler queues a focus it judged native with no second judgement: it does not probe the read's window for it, and does not drop it on its own verdict. So a disagreement in which the outpost dropped its copy costs a read, never a lost focus; one in which the outpost spoke its copy through MSAA and the listener read the UIA focus speaks the focus twice, a stated limit (Edge cases)."

### Who owns each duplicated fact: the gap's evidence

In the gap's evidence bullet, "and a drop for no window above the sender, or for not UIA or unknown, after which the outpost reads the application's focus at once in its place when its own verdict is UIA." becomes:

"a drop for no window above the sender, which answers a copy in any window to be read at once, sent as focus-missing; and a drop for not UIA or unknown, which answers a copy in its top-level window and carries that copy to the outpost with the drop, and a copy observed up to 100 ms after such a drop in that window, which is sent as focus-missing at once. Every answer is final for the backend: the outpost drops no listener read on its own verdict, so a record answered by a read is answered by a focus that is reported, and the outpost's only drops of a read after its record is answered are those NVDA makes too (another process's window, Core's hidden frame, a hung window until step 13 removes the retest, and from M5 a module's veto)."

### Mechanisms removed and added

In "Added", "the unknown verdict, the outpost's read after a listener's drop in a window it judges UIA and its drop of a listener's report in a window it judges MSAA" becomes "the unknown verdict, and the outpost's read after a listener's drop that carries a copy the outpost dropped".

### The audit's differences

Difference 11 becomes:

"Two verdicts for one window (11): each process judges with the same code, and each focus event is judged once, by the process that receives it, on the window it names. Heard: the focus once, late by a read at worst; twice, through MSAA and then UIA, when the outpost spoke its MSAA copy and the listener's probe of the same window then said UIA."

### Edge cases

The "Verdicts" bullet becomes:

"Verdicts. A window that answers one process's probe and times out the other's, or gains a provider between them, is judged differently. When the outpost dropped its copy as UIA and the listener dropped the UIA focus as not UIA or unknown, the outpost reads at once, so the focus is spoken once, late by a read. When the outpost handled its copy through MSAA and the listener read the UIA focus, the focus is spoken twice, the MSAA object and then the UIA focus, a stated limit. A UIA focus with no `WinEvent` copy that the listener drops as not UIA or unknown is not read, as NVDA drops it. Measured: probes per focus in `call_counts` against mockapp; and, in `conhost_two_windows`, `windows_terminal_two_windows` and `explorer_folder_window` ten times each, no focus lost, with the windows each process judged differently, from both processes' verdict logs, and the focuses reported through both backends, counted."

In "The read after a listener's drop is made once", "and the outpost judges the window UIA" becomes "and the outpost judges UIA the window of the copy the drop carries".

### Order of work

At the end of step 5, add:

"Its fix, from the step's review and the backend ownership study, is one commit: the records kept for every application, a forwarded copy's focus-missing held by the supervisor for the starting outpost; the bound timed by a one-shot threadpool timer; a later focus closing a record elsewhere only when it passes NVDA's acceptance test; the idle barrier's test that the review found missing; and the outpost reporting every listener read with no verdict of its own, the listener's verdict drop carrying the copy it answers, and the outpost's read after it keyed on that copy, which replace step 3's drop of a listener read in a window the outpost judges MSAA, its probe before the report, and its read keyed on the drop's own window."

### Tests

In step 3's tests, add: "At step 5's fix, `a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped` is removed, `a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read` becomes `a_focus_the_listener_dropped_on_its_verdict_is_read_when_the_outpost_dropped_its_copy`, and `call_counts`' cold UIA pins lose the outpost's probe."

In step 5's tests, the "Added" list gains:

- listener unit tests `a_verdict_drop_carries_the_copy_it_answers`, `a_copy_after_a_verdict_drop_is_sent_as_focus_missing`, `a_record_opens_for_an_application_whose_outpost_has_no_hook`, `a_focus_nvda_would_refuse_closes_no_record_elsewhere` and `an_answered_record_cancels_its_timer`, the timer injected and not waited for;
- the idle-barrier test the review asked for, `the_idle_answer_waits_for_an_open_record`;
- mockapp tests `a_listener_read_in_a_window_the_outpost_has_not_judged_is_reported_with_no_probe`, its count of the outpost's window messages to that window being zero, and `a_verdict_drop_with_no_copy_reads_nothing`.

### Related records

`docs/design/listener-without-connections.md`, decision 11: "A UIA focus's read, a verdict of NVDA's checks on the event, a drop on the listener's verdict, ... answers the wait" gains "a drop on the listener's verdict carrying the copy to the outpost, which reads when it dropped that copy". The verbatim-outpost crate guide (`docs/crates/verbatim-outpost.md`, the verdicts and the gap record) is brought up to date with the build, as the order of work requires.

## What the reviewed fix work becomes

- The records for every application: under A and C, built as designed; under B, not needed.
- The threadpool timer: under A and C, built as designed; under B, it moves to the outpost.
- The acceptance filter: under A and C, built as designed; under B, removed, because only a record's own application closes it.
- The idle-barrier test the review found missing: under A and C, a listener test of `open_through`; under B, an outpost test, `the_idle_answer_waits_for_the_wait_for_a_uia_focus` restored to the real outpost.

## Evidence

No experiment was run. Every claim follows from the code at 28be3d08 (`crates/verbatim-outpost/src/gap_record.rs`, `crates/verbatim-outpost/src/focus_read.rs`, `crates/verbatim-outpost/src/listener.rs`, `crates/verbatim-outpost/src/outpost/worker.rs`, `crates/verbatim-outpost/src/outpost/intake.rs`, `crates/verbatim-outpost/src/outpost/read.rs`, `crates/verbatim-outpost/src/arbitration.rs`), from the commits 094a8dc8, 538bdca3, 01cba97e and ad78f2a8, and from the pinned NVDA source (`nvda/source/UIAHandler/__init__.py`, `nvda/source/IAccessibleHandler/__init__.py`, at 732d1ce2). Two figures are not known and are measured in the fix step's runs:

- how often the two processes judge one window differently;
- whether Core's duplicate rule equates two MSAA reads of one focus, which only C depends on.
