# The focus pipeline rework

Status: scope and order of work agreed by Dickson on 2026-10-25, to be built in a session of its own (`docs/roadmap.md`, M4, "The focus pipeline rework"). The study of the present pipeline follows them, with file and line references into the code as of 2026-10-25; the process and thread model it starts from is `docs/architecture.md`, section 1. The scope was widened on 2026-10-29 to every reconciliation below and every difference in [the NVDA audit](focus-pipeline-audit.md).

## What was found

The pipeline's shape is forced: three measured facts about UI Automation (an element cannot leave the process that received its event; a focus registration is desktop-wide; a hung window named in a focus WinEvent holds every UIA event of a process registered for focus), decision D9, and Dickson's refusal of a one-process host on 2026-10-20 require a focus listener that reads every UIA focus, one outpost per application that holds everything else, and a Core that decides. What has grown is the machinery that reconciles the two routes and the copies of "the focus" they leave: of about ninety mechanisms, about half port NVDA or answer a constraint and about half make Verbatim's processes agree, and six of those were each patched three to seven times between 2026-10-20 and 2026-10-25, each round tuning a disagreement between two copies of one fact. The rework removes reconciliations inside the present shape; no smaller shape meets the requirements.

## Decisions (Dickson, 2026-10-25)

- Repeats are judged in Core. The listener keeps the last focus each application's reading thread read. A focus event on that element, judged by NVDA's own checks, `CompareElements` against the kept element and a live `CurrentHasKeyboardFocus` on it, never inferred from a cache, is not read again: the listener sends the kept facts, with the event's cached states, as an ordinary focus fact, and Core's duplicate rule drops it or takes it. The outpost does the same for a repeated MSAA focus on the object it read last. There is no protocol change. This removes `CoreFocus` and its recipients, the listener's held elements, the outpost's queue of unjudged focuses, the trace handshake, and the follow-up on reads a watchdog abandons with the fix approved for it, and it closes the hole where a focus returning to an element while Core's notice is in flight is never spoken. It replaces the judging before the read of the D14 rework of 2026-10-25.
- The wait for a missing UIA focus (the gap) moves into the listener, whose desktop-wide hook already receives every focus WinEvent beside the UIA focus, on one clock. At its one-second bound the listener tells the application's outpost the focus is missing, and the outpost reads it in its ordered stream, as now, so the order of the application's events stays the outpost's. This replaces the placement of 2026-10-21.
- The outpost's own read of a focus UIA did not deliver stays (decision 11).
- The hand-over to a new outpost holds the outpost's own hook copies until the moment forwarding ended is known, instead of matching the listener's copies by counting.
- Every observation is stamped in microseconds from one clock read, in every process, so the order of focuses is a total order.
- The marker that holds an application's later events until the listener's read arrives stays.

## Order of work

1. Measure first: the repeats per scenario from the listener's and outposts' debug logs (`notepad_`, `windows_terminal_`, `explorer_folder_window`, `rapid_tabbing_in_settings`), with the read's cost, as the cost of judging repeats in Core; and the relay's latency at an outpost's start over fifty starts, as the cost of the hand-over's hold.
2. Repeats judged in Core, with the kept focus resent, for UIA and MSAA; check that a name or value changed since the kept read reaches Core, and that kept ancestors are as fresh as today's reuse.
3. The gap in the listener.
4. The hand-over's hold.
5. One time unit.
6. Build `text_box_first_character_after_tab` (`docs/design/focus-pipeline.md`, section 14), which measures what removing the marker would lose.
7. Check, and fix where real, the possible defects the study found: the hand-over's maps never trimmed when forwarding's end never arrives; an ancestor released by `NodesHeld` spliced into a later report; the outbound merge leaving the outpost's states read out of step with Core; a focus-now answer's unjudged entry never drained.
8. Count how often the outpost's own focus read fires with NVDA closed and with it running (`settings_system_page` and `measure_focus_explorer`, about ten runs each), to test whether NVDA's presence causes UIA's undelivered focuses.
9. The documents: D14 and the focus pipeline and listener designs describe the result.

The study follows. Where its options differ from the decisions above, the decisions hold.

## Terms

- The listener is the one permanent `verbatim-outpost.exe --listener` process: it holds the desktop-wide UIA focus registration and desktop-wide `WinEvent` hooks, and reads each UIA focus on a thread per application.
- An outpost is the process that serves one application: its own process-scoped `WinEvent` hook, its UIA property and text registrations, one worker thread, a watchdog.
- Core is the reducer in `verbatim-core`, the only decider; the app is `verbatim.exe`'s thread that feeds it and routes its answers.
- A route is the path one of an application's events takes to its outpost: the listener's (a UIA focus, read in the listener and relayed through the app) or the outpost's own (its hook and registrations). The two-route structure is that one application's events arrive by both.
- A judge is a process that applies a focus rule before an element is read: the listener or an outpost. Core judges after the read.
- The marker is the message the listener's UIA focus callback sends an outpost the moment a focus event arrives; the outpost holds every later entry behind it until the listener's read or drop takes its place.
- The gap is an outpost's wait of one second after a `WinEvent` focus in a UIA window for the matching UIA focus, after which it reads its application's focus itself.
- The hand-over is the protocol by which the listener stops forwarding an application's `WinEvent`s once its outpost's hook is in, and the outpost drops the second copy of an event both hooks took.
- `CoreFocus` is the notice Core sends the judges after every focus event it judges and every change of its focus, so they judge repeats and change targets against the focus Core holds rather than a copy of what they last reported.
- A verdict is a window's backend decision, UIA or MSAA, from NVDA's class lists or a probe; each process keeps its own verdicts.

## The constraints that force the shape

Each is measured or decided, with its record.

- A UIA element cannot leave the process that received its event. `CoMarshalInterface` succeeds and `CoUnmarshalInterface` fails in the other process, by every route (E-marshal, focus pipeline section 11; `docs/uia-bugs.md`, "Elements cannot be passed to another process"). So whoever receives a UIA focus event must judge and read it; that is why the pre-October design, in which the outpost found the element again by runtime id, name and position, grew the moved-on rules and failed (section 1 of the focus pipeline).
- A UIA focus registration is desktop-wide. The scoped form is undocumented, refused by the direct call, and misses every proxied Win32 window (`docs/uia-bugs.md`, "Focus events cannot be limited to one application"). So one process receives every application's UIA focus events, or every outpost receives all of them at 47 to 90 provider calls each per focus change (E-cost), which Dickson ruled out.
- A hung window named in a focus `WinEvent` holds every UIA event delivered to a process that holds a focus registration, for twice the connection timeout per event (`docs/uia-bugs.md`, "A hung window named in a system focus event"). So the process holding the focus registration must hold nothing whose stall would be heard; Dickson declined one UIA host process on 2026-10-20 for this reason (D13 as amended that day).
- A thread blocked in a cross-process call cannot be cancelled; only process exit reclaims it (D9). So one outpost per application, abandonable workers and reading threads, and the kill-and-replace ladder.
- UI Automation delivers a newly started process's focus events late or never, and sometimes after their senders are gone (risks 2 and 13 of the listener design; `docs/uia-bugs.md`, "A new process's focus events reach the desktop-wide handler late"). NVDA loses those focuses by its source; Dickson decided to recover them (decision 11, 2026-10-18 and 2026-10-21), which is Verbatim's one deliberate departure from NVDA on the focus path.
- `WinEvent`s and UIA callbacks arrive with no common order; NVDA restores one by passing both through one process's threads and one queue. Verbatim has no one process, so every ordering it needs it must build.
- Windows names a console window's owner as the console's client while the console host serves it, so one application's events arrive from two processes' windows.
- Two Verbatim processes connecting to one application collide inside UIA (`docs/uia-bugs.md`, the first entry), so only an application's outpost connects to it and the listener reads with calls on held elements only.
- NVDA parity for everything spoken, from captures and the pinned source: the acceptance test, the limiter, the duplicate rule, the foreground tests, presentability, the shell's and the switcher's rules.
- Verbatim's own GUI lives in a watched process, so its hidden frame must never be announced.

From the first four, the listener must receive and read every UIA focus and each outpost must hold everything else; Core, the only process that sees every application, must decide. The two-route structure is therefore forced, given Dickson's two decisions (outposts kept, no one-process host). Nothing below proposes undoing it.

## The inventory

About ninety distinct mechanisms sit between an event and Core's decision (34 in the listener, 39 in the outpost and 31 in Core, the app and the supervisor, of which about thirteen are one protocol counted from each end). Each is named below with what it does, why it exists, and its class: a root element (forced by a constraint above), a parity port (follows NVDA's source), or a reconciliation (exists so that Verbatim's processes agree). Patches on another mechanism are called out as such. Line references are to the files named; commits are hash and date.

### The listener, as a focus meets it

- The desktop-wide registration, shaped as NVDA's base cache request (`listener.rs` 1255 to 1360; 3da68c09, 2026-10-01). Root.
- The drop of UIA events received before the subscriptions are installed (`listener.rs` 610 to 639; 4008991b, 2026-10-17). Parity port, for the Start button arriving as the initial focus on GitHub's runner.
- Ignored processes, held open and dropped before queueing (cef8587b, 2026-10-08). Harness aid.
- The `WinEvent` thread with its hook set, nudges and sync marker (`event_thread.rs`; 38bb0eb0, 2026-10-01; the one-hook-set-per-thread rule 2d47a3da after a second live set killed delivery). Root.
- Console windows routed by the raising thread to the console host (`listener.rs` 1483 to 1548; ece62cdf, 2026-10-11; the drop of an unreadable host 54f929a8, 2026-10-19). Parity rule on a Windows constraint, needed because outposts are per process.
- The menu and switcher end's 50 ms grace (f2117e1d, 2026-10-02; 69c06239 for a racing callback). Parity port.
- The outgoing backlog with NVDA's one-waiting-fact-per-element-and-kind rule, which never coalesces a marker, read or drop (`listener.rs` 160 to 340). Parity rule in content; exists because facts cross a pipe.
- The hand-over's listener half: the hooked map, "forwarding ended" naming a tick, `HookInstalled`, `HookRemoved`, `--hooks-in` for a replacement listener (217a8c8b, 2026-10-11). Reconciliation of the two routes.
- Stamping on the shared clock, in microseconds, as the callback begins (19cf75e0, 2026-10-13; e9cd3b3e and 6ab6b25d, 2026-10-24). Reconciliation across outposts; two patches in one day, the second from a review that found the stamp taken after the hook's bookkeeping.
- The foreground learnt for the keyboard hook, with the console-thread watch (`foreground.rs`; 8e27671d and fa7f67d6, 2026-10-24). Root need, placed here by choice.
- The marker sent from the callback, and the agile keep of the sender (`listener.rs` 1208 to 1253; a4fcbcf1, 2026-10-11). The keep is root; the marker is the two-route ordering mechanism.
- A menu opening dropped while a focus is pending (b772abd8, 2026-10-12). Parity port.
- Selection and notification checks from the sender's nearest window (b370a7a5). Parity port.
- One reading thread per application, in arrival order, each with its own UIA client (`focus_read.rs` 1656 to 2153). Root.
- The watchdog with `READ_BUDGET` 2 s, `READ_DEADLINE` 11 s, `MOVED_ON_GRACE` 500 ms, `ABANDONED_LIMIT` 2 and the once-per-application bound on waiting events (`focus_read.rs` 135 to 185, 2174 to 2394). The deadline is root; the moved-on grace a parity port (5204f351, 2026-10-20); the limit and bound a workaround for a hung application spawning threads without end (71126b8d).
- The read's time budget that excludes the raw probe's wait (8d6ca076, 2026-10-20) and the out-of-time judging paths (30d808b2, 52b64765, fa255063, 9e517cad, c1b36869, e46d4183, all 2026-10-20). Six commits in one day on how a focus is judged once its read's time is up; patches on the budget, which is a consequence of the listener making several calls where one remote operation would be one transaction, which is itself the consequence of the no-connection rule.
- `WatchedRead`, so a windowless sender's read can be judged moved on and an abandoned read cannot accept late (e46d4183). Patch on the watchdog.
- One waiting event per element and kind (0eaf540e). Parity port.
- Reading threads ended with their outpost, through `HookRemoved` (ad4145e5). Reconciliation; a thread could end with its process instead, which the supervisor already watches.
- The judgement in NVDA's order (ffacb9b5, 2026-10-12): the hung check; the native check with the raw probe, the console check from the held sender and the Windows Forms class check (the three replacements for calls that connected, b9f69805, 59f0f077, 4d2962c3, 2026-10-19); the repeat check against the focus Core holds; the menu redirect; the acceptance test; the console-window and hidden-frame refusals; the sender's keyboard focus read live with the cached fallback. Parity ports, with the no-connection replacements as reconciliations of the collision constraint.
- The verdict cache: yes for the window's life, no for 500 ms, a timed-out probe kept as no for the worker thread whose entry made it (`arbitration.rs` 79 to 338; 60de4e41, 2026-10-03; a133815e, 23884a3d, a93e8744, all 2026-10-25). Parity rule; the three commits of 2026-10-25 are a patch chain on the outpost's entry model, the last because abandoned workers share one arbitrator.
- The held elements and the repeat rule against the focus Core holds (`focus_read.rs` 543 to 735; a46d8b53 and a17676f6, 2026-10-11; c0bbf19b and 9f502def, 2026-10-25). Parity rule; the held-element bookkeeping is a reconciliation with Core.
- The classic ancestor walk, reusing the chain above the first known ancestor, and the report from the cache with ancestors unknown when the read fails (638064dc, 2026-10-19; e07351fe and 2b13487b, 2026-10-22). Root under the no-connection rule.
- The drop reasons and their three classes for the outpost's gap (`focus_read.rs` 286 to 310, 420 to 541; f05cdc6b, fba1dfe8, 8c4d5df4, all 2026-10-21, one of them reversing the one before). Reconciliation with the gap; a patch chain of four in one day.

### The relay and the supervisor

- The single input channel into Core, in arrival order (15cae8a8, 2026-10-11). Root, standing in for NVDA's one queue.
- The live-incarnation gate and the node-id namespace per incarnation (7271f3f9 and 36be35d8). Root under D9.
- Held facts for a starting outpost, merged by NVDA's limiter rule, released on `Ready` (5d8b945b, 2026-10-01). Root under D9.
- Spares, reincarnation, one failure path with backoff (878a586c, ded1988a, 2026-10-16). A decided latency optimization.
- The hand-over's supervisor half, relaying `ForwardingEnded` in order behind the facts. Reconciliation.
- The marker, read, drop and reads-ended messages with hand-over priority in the writer. The two-route ordering mechanism's wire form.
- `CoreFocus` to the listener (whole, no ancestors), to the judged outpost, and to the outposts the focus moved between, after every judged focus event and every change (c0bbf19b, 63ea08c1, 9f502def, 2026-10-25). Reconciliation with Core; it replaced `FocusQueued` (8fe59b1b, 2026-10-12) and was corrected twice on 2026-10-25.
- Views and `NodesHeld`, and the supervisor's own copy of attention for retirement. Root under D9.
- The idle barrier, a round through every child, holding for open gaps and markers. Harness aid.
- Console client as the application and the console host's outpost serving it, with the focus-now query routed to the host (ece62cdf; c35219af, 95b2b012, 2026-10-19). Windows constraint made two-routed by per-process outposts.
- The drop of a focus in an ignored process's window, made in the app and told to the judges as Core's refusal (cef8587b). Harness aid, and the one judgement in this scope made outside Core.

### The outpost, as an event meets it

- The own hook, the early drop of a UIA window's change events, and the hook narrowing while focus and foreground are both UIA (217a8c8b; 19533f45; 70b40f1f). Root; the drop and narrowing are wake-up optimizations; the narrowing's nudges from routed facts exist because of the two routes.
- The hand-over's outpost half: two multisets of unmatched copies by identity, `TWIN_BOUND` 15 s, and `ListenerBeforeHook` for ordering (`handover.rs`; 217a8c8b; 71126b8d, 89f94d33). Reconciliation.
- One waiting entry per key (NVDA's limiter keys), the batch limits of 4 focus and 10 other events per thread, the runs tried newest first, the redirected focus, the menu opening last (`intake.rs`; 38bb0eb0; 36c14789; 13dcd2db; be0436e9, acfc7ae6). Parity ports.
- The run ordered by raise time, with the hand-over's knowledge for one tick, and the foreground change removing a waiting client focus by raise time (f8adda43, 71126b8d, e22d3a4c, 280213dc, 172ac80c, 5d179968, 2026-10-20 and 2026-10-21). Seven commits in two days; every one exists because during a hand-over a listener copy can arrive after the hook's copy of a later event. Patches on the hand-over's no-hold choice.
- The marker and the wait behind it, resolved by a read, a drop, or the end of the listener's reads (`intake.rs` 702 to 913). The two-route ordering mechanism.
- Marker news, carrying to the worker the drops the intake resolved without a read (3d99e52c, 8c4d5df4). Patch for the gap.
- The old focus's state changes planned ahead of a focus inside it (05fe1594, 4d442eb5, 2026-10-22). Patch on the marker: the marker put a state change raised before the focus behind it, and this puts it back.
- The idle query as a batch boundary (10f01254). Harness aid.
- The watchdog with `HANDLING_DEADLINE` 10 s and `MOVED_ON_GRACE`, abandonment, publishing under the generation claim, and the supervisor's eight-abandoned-workers rule (38bb0eb0; f49cd3e3). Root.
- The acceptance test against the foreground as the entry is handled, the readable-window check, the hidden-frame drop, the foreground change's in-front check and NVDA's two tests, the MSAA focus's redirect, repeat, focused-state and enrichment steps (`acceptance.rs`; `worker.rs` 381 to 434, 3683 to 3815). Parity ports; the null-foreground guard (bf467cec, 2026-10-25) is a regression fix from that day's shared-rule rework.
- The backend arbitration with the listener's yes verdict copied in (`record_probe`, ffacb9b5). Parity rule plus a reconciliation of two arbitrations.
- The gap: open on a dropped `WinEvent` focus copy in a UIA window unless a UIA focus within 100 ms before answered it; closed by a read in the same top-level window, a drop that ends the wait, the end of the listener's reads, a later focus of this outpost, or Core's focus moving elsewhere; read at once for a sender with no window; otherwise read at the one-second bound; the idle answer held meanwhile (`gap.rs`; 3d99e52c, f05cdc6b, fba1dfe8, 8c4d5df4, 2026-10-21). Recovery from a hard constraint, shaped entirely by the two routes: the half of the evidence it needs from the listener arrives as drop classes on the wire.
- The console window read again through its proxy inside a remote-operation walk (8c4d5df4). Workaround for a UIA defect, exercised by the gap's read and the focus-now read.
- The UIA focus from the listener's read: reissuing a node for a reused runtime id, minting nodes from runtime ids, the chain from the listener's read spliced and continued through MSAA past a process boundary, finding the outpost's own element by path afterwards, or first for a text focus (`worker.rs` 3834 to 3972; `focus_path.rs`; `read.rs` 352 to 479). Forced once the listener reads.
- Held: the focus Core holds, the unjudged reports drained by the judged trace, the states last read (`held.rs`; c0bbf19b). Reconciliation with Core; the unjudged queue is the patch for the notice's latency.
- Tracking: the focus the outpost follows, its chain, window, role and batch, running beside Held (`worker.rs` 279 to 379). A second copy of the focus, older than Held.
- MSAA and UIA change and selection targeting against the judged-against focuses (c0bbf19b). Parity port of NVDA's focus gate as a pre-read cost filter.
- `emit_focus` with the report's window facts (relation to the foreground, class, application name, Alt held), the focus-now query with the proxy rule and `served_by`, the console client names held open (`worker.rs` 2297 to 2385; `read.rs` 908 to 1058; 832df39e, 2026-10-25). Root and parity; the names' process handles are a patch on the application name of 2026-10-24 (48d8432d), itself added for the switcher rule.
- Outbound merging while a write is in flight (510b7f82). Root under a pipe.

### Core and the app

- The pre-judgement gate: application refusal, duplicate rule, staleness, in that order (`reduce.rs` 550 to 579). Root ordering.
- The duplicate rule by node id while the focus is alive, its newer reading kept unspoken (`reduce.rs` 356 to 413; 15cae8a8). Verbatim's own, needed by any design with two backends.
- The stale-focus drop across outposts by observation time, to the microsecond, with the same-top-level exception (`reduce.rs` 326 to 354; ea3d1f29, 2026-10-02; e9cd3b3e). Reconciliation across outposts; the exception a patch on it.
- The acceptance relation judged a second time, from the facts the outpost read at the report (`reduce.rs` 232 to 324). Parity port, made twice by design.
- Attention and the foreground move, a windowless focus adopting the attention window, a focus in the system foreground moving attention without a foreground report (`reduce.rs` 851 to 892, 984 to 1018). Root under D14, with two patches for a missing or late foreground fact.
- The background drop across a foreground change (15cae8a8). Reconciliation of cross-process arrival order.
- The top-level change: speech cut, text activity ended, the window as the outermost entered container, entered ancestors, presentability, the entered menu's cut (`reduce.rs` 1020 to 1130, 2343 to 2437). Parity ports.
- Unknown ancestors keeping the previous chain (13dcd2db; 2b13487b). Workaround for a busy application that NVDA meets by blocking.
- The switcher frame's refusal, the switcher item's dropped ancestry, the staging windows cut-and-silent (`applications.rs`; e4b959c3, 48d8432d, 630d6a6e, 1e108473). Parity ports; the frame rule was corrected once from the window's class to the element's own.
- Speech validity, expiry and the menu-item exception (`reduce.rs` 946 to 955, 1132 to 1149). Root: Verbatim queues focus speech and culls it where NVDA cancels at the queue.
- Dead focus and the silent re-read after an incarnation ends (`reduce.rs` 1162 to 1248). Root under D9.
- Change and selection targets against the live focus, with `reported_focused` (`focus_rules.rs` 151 to 260). Parity port; `reported_focused` a workaround for a state change arriving before the slower focus read.
- The fake focus after a menu or the switcher closes, guarded by "unless a focus observed since was applied" (`shell.rs` 170 to 184). Parity port with an ordering guard.
- The foreground window for the keyboard hook (8e27671d). A fourth copy of the foreground in Core.
- Focus-now at start, after a listener replacement, after an outpost replacement (`main.rs` 1375 to 1428). Parity at start; the two replacement cases root under D9.
- `CoreFocus` built and sent from the app, comparing the focus's owner across each input. Reconciliation.

### Counts

About ninety mechanisms. About twenty-five port NVDA's rules and about twenty answer a hard constraint directly; the remaining forty-odd exist so that Verbatim's processes agree, and of those about twenty trace to the two-route structure in particular (the hand-over and its ordering rules, the marker and the old-focus reorder, the gap with its news and evidence classes, the copied verdict, the element found by path, the chain from the listener's read, `CoreFocus` with the held elements and the unjudged queue, the stamps). The patch chains, each on one mechanism and each within six days: the verdict cache's timed-out probe (three commits on 2026-10-25); the gap's drop classes (four on 2026-10-21, one reversing another); the hand-over's run ordering (seven on 2026-10-20 and 2026-10-21); the stamps (two on 2026-10-24, with a stated residual); `CoreFocus` (three on 2026-10-25, plus the null-foreground regression); the listener's out-of-time judging (six on 2026-10-20); the application name (two in two days). Every chain but the last two is a disagreement between two processes' copies of one fact being tuned rather than removed.

## Where state is duplicated, and how the copies disagree

Each item names the copies and gives one sequence from the code in which they differ.

- The focus: Core's `FocusContext` (`state.rs` 17 to 68); the listener's held elements marked waiting, held or done by `CoreFocus` (`focus_read.rs` 571 to 700); the outpost's `Held` with Core's focus, the unjudged reports and another application's observation time (`held.rs` 70 to 200); and the outpost's older `Tracking` (`worker.rs` 279 to 379). Sequence one, a correctness hole: Core holds E from a listener read; an MSAA focus in another application is applied and the notice that E is done is written; before it reaches the listener, E regains the focus and raises a UIA event; the reading thread finds E held and still focused and drops it as a repeat (`focus_read.rs` 1236 to 1323), so Core never hears the focus return. NVDA has no such window because its last queued focus is set synchronously. Sequence two, conservative by design: an outpost reads a state change for a report Core has meanwhile judged stale, and Core drops it; the cost is a read, not speech. Sequence three: `adopt_focus` runs as the report is published, before Core judges it, so an outpost follows and reads the caret of a node Core refused (`worker.rs` 1470 to 1479, 2372). Sequence four: a focus-now answer is pushed to the unjudged queue under the query's trace, and unless Core's notice names that trace it stays until a later judged trace drains past it (`held.rs` 104 to 108; `worker.rs` 4247 to 4257).
- The verdicts: the listener's arbitrator, one per reading thread and started empty (`focus_read.rs` 790), and the outpost's, which copies only the listener's yes (`worker.rs` 3861). Sequence: the listener's probe of a new File Explorer window times out and its focus is dropped as not UIA, which reaches the outpost as inconclusive; the outpost's own probe of the same window says UIA and is kept for life; the outpost drops its `WinEvent` copy and waits a second for a UIA focus the listener has already refused (`focus_read.rs` 455 to 464 names this). The negative verdicts also expire on two clocks, so one process can say MSAA and the other UIA within one event.
- Which applications have hooks in: the listener's hooked map and the supervisor's records (`owner.rs` 1419 to 1425). Sequence: a ready outpost dies; until `HookRemoved` reaches the listener, it forwards none of that application's `WinEvent`s and no hook holds them, so a menu or alert raised then is lost to both routes; the focus is recovered by the replacement's focus-now, the menu is not.
- The gap's "a UIA focus answered it" and the listener's knowledge: matched by observation time within 100 ms across two processes' stamps, and by top-level window for a read but by time alone for a drop (`gap.rs` 27 to 29). Sequence: a `WinEvent` focus in window A opens a gap; the listener's read of a focus in window B of the same application times out and is dropped with focus evidence; the news closes A's gap and nothing is read, though nothing is known of A (`gap.rs` 144 to 152).
- The foreground: Core's attention window, foreground node, latest focus's top level and the hook's foreground view; the supervisor's attention by message; the app's live read for the fake focus; the listener's test at read time; the outpost's test when handled and again when reported. Sequence: a UIA focus the listener accepted with its window in front is handled after Alt+Tab and reported `in_foreground: false`, and Core drops it, where NVDA's one earlier test would have spoken it. A foreground report for the window already holding the focus "changes nothing" yet moves the latest-focus stamp and its outpost (`reduce.rs` 655 to 664 against 856 to 861).
- The hand-over: the outpost's `hook_from` and `forwarded_through` against the listener's tick. If `ForwardingEnded` never arrives, `may_have_twin` is true for the outpost's life and the two maps are never trimmed (`handover.rs` 241 to 243); a listener backlog that replaced a waiting fact leaves a hook copy unmatched and misorders that tick.
- The last reported focus the intake reorders by, which lags a read taken in but not handled (`intake.rs` 136 to 139, stated as a limit).
- The event thread's narrowing pair against the worker's tracked window: a destroyed focus window is forgotten in the window cache but not in the pair (`narrowing.rs` 134 to 141).
- The states last read by the outpost against Core's, when the outbound merge replaces a waiting state change (`outbound.rs` 148 to 163; `held.rs` 197 to 207).
- Readiness known by the owner as it reads `Ready`, by the app when the frame reaches the reducer, and by the control plane.
- The listener's per-thread last focus against Core's: the next walk stops under a focus Core refused, so a container between two focuses can be reported or omitted differently from Core's own ancestry.

Eleven of these exist because one fact is held in two or three processes with no single owner; the gap's look-back, the marker news, the copied verdict, the hand-over's counting and the unjudged queue are each a patch on one of them.

## Which mechanisms exist only because of earlier design choices

The two-route structure itself is forced, as the constraints section says. Within it, these follow from choices rather than constraints:

- The gap's placement in the outpost's worker (decision 11 as amended 2026-10-21). The reason given, that only the worker sees both halves, is not so: the listener's desktop-wide hook receives every focus `WinEvent` and drops a served application's in its backlog (`listener.rs` 227 to 231, `Waiting.hooked`), and its UIA callback receives every UIA focus. The listener is the one process that holds both halves with one clock.
- The hand-over's matching by counting rather than a hold. Counting was chosen so no copy waits; its price is the seven ordering commits, because the two sources' copies arrive interleaved.
- Judging repeats before the read in the judges, which needs Core's focus there (`CoreFocus`, the held elements, the unjudged queue, the trace handshake). The repeat is dropped by Core either way; the pre-read drop saves two to three calls per repeat and costs the notice machinery and the hole above.
- The listener making several calls where one remote operation was one transaction, hence the budget, the deadline's derivation and the out-of-time paths. This is the price of the no-connection rule (2026-10-19 and 2026-10-20), which removed the lock; it is a decided trade and not revisited here.
- The reading threads ended by `HookRemoved` rather than by the application's exit.
- Stamps in two units.
- Core's four notions of the foreground and two of the latest focus, updated at different points of `reduce_event`, with three patches reconciling them.
- The in-ignored-window judgement in the app.

## The design recommended: the present shape with four reconciliations removed

Stated as mechanisms, each with what needs it. Everything not named is unchanged.

### The gap, in the listener

The listener's hook receives a focus `WinEvent` in a window of application A. Its reading thread for A, which already judges A's UIA focuses, records the copy (window, top-level window, time) when A has an outpost with its hook in and the window's verdict, from the class lists, a kept verdict or the raw probe on that thread, is UIA. The record is answered by a UIA focus event of A observed within 100 ms before it or after it whose nearest window shares the copy's top-level window, whichever way that event is judged, except a drop for no window above the sender, which answers it at once with a read; by a `WinEvent` focus or foreground change observed later in another window of A or in another application (the user moved on); and by the listener giving up on A. Unanswered at one second from the copy's observation, the thread sends A's outpost a focus-missing fact naming the window and the copy's time, through the relay like every fact. The outpost handles it as the focus-now read it makes today (`read::focused_control_in`), unless it has reported a focus in that top-level window observed after the copy, and reports what it reads as a UIA focus; Core takes a later UIA report of the same element as a duplicate, as today. The listener holds its idle answer while a record is open, as it holds it behind the menu end's grace.

Needed because UIA delivers late or never (the constraint), placed here because both halves arrive in one process. It handles each case the present gap was built for: late delivery (`conhost_two_windows`, the second console's UIA focus 2.15 s late: the listener's record is unanswered at one second, the outpost reads, the late UIA focus is Core's duplicate); never delivered (`settings_system_page`'s `CoreWindow` with no UIA focus in 14.5 s: the same); the burst of gone senders (a drop for no window answers the record at once with a read, as `NoWindow` does today); a UIA focus in another window of the application (the folder window a new window was opened from: not an answer, as today's top-level rule); the user moving on (another application's or another window's `WinEvent`, which today needs Core's notice of its focus moving elsewhere); the listener giving up on the application (its own knowledge, no wire class); fast navigation's not-focused drops (not an answer, left to the bound, as today's inconclusive class); a worker held by an early connection (no longer relevant, since the listener's clock counts from the copy, which today's design had to re-derive on the outpost's clock). What it removes: `gap.rs`, `MarkerNews`, `next_with_gap`'s wake, `DropEvidence` on the wire and `focus_read::drop_evidence`'s three classes, `Held.elsewhere` and the gap's reading of `CoreFocus`, the 100 ms look-back across two clocks, and the idle hold's two-process bound. What it adds: one record per application in the reading thread's state, one timed wait in the watchdog's existing loop while a record is open (no wake-up while none is), one fact kind, and one local check in the outpost. Cost per focus in steady state: one local `GetAncestor` for the copy's top-level window, as today, now in the listener; a probe only for a window with no verdict, which the listener probes at that window's first UIA focus anyway. One disagreement goes: the two arbitrators deciding one window differently no longer makes a gap wait for a focus the other process refused.

### The hand-over's hold

The outpost holds the focus, foreground, menu, alert and tooltip copies its own hook takes until `ForwardingEnded` arrives, then drops those raised at or before its tick and queues the rest in raise order. The listener's copies are handled as they arrive, as today. Needed because an outpost's hook goes in after its application's first events (the constraint that makes the listener forward at all). It handles the cases the matcher handles: an event raised before the hook (listener copy only, handled once); after the tick (hook copy only, handled once after the hold); in the overlap (listener copy handled, hook copy dropped by its stamp); two identical events in one tick (both from the listener, both handled; the hook's two dropped). What it removes: the two multisets, `TWIN_BOUND`, `ListenerBeforeHook`, `in_raised_order`, the raise-time half of the foreground change's removal of a waiting client focus, and `focus_raised_first`. The limiter's rule then compares arrival order, as NVDA's one hook does. What it costs: the hook's first copies wait for the relay once per outpost start: 0.05 to 0.15 ms at the median, up to 36 ms at the 90th percentile and 146 ms at the maximum measured (focus pipeline section 12.4), and never the first focus, which comes by the listener's copy. A `ForwardingEnded` that never arrives (the listener died) must release the hold: the supervisor's `UiaFocusReadsEnded` already tells every outpost the listener is gone, and releases the hold the same way.

### Repeats judged in Core alone

If Dickson chooses it: the listener reads every UIA focus event that passes NVDA's hung, native, acceptance and keyboard-focus checks, keeping its per-thread last element for `same_as_last` only; each outpost acquires every MSAA focus event that passes acceptance and the focused-state check; Core's duplicate rule, which already drops a focus on the node it holds and keeps the newer reading, is the one repeat rule. The outpost's change and selection targeting, a cost filter, judges against the focus it last reported and reads one change more when Core's focus differs, which Core drops after the read, as it does today for the unjudged window. Needed by nothing but cost; what it removes is `CoreFocus` and its three recipients, `HeldElements`, `Held`'s queue and trace handshake, the "a focus waiting for judgement is no repeat" rule and its tests, the `Reading::abandoned` bookkeeping that keeps a focus for Core's judgement, and the roadmap follow-up on abandoned reads with its approved fix. It fixes the correctness hole above. Its cost: a repeated UIA focus event costs the full read (five calls) where today it costs the comparison and one keyboard-focus read; a repeated MSAA focus costs an acquisition and a states read where today it costs nothing. The count of repeats per real scenario decides it: `focus_read.rs` logs each drop with its reason, so the logs of `notepad_` (the menu bar raises each item twice), `windows_terminal_` (the second identity), `explorer_folder_window` and `rapid_tabbing_in_settings` give the figure without a new build. NVDA's own order is kept for everything but the pre-read repeat drop, which the parity ledger already lists as a deliberate difference out of time.

If Dickson keeps today's exact judging, the hole closes only by not dropping a repeat whose element the notice has not yet confirmed as still Core's, which is the same as the first option for the listener's side; the outpost's side has no such hole, since it drops only what repeats every held focus.

### One time unit

Every observation stamped in microseconds from one clock read, in the listener, the outposts and the app's focus-now answer, with the millisecond field derived from it on the wire and never compared. `observed_no_earlier` then compares one number and is a total order. Removes the mixed case of stamps in two units, which a review left in f2d19387.

### Smaller merges, none urgent

- Core's foreground record: one struct holding the attention window, the foreground node and the latest focus's top level, set at one point of `reduce_event`, so a foreground report that changes nothing changes nothing.
- The listener's reading thread ended by the application's exit, which the supervisor watches, rather than by `HookRemoved`.
- The in-ignored-window drop moved into Core as a rule over the ignored set, so every refusal the judges are told of is Core's.
- The arbitrator's timed-out verdict kept with the entry rather than keyed by thread: the same behaviour as a93e8744, owned by the entry that probed.

### What the recommended design leaves as it is, and why

- The listener reading every UIA focus with calls on held elements, no remote operation and no connection: forced by the element, the registration and the collision constraints and by the no-connection rule.
- The marker and the wait behind it: the one ordering mechanism across the two routes; the old-focus reorder stays with it. Its benefit is unmeasured and should be, by the design's own scenario, before anyone proposes its removal; until then the reasoning of section 13.1 stands (a change raised just after a focus would be judged against the old focus and lost, never repeated).
- The spares, the failure path, held facts, the idle barrier, the single channel, the node-id namespace, speech validity, the dead focus and silent re-read: each is the shape's price, decided and measured.
- The out-of-time judging and the budget: the price of the no-connection rule, decided on 2026-10-20.
- Every parity port, in NVDA's order.

### Costs, together

No call into any application is added on the steady-state focus path. The gap's move adds a probe only where a window's first `WinEvent` focus comes before its first UIA focus and no verdict exists, which the listener would probe at the UIA focus anyway. The hand-over's hold adds up to the relay's latency once per outpost start, on the hook's copies only. Repeats judged in Core alone add one read per repeated focus event, to be counted. Wake-ups: none added; the gap's one wake per open record moves from the worker to the listener's watchdog, and the hand-over's hold adds none. Memory: one record per application in the listener; the hand-over's maps go. Messages: one fact kind added (focus missing), one removed (`CoreFocus`) if repeats move to Core, and the drop message loses its evidence field.

### What must be measured before trusting it

- The relay's latency at the moment of `Ready`, over fifty outpost starts (`return_after_retirement`, `second_application_and_verbatim_menu`, `explorer_folder_window`): the hold's cost is its distribution, and the 146 ms maximum came from the supervisor's owner thread before communication step 6 moved writes to the producing thread, so it may be lower now.
- The repeat count per scenario from the listener's and outposts' debug logs, as the cost of judging repeats in Core alone.
- The gap in the listener against the three live cases (`conhost_two_windows` ten times for the late console; `settings_system_page` and `explorer_folder_window` ten times; `measure_focus_explorer` for the folder window taking the focus back), counting reads made, reads in vain and focuses announced once, against the figures in the step 12 results of the listener design.
- The marker's benefit: build `text_box_first_character_after_tab` as section 14 specifies and run it with the marker and, in a scratch build, without, so the decision to keep it rests on a count.
- The listener's reading thread wake-ups with the timed wait in place, over the Notepad menu bar trace of the listener design's section 8, to show none is added while no record is open.

## Comparison

- Today's design: meets every requirement; about ninety mechanisms, half of them reconciling three copies of the focus and two routes; seven patch chains in six days, each tuning a disagreement between copies; one correctness hole in the repeat drop; two clocks and four drop classes on the wire for one wait.
- A one-process host or a listener holding every UIA registration: rejected by Dickson on 2026-10-20 on measured grounds (the hung-window hold on every application's UIA events; blast radius), not reopened here, since the study found no new evidence.
- The outposts holding the focus registration: ruled out by E-cost and the hung-window hold on every outpost.
- The present shape with the four removals above: the same requirements met, about twenty fewer mechanisms, three of the eleven disagreements gone (the gap's, the verdicts' effect on the gap, and the repeat hole), one wire protocol simpler, and the remaining reconciliations each the only one of its kind: the marker for order, the hold for the hand-over, the stamps for staleness, the node-id namespace for incarnations.

Why no smaller design exists: the constraints section shows the two routes are forced; given two routes, an application's events need an order (the marker), a new outpost needs its first events (the hand-over), a missed UIA focus needs a recovery (the gap), and several outposts need a common order at Core (the stamps). Each of those four is kept, once, in the one process that holds its evidence. Everything else that reconciles processes exists to save a call before a read, and that is the trade Dickson is asked to decide.
