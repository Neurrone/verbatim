# verbatim-replay

Replays a flight-recorder dump ([verbatim-core](verbatim-core.md), "The
dump format") through Core's reducer and renders each input with the
effects it produced on one line. A tool crate: its binary is what `cargo
xtask replay` runs ([the end-to-end guide](../../how-to/e2e.md), "Replaying
a run"), and the end-to-end suite renders the flight recorder's inputs in
a failure's report with its library. It builds on every host, since
nothing in it is Windows-specific. Its API is in its rustdoc (`cargo doc
-p verbatim-replay --no-deps --open`).

## The replay and its lines

- `replay_dump(contents)` replays every input of a `DumpContents` from its
  base state with `verbatim_core::replay` and returns one `Replayed` per
  input: its line in the file (`FIRST_INPUT_LINE`, 3, is the first, after
  the header and the base state), the input with when it arrived, the
  effect count the live
  session recorded, and the effects the replay produced. `differs()` is
  true when the two counts disagree: the code changed since the run, or
  the reducer is not deterministic.
- A line renders its input and effects with the built-in default theme,
  its sounds found in the repository's `sounds` folder. An input carries
  a trace when a `trace_id` or `trace` field at any depth holds it.
- `failure_inputs(replayed, renderer, trace, from_us, to_us)` is what the
  end-to-end suite's failure report shows of a dump: each input carrying
  `trace`, and each that arrived from `from_us` to `to_us`, as a `Shown`
  with its line, the time it arrived and its rendered line, after
  `(failing trace)` for one carrying the trace.
- The binary prints a malformed dump's error, which names the line, and
  for a dump cut off mid-line ends with the last line it read.
- The binary first prints the commit the dump's build was made from and
  the commit it was itself built from, which its build script captures as
  `verbatim-app`'s does ([verbatim-app](verbatim-app.md)), each with
  whether its working tree had uncommitted changes, then a warning when
  the commits differ, either is unknown, or either build had uncommitted
  changes, since any of these can change effect counts as a change in
  behaviour does:

  `the dump was built from commit 4eb4c413d89889a844ff4d82f0f945b7ba072d02`

  `this replay was built from commit 82187a9706656747c95a6199d8df5043ee6ff72b`

  `warning: this replay was built from a different commit than the dump; effect counts may differ for that reason`

## The line

A line is the input's file line, a colon, when it arrived, the input,
then its effect count and its effects, separated by semicolons:

`5: at_us 3000, Queue trace 3, input Tell from (generation 1, reader "Listener"), tell (...). 2 effects: Queue Send (...); Queue WakeAt 503000`

- An input is its variant's name, then its fields, each a name and a
  value, `trace_id` written `trace`: the trace, the source and the times
  first, then the rest by name. An event names its kind after `Event`,
  then its node, then its own fields. Fields with no value (null, an empty
  list) are left out.
- A node is its role and name, a node id its outpost and number (`0:1`),
  a list its length (`3 items`), and another structure its fields in
  parentheses.
- An utterance is `Speak` and the text Verbatim reports as queued for it,
  a theme's sound as `sound:` and its indication, as the end-to-end suite
  hears it, with its priority when it is not queued; an earcon is
  `PlayEarcon` and its name; anything else is its variant and fields.
- A text longer than 80 characters is cut to its first 80, followed by
  `...`, and its length is given after it.
- When the replay's effect count differs from the dump's, the count is
  followed by `(differs: the dump recorded N)`.

Fields are rendered from each input's serialized form, so a new input or
effect is rendered without a change here; only the special cases above
are written by hand.
