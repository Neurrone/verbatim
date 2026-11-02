# verbatim-outpost-logic

The outpost's reducer (`docs/design/ownership.md`): the pure state machine
that holds every remembered fact of an outpost's application, with no
Windows type and no application object. It is a logic crate: in the GPL
tier, with no Windows dependency, so it builds and is tested on any host.
It depends on `verbatim-model`, on `verbatim-core` for `focus_rules`, and
on [verbatim-reader-rules](verbatim-reader-rules.md) for the work in
flight (`InFlight`, with `Request` and `Reason`, which it re-exports) and
the UIA focus wait it shares with the focus listener.

The outpost ([verbatim-outpost](verbatim-outpost.md)) runs it on a thread
of its own, its shell, which applies its effects and keeps its time; that
guide's "The outpost" section describes the reducer with the shell, the
worker and the mailbox around it, and its "Implementation notes" the
reducer's rules for each kind of event.

Public API:

- `Reducer` with `input(input, now)`, which takes one `Input` in
  mailbox order, given the time (`Now`: the shell's monotonic clock, the
  observation clock and the shared clock), and returns the `Effect`s in
  the order the shell applies them; `tick(now)`, which the shell calls
  after every drain of its mailbox, a timer due or not, so a tick may
  find nothing due; and `next_timer(now)`, the time of the shell's timed wait. It makes
  no call of any kind.
- `Input`: an entry from a producer, a verdict, a commit, a walk armed or returned, a
  program finished, a release's answer, the gap record's bound, the answers to its asks (`MovedOn`, `Published`, `GapBoundTested`), and
  the shell's close. `Effect`: dispatch a `Program`, send a message,
  release, move the subscription (`Subscription`), drop elements or
  objects, abandon a worker generation (`Abandon`, with the dead
  generation, its replacement, the reason in words and what the program
  had in hand, `InHand`: the kind of entry or other work, the entry's
  trace and the window it waits on, for the log; the shell alone
  counts the abandoned workers that have not returned), publish the
  facts' marks, given after a transition that changed them and ahead of
  its other effects, or the verdicts, set the gap record's timer, given
  after a transition that changed the open record's bound, and three asks:
  `TestGapBound`, the gap record's test at its bound of its top-level
  window against the foreground, with its window text sent as a
  whole-window focus when the window is in front, minted under a request
  of its own that no program runs as, asked after the same tick's ask of
  the program in flight and so answered after that ask's answer: a record
  that answer's report closed commits nothing, and a generation that answer
  abandoned still owns the request the text is committed under, without
  the acceptance test, since the text is the shell's own read made in line,
  not a worker's result; no record is tested, or timed, once the outpost
  stops; `AskMovedOn`,
  whether the user has left the program's window for another thread of
  its application, and `Publish`, the publication of a walk armed, with
  the facts it is judged against and `then_abandon`, the abandonment
  that asked for it (a `Reason`), which comes back in the answer so the
  reducer keeps nothing between the two.
- A program's deadline is its entry's (`budget`): its kind's from when
  the program begins, or, for a shell items or position query, the
  `answer_by_us` the query carries, on the run's shared clock, so a query
  that waited behind other work has only what is left of it. One reached
  at or after the `walk_until_us` it carries is too late to walk: the
  reducer answers it at once with nothing read, an empty list or no
  candidates and incomplete, as a walk out of time answers, and dispatches
  no program, since a walk makes its first call regardless and could
  outlive the deadline, abandoning the worker. A query's `RequestId` is
  stored and echoed in its reply, and decides nothing.
- The program in flight is the reducer's work in flight
  (`InFlight<Current>`): its request, deadline, walk and moved-on rule
  are kept there, and the reducer keeps beside them only the query the
  program answers, its focus group try and whether it is deferred work.
  The reducer calls it where it judges: a tick, a walk returned, and
  the asks' answers; a walk armed only records its bound, and a
  foreground fact judges nothing. The
  moved-on rule is judged at the grace's end and every half second after
  for a program with a window, until its deadline. On an abandonment the
  reducer emits `Abandon` and answers its own obligations: the running
  query, the owed work of the dead generation, the open watch, and the
  focus group's older entries unless the try was published. A cut caret
  read's finish is handed on to a new request (`InFlight::hand_on`),
  keeping the read's deadline, walk and rule.
- The rule for asks: the shell answers each ask exactly once, as the
  reducer's next input, before it takes another message, and the reducer
  applies an answer only while its request is the program in flight, but
  `GapBoundTested`, which concerns the gap record, not a program, and is
  applied whatever is in flight. Of the program's asks a transition makes
  at most one: a tick judges the deadline first
  (abandoning, by one `Publish` with `then_abandon` when a walk is armed),
  then the walk's bound (`Publish`), then the moved-on rule's next
  judgment (`AskMovedOn`), the first that applies deciding; a judgment
  left over is made by the publication's answer against its own `Now`. A
  true `MovedOn` for a program already published abandons it at once.
- `Program` and `Work`: what the worker generation in charge runs, with
  its copies of the facts (`Facts`), whose commits come back as `Change`s
  and are applied while their `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `Source`, `Marks` (with `TerminalMark`) and `Object`;
  `Front`, the outpost's one test of an MSAA focus or a foreground change
  against the foreground, which the shell makes from Windows' state as it
  hands the entry to the reducer and the entry carries to its program; the follow
  state (`Follow`, `FollowState`, `Target`, `Owed`); the caret key's
  watch (`Watch`, `OpenWatch`, `Checked`, below); the
  reports Core may hold (`Held`, `Focus`, `Evidence`, `InFront`); the verdicts
  (`Verdicts`, `VerdictChange`, `Probe`, `Probed`); each node's text facts
  (`NodeAnchors`, `Anchor`, `TextChange`, `ReportCut`, `AnswerCut`); and
  `WinKind` and `UiaKind`, its own copies of the backends' event kinds,
  since it names no backend crate.

The caret key's watch (`src/watch.rs`) is one state in the follow state,
`Watch`: `None`, `Waiting` until its bound, `Polling` with its next and
last read, or `Checking` while the program in flight holds it, with what
it goes back to. The reducer alone decides its times, on the run's shared
clock (`Now::wall_us`); a program's copy applies the same transitions
with no clock, its times zero. Its transitions are made at five points:
a caret key's dispatch ends the watch it replaces before the copy is
taken; a commit opens it (`WatchChange::Open`), takes it
(`WatchChange::Take`) or gives the check's result
(`WatchChange::Checked`), the reducer then sending the key's reply; an
abandonment answers a held watch `Abandoned`; a program that ends still
holding it ends it; and with no program in flight, no release pending
and nothing else ready, its due time ends a waiting watch at its bound
or dispatches a poll read (`Work::PollRead`). A report on another node or
a replaced node ends it in any state, and a release of its node ends it.
A committed foreground fact on another node ends it at once, even while
the program in flight holds it: the gap record's window text, the one
report the reducer commits for itself, does so whatever program is in
flight, and that program's later check is ignored, since
`WatchChange::Checked` applies only to a watch that is `Checking`
(Claude, 2026-11-03, pending Dickson's acceptance). The reducer otherwise
changes the watch only at a dispatch, at a publication that claims the
program, or with no program in flight.
`next_timer` gives its due time only when it would be acted on.

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick`, its
asks answered by recorded answers as the shell answers them
(`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay. The watch has a
generated test (`crates/verbatim-outpost-logic/src/tests/generated.rs`):
every sequence of up to six steps of the inputs that move it, enumerated
without randomness (104,682 sequences, about 1.5 s in a debug build),
compared after each step with a reference model, the open query and
whether a program holds it, and checked for four invariants: each key's
query answered at most once, and by the sequence's end answered once or
left to the outpost's end, no
time for the watch while a program or a release is in flight, the
program's copy equal to the reducer's watch apart from times at each
accepted commit, and no idle answer while the watch is open or the
program in flight deferred.
The reducer has a generated test against a reference model
(`crates/verbatim-outpost-logic/src/tests/reducer.rs`,
`docs/design/logic-tests.md`, "The outpost's reducer"): lists of actions
weighted toward the lifecycle, run through proptest's `TestRunner::run`
under a fixed seed and 1,500 cases (about 4.3 s in a debug build), each
action made valid when applied by resolving its target against the model
(the program in flight, the oldest open query, the newest request no
longer live) and carrying the answers to the asks it may cause, which are
answered in line in the shell's order. The actions are entries of every
kind from the hook, the listener and the rest, several focuses pending at
once among them; commits of every kind, and each program kind's own
result; walks armed and returned; programs ended, cut and panicked;
inputs of requests no longer live; verdicts of the generation in charge
and of the one before; the hold; cancels; the close; and time on the
three clocks, each advancing on its own, to the next timer or by chosen
amounts. The model tracks the generation and the requests issued, the
program in flight (its kind, its query, its walk, its deadline by its
kind's budget, its window), the entries pending as a set rather than the
intake's plan, the open queries, settles and rounds, the watch's query
and whether a program holds it, the hold, a release, the owed work and
stopping. The gap record is not modelled: each call's change of it is
checked against what the design lets change it (opened or restamped by a
foreground change found in front or a copy dropped in its window, closed
by a report it answers, an unrelated window at its bound, a drop for no
window or a copy elsewhere, its bound passed only once due), never against
its stamps' arithmetic. After every call it checks the design's sixteen
invariants and its postconditions: each dispatch under the next request
with its kind's deadline, a late shell query answered with nothing read,
a cancel, a commit's message ahead of its other effects, a cut read's
finish alone, a program's end answering its watch and its query before
the next dispatch, a tick deciding the deadline, then the bound, then the
moved-on rule, then the gap's bound, and an abandonment answering the
watch and the query before the next dispatch, and the hold's release on
the end of forwarding, the end of the listener's reads or no listener,
giving back the held copies in the hook's order but those of the kinds
the listener forwards raised at or before the end, or at or before the
newest such copy the listener forwarded. Each case ends by draining
(every program finished, every timer reached, the close), and then every
query has one reply or is left to the shell's token, which only a query
never taken from the intake, or the watch's, may be; and no focus group a
try reported, or was published in, is tried again. Each case runs twice
and the two runs' effects are compared, and the test fails if a listed
lifecycle state is never reached. The cases the test and its exploration
found are kept as fixed cases. The model leaves out the intake's plan,
text facts, held reports, the subscription, the marks' contents, verdict
contents beyond whether the record changed, and exact deadlines beyond
each kind's budget; and its generator sends a foreground change from the
listener, never as the hook's held copy.

The intake has a generated test of its own
(`crates/verbatim-outpost-logic/src/tests/intake_order.rs`, 800 cases,
about 2.0 s): pushes of every kind of entry it orders, hand-outs, cancels,
a noted caret and the close, with a barrier reached while a noted caret
is kept put behind it as the reducer does. It plans nothing itself: it
sees which entries each call leaves waiting (`Intake::traces`, a test
accessor) and checks each change against the intake's documented rules,
never its keys or code. A push removes only the markers it resolves, at
most one waiting entry of its object, kind and thread with no idle
barrier between them, and for a window's foreground change at most one
waiting focus on its client area from its thread, and leaves no such pair
undivided. A batch is what waited before the first marker, through the
first idle barrier; it keeps only entries it took; an entry the limiter
counts (the newest four focus events and foreground changes, the newest
ten other events per thread, a foreground change counted by both) is kept
exactly when fewer newer entries than one of its groups' limits are kept;
and of its menu openings it keeps the newest alone. It also checks that
each batch's number rises by one, that each entry handed out belongs to
its batch, the noted caret alone handed out outside one and only when
nothing is ready, that each entry is handed out once or dropped for one
of those reasons, that nothing is handed out
ahead of an earlier arrival but by a named exception (a focus run tried
at its newest's place, a menu opening at its batch's end, a read in its
marker's place, the noted caret and its barrier put ahead), and that an
open marker holds back everything after it.
Beside each, an ignored exploration test (`explore_reducer`,
`explore_intake`, in `src/tests/explore.rs`, the crate's one file that
reads the environment) runs the same cases under the seed and case count
in `VERBATIM_EXPLORE_SEED` and `VERBATIM_EXPLORE_CASES`; only the nightly
workflow (`.github/workflows/explore.yml`) runs them.
