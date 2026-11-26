# ADR 0006: One owner for every fact

Status: decided, 2026-10-30, by Dickson; the builders' departures were accepted on 2026-10-31. [Ownership](../../explanation/design/ownership.md) explains how the rules hold today, and the crate guides it links hold the mechanisms. [ADR 0003](0003-readers-tell-core-asks.md) later moved every judgement of an event into Core, which these rules made possible; where the two differ, ADR 0003 wins.

## The problem

Most defects of the focus pipeline rework had one cause: a fact about one application was decided in two or three processes, and a decision waited on a message from another process arriving first. Two kinds of message let the class grow back after each fix: a message carrying Core's state to a reader for it to judge against, and a reader skipping work on a judgement only Core can make. Inside an outpost, the same class arose between threads sharing state under locks. The aim was a model under which such races are impossible by construction where they can be, and confined to one named owner where they cannot, for all of Verbatim and its planned milestones. A change was in scope only when it pointed to a concrete past defect, or showed a sequence that would go wrong in the code or in a planned milestone.

The processes stay split as they are: a UI Automation call cannot be cancelled, so only process exit reclaims a stuck thread (D9); a UI Automation element cannot leave the process that received its event; and the desktop-wide focus registration stalls with hung windows, so only the listener holds it.

## Decisions

Each rule is decided with its reason (Dickson, 2026-10-30).

- One owner per fact. Every fact has one writer, one thread or one state machine run by one thread; others learn it from one-way reports or from a view. A view inside one process is allowed only when it is ordered with what it judges, keyed by a fact its reader observes, or, for lifecycle only, a lagging copy whose staleness costs no word. A monitor, one module whose every operation runs under its own private lock, never held across a call, never exposing its guard and never running a caller's code, is one owner too. Reason: every reconciliation between two writers had to be patched again for each new sequence.
- Pure reducers, with calls as effects. Every owner whose decisions depend on order is a pure state machine: inputs in, effects out, no clock, no call, no blocking; local reads of Windows' own state come through an injected query. Reason: a pure owner's orderings are enumerated by replay tests, where an impure one's are found only by review.
- Every request has exactly one answer. Across processes the asking side's request table gives each request its one ending, since a killed process runs no code to answer; within a process the answering side holds a responder whose drop answers. Reason: a request with no answer leaves its asker waiting for ever.
- Messages are of declared kinds: a request, a one-way report, a release, a lifecycle message, a routed report, or configuration. A message carrying Core's state for a child to judge against fits none. Reason: a kind of message nothing classifies is how such a message was added.
- Nodes are released by message position: a node leaves an outpost only when Core's held list acknowledges a later message and does not name it. Reason: a node must outlive every message Core may still read it from.
- A fact carries the version of the state it depends on, and its receiver recognises a fact made under a version it has moved past: the outpost generation in every node id, the worker generation, and, from M5, the configuration the readers judge under. Reason: two owners acting on one changed state at different times otherwise disagree silently.
- No back-channel: no message tells a reader what Core holds. Reason: a reader's copy of Core's state is always one message old, so a focus returning while Core's notice is in flight would never be spoken.
- No state lent across a call into an application: a thread works on a copy, and only its result records the change. Reason: a thread abandoned inside the call keeps what it holds, and its replacement blocks on it.
- One route and one minter for user input: every input enters Core by one route and is numbered by the keyboard hook's thread, which takes only keys. Reason: speech is fenced by key order, and a command must run in key order with held keys.
- Lifecycles as typestates: a lifecycle is an enum whose transitions consume the old state. Reason: impossible combinations of flags get written.
- Who may block on whom: Core never blocks on a reader, nor a reader on Core; the hook takes no lock; the audio thread takes no lock held across a blocking call, does no I/O but the device's, and allocates nothing per pass. Reason: each is a priority inversion or a stall of every key or of all speech.

With them (Dickson, 2026-10-30): a repeated focus is read in full and judged in Core; the outpost's reducer runs on a thread of its own, because a UI Automation call cannot be cancelled; the work takes in the settings saver, real-time audio, the synthesizer's display name, the key number on every input and review commands on the keys' route; the configuration's version is built in M5; and reading an application's focus when no UI Automation focus follows a `WinEvent` focus was discarded, since NVDA has no such read ([ADR 0001](0001-focus.md), "A focus UI Automation does not deliver"). The refactor changed who owns each fact, not what is heard, except three changes Dickson kept: a review command pressed while a caret key is held reads the moved line; a focus returning while another is judged is spoken; and a menu opening is compared with the reader's own last read.

## Weighed and rejected

- A reader predicting Core's verdict to skip work, with Core correcting it afterwards. Rejected: each prediction needed a patch for every new sequence, and a wrong one is a word unsaid.
- Node identity that outlives the object, in place of release by message position. Rejected: UI Automation reuses a runtime id once its element dies, most MSAA objects have no stable key, and a map of keys grows for the outpost's life.
- Gestures numbered in Core while the hook numbers keys. Rejected: two numberings of one input race, and a gesture's speech can be fenced and silent.
- A barrier key the test harness injects and swallows, to learn when Core has handled every key. Rejected: injection fails silently under UIPI, and a hook that overruns its timeout is removed silently.
- Updating the synthesizer dialog's copy of a setting from the driver's answer alone. Rejected: a commit made before the answer would still save a value the driver refused.
- A lock on the audio thread, or a hand-written park handshake or flags with fences to wake it. Rejected: a lock there is a priority inversion on every producer, and the hand-written wakes raced; plain events and bounded lanes are used.
- A watchdog thread that posts timer inputs to the outpost's reducer. Rejected: one high-resolution timer on the reducer's own wait costs one wake per timer, where the watchdog took two.

## Limits accepted

- The rules do not cover the applications themselves, which are bounded by due times, abandonment and replacement; version skew and untrusted peers, which are validated at their boundaries; a second instance in M8.7, which shares no writable fact; the supervisor's wall-clock decisions; and thread-affine backends, recovered only by replacement.
- Some rules are held by review rather than the compiler: whether a message carries Core's state, the single minter, and a reducer naming nothing impure, which the ownership check in `cargo xtask ci` reads as text.
- Reads for a node Core no longer names cost calls until the next release.
- A change to the settings is on disk one write after it is made; a commit unwritten by the exit bound is lost and logged.
- After a driver refuses or clamps a value the control snaps back at once, where NVDA leaves it as set; Verbatim writes the settings file on every change, where NVDA writes on exit or its save command.
- The races of the audio thread's wakes and cancels cannot be produced by a test; the crate guides state the rules for a reviewer.
