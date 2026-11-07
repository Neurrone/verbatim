# verbatim-outpost-logic

The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine
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
- `Input`: an entry from a producer, a verdict, a commit, a
  program finished, a release's answer, the answers to its asks
  (`Judged`, `HangSeen`, `FrontReadable`), and the shell's close.
  `Effect`: dispatch a `Program`, send a message,
  release, move the subscription (`Subscription`), drop elements or
  objects, abandon a worker generation (`Abandon`, with the dead
  generation, its replacement, the reason in words and what the program
  had in hand, `InHand`: the kind of entry or other work, the entry's
  trace and the window it waits on, for the log; the shell alone
  counts the abandoned workers that have not returned), publish the
  facts' marks, given after a transition that changed them and ahead of
  its other effects, or the verdicts, and two asks. `AskJudge` is the
  check of the program's window, known at dispatch or, for `None`, the
  one its worker named in its charge: whether it is hung or ghosted,
  answered `HangSeen` with the window and the program's request when it
  is, then whether the user has left it for another thread of its
  application, answered `Judged` (`moved_on`). `CheckFront` is the check
  of the window in front while a listener read's marker waits: answered
  `HangSeen` with no request when a window of the application is hung or
  ghosted there, a ghost mapped to the window it stands in for, and
  `FrontReadable` when it can be read; another application's window in
  front answers nothing. A hang abandons nothing.
- A program's deadline is its entry's (`budget`): `HANDLING_DEADLINE`
  (11 seconds, `verbatim-model`'s) from when the program begins for every
  program but two, a focus read (`Query::ReadFocus`) and a listener read's
  report, which have `FOCUS_READ_DEADLINE` (13 seconds); or, for a shell
  items or position query, the `answer_by_us` the query carries, on the
  run's shared clock, so a query that waited behind other work has only
  what is left of it. One reached at or after its `answer_by_us` is too
  late: the reducer answers it at once with nothing read, an empty list or
  no candidates and incomplete, and dispatches no program, which would
  only be abandoned at once. A test's handling deadline
  (`Input::HandlingDeadline`) takes `HANDLING_DEADLINE`'s place, and the
  two seconds `FOCUS_READ_DEADLINE` adds to it more for the two. A query's `RequestId` is
  stored and echoed in its reply, and decides nothing.
- The program in flight is the reducer's work in flight
  (`InFlight<Current>`): its request, deadline and moved-on rule are
  kept there, and the reducer keeps beside them only the query the
  program answers, its focus group try, whether it is deferred work, the
  windows destroyed while it runs, its hang mark and whether moving on
  abandons it (`moved_on_abandons`: not for a tree dump or a shell item
  query, which are not about the focus, as NVDA's watchdog judges a
  moved-on user against the focus object only, so their window is checked
  for a hang and their deadline alone ends them). A program is
  abandoned at its deadline or as moved on, never for a hang. The reducer calls it where it judges: a tick and the asks'
  answers; a foreground fact judges nothing. The program's window is
  checked at the grace's end and every half second after for a program
  with a window, until its deadline. A program is abandoned only at its
  deadline or once the user moved on. On an abandonment the reducer emits
  `Abandon` and answers its own obligations: the running query, Core's
  live re-read the entry answers (`LiveRereadGone`, sent too when a
  program ends without answering it), the owed work of the dead
  generation, the open watch, and the focus group's older entries. A cut caret read's finish is handed on to a new request
  (`InFlight::hand_on`), keeping the read's deadline and rule.
- The rule for asks: the shell answers each ask at once, as the
  reducer's next inputs, before it takes another message, and the
  reducer applies a check's answer only while its request is the program
  in flight. A tick judges the program's deadline first, abandoning it,
  then its check's next time (`AskJudge`); and, apart, the front check
  when it is due. A program with no window at dispatch that calls into
  the application, a query, the owed work, the console poll's read, is
  dispatched with `RuleWindow::Named`, checked on the grace's schedule by
  the window its worker names, a tree dump's the application's main
  window and a shell item query's the taskbar's; a release's drop, which
  calls into many windows, is dispatched with `RuleWindow::Never` and
  judged by its deadline alone. `Program::window` carries the window the
  program's entry names, which its worker names in its charge at once.
- The front check: each UIA focus marker waiting in the intake for its
  read keeps when the window in front is next checked, half a second
  after it arrived (`Intake::push` is given the shell's clock), then every
  half second while it is the first marker waiting
  (`Intake::front_due`, `Intake::front_check`). `next_timer` times the
  first marker's check, and a tick at it emits `CheckFront`; once the
  marker resolves, by its read, its drop or the end of the listener's
  reads, nothing is timed for it. The check judges the hang alone, never
  the read, so the marker still has its one answer path.
- The record of hung windows, kept in the application's own reducer: a
  window enters it at a `HangSeen`, of the program in flight's check, of
  a request no longer live, or of the window in front, or at an accepted
  commit naming it `Found::Hung` (its check before a call found it hung,
  and no call was made), and `OutpostToCore::NotResponding` is sent
  only as the record goes from empty to holding a window, once per hang of
  the application: a window entering while another is recorded sends
  nothing. A window leaves only on evidence that it answers,
  an accepted commit naming it `Found::Answered` (a counted call into it
  returned) or a `FrontReadable` naming it, its destroy event, or the
  outpost's end. A program that made no call into the window is never
  that evidence. A destroy wins over the program in flight: a hang it
  names, by its check or a commit, in a window whose destroy event reached
  the reducer while it ran (`Current::destroyed`) enters nothing. The
  destroy takes effect as it first reaches the reducer, from the hook or
  the listener, even as a hook copy the hand-over's hold keeps back.
- The hang mark: a `HangSeen` whose request is the program in flight's,
  of the generation in charge, sets that program's mark, dropped with the
  program. The abandonment's `Effect::Abandon` carries it as `on_hung`,
  which the shell counts apart for the pong, so the supervisor's limit on
  abandoned workers never replaces an outpost for its application's hang.
  The record of hung windows cannot stand in for it: a program whose
  worker names its window has none the record could be matched with at
  abandonment.
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
the program in flight holds it, and that program's later check is
ignored, since `WatchChange::Checked` applies only to a watch that is
`Checking` (Claude, 2026-11-03, accepted by Dickson the same day). The
reducer otherwise changes the watch only at a dispatch, at an
abandonment, or with no program in flight.
`next_timer` gives its due time only when it would be acted on.

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick`, its
asks answered by recorded answers as the shell answers them
(`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay. The watch has a
generated test (`crates/verbatim-outpost-logic/src/tests/generated.rs`):
every sequence of up to six steps of the inputs that move it, enumerated
without randomness (about 1.5 s in a debug build), with no model of
the watch's lifecycle, and checked after each step for four invariants: each key's
query answered at most once, and by the sequence's end answered once or
left to the outpost's end, no
time for the watch while a program or a release is in flight, the
program's copy equal to the reducer's watch apart from times at each
accepted commit, and no idle answer while the watch is open or the
program in flight deferred; and a due time is acted on as soon as
nothing else is ready. The bound, the poll's timings and each reply's
reason are the examples of `watch.rs` and the replay tests.
The reducer has a generated test against a reference model
(`crates/verbatim-outpost-logic/src/tests/reducer.rs`): lists of actions
weighted toward the lifecycle, run through proptest's `TestRunner::run`
under a fixed seed and 1,500 cases (about 4.3 s in a debug build), each
action made valid when applied by resolving its target against the model
(the program in flight, the oldest open query, the newest request no
longer live) and carrying the answers to the asks it may cause, which are
answered in line in the shell's order. The actions are entries of every
kind from the hook, the listener and the rest, several focuses pending at
once among them; commits of every kind, and each program kind's own
result; hangs seen by a program's check, in front or for a request no
longer live, and readable windows in front; programs ended, cut and
panicked; inputs of requests no longer live; verdicts of the generation in charge
and of the one before; the hold; cancels; the close; and time on the
three clocks, each advancing on its own, to the next timer or by chosen
amounts. The model tracks the generation and the requests issued, the
program in flight (its kind, its query, its hang mark, its deadline as
dispatched, its window), the markers waiting with their front checks'
due times, the windows' hang intervals, the entries pending as a set
rather than the intake's plan, the open queries, settles and rounds, the
hook's copies held in their order, a release, the owed work and
stopping. After every
call it checks the design's invariants and its postconditions: each dispatch under the next request
with a deadline after its dispatch, a poll read holding the watch, a late shell query answered with nothing read,
a cancel, a commit's message ahead of its other effects, a cut read's
finish alone, a program's end answering its watch and its query before
the next dispatch, a tick deciding the deadline, then the program's
check, an abandonment only at the deadline or in the answer that the
user moved on, `on_hung` equal to the program's hang mark,
`NotResponding` sent exactly as the hang record goes from empty to
holding a window, the front checked exactly when a tick finds the first
waiting marker's check due and never otherwise, the front check timed
while a marker waits, and an abandonment answering the
watch and the query before the next dispatch, and the hold's release on
the end of forwarding, the end of the listener's reads or no listener,
giving back some of the held copies in the hook's order, every one with
no listener. Each kind's deadline and which copies a release keeps are
the examples of the replay tests and `hold.rs`. Each case ends by draining
(every program finished, every marker dropped, every timer reached, the
close), and then every query has one reply or is left to the shell's
token, which only a query never taken from the intake, or the watch's,
may be; and no focus group a try reported in is tried again. Each case runs twice
and the two runs' effects are compared, and the test fails if a listed
lifecycle state is never reached. The cases the test and its exploration
found are kept as fixed cases, pasted as the failure prints them
(`books/dev/src/reference/testing.md`, "Exploration tests"); the intake's and the watch's
enumeration's are kept in their tests' `found()` lists. The model leaves out the intake's plan,
text facts, held reports, the subscription, the marks' contents, verdict
contents beyond whether the record changed, the caret watch's
transitions, and exact deadlines; and its generator sends a foreground change from the
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
