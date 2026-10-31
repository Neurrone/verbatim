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
  the order the shell applies them; `tick(now)`, for a timer that came
  due; and `next_timer(now)`, the time of the shell's timed wait. It makes
  no call of any kind.
- `Input`: an entry from a producer, a verdict, a commit, a walk armed or returned, a
  program finished, a release's answer, the gap record's bound, the answers to its asks (`MovedOn`, `Published`), and
  the shell's close. `Effect`: dispatch a `Program`, send a message,
  release, move the subscription (`Subscription`), drop elements or
  objects, abandon a worker generation (`Abandon`, with the dead
  generation, its replacement and the reason in words; the shell alone
  counts the abandoned workers that have not returned), publish the intake's marks or the
  verdicts, set the gap record's timer, and two asks: `AskMovedOn`,
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
  The reducer calls it where it judges: a tick, a walk armed or
  returned, and the asks' answers; a foreground fact judges nothing. The
  moved-on rule is judged at the grace's end and every half second after
  for a program with a window, until its deadline. On an abandonment the
  reducer emits `Abandon` and answers its own obligations: the running
  query, the owed work of the dead generation, the open watch, and the
  focus group's older entries unless the try was published. A cut caret
  read's finish is handed on to a new request (`InFlight::hand_on`),
  keeping the read's deadline, walk and rule.
- The rule for asks: the shell answers each ask exactly once, as the
  reducer's next input, before it takes another message, and the reducer
  applies an answer only while its request is the program in flight. A
  transition makes at most one ask: a tick judges the deadline first
  (abandoning, by one `Publish` with `then_abandon` when a walk is armed),
  then the walk's bound (`Publish`), then the moved-on rule's next
  judgment (`AskMovedOn`), the first that applies deciding; a judgment
  left over is made by the publication's answer against its own `Now`. A
  true `MovedOn` for a program already published abandons it at once.
- `Program` and `Work`: what the worker generation in charge runs, with
  its copies of the facts (`Facts`), whose commits come back as `Change`s
  and are applied while their `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `Source`, `Marks` and `Object`; the follow
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
