# The outpost's reducer build

Status: built in two builds, the store build and the reducer build, with the departures and open parts listed below; designed on 2026-10-31 against main at 98b0388d, with its adversarial review's findings folded in the same day (verdict: achievable with fixes). It applies [the ownership model](ownership.md)'s rules "Pure reducers, with calls as effects", "No state lent across a call into an application", "Nodes are released by message position" and "Queries have exactly one answer" to the outposts; its brief, mechanism draft and known traps are "The outpost's reducer build" in [the ownership builds](ownership-builds.md). The refactor preserves what is heard: this build changes no speech. It undoes recently finished work in two places, both by design: the commit point and its lock order (`docs/design/focus-pipeline-rework.md`, "The outpost's commit point"), and the hand-over's counting (`outpost/handover.rs`), which the hold Dickson decided on 2026-10-25 replaces. The output build runs beside it and shares no code.

Paths below are under `crates/verbatim-outpost/src/outpost/` unless named in full.

## Feasibility

The goal is achievable against the current code. No sequence was found in which a pure owner on its own thread must wait on a call, hold an application's object, or read a fact another thread writes mid-transition. Five facts of the code change the draft, and the design takes each into account:

- Intake entries hold application objects: a UIA event's element as an agile reference (`intake.rs` lines 242 to 252) and an active text position's range (line 223). The reducer may hold none, so the shell parks each in the store under a key at push time, and the entry the reducer sees carries the key.
- A caret read decides mid-read whether to compare in general terms by syncing with the `WinEvent` thread and looking for a waiting value change in the intake (`text_reads.rs` lines 666 to 672 and 732 to 768), with the edit anchors' guard held across the sync. In the reducer the waiting entries are the reducer's own, so the check becomes the reducer's, made on the read's result ("The value change check").
- The arbitrator's verdicts are read mid-walk and recorded by the worker (`read.rs` lines 272 to 281), and read by the event thread before it queues an event (`outpost/mod.rs` lines 760 to 776). They become reducer state, given to each program and to the event thread as an immutable snapshot ("Verdicts").
- Neither registry has an outer lock or a generation in its ids (`crates/verbatim-uia/src/registry.rs` line 71, `crates/verbatim-ia2/src/registry.rs` line 122), no handle-to-node map exists (plain windows are MSAA keys `(hwnd, OBJID_WINDOW, 0)`, `read.rs` lines 633 to 635), and the watchdog mints under the watch lock at the bound (`worker.rs` line 5059, `bound.rs` line 117). The store is new, not a move.
- Text objects are dropped under the four text locks (`text_reads.rs` lines 985 to 996, `crates/verbatim-outpost/src/text/mod.rs` lines 692 to 700), and the anchors' guards are held across application calls in nine places (`text_reads.rs` lines 387 to 445, 508 to 527, 628 to 677, 863 to 870). The store ends both.

## The build splits in two

This is the largest build, and it splits cleanly along one seam, the store's API, which neither half changes for the other. Recommended (the first open question):

- The store build: the object store, owning both registries, the handle map, the text objects and the parked event objects; the interning trait in `verbatim-uia` and `verbatim-ia2`; mint tags; `Recorded`; release in the store; text calls on clones. It runs on today's worker and watchdog, the worker in charge invoking the release as today. Its tests are store unit tests and the two release tests of the brief.
- The reducer build: the reducer thread and its mailbox, call programs, the timers, the hold, reply tokens, intake planning in the reducer, the ownership check and the replay tests.

The store build leaves two things for the reducer build to rework: the release's caller (`worker.rs` line 2421), a few lines, and the text records' API. `NodeText` and `KeptSelection` mix the reducer's facts (offsets, the kept selection's ends as positions, the note, the remembered reports) with provider ranges, so the store build moves the ranges behind the store with today's types, and the reducer build divides those types, the facts into reducer state and the ranges into the store under keys. The reverse order would build programs against registries that then change. The rest of this document is one design for both; each section says which half it belongs to.

## Terms

- The reducer: the outpost's pure state machine (`reducer`, planned), inputs in, effects out.
- The shell: the code on the reducer's thread that drains the mailbox, runs a transition, and applies its effects.
- The mailbox: today's intake reduced to an append-only queue under one lock, which every producer appends to and only the shell drains.
- A call program: a whole sequence of calls into the application with pure stopping rules, run by a worker on copies, whose result is posted to the mailbox.
- A mint tag: the worker generation and request a node or anchor was minted or looked up under.
- The store: `store` (planned), a monitor owning every object of the application the outpost keeps.

## The current code

- One worker thread plans, judges and calls (`worker.rs` lines 822 to 862, `run_entry` 891 to 974), reading and writing shared state between calls: the watch's follow state (`WatchState`, lines 105 to 127, under `Watch::state`), `held` (`outpost/mod.rs` line 201), the arbitrator (line 193), the text records (lines 232 to 243), the hand-over (line 273) and the intake's own state (`intake.rs` lines 506 to 541).
- The intake's state has several writers: pushers on the event thread, UIA's callback threads, the I/O thread and the gap timer's threadpool thread; the worker; and the watchdog, through a publication's `adopt` (`commit.rs` line 605). Its push-time drops read two flags the worker writes (`drops_on_arrival`, `intake.rs` lines 550 to 563), and `old_focus_events_first` reads `focused`, which the worker or the watchdog writes.
- The gap record lives in the intake under its lock (`intake.rs` line 540), written by the I/O thread's push (`apply_gap_outcome`, lines 642 to 683), the worker (`gap_copy_dropped`, `gap_accepted`, `gap_bound_reached`, `gap_window_destroyed`) and timed by a threadpool timer (`gap_timer.rs` lines 75 and 162).
- The watchdog abandons and publishes under the watch lock (`wake`, `worker.rs` lines 740 to 798); every change of follow state is a commit under that lock, with the judgement list, intake, outbound queue and both registries nested inside it.
- The hand-over counts the two sources' copies under its own lock, taken by the event thread and the I/O thread, and pushes to the intake while holding it (`outpost/mod.rs` lines 881 to 903; `handover.rs` lines 111 to 127, `TWIN_BOUND` line 80).
- A query is answered from the watch's `running` slot (`worker.rs` line 109) by whichever of the worker, the watchdog (line 699) and the I/O thread (`NotStarted`, `outpost/mod.rs` lines 1516 to 1527) comes first.
- The release (`worker.rs` lines 2421 to 2494) takes the watch lock, the outbound backlog lock and each registry's lock inside it, then the text locks one after another.
- The console caret poll makes one worker entry per 5 ms tick (`Item::Wake`, `worker.rs` lines 1535 to 1588), handed out only when nothing else is ready (`intake.rs` lines 984 to 989), so waiting entries go first.

## The model this build applies

### The store (the store build)

`store` owns, as private fields, the UIA and MSAA registries (each keeps its own `Inner`, and the store is the only holder of a handle to it), a handle map from window handle to the node minted for it, the text objects (each node's patterns and anchors' provider ranges), the terminals' kept ranges, the parked event objects, and each node's and anchor's mint tag. Reason: identity and its release are one fact, so they have one owner.

- Its API returns owned clones of agile references and owned removed objects, never a guard, and takes no closure. The registries' `retain` with a closure is called only inside it. Reason: a caller can then never hold the store's lock across a call (d0ec3a89).
- The walks in `verbatim-uia` (`shape/shapes.rs` line 698, `client.rs` line 1107), `verbatim-uia-rops` (`tree.rs` line 48) and `verbatim-ia2` (`acquire.rs` line 2271, `equality.rs` lines 129 and 199) take an interning trait each crate defines, implemented by the store's handle for one program, which carries that program's mint tag. Every mint and every look-up through it tags the node, so `existing_id`'s results and spliced entries are tagged as mints are.
- Plain windows and a foreground's client object mint through the handle map: a plain window keeps today's MSAA key, so ids do not change; the map only lets the reducer's local reads mint by handle at the bound.
- A message that carries nodes is built from a `Recorded`, whose constructor is private to the store's module. `Recorded` lists the nodes and anchors the snapshot names, which the outbound queue records at the message's position as it numbers it at write (`outbound.rs` `number`, line 147), replacing `take_touched` (`worker.rs` line 534) and the registries' `touched` lists.
- Release (`Store::release`, called with Core's held lists, the acknowledged position and the live mint tags): under the outbound backlog lock and then the store's lock, the one nesting in the outpost, it frees a node or anchor only when no live request can still name it: its tag names a dead worker generation or a claimed or ended request, and either it was never carried, or it was carried at or before the acknowledged position, no waiting message carries it and Core does not hold it. A live tag keeps a node whether or not it was carried. It returns the released ids and the removed objects, owned. Reason: a node can be named by Core, by a waiting message, or by a request still running, and the release must ask all three; carried positions, waiting messages and tags are read in one hold, so no message can be queued between the decision and the removal.
- A program's copied lines are tagged too. When the reducer dispatches a program, the store re-tags every node of the lines and stop windows it copies with that program's request, so a walk that stops at a copied entry by its MSAA key or by equality never names a node a release freed meanwhile. Today the worker in charge releases between entries (`worker.rs` line 2421), which gives the same protection by order.
- The release's result is a type property: `Store::release` returns the removed objects owned, and no store method drops one, so nothing is dropped under the store's or the backlog's lock.
- `forget_window` and the text caches no longer drop objects under a lock: they are returned like the rest.
- The focus-following subscription keeps its own clones of the elements it is given (`commit.rs` lines 484 to 506), so a move releases the clones of the elements it no longer follows on the subscription's thread, and the store's copies are released only by the release rule.
- A text call takes clones of the node's pattern and anchors' ranges, runs, and its result's transition records new anchors and the kept selection. That ends the anchors' guards held across calls and 5797c075's workaround.

### The reducer and its thread (the reducer build)

`reducer` (planned) holds every remembered fact of the application, with no Windows type: the intake's order (markers, the limiter, the newest-first runs, the menu opening last, `caret_after_update`); the gap record and its timer; the verdicts; `held`; the follow state (`Tracking`, the followed and kept groups by node, owed work, the caret watch, the caret-read stamp, the focus path, the terminals' models); the hand-over's hold; the worker generation in charge, its deadline and the moved-on grace; the request records; and text support per node. Inputs: mailbox entries in mailbox order, program results tagged by generation and request, timer expiries, and Core's commands. Effects: dispatch a program, send a message, set or cancel a timer, release, move the focus-following subscription, nudge the event thread, park or drop parked objects, answer a query, and publish the push-time copies.

Local reads that never wait on an application (`GetForegroundWindow`, `GetAncestor`, `InternalGetWindowText`, `IsHungAppWindow`, the window's thread and process) and minting a plain window by handle reach the reducer through an injected trait, `LocalWindows` (planned), as `WindowQueries` reaches the acceptance test today (`acceptance.rs`), implemented live by the shell and by a recording in tests.

The thread. The shell waits on the mailbox's alarm (`alarm.rs`, an event and a high-resolution timer) set to the reducer's earliest timer: the event deadline, the moved-on grace, the walk's bound, the gap's bound and the caret watch's bound. So the watchdog thread and the gap's threadpool timer are removed: the reducer never blocks, so it can keep its own time, and each timer reaches it with one wake where the watchdog took two (its own, then a post). This departs from the draft, which kept a watchdog posting timer inputs (the second open question). A foreground fact, which today wakes the watchdog to judge the grace again (`worker.rs` line 240), is an ordinary input. The rule that lets the reducer keep time: every lock the reducer's thread takes (the mailbox's, the backlog's with the store's inside it for a release, the progress slot's) is a short hold with no call into an application and no wait inside it, so a timer is never late by more than such a hold.

The mailbox. The event thread, UIA's callbacks, the I/O thread and the workers only append, under its lock, reading the entry's window and thread with local calls before taking it as today. The push-time drops stay at push, on copies the shell writes only from reducer effects, under the same lock: the absorbed console window, `system_caret_unused`, and the verdict snapshot for the event thread's drop of an MSAA event of a known UIA window. An entry's application object (a UIA event's element, an active text position's range) is parked in the store only after the push-time drops have judged the entry, so a dropped entry parks nothing; an entry the limiter or a keyed replacement later removes has its parked object dropped by a worker, through a release program, never on the reducer's thread. Everything else of today's push (the marker's place, the gap outcome, keyed replacement, `remove_client_focus`, `old_focus_events_first`) moves into the reducer's transition on the drained entry, in the same order. Results are appended to the same queue, so a result comes after every event pushed before it.

What it removes: the watch lock, the commit's nested locks and their lock order, the watchdog's publication under a lock, `running`, and the gap record's three writers.

### Workers and call programs (the reducer build)

One worker generation is in charge; it takes one program at a time from its inbox. A program receives copies of what it reads: the held reports' lines and stop windows, the focus path, the verdict snapshot (an `Arc` the reducer replaces on change, so a copy costs a reference count), the fetches, and the store handle with its mint tag. The programs are today's handlers cut where the worker reads shared state or decides between entries:

- An event's handling (`Worker::msaa_event`, `uia_event`, `fact`, `alert`, `tooltip_shown`): acceptance, the object's identification, and the named property's read.
- A focus (`msaa_focus`, `uia_focus`, `foreground`, `menu_opened`): acquisition, the focused-state test, the walk, its caret read, and finding its own copy. One program, since every decision inside depends only on its copies and its calls. A run tried newest first is one program per try; the reducer dispatches the next try on a result that reported nothing.
- The gap's read, a focus-now answer, a query, a terminal read, a text operation, a caret key's first check, a probe, the owed work, the console caret poll, and a release, which drops removed objects.

Each makes exactly the calls today's code makes, so every `call_counts` test passes unchanged. The program commits each verdict it probes as the probe returns and uses it itself for the rest of its walk; the reducer records it only from the generation in charge, but a probe that timed out and a destroyed window's forgetting from any generation, as the arbitrator recorded them before this build ("Verdicts").

The walk's progress stays a slot (`FocusProgress`, `bound.rs` line 57) written by the worker between calls under a lock held for one copy. Its focus's object (`BoundFocus::object`, line 47) is parked in the store when the walk is armed, and the slot keeps its key, so the copy the reducer takes holds no application object. At the bound the shell copies it into the timer's input; the reducer makes the chain from it (`chain_at_bound`, completing with plain windows through `LocalWindows`), publishes, and marks the request claimed. A result of a claimed request or a dead generation changes nothing but its mints, which the store frees by their tag.

Abandonment is the reducer's transition on the deadline or the grace: it publishes an armed walk first in the same transition, marks the generation dead, answers the query "abandoned", spawns a replacement through an effect, and hands it the owed work as its first program, as today (`worker.rs` lines 749 to 798, 980 to 990). A returning abandoned worker posts that it returned, and the reducer lowers the abandoned count, which the shell publishes to the atomic the I/O thread reads for `Pong`.

### The value change check (the reducer build)

The edit control's caret read becomes two parts. The program reads the caret and then syncs with the `WinEvent` thread (`Nudger::sync`, `event_thread.rs` lines 229 to 247), as today, without the anchors' guard. Every value change the control raised before the sync's marker was pushed by the hook's callback before the sync returned, and so precedes the result in the mailbox. The reducer's transition on the result looks for a waiting value change of that edit among its own undispatched entries, exactly the set `value_change_waiting` reads today, and compares in general terms when one waits. The cut is fixed by the code: in `text::caret_report_checking` the check is asked lazily, inside `content_changed` (`crates/verbatim-outpost/src/text/mod.rs` lines 1357 to 1374, called at 1299 to 1306), and only after it come the comparison's calls, `general_changes` or `selection_changes` (lines 1307 to 1322), and `report_for` (line 1323). So the first program reads the caret and returns its state as offsets; when the comparison needs the check, the reducer decides it and dispatches a second program that makes the comparison and `report_for`; a caret move with no selection before or after asks nothing and stays one program. The calls are today's; the extra hop, two wakes, is paid only for a selection in an edit control, and no object crosses, only offsets and the store's keys.

### Verdicts (the reducer build)

The arbitrator's record moves into the reducer; the class lists, the override and the clocks stay configuration until M5. The reducer hands every program, and the event thread through the push-time copies, an immutable snapshot, an `Arc` it replaces on change. A program commits each probe as soon as it returns, alone in its commit, and applies it to its own copy, so a timed-out probe stays recorded when its worker is abandoned later. The reducer records a probe only from the generation in charge, but a timed-out probe and a destroyed window's forgetting from any generation, so a window is not probed again more often than NVDA probes it, which is what the arbitrator did before; the shell therefore passes a commit's verdicts on even when it applies nothing else of the commit. Expiry is still judged at observation times in microseconds, each program judging at its entry's observation. The event thread's `EventWindows` keeps asking until `lasting_verdict` as before; a verdict a program probed reaches it one transition later, which only lets a few more MSAA events through to be dropped by the program. The event thread's narrowing nudge, which the worker sent when it kept a UIA verdict, is a reducer effect the shell applies after replacing the push-time copy. The mechanism is in [the crate guide](../crates/verbatim-outpost.md), "The arbitrator".

### The hand-over's hold (the reducer build)

The reducer holds the hook's whole stream from the hook's install until `ForwardingEnded`, as Dickson's design of 2026-10-25 says, and handles the listener's copies as they come. At `ForwardingEnded` it drops the held copies of the forwarded kinds raised at or before the named tick, since the listener forwarded exactly those (it filters facts raised after its tick, `crates/verbatim-outpost/src/listener.rs` lines 268 to 276), keeping the earlier tick when a replacement listener sends another, and releases the rest in the hook's order as one batch, planned as today's batch is, newest first, ahead of anything the hook delivers later. If the listener's reads end first (`UiaFocusReadsEnded`), the hold is released, dropping held copies raised at or before the newest listener copy handled. An outpost with no listener (`outpost/mod.rs` line 1939) starts with nothing held. Reasons: holding the whole stream keeps the hook's own order, which `old_focus_events_first`, `caret_after_update` and the caret watch read, where holding only the forwarded kinds would let a caret or value change pass a focus raised before it; and the cost is paid only at an outpost's start.

`in_raised_order` keeps ordering a run by raise time, but its tie-break by the hand-over's `Arrival` (with `Arrival::ListenerBeforeHook`, `handover.rs` lines 102 and 161) goes: a released batch holds only the hook's copies, so ties keep the hook's delivery order, and a listener copy is handled when it arrives, before the batch. `TWIN_BOUND`, the multisets, `Arrival` and the hand-over's lock go.

### The caret watch and the console poll (the reducer build)

The watch is reducer state; a caret event, text change or selection change of the watched node dispatches today's check (`check_open_watch`, `worker.rs` line 1425). The console poll is one program that loops on the worker's own high-resolution wait (`caret_poll.rs`, `INTERVAL` 5 ms, `BOUND` 50 ms), one wake per tick as today, applying the evidence check, a pure function, to a copy of the pinned baseline after each read. Before each read it looks at its inbox: a waiting program other than a release makes it return at once with a yield carrying its state, and the reducer dispatches it again after that program, so a newer key's read goes first as it does today. A release waits, at most the poll's 50 ms. Every program dispatched while the poll waits rings the worker's inbox (`inbox.rs`), a caret event of the watched console included, so the poll yields at once and the event's own check reads at once, as today's event makes its own check at once (`check_open_watch`). This departs from the draft, which had the poll's next tick serve as that check, a delay of up to 5 ms. A newer key's request ends the old watch, answered `WatchEnded` through its reply token, and the old poll is not dispatched again. The reducer keeps the poll in a slot of its own beside the program in flight; the waiting program's deadline starts when the poll ends, and the worker applies the pure part of the poll's commits to that program's copies, since the reducer may have dispatched it before taking them in. An abandonment while the poll is in flight ends its watch answering `Abandoned` and dispatches the waiting program again to the replacement.

### Reply tokens (the reducer build)

Every query the outpost takes gets one answer from a token held in the shell's request table, keyed by request id, never in the reducer's state (Dickson accepted this on 2026-10-31). The reducer emits the answer as an effect: the reply, `Abandoned` at the deadline, `Failed` for a program that panicked, `NotStarted` for a `Cancel` that arrived before dispatch, `WatchEnded` for a caret watch. The token's `Drop` answers "gone", the model's one outcome for a responder dropped unanswered, which on the outpost's wire is `QueryOutcome::Failed`, since the outpost's process lives; the request table's own "gone" at an `Ended` stays the app's. The I/O thread no longer answers `Cancel` itself: it appends it, and the reducer withdraws the query if it has not dispatched it. In debug builds the shell asserts after each transition that the tokens it holds are exactly the requests the reducer still records; replay tests assert one answer per query.

### The ownership check (the reducer build)

The pure rule of `xtask/src/ownership.rs` gains the `reducer` directory, every file in it, with the allow-list of the draft (the module's own items, `verbatim_model`, `verbatim_core::focus_rules`, `std::collections`, `std::cmp`, `std::mem`, `std::sync::Arc`, `std::time::Duration`), skipping `#[cfg(test)]` modules. The check today reads one file per entry (lines 37 to 71), so it learns to read a directory.

## Where each part runs

- The reducer thread: the shell, the reducer's transitions, its timers, the request table's tokens, the release's decision in the store, and every message but pongs.
- A worker: one program at a time; the console poll's wait; dropping removed objects. Abandoned workers finish their call and exit.
- The event thread: the hook's callbacks, which append; the narrowing; answering a sync.
- UIA's callback threads: append, parking their element in the store.
- The I/O thread: decodes Core's commands and appends them; answers pings from the published abandoned count.
- The focus-following subscription's own thread: moves groups as the shell asks, as today.

## Edge cases and what is heard

All as today; the cases below are those whose path changes.

- An MSAA focus in a hung window whose walk passes its bound: the reducer publishes at the bound timer from the slot; Core speaks the focus with the ancestors read and plain windows above, as today. When the walk returns, nothing more is sent.
- The same worker is then abandoned at 10 s: the walk was already published, so the abandonment answers no query and nothing is heard; the replacement attempts owed work first.
- A text query to a hung control: "abandoned" at its deadline; Core says nothing, as today.
- `NodesHeld` arrives while a focus walk runs and acknowledges the report whose line the walk stops at: the dispatch tagged that line's nodes with the walk's request, so they are kept; the focus is spoken with that ancestor as today.
- A key in the console host, then a second key 10 ms later while the first's poll waits: the poll yields, the second key's read runs, the first's watch ends `WatchEnded` (silent); the second key's line is spoken, as today.
- An outpost starting while the user moves focus: the listener's copies are spoken as they come; the hook's copies after the tick are released at `ForwardingEnded` as one batch tried newest first, so the newest focus is spoken, as today.
- A Windows Forms text box: a typed character over a selection says "selection removed", as today (the value change check).
- A `Release` that hangs: the release program is abandoned at its deadline, the programs behind it go to the replacement, and nothing is heard.

## Sequences that would break it

- A release frees a node a result still in the mailbox carries: a walk looks up W, posts its result; `NodesHeld` acknowledging W's last carrier is drained first; the store frees W; the reducer sends the focus naming W, and Core holds a dead id. Prevented: the look-up tagged W with the walk's request, and a live tag keeps a node whether or not it was carried; the request stays live until the reducer has queued the message (and so recorded W waiting) and only then marks it claimed, in that order within one effect application.
- A running program's copied line names a freed node: a walk is dispatched with a held line containing A; `NodesHeld` omits A and acknowledges A's last carrier; A is freed; the walk stops at A by its MSAA key or by equality and names it. Prevented: at dispatch every node of the copied lines is tagged with the program's request, so A is kept until the request is claimed or ended.
- A release decided between a message's queuing and its numbering. Prevented: the release reads waiting messages under the backlog lock, inside which the store's lock is taken, the one nesting.
- A worker abandoned during a walk keeps minting after its replacement started. Prevented: its tag names a dead generation, so the next release frees what it minted, and its result is discarded.
- The bound's publication and the walk's own result both reporting the focus (2bd69a85, c47d57ae). Prevented: both are inputs of one thread; the first claims the request, the second is discarded.
- A program deciding on a copy that a later transition changed. Prevented: one program is dispatched at a time, and the only transitions that change what a running program copied are a publication at its bound, which claims its request and discards its result, and a release, which cannot free a node the program copied or touched while its request is live; the poll is the one program running while another waits, and it yields before its next read.
- An object dropped on the reducer's thread or under a lock, a call that can hang. Prevented: the reducer holds no object; the store returns removed objects owned; the shell hands them to a worker's release program.
- A worker blocking on the reducer, or the reducer on a worker. Prevented: workers only append; the reducer only appends to inboxes; the sync is the worker's wait on the event thread, which never waits on either.
- A push-time drop judged on a copy older than the transition that set it: an event pushed between the transition and the copy's write is judged by the old copy, as an event pushed before today's commit is. Not a new window.
- The value change raised after the sync's marker. Not waited for, as today.
- Released ids used after release: the shell applies the released ids to the reducer as an input before draining the next mailbox entry, so no dispatch names one.
- A query answered twice, by the deadline and by a late result. Prevented: the token is consumed by the first answer; the late result's request is no longer recorded.

## The facts and their owners

- Every remembered fact of the application: the reducer.
- Node identity, application objects, mint tags, carried positions' release: the store, a monitor with four writers behind its one lock, each through its own operations: a worker's program (minting and looking up through the interning trait), the reducer's shell (tagging copied lines at dispatch, claiming requests, releasing), UIA's callback threads and the event thread (parking an entry's object), and the outbound queue (recording carried positions at numbering, inside the backlog lock the release also takes). None holds the lock across a call.
- The walk's progress: the worker in charge, a view the reducer copies at the bound.
- A query's answer: the shell's token.
- The abandoned count: the reducer, published to an atomic.

Facts left with two writers: the fetch configuration and the backend override, written by the I/O thread into the context and read by programs and the event thread, configuration whose version Dickson put in M5.

## Limitations

- A dead generation's mints wait for the next `NodesHeld`, at most the relay's 3-second ping.
- At an outpost's start, the hook's whole stream waits for `ForwardingEnded`: median 20.6 ms, at most 591.3 ms over 126 starts ([the remaining builds](focus-pipeline-rework-remaining.md), "The hand-over's hold"). A hold whose `ForwardingEnded` never comes lasts until the listener's reads end.
- A release waiting behind the console poll waits up to 50 ms.
- The purity check reads text; replay is the real test.

## Cost

- Wake-ups: an event that makes calls wakes the reducer, then the worker, then the reducer (three, where the worker alone woke once). An event the reducer drops, such as one refused by acceptance, wakes only the reducer. Each further program of one entry (a run's next try, the value change check's second part) adds two. Each timer costs one wake, one fewer than the watchdog's. The poll's ticks wake only the worker, as today.
- Latency: two thread hand-offs per program, each a `SetEvent` and a wait's return, expected at tens of microseconds; the builder measures them on `measure_focus_explorer` and `measure_keys_conhost` and states them in `docs/performance.md`.
- Processor time: per program, a copy of the reports' lines (reference counts for the shared pieces) and two context switches.
- Memory: one more thread's stack; the parked objects, each held only until its entry is handled.
- No new UIA call and no new sequence of them: every program makes today's calls, which already use remote operations where they help.
- Measured, at most 20 runs each: `measure_focus_explorer`, `measure_focus_text_box`, `measure_keys_windows_terminal`, `measure_keys_conhost`, and the terminal baseline of 2026-10-23, before and after, for wake-ups, processor time and latency.

## Tests

Every existing intake, worker, gap, commit and mockapp test keeps what it asserts; the unit tests move onto the reducer's inputs.

### Added

The store build: a walk in flight crossing a `NodesHeld` keeps its tagged nodes, carried before or not; a node of a running program's copied line is kept across a `NodesHeld` that omits it; a dead generation's nodes are freed; a spliced or looked-up node is recorded as carried. That nothing is dropped under a lock is the release's type, not a test.

The reducer build, as replay tests on recorded inputs and a recorded `LocalWindows`:

- the marker against the copy; the copy against the gap's timer (f2f8209a) and the gap's fast failure (e02c7854);
- a foreground change against a focus in one batch;
- a call returning before and after its bound, and its abandonment;
- one test for each round on the publication: c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7;
- 463f2ef9's three sequences; the poll yielding to a waiting caret read, ending on its check, and not yielding to a release;
- an abandoned worker inside a text call, then a caret read that does not wait;
- a focus group's older entries kept across an abandoned worker;
- the value change check, with the value change ahead of the result and behind it;
- the hold: held copies at the tick dropped, the rest released as one batch newest first, and the release at the end of the listener's reads;
- each query answered once, for each ending.

### Removed, each once its replacement passes

- The five mockapp tests of `crates/mockapp/tests/focus_reports.rs` the brief names (lines 1080, 1139, 1426, 1950 and 2020), which wait on the real one-second bound or race real orderings, become replay tests with injected timers, keeping one mockapp test per path into a real provider.
- 6dbcca25's hand-driven unit test with two 10-second deadlines becomes a plain replay test.
- `handover.rs`'s ten tests become the hold's replay tests; `gap_record.rs`'s ten move onto the reducer's inputs; the watchdog's and commit's tests in `worker.rs` (from line 5382) become replay tests of the deadline, bound and result inputs.

### Scenarios

The full suite once, then the `windows_terminal_` and `conhost_` scenarios, `text_box_editing`, `notepad_editing`, `outpost_crash_recovery` and `second_application_and_verbatim_menu`, three runs each. The store build alone: `explorer_folder_window`, `settings_system_page`, `text_box_editing`, `notepad_review_cursor`.

## The known traps, against the code

- Review 3, finding 6: the hand-over is placed once, here.
- Review 1, finding 6: identity stays in the walk, through the store's interning trait; the bound publishes from the slot.
- Review 1, finding 7: the poll is one yielding program, with one wake per tick.
- Review 1, finding 9: the store has no guard or closure in its API, so no call wrapper is needed.
- Review 2, finding 3: mints are tagged by generation and request, and a look-up re-tags.
- Review 2, finding 4: checked against the code: the registries are bare, ids carry no generation at mint, and no handle map existed; the store is built as above.
- Review 2, nits: positions numbered at write, `Recorded` marked then; a release runs under a deadline; the poll's high-resolution wait and old key's answer; local reads through `LocalWindows`.

The builder updates `docs/crates/verbatim-outpost.md`, `docs/crates/verbatim-uia.md` and `docs/crates/verbatim-ia2.md`, and `docs/performance.md` with the hop's cost.

## Departures in the store build, pending Dickson's acceptance

- The parked event objects are not built in the store build: their lifecycle, parked after the push-time drops and dropped by a release program, needs the reducer build's mailbox, so they come with it.
- There is no separate map from window handle to node: plain windows and a foreground's client object mint by handle through their MSAA keys (`Nodes::window`, `Nodes::client`), which the MSAA registry's key map already resolves, so ids do not change.

## Departures in the reducer build, pending Dickson's acceptance

The mechanism as built is described in [the crate guide](../crates/verbatim-outpost.md), "Inside an outpost".

- A program posts each commit as it makes it, applying its pure part to its own copies with the reducer's own function, rather than one result at its end, so a focus's report is sent as soon as it is read, as before; the reducer applies each only while the program's request is live.
- The owed work and the gap's read a dropped copy asks for are programs of their own, which the reducer dispatches before anything else, rather than calls at the end of the program that owed them: the reducer then decides between them and the next entry, and each costs two more wakes.
- A worker generation's charge (`worker::Charge`) holds the walk's progress slot and whether the generation was abandoned, under one lock: a walk's step, a publication's copy and a program's text write to the store, made under it with the store's lock inside, judge liveness in the same hold, as the watch lock did.
- What the shell lets go of, an element no longer followed, a parked object never taken, what a publication's lookups replaced, is dropped by the worker before its next program, under that program's deadline; a `NodesHeld`'s release is a drop program of its own.
- A caret key's check counts a waiting caret event from a copy of the waiting caret events made as its program is dispatched.
- A focus walk's object (`BoundFocus::object`) is its event address or runtime id, no application object, so nothing is parked for it.

Not built in this build, and open:

- The value change check's cut into two programs. A caret read's comparison asks, after the `WinEvent` thread's sync, the program's copy of the waiting value changes and the mailbox's record of those it took or holds since the program was dispatched, which is exact and adds no hop.
- The division of `NodeText` and `KeptSelection` between reducer and store: the text records stay in the store, written while a program's request is live.
- The removal of the five mockapp tests the brief names, which wait for their replay tests' review.

## Open questions for Dickson

- Split into the store build and the reducer build, in that order? Recommended: yes; the seam is the store's API, and each half is reviewable whole.
- Fold the watchdog and the gap's threadpool timer into the reducer's own timed wait, rather than keep a watchdog that posts timer inputs as the draft says? Recommended: fold; the reducer never blocks, every lock it takes is a short hold with no call, and each timer costs one wake instead of two.
- Hold the hook's whole stream until `ForwardingEnded`, as your design of 2026-10-25 says, rather than only the kinds the listener forwards? Recommended: the whole stream, since holding only those kinds would reorder the hook's own stream, which `old_focus_events_first`, `caret_after_update` and the caret watch read; the cost is paid only at an outpost's start, a median of 20.6 ms.
