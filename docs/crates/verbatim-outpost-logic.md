# verbatim-outpost-logic

The outpost's reducer (`docs/design/ownership.md`): the pure state machine
that holds every remembered fact of an outpost's application, with no
Windows type and no application object. It is a logic crate: in the GPL
tier, with no Windows dependency, so it builds and is tested on any host.
It depends on `verbatim-model`, on `verbatim-core` for `focus_rules`, and
on [verbatim-reader-rules](verbatim-reader-rules.md) for the moved-on
grace and the UIA focus wait it shares with the focus listener.

The outpost ([verbatim-outpost](verbatim-outpost.md)) runs it on a thread
of its own, its shell, which applies its effects and keeps its time; that
guide's "The outpost" section describes the reducer with the shell, the
worker and the mailbox around it, and its "Implementation notes" the
reducer's rules for each kind of event.

Public API:

- `Reducer` with `input(input, now, locals)`, which takes one `Input` in
  mailbox order, given the time (`Now`: the shell's monotonic clock, the
  observation clock and the shared clock) and its local reads, and returns
  the `Effect`s in the order the shell applies them; `tick(now, locals)`,
  for a timer that came due; and `next_timer(now)`, the time of the
  shell's timed wait.
- `Input`: an entry from a producer, a verdict, a commit, a walk armed or returned, a
  program finished, a release's answer, a worker returned, the gap
  record's bound, and the shell's close. `Effect`: dispatch a `Program`,
  send a message, release, move the subscription (`Subscription`), drop
  elements or objects, abandon a worker generation, publish the intake's
  marks or the verdicts, set the gap record's timer.
- `Locals`: the local reads it makes, which never wait on an application:
  a hung window, whether the user moved on, a walk's publication at its
  bound. The shell implements them live; the tests by a recording.
- `Program` and `Work`: what the worker generation in charge runs, with
  its copies of the facts (`Facts`), whose commits come back as `Change`s
  and are applied while their `Request` is live.
- The vocabulary of its state, used by the shell and the worker: the
  intake's `Entry`, `Item`, `Source`, `Marks` and `Object`; the follow
  state (`Follow`, `FollowState`, `Target`, `OpenWatch`, `Owed`); the
  reports Core may hold (`Held`, `Focus`, `Evidence`); the verdicts
  (`Verdicts`, `VerdictChange`, `Probe`, `Probed`); each node's text facts
  (`NodeAnchors`, `Anchor`, `TextChange`, `ReportCut`, `AnswerCut`); and
  `WinKind` and `UiaKind`, its own copies of the backends' event kinds,
  since it names no backend crate.

`cargo xtask ci` reads every file of the crate by the pure rule
(`xtask/src/ownership.rs`): no clock, no lock, no thread, no Windows type.

Tests: the reducer's unit tests drive it through `input` and `tick` with a recording
of `Locals` (`crates/verbatim-outpost-logic/src/tests.rs`, and each
module's own), so every ordering of inputs is a replay.
