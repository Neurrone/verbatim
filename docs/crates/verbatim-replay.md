# verbatim-replay

Replays a flight-recorder dump ([verbatim-core](verbatim-core.md), "The
dump format") through Core's reducer and renders each input with the
effects it produced on one line. A tool crate: its binary is what `cargo
xtask replay` runs ([the end-to-end guide](../tooling/e2e.md), "Replaying
a run"), and the end-to-end suite renders the flight recorder's inputs in
a failure's report with its library. It builds on every host, since
nothing in it is Windows-specific.

## Public API

- `replay_dump(contents)` replays every input of a `DumpContents` from its
  base state with `verbatim_core::replay` and returns one `Replayed` per
  input: its line in the file (`FIRST_INPUT_LINE`, 3, is the first, after
  the header and the base state), the input, the effect count the live
  session recorded, and the effects the replay produced. `differs()` is
  true when the two counts disagree: the code changed since the run, or
  the reducer is not deterministic.
- `Renderer::new(sounds)` renders with the built-in default theme, finding
  its sound files in `sounds`, the repository's `sounds` folder. `line(replayed)`
  is one input's line; `effect(effect)` one effect.
- `describe_input(input)` is an input's part of the line, and
  `input_time_us(input)` the time it carries, in microseconds.

## The line

A line is the input's file line, a colon, the input, then its effect
count and its effects, separated by semicolons:

`3: Event FocusChanged, node Slider "Rate", whole_window false, trace 1, source 700, observed_at_us 0, backend "Uia". 2 effects: DropExpiredSpeech; Speak "Rate slider 50"`

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
  hears it, with its priority when it is not queued; a query is `Fetch`,
  what it asks, the node and the query's number; an earcon is
  `PlayEarcon` and its name; anything else is its variant and fields.
- A text longer than 80 characters is cut to its first 80, followed by
  `...`, and its length is given after it.
- When the replay's effect count differs from the dump's, the count is
  followed by `(differs: the dump recorded N)`.

Fields are rendered from each input's serialized form, so a new input or
effect is rendered without a change here; only the special cases above
are written by hand.
