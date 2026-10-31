# The ownership rules for the planned milestones

The ownership refactor's builds are all built: [the one judge build](ownership-one-judge.md), [the input build](ownership-input.md), [the messages build](ownership-messages.md), [the output build](ownership-output.md) and [the outpost's reducer build](ownership-outpost-reducer.md), under [the ownership model](ownership.md). What remains of the plan is the list below: the rules each planned milestone follows under the model, to be moved into the roadmap's milestone sections as each is designed. The evidence list names the commits behind each rule; the study, the audits and the reviews are in `experiments/ownership-study/`.

## Rules

- The caret and selection rework (`docs/design/caret-selection-rework.md`).
  - The last read per node and the pinned baseline are reducer state, released by the release rule.
  - Caret and text events are read for every node the outpost has not released, not only its newest focus, and Core decides which is spoken.
  - A repeat is read in full and compared with the last read, and Core's choice between a repeat and a new focus decides what is spoken, which removes the Control+A loss.
  - The key request is a stream responder; the 100 ms and 1-second bounds are reducer timers.
  - The auto-repeat mark, the edge facts and the chords' gesture travel with the report.
  - The clause against merging a caret report across a reply becomes the barrier on replies.
- M4.3.
  - Each setting has one owner and reaches it as a delta, and every setting writes through the saver.
  - The hook's settings and gesture map are one snapshot, loaded once per key.
  - Cancelling expired focus speech is Core leaving out `DropExpiredSpeech`.
- M5.
  - The loader owns the rule tables, configuration with a generation.
  - A listener veto names its generation, and the outpost's rules apply to recovery reads under its current generation.
  - A module's `focus()` is the outpost's newest report of a control, its own fact; a module that reacts to changes is given every change of a node the outpost has not released, and Core decides what is spoken.
  - A hook runs on a worker in a call program, and its answer is a reducer input.
  - The wasmtime store is owned by the worker generation in charge, and abandonment while it is lent is the module's fault: the store is discarded and the module instantiated again.
  - Load, swap and `on-command` are queries with responders.
  - A module's speech is a report judged against attention.
  - Module commands enter by the one input route.
  - Sleep mode is a view keyed by the foreground window.
  - Application identity is one function of the held process.
  - The application rules move into modules once.
- M6.
  - One owner per host, with users holding leases.
  - A non-active synthesizer's voices are a query.
  - Adapter callbacks only write frames.
  - The host's owed state is a typestate.
- M6.1: MSAA and IA2 only, since UIA cannot be cancelled.
  - The worker calls `CoEnableCallCancellation` at its start.
  - The worker publishes its current call ticket under a ticket lock as it enters and leaves each call.
  - The canceller holds that lock across `CoGetCancelObject` and `ICancelMethodCalls::Cancel`, and does nothing for a ticket no longer current.
- M6.2: starting an outpost generation takes proof that the previous one exited; a marker's outcome names its generation.
- M6.3.
  - The queue thread owns each utterance's ending, and the mixer only each attempt's.
  - `Source::register` consumes an admission token (3ebb7be0).
  - The awake period is configuration, under the real-time rule.
- M6.4: replies name the start they read from; an owed read has a timer input.
- M6.5: the classic edit control's drop of a caret event equal to the last read is a skip on the reader's own record, which is allowed.
- M7.
  - The helper talks only to its outpost.
  - A buffer in the helper sends numbered one-way updates.
  - Calls to the helper can be cancelled with `CancelIoEx`.
  - The helper's UI-thread work is thread-affine.
  - The store keys IA2 nodes by window and unique id.
- M8.
  - Core decides the active profile.
  - A switch is one transition whose effects carry its generation, with the theme and synthesizer settings ordered before the `Speak` the trigger caused.
  - Triggers are suspended while the settings dialog is focused.
  - Overlays are written only by reducer effects.
  - An OCR read is a query.
  - Words stay in Core.
- M8.1: two instances share no writable fact; the integrity of the window in front stays the listener's report.
- M9: logs are one-way and sent in batches; the Verbatim+F1 report is a query.
- M10: the console reads Core only by queries; its node reads are requests with responders.
- M11: a theme switch is an input and an effect ordered with `Speak`.
- M12: remote input enters the one route; the protocol is versioned and validated.
- M13: Java objects are released by the release rule; establish first whether the bridge is thread-affine.
- M14: drawing threads only append; one helper thread owns the model, sending numbered snapshots that Core judges.
- M15.
  - Core alone decides content and tethering.
  - Braille keys enter the one route with the buffer generation and a key number.
  - Speech and braille stay in step through the existing marks.

## Evidence

- Facts with more than one owner, and their defects: `architecture-study.md`, section 1.
- The back-channel's chain: 43de3932, 8fe59b1b, d893e07f, c0bbf19b; and `unbuilt-focus-designs/core-flow-review.md`, findings 1, 3 and 5.
- The foreground: b1917eed; audit difference 12; 06594d66 and 9661f0d8; 5f1c8e81, 3578a437, e60a7a58, 38cab859.
- The gap's moves: 01cba97e, ad78f2a8, 9ba7625b, 5242658c, 28be3d08, 22256896, f2f8209a, e02c7854.
- The publication's rounds: c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7.
- The listener's scheduler: 5204f351, e46d4183, 0b5826cd, 073d63b0, 69c06239.
- Verdicts: 094a8dc8, 07db29a8, a3d6bc23, 248f85d3, defect B.
- Identity: b94d2ea2, 9fd28a66, 8826d457, 5797c075, and defect A from 3abda293 with 10c9e83a.
- Lifecycle: 1a3dc022, 8d01bbd1, a39f01f8, de7f481e, df4c15d9, ded1988a.
- Requests: 886d8e73, 76180512, 348262b8.
- Merges: dd363ecc, 913bb0e8, 89bbbc7c, 71322fae, 06256225.
- Lent state: d0ec3a89, the anchors' follow-up, 2bd69a85's terminal copy.
- Input order: bf4fbedb, ba7064e5, e180f14a, 04214e7f, 49fcca0e.
- Output: 3ebb7be0, 21fb0e51, 6e303a61, 0d791a13, 2bdfffb7.
