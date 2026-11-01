# verbatim-faults

Fault points: the hand-offs whose failure paths held defects, made to fail
on demand so a test runs the production failure path
(`docs/design/logic-foundations.md`, "Failure paths and locks"). It is a
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
  index)`).
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
fault step on Windows: clippy of each of those crates and mockapp with the
feature, one package at a time, then the fault test targets
(`verbatim-supervisor`'s `fault_paths` and mockapp's `failure_paths`),
each declared with `required-features = ["faults"]` so the ordinary steps
skip it, into `target/debug`. The points of `verbatim-app`'s hook posts and
of `verbatim-listener`'s reading thread are `cfg(test)` only, reached by
those crates' own unit tests.

Tests: unit tests check that a point fires only for the subject and type
selected and only as often as armed, and that a guard whose point never
fired fails its test.
