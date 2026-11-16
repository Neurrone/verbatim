# verbatim-outpost-logic

The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine
that holds every remembered fact of an outpost's application, with no
Windows type and no application object. It is a logic crate: in the GPL
tier, with no Windows dependency, so it builds and is tested on any host.
It depends on `verbatim-model`, on `verbatim-core` for `focus_rules`, and
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
  a poll after each check began, the check due at the bound the last.
  Each check or final read is a `Work::Watch` (`WatchProgram`, its
  `WatchWork` a `Check` with whether to compare the word and whether it
  is the last, or the `Final` read), dispatched ahead of every other
  program, the final read ahead of a check, on the shell's timer
  (`Reducer::next_timer`). A program that ends the watch answers through
  `Input::Answered`; a check that answers nothing showed no effect, or
  failed, and the watch goes on; a last check or final read that answers
  nothing is answered failed. Where the watch names the event rule, a UI
  Automation caret or text-change entry within `EVENT_WINDOW_US` of the
  pass, or one heard while the key's starting read was out, makes the
  final read next. Cancel, window destruction, `NodesHeld` (whose
  release spares the watches' starting points) and the close follow
  Table 15.
- `Reducer` with `input(input, now)`, which takes one `Input` in
  mailbox order, given the time (`Now`: the shell's monotonic clock, the
  observation clock and the shared clock), and returns the `Effect`s in
  the order the shell applies them; `tick(now)`, which the shell calls
  after every drain of its mailbox, a timer due or not, so a tick may
  find nothing due; and `next_timer(now)`, the time of the shell's timed wait. It makes
  no call of any kind.
- `Input`: an entry from a producer, Core's request, its cancel and a
  program's answer to it, a verdict, a commit, a program finished, a
  release's answer, and the shell's close. `Effect`: dispatch a
  `Program`, send a message, release, move the subscription
  (`Subscription`), drop elements or objects, abandon a worker generation
  (`Abandon`, with the dead generation and its replacement; the shell
  alone counts the abandoned workers that have not returned), or publish
  the facts' marks, given after a transition that changed them and ahead
  of its other effects, or the verdicts.
- A program runs until it ends: it has no deadline, and the outpost
  judges no hang and no user moving on. Core bounds its requests by their
  due times and judges a hang itself. A shell items or position query
  reached at or after the `answer_by_us` it carries is answered at once
  with nothing read, an empty list or no candidates and incomplete, and
  no program is dispatched for it. A query's `RequestId` is stored and
  echoed in its reply, and decides nothing.
- The program in flight is the generation in charge's (`InCharge<Current>`,
  the `charge` module): its request, and beside it the query the program
  answers, the Core request it runs, and whether it
  is deferred work. A generation is abandoned only when Core cancels the
  request its program runs because the foreground moved away; the reducer
  then emits `Abandon` and answers its own obligations: the running query
  and the owed work of the dead generation. A cut caret read's
  finish is handed on to a new request (`InCharge::hand_on`).
- `Program` and `Work`: what the worker generation in charge runs, with
  its copies of the facts (`Facts`), whose commits come back as `Change`s
  and are applied while their `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `Marks` (with `TerminalMark`) and `Object`; the follow
  state (`FollowState`, `Followed`, `Origin`, `Subscribed`, `LastReported`,
  `Owed`, and a commit's `FocusCommit` and `MarkChange`, below); the caret key's
  watch (`Watch`, `OpenWatch`, `Checked`, below); the verdicts
  (`Verdicts`, `VerdictChange`, `Probe`, `Probed`, from `verbatim-reader-rules`, and
  `KeptFromAnyGeneration`, this crate's rule of which a generation no
  longer in charge may record); each node's text facts
  (`NodeAnchors`, `Anchor`, `TextChange`, `ReportCut`, `AnswerCut`),
  among them the reads held behind ids Core names (`NodeAnchors::held`: a
  caret key's starting point and a selection baseline, numbered and
  released as anchors are, Table 16); and
  `WinKind` and `UiaKind`, its own copies of the backends' event kinds,
  since it names no backend crate.

The followed focus (`src/follow.rs`) is one record, `Followed`: the node,
its role, object, window, chain and titles, how it was read (`Origin`: the
listener's read with its focus path, an MSAA focus with what a live re-read
needs, a focus-now answer, a menu opening), its terminal mark, its redirect
and system-caret marks, its subscription and what it owes. A
commit's `FocusCommit::Adopt` replaces the record whole, so that no fact of an
older focus survives beside a newer one; the subscription it replaces keeps
its group among `kept_groups` until its node is released.
`FocusCommit::Again`, a focus-now answer on the focus already followed, moves
the subscription and replaces the owed work and nothing else.
A caret key's watch on another node than the one an adoption names ends. An element found after its report moves the subscription only while
its node is followed; otherwise its tokens are let go. `MarkChange` is the
one change of a mark a commit makes. `FollowState::absorbed`, beside the
record, is a held console's window, which need not be the followed focus's:
an adoption clears it, as does anything that may end the hold. What the
mailbox and the intake read is computed from the record and it
(`Facts::marks`), and `Effect::Marks` is sent when the computed value
changes. A commit the state table says cannot occur, a
`FocusCommit::Again` or a mark of the record's changed with nothing
followed, changes nothing and comes back as `Effect::LogicError`, which the shell logs as an
error.

A node's note (`docs/design/caret-selection-rework.md`) is kept in the
text facts with the node's record (`text.rs`): `KeyNote`, its key's unit and
motion, when it ends, and what it causes to be read besides the node's
caret events (`NoteReads::Field` or `NoteReads::Terminal`, each
`CaretAndText`, or `CaretOnly` once a read showed the key's effect, and a
terminal's `Poll`, the console host's). It is set as a before
read's request or a one-way `CoreToOutpost::KeyNote` arrives and cleared by
the key's final read, which goes ahead of queued reads, or Verbatim's own
caret move; a note past its end counts as absent where it is read, so
no timer ends it and no key's progress is held here. The console poll's due
time is the reducer's (`poll_due`, `reach_poll`), driven by the run's shared
clock, and a poll read is one `Work::NoteRead`. Core alone judges a read's
facts as a key's evidence.

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick`
(`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay; Core's requests
through the reducer are `src/tests/asked.rs`'s, and their queue's own
cells (Table 14) `waiting.rs`'s; the watch's cells (Table 15) are
`watch.rs`'s, and the watch across Core and the outpost is the generated
test `tests/caret_watch.rs`.

The worker's choice between adopting and following again is
pinned by the outpost's `a_focus_now_answer_is_followed_again_only_on_the_followed_focus`.

The intake (`src/intake.rs`) queues what the outpost still hears, its
`Follow` subscription's UI Automation events and a console host's console
events, with Core's queries, lists of held nodes and idle queries, in
arrival order: one waiting entry per object and kind, a batch ending at
an idle query, and the focused terminal's caret event after its text
change. Its rules are pinned by its own tests.
