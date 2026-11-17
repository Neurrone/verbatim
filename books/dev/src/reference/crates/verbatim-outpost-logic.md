# verbatim-outpost-logic

The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine
that holds every remembered fact of an outpost's application, with no
Windows type and no application object. It is a logic crate: in the GPL
tier, with no Windows dependency, so it builds and is tested on any host.
It depends on `verbatim-model` and
on [verbatim-reader-rules](verbatim-reader-rules.md) for the verdicts and
the window type it shares with the focus listener.

The outpost ([verbatim-outpost](verbatim-outpost.md)) runs it on a thread
of its own, its shell, which applies its effects and keeps its time; that
guide's "The outpost" section describes the reducer with the shell, the
worker and the mailbox around it, and its "Implementation notes" the
reducer's rules for each kind of event.

Public API:

- Core's requests (`Asks`, the `waiting` module;
  `docs/design/readers-tell-core-asks-build1.md`, Table 14): an
  `Input::Ask` is queued in the order Core sent it and dispatched as
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
- A caret key's watch (`Watches`, the `watch` module; the design's "The
  watch" and Table 15): an `Input::Ask` naming `TextOp::Watch` waits in
  the table instead of the queue, its first check due at once, then one
  a poll after each polled check began, the check due at the bound the
  last. Each check is a `Work::Watch` (`WatchProgram`, its `WatchWork` a
  `Check` with whether to compare the word and whether it is the last;
  the table never asks for the `Final` read), dispatched ahead of every
  other program, the earliest due first, on the shell's timer
  (`Reducer::next_timer`). A check that shows the effect, and the last
  check, makes the final read in the same program and answers through
  `Input::Answered`; a check that answers nothing showed no effect, or
  failed, and the watch goes on; a last check that answers nothing is
  answered failed. Where the watch names the event rule, a UI Automation
  caret or text-change entry within `EVENT_WINDOW_US` of the pass brings
  a check forward, due at once (after the check running, if one runs),
  which compares the word too and ends the watch only with the effect;
  the polls after it keep their times (Dickson, 2026-11-17). Cancel, window destruction, `NodesHeld` (whose
  release spares the watches' starting points) and the close follow
  Table 15.
- `Reducer` with `input(input, now)`, which takes one `Input` in
  mailbox order, given the time (`Now`: the shell's monotonic clock, the
  observation clock and the shared clock), and returns the `Effect`s in
  the order the shell applies them; `tick(now)`, which the shell calls
  after every drain of its mailbox, a timer due or not, so a tick may
  find nothing due; and `next_timer(now)`, the time of the shell's timed wait. It makes
  no call of any kind.
- `Input`: an entry from a producer, Core's request (`Ask`), its cancel
  (`CancelAsk`) and a program's answer to it (`Answered`), a window Core
  named destroyed, a verdict, a commit, a program finished, a release's
  answer, and the shell's close. `Effect`: dispatch a `Program`, send a
  message, release, let go of parked elements (`Unpark`), forget text
  ranges (`ForgetRanges`), abandon a worker generation (`Abandon`, with
  the dead generation and its replacement; the shell alone counts the
  abandoned workers that have not returned), or report a settle reached
  (`Settled`).
- A program runs until it ends: it has no deadline, and the outpost
  judges no hang and no user moving on. Core bounds its requests by their
  due times and judges a hang itself.
- The program in flight is the generation in charge's (`InCharge`, the
  `charge` module), numbered by `Request` within its generation so that a
  dead generation's and an earlier program's results are refused. A
  generation is abandoned only when Core cancels the request its program
  runs because the foreground moved away; the reducer then emits
  `Abandon` and answers that request.
- `Program` and `Work` (an intake `Entry`, a drop of released objects,
  Core's `Ask`, or a watch's program): what the worker generation in
  charge runs, with its copies of the facts (`Facts`: the verdicts and
  each node's text facts), whose commits come back as `Change`s (the
  program's tell and its text changes) and are applied while their
  `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `UiaEvent` and `Asker`; the focus line's
  paths a UI Automation completion finds its elements by (`FocusPath`,
  `Route`); the caret key's watches (`Watches`, `WatchProgram`,
  `WatchWork`, above); the verdicts (`Verdicts`, `VerdictChange`,
  `Probe`, `Probed`, from `verbatim-reader-rules`, and
  `KeptFromAnyGeneration`, this crate's rule of which a generation no
  longer in charge may record); each node's text facts (`TextFacts`,
  `NodeAnchors`, `Anchor`, `LastRead`, `KeptSelection`, `ReadSelection`,
  `TextChange`), among them the reads held behind ids Core names
  (`NodeAnchors::held`: a caret key's starting point and a selection
  baseline, numbered and released as anchors are, Table 16); and
  `WinKind` and `UiaKind`, its own copies of the backends' event kinds,
  since it names no backend crate.

The outpost follows no focus of its own: it keeps the registration Core's
`Follow` request names, in the outpost's shell, and tells Core what that
registration hears. It keeps no key note: a caret key's starting read and
watch are Core's requests, and Core alone judges a read's facts as a
key's evidence.

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick`
(`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay; Core's requests
through the reducer are `src/tests/asked.rs`'s, and their queue's own
cells (Table 14) `waiting.rs`'s; the watch's cells (Table 15) are
`watch.rs`'s, and the watch across Core and the outpost is the generated
test `tests/caret_watch.rs`.

The intake (`src/intake.rs`) queues what the outpost still hears, its
`Follow` subscription's UI Automation events and a console host's console
events, a window Core named destroyed, and Core's lists of held nodes and
idle queries, in arrival order: one waiting entry per object and kind, a
console's events merged per window and kind, a newer list of held nodes
replacing a waiting one, and a batch ending at an idle query. Core's
requests do not pass through it: they wait in `Asks`. Its rules are pinned
by its own tests.
