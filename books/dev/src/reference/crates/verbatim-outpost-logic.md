# verbatim-outpost-logic

The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine
that holds every remembered fact of an outpost's application, with no
Windows type and no application object. It is a logic crate: in the GPL
tier, with no Windows dependency, so it builds and is tested on any host.
It depends on `verbatim-model` and
on [verbatim-reader-rules](verbatim-reader-rules.md) for
the window type it shares with the focus listener.

The outpost ([verbatim-outpost](verbatim-outpost.md)) runs it on a thread
of its own, its shell, which applies its effects and keeps its time; that
guide's "Inside an outpost" describes the reducer with the shell, the
worker and the mailbox around it, and its "Implementation notes" the
reducer's rules for each kind of event. Its API is in its rustdoc
(`cargo doc -p verbatim-outpost-logic --no-deps --open`).

## Core's requests

Core's requests (`Asks`, the `waiting` module): an `Input::Ask` is queued in the order Core sent it and dispatched as
`Work::Ask` ahead of every other program, one at a time, with no
deadline and no moved-on rule, Core's due time and cancel bounding it.
`Input::Answered` sends the program's answer while its request is in
flight; a program that ends without one is answered ended, "failed".
`Input::CancelAsk` answers a queued request ended, "cancelled", and
never runs it; a running one cancelled because the foreground moved
away is answered ended, "abandoned", and its worker abandoned and
replaced (`Effect::Abandon`), its later answer refused; for any other
reason it runs on and its answer is sent. At the close the queued
requests are dropped unanswered.

## A caret key's wait

A caret key's wait (`Watches`, the `watch` module): an `Input::Ask` of a batch whose first step is
`WaitForCaret` on a node (`watch::caret_wait`) waits in the table
instead of the queue, its first check due at once, then one a poll
after each polled check began, the check due at the bound the last.
Each check is a `Work::Watch` (`WatchProgram`, holding the batch, its
`WatchWork` a `Check` with whether to compare the word and whether it
is the last), dispatched ahead of every other program, the earliest
due first, on the shell's timer (`Reducer::next_timer`); a check that
shows the effect, and the last whatever it shows, runs the batch's
later steps and answers it. A check that shows the effect, and the last
check, makes the final read in the same program and answers through
`Input::Answered`; a check that answers nothing showed no effect, or
failed, and the watch goes on; a last check that answers nothing is
answered failed. Where the watch names the event rule, a UI Automation
caret or text-change entry within `EVENT_WINDOW_US` of the pass brings
a check forward, due at once (after the check running, if one runs),
which compares the word too and ends the watch only with the effect;
the polls after it keep their times (Dickson, 2026-11-17). Core's
cancel ends a watch "cancelled", or "abandoned" when its check runs and
the reason abandons the call, and a check running commits nothing once
its watch is gone. A destroyed window answers a watch on it "not there",
at once while it waits, at its check's end while one runs. A release
(`NodesHeld`) spares the starting points the waiting watches name, and
at the close every watch is dropped unanswered.

## The reducer

- `Reducer` with `input(input, now)`, which takes one `Input` in
  mailbox order, given the time (`Now`: the shell's monotonic clock, the
  observation clock and the shared clock), and returns the `Effect`s in
  the order the shell applies them; `tick(now)`, which the shell calls
  after every drain of its mailbox, a timer due or not, so a tick may
  find nothing due; and `next_timer(now)`, the time of the shell's timed wait. It makes
  no call of any kind.
- `Input`: an entry from a producer, Core's request (`Ask`), its cancel
  (`CancelAsk`) and a program's answer to it (`Answered`), a window Core
  named destroyed (`WindowDestroyed`: the queued reads its MSAA ids name,
  a read by ids and a run's candidate, never a request merely asked on
  the window, and the watches waiting on it are answered "not there", and
  the window is owed a forget; this input alone does not dispatch, so the
  windows destroyed in one drain are forgotten together at the tick), a
  a commit, a program finished, a release's answer, and the
  shell's close. `Effect`: dispatch a `Program`, send a
  message, release, let go of parked elements (`Unpark`), abandon a
  worker generation (`Abandon`, with
  the dead generation and its replacement; the shell alone counts the
  abandoned workers that have not returned), or report a settle reached
  (`Settled`).
- A program runs until it ends: it has no deadline, and the outpost
  judges no hang and no user moving on. Core bounds its requests by their
  due times and judges a hang itself.
- As it hands an entry to the worker, the reducer steps its marks of the
  application's hung threads (`verbatim_model::hung::Marks`) with the
  window state the entry's callback read: a hung window whose thread is
  not marked is reported to Core (`OutpostToCore::Hung`) before the
  entry is dispatched, so before its own tell, once per stall of the
  thread; a responsive window unmarks its thread. The marks end with the
  outpost, which serves one application. The intake's merge keeps the
  newer entry, so the later read stands. A limit: an entry is handed out
  only once the program in flight ends, so while a worker is inside a
  call into the stalled application the report waits with it, and may
  reach Core after the window answers; the listener's report of the same
  stall is not held so.
- The program in flight is the generation in charge's (`InCharge`, the
  `charge` module), numbered by `Request` within its generation so that a
  dead generation's and an earlier program's results are refused. What it
  runs is one record kept with it (`Current`): Core's request until it is
  answered, a watch's check, or other work; `Asks` holds only the queued
  requests and `Watches` a check's phase, with a window destroyed during
  the check inside that phase. A
  generation is abandoned only when Core cancels the request its program
  runs because the foreground moved away; the reducer then emits
  `Abandon` and answers that request.
- `Program` and `Work` (an intake `Entry`, a drop of released objects,
  Core's `Ask`, a watch's program, or `Forget`, the windows owed a
  forget, dispatched first of all, so every request Core sent after a
  window's destruction runs after its forget): what the worker generation in
  charge runs, whose commits come back as `Change`s (the program's tell)
  and are applied while their `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `UiaEvent` and `Asker`; the focus line's
  paths a UI Automation completion finds its elements by (`FocusPath`,
  `Route`); the caret key's watches (`Watches`, `WatchProgram`,
  `WatchWork`, above); each node's text facts (`TextFacts`,
  `NodeAnchors`, `Anchor`, `LastRead`, `KeptSelection` and
  `ReadSelection`, re-exported from `verbatim_reader_rules::text::facts`,
  and `TextChange`), among them the reads held behind ids Core names
  (`NodeAnchors::held`: a caret key's starting point and a selection
  baseline, numbered and released as anchors are), which the
  outpost's object store holds and the reducer does not, since no rule of
  it reads them, their type here as pure data tested on any host
  (Dickson's confirmed default, 2026-11-20); and
  `WinKind` and `UiaKind`, its own copies of the backends' event kinds,
  since it names no backend crate.

The outpost follows no focus of its own: it keeps the registration Core's
`Follow` request names, in the outpost's shell, and tells Core what that
registration hears. It keeps no key note: a caret key's starting read and
watch are Core's requests, and Core alone judges a read's facts as a
key's evidence.

## The intake

The intake (`src/intake.rs`) queues what the outpost still hears, its
`Follow` subscription's UI Automation events and a console host's console
events, and Core's lists of held nodes and
idle queries, in arrival order: one waiting entry per object and kind, a
console's events merged per window and kind, a newer list of held nodes
replacing a waiting one, and a batch ending at an idle query. Core's
requests do not pass through it: they wait in `Asks`. Its rules are pinned
by its own tests.

## Tests

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick`
(`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay; Core's requests
through the reducer are `src/tests/asked.rs`'s, and their queue's own
rules `waiting.rs`'s; the watch's are `watch.rs`'s, and the watch across Core and the outpost is the generated
test `tests/caret_watch.rs`.
