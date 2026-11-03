# verbatim-faults

Fault points: the hand-offs whose failure paths held defects, made to fail
on demand so a test runs the production failure path
("Failure paths", below). It is a
Windows-tier crate that depends only on `workspace-hack`, and only tests
build it: every point is compiled under `cfg(any(test, feature = "faults"))`
in the crate that places it, so a release build and the end-to-end build
carry none.

Public API:

- `Fault` — each point, and the subject it passes: `PortRefusesPost`
  (the item a port queue's `Poster::send` posts), `RelayWake` (the item
  the relay's mailbox sends), `KeyPostRefused` (the keyboard hook's post),
  `ThreadStart` (a reading thread's `(name, pid, generation)`, a worker's
  `(name, generation)`), `RemoveGroup` (`(registration, runtime id)`),
  `AddFocusHandler` and `RemoveFocusHandler` (`(registration, handler
  index)`), and three observation points: `ShutdownWaitsForCalls` (an
  outpost's target process id), where an outpost's shutdown begins to
  wait for the calls in progress; `RegistrationCallPassedTimeout` (the
  registration's id), each of `verbatim-uia`'s registration calls whose
  time passed UIA's connection timeout, since such a call reports
  success; and `CaretWatchEnded` (the reducer's reason, a `&'static
  str`), where an outpost's shell carries out a caret key's watch ending
  without evidence.
- `arm(fault, selects, times)` — arms a point to fire `times` times, each
  at a call whose subject, of the type `selects` takes, `selects` picks,
  so a test names the one call that fails. Returns a `Guard`, whose drop
  disarms the point and asserts it fired exactly `times` times, since a
  fault that never fired proves nothing. `Guard::fired()` counts the
  firings so far, and `Guard::wait_fired(timeout)` waits for the last of
  them, the evidence that the failure has happened.
- `hit(fault, &subject)` — what a point calls immediately before the
  operation it stands for: true when an armed selector picks the subject
  and has firings left, and the caller then takes the path the real
  failure takes, with the error the real call returns.
- `reach(fault, &subject)` — what an observation point calls where
  nothing fails: it counts as a firing of every armed point whose
  selector picks the subject, even past the times armed, so a test waits
  for the evidence that the code got there with `Guard::wait_fired`, or
  counts how often it did, and the guard's drop fails a test whose code
  got there more often than armed; the code goes on either way.
- `hit_with(fault, subject)` — `hit` with the subject built by the
  closure `subject` only while `fault` is armed, for a subject whose
  making is itself a call, such as reading an element's runtime id, which
  the build without fault points does not make.

How it works: one process-wide table of armed points under one lock, and
a condition variable notified at each firing. A subject of another type
than the selector's never matches. Tests that arm points run one at a time
in their binary, or select subjects no other test of the binary makes.

Where the points are and who turns them on: the crates whose points
another crate's tests reach have a `faults` feature, `verbatim-process`,
`verbatim-supervisor`, `verbatim-outpost` and `verbatim-uia`, and mockapp's
`faults` feature turns on optional normal dependencies on the last two,
since a feature cannot name a dev-dependency. Nothing turns a `faults`
feature on but `cargo xtask faults`, which `cargo xtask ci` runs as its
fault step on Windows: clippy of each of those crates, `verbatim-audio`
and mockapp with the feature, one package at a time, then the fault test
targets (`verbatim-supervisor`'s `fault_paths` and mockapp's
`failure_paths`, `shutdown`, `terminal_reading` and `time_limits`), each declared with `required-features =
["faults"]` so the ordinary steps skip it, and `verbatim-audio`'s `mixer`,
into `target/debug`. `verbatim-audio` is platform-neutral, so it cannot
depend on this crate: its `faults` feature has no dependency and compiles
its own observation point, a producer's wait for room
([verbatim-audio](verbatim-audio.md), `SourceControl`), and the three
`mixer` tests that wait on it are compiled only with the feature, the
others running in both steps. The points of `verbatim-app`'s hook posts and
of `verbatim-listener`'s reading thread are `cfg(test)` only, reached by
those crates' own unit tests.

Tests: unit tests check that a point fires only for the subject and type
selected and only as often as armed, that an observation point reached
counts as a firing, that one reached more often than armed fails its
test, and that a guard whose point never fired fails its test.

## Failure paths

The rules the failure paths follow, each with its reason:

- A fault point stands immediately before each hand-off whose failure path held a defect, so a test runs the production failure path. The points compile only into tests, behind a Cargo feature that only `cargo xtask faults` turns on, absent from release builds and the end-to-end build; the fault tests are built in `target/debug` beside the shipped build and rewrite only `mockapp.exe` (Dickson, 2026-10-31).
- An outpost whose worker cannot start, at its start or as a replacement, keeps running: the generation stays without a worker, each program is answered as failed, each dispatch tries once to start one, and the shell keeps its garbage until a worker starts (Dickson, 2026-10-31).
- A post a port refuses is kept and posted again ahead of the next one, by the relay's outbox and on the hook thread, with no timer. Reason: a refusal comes only from a system out of memory, and a timed retry would wake on a clock with no evidence.
- A wake is decided by reading the state under the lock that holds it, never from a flag another thread sets: the relay's mailbox, the cancel token and the harness's readiness keep their facts under one lock, and every audio device notice wakes the audio thread, which decides from its pass. Reason: two setters of separate flags can each miss the other's. A mark that could stay set wrongly, the GUI's `switching`, clears through a `Responder` whose drop answers.
- The debug-build lock check, which fails a call into an application made while one of the outpost's locks is held, is described in [verbatim-win32](verbatim-win32.md), `held_locks`.

Limits: a message the app's port refuses, or a mailbox wake the relay's port refuses, waits up to a ping interval, 3 s; a key's post a port refuses waits for the next key, with no time limit, and a harness's `AwaitIdle` then fails at its timeout naming the input; both happen only when the system is out of memory. A fault point stands for the operation's failure, not for the system's state behind it. The synthesizer switch's responder cannot answer while the synth thread holds the request in a driver call that never returns.
