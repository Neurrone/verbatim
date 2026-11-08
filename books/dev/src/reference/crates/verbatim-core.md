# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format. What each behavior says, and why, is in the
parity ledger (`docs/parity/`) and the NVDA prose (`docs/nvda/`); this
guide maps those behaviors onto the crate's types and modules.

## Public API

- `reduce(&mut state, &input)`: deterministic, no I/O, no clocks; advances
  the state in place, without copying it, and returns the effects to
  execute. Every `Speak` carries the triggering input's trace id, which
  is what makes end-to-end latency timelines possible, and the number of
  the user input that caused it, if one did (below, "Key numbers").
- `SrState::holds_inputs_back` and `SrState::announces_toggle_key`: whether
  a key or command waits in Core's held-key line, and whether a lock key's
  announcement is still to come, which the shell's idle conditions read.
- `SNAP_RESULTS_ACTIVITY`: the activity id of the shell's window-snap
  results notification, the one UIA notification spoken from an
  application without the focus; the supervisor starts an outpost for no
  other notification.
- `focus_rules`: the focus rules the reducer shares with the focus
  listener and the outposts, each defined once (`books/dev/src/explanation/architecture.md`,
  section 2, "Where the rules run (D14)"). The reducer applies them after
  an event is read; the listener and the outposts apply them where a
  judgment must come before an element is read, against the focus Core
  holds. The rules ported from NVDA that only the listener and the
  outposts make, the acceptance test and NVDA's foreground tests, are in
  [verbatim-outpost](verbatim-outpost.md), since ported code stays in the
  Windows-specific crates.
  - `ForegroundRelation`: how a window stands to the foreground window (in
    it, topmost, or a `Windows.UI.Core` window under the active window),
    which an outpost reports in an event's window facts, for the
    attendance of events other than focuses; Core judges a focus by its
    foreground record's front instead.
  - `RepeatEvidence` and `repeats_focus(evidence)`: a focus repeats the
    held one when it names the element of the focus Core holds and that
    element still has the focus; only Core judges repeats.
  - `ChangeKind`, `ChangeTarget`, `change_target` and `change_candidates`:
    a name, description, or value change is spoken on the focus, a state
    change on the focus or an ancestor whose state changes are spoken.
    `SelectionTarget`, `selection_target` and `is_selection_container(role)`:
    a selection of the focus or an ancestor is a change of its states; any
    other is an item spoken only inside a focused list or tab control of
    the same outpost.
  - `is_sensitive_slider(node)`: whether a node is a UI Automation slider
    of class `SensitiveSlider`, which gives the focus to its thumb; the
    reducer speaks it entered as a focus and its value changes while it is
    the focus's parent, and an outpost follows its value changes while it
    is (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
  - `same_object(old, new)` with `EnteredKey`, which gives an entry's
    `ObjectKey`: whether an entry of a previous focus line and one of a
    new line are the same object by `ObjectKey::is_same_object`, one node
    id, or one window's object, of one backend or a stand-in above the
    process boundary, when both are windows.
    `meeting((previous, previous_focus), (new, new_focus))` gives where a
    new focus line meets the previous one, as NVDA's `api.setFocusObject`
    finds it, by `same_object` alone, the indexes of the two entries, or
    `None` when the lines share no object; the reducer keeps the previous
    line to that entry and enters the new entries below it.
    `already_entered(previous, new)` counts the new line's entries,
    outermost first, at or above where it meets `previous`, by which an
    outpost gathers a newly entered dialog's own text. Neither allocates.
  - `HeldNode`: one node of a focus line as an outpost records its
    reports (id, backend, role, name, states, the window a node is, when
    it is one, and its stand-in mark); `HeldNode::of(snapshot)` builds one
    from a snapshot.
- `is_selection_container(role)`: re-exported from `focus_rules`.
- `SrState`, the reducer's state:
  - `new()`.
  - `focused()`, the live focus and its application; `focus_source()`, the
    application the focus belongs to; `focus_report()`, the focus's
    snapshot, its ancestors, and the navigator object, which the control
    plane's `DumpFocus` answers with.
  - `latest_focus_observed_at()`, when the newest focus the reducer took
    was observed, in microseconds (the focus object's own observation
    time, `None` when it is not known), which lets the
    shell skip a fake focus after a menu closes when a later focus has
    been taken.
  - `attention()`, the application holding attention, which the shell
    sends to the supervisor.
  - `held_nodes()` and `held_anchors()`: every node id and every text
    anchor the state refers to, grouped by the outpost that issued it
    (the anchors are the caret's line and selection, the review cursor's
    line or point, the start marker, and say-all's positions). The shell
    sends each outpost its own, which keeps them alive
    ([verbatim-model](verbatim-model.md), "The text protocol").
  - `fetches()`: the details the active theme wants fetched, as the last
    `Input::Fetches` set them (everything until then), which the shell
    gives every outpost so that a detail whose indication is off is never
    fetched ([verbatim-model](verbatim-model.md), "Themes"). It is the one
    place the reducer consults the theme.
  - `settings()`: the reader settings as the reducer has them, toggle keys
    included, which the shell merges the settings dialog's Terminal page
    changes into.
  - `holds_caret_keys()` and `hook_foreground()`: what the keyboard hook
    is told after each input (see "Held caret keys" and "The window in
    front" below).
- `flight_recorder` and `FlightRecorder<T, S>`: see "The flight recorder".
- `RecordedInput`, `ReducerRecorder` and `replay(initial, inputs)`: see
  "The flight recorder".
- `dump`: `DUMP_FORMAT_VERSION`,
  `UNKNOWN_COMMIT`, `DumpHeader`, `DumpContents`,
  `DumpReadError`, `write_dump` and `read_dump`; see "The dump format".
- `read_of(&TerminalSnapshot)`, `snapshot_of(&Read, waited, colours)` and
  `read_at(&TerminalSnapshot)`: see "Core's terminal model".
- `supervisor`: Core's supervisor's pure decisions, which
  [verbatim-supervisor](verbatim-supervisor.md)'s owner carries out.
  `generation::Generation` is one application's outpost generation as
  typestates (`Launching`, `SpareGiven`, `Starting`, `Ready`, `Waiting`),
  each transition consuming the state and returning the next with its
  `Notice`s in order; the app's notices among them are a plain
  `Lifecycle` (`Started`, `Ended`, `Regenerated`, `NotWatched`), which only
  the supervisor's owner turns into the sealed message the app takes as
  the supervisor's own. A running generation's liveness (`Live`) is
  private and changed and judged only by its transitions: `ponged`, which
  keeps the pong's time and its `policy::Parked` counts, and
  `heartbeat(now_us, limits)`, which answers `Heartbeat::Ping` or
  `Wedged(reason)`. `policy` decides when an outpost is wedged
  (`wedge_decision` under `WedgeLimits`, which counts only the workers
  parked on windows the outpost did not judge hung, `Parked::other`, the
  outpost being the one judge of a hung window; and `Liveness` for the
  listener and the spares, which report no parked workers), when an idle one is
  retired (`retirement_decision`), how long a failed outpost's
  replacement waits (`Backoff`), in which order the applications whose
  waits ended start (`due_starts`: in the order the waits ended, then by
  process id), how facts that arrive during a start are held and merged
  (`HeldFacts`, which keeps the time it is given with the first fact
  held; `docs/parity/backends.md`, "MSAA winevent flood control"), which
  facts may start an outpost (`may_start_outpost`;
  `docs/parity/focus-events.md`, "Background events"), and which spares
  are kept and given (`Spares`). Every time these take is in
  microseconds on the run's shared clock. `EndReason` is why a generation
  ended; `Received` is when verbatim.exe read a fact.
- `requests::RequestTable<A>`: the one owner of each query the shell
  sends an outpost for its own askers and of its one outcome, generic over
  who asked (below, "The request table"). `SrState::outstanding_queries`
  is the count of the reducer's own queries the idle barrier waits for
  (below, "Core's queries").

## Core's queries

Every query the reducer asks, of an outpost or of a terminal's memory,
is recorded in one field of `SrState`, `queries` (`src/queries.rs`), from
its request to its one ending, under the `QueryId` its request goes out
as (`protocol::RequestId::reducer`). An entry holds the outpost asked,
the trace its request runs under, who answers it (`Asked`: `Read`, which
the idle barrier waits for, `Watch`, a caret key's watch, which it does
not, or `Memory`, a request Core answers from a terminal's memory), and
what its answer continues (`Then`).

- One way to ask: `Queries::ask(asking, then)` records the query and
  returns the request's effect (`Effect::Fetch`, `Effect::Text` or
  `Effect::Activate`); `ask_memory(node, then)` records one only Core
  answers, a command mark's move. `terminal_model::intercept` turns a
  request it takes out of the effects into a `Memory` one
  (`answer_from_memory_later`).
- One ending each, after which the continuation runs once, routed by the
  entry's `then` (`queries::ended`): the outpost's answer,
  `Input::Answered`, taken only from the outpost asked and never for a
  `Memory` entry (`queries::answered`); the outpost's end, inside
  `Input::OutpostEnded`, where every entry still open there ends "gone" in
  number order, under its own trace or the end's (`outpost_gone`); its
  owner's withdrawal, the held key's read at its deadline
  (`Queries::withdraw`, `Effect::Withdraw`); and Core's answer from
  memory (`answer_from_memory`), or the removal of the terminal's model,
  which ends its reviews' entries with nothing run
  (`SrState::drop_models`, the only way a model goes). An answer with no
  entry is dropped. An ending whose number is the watch the held keys wait
  on (`HeldKeys::awaiting_watch`) resumes them, however it ended.
- The outcome becomes the result the entry's kind takes: a navigation's
  `FetchResult` (`Gone` for a gone node, `Unanswered` for any failure), an
  activation's action, said "No action" for a failure and nothing for an
  abandoned one, which includes one refused because its window is hung, or a `TextReply` (`Gone`, or `Unanswered` for any
  failure).
- A query superseded or cleared is turned to `Then::Nothing`
  (`Queries::forget`): its entry stays until its answer, which runs
  nothing, so the idle barrier still waits for a superseded read; a
  `Memory` entry is removed at once instead. A newer caret key on the same
  node turns the waiting key's entry into `SupersededCaret`, whose answer
  may still say a change of the selection; a cancel turns the flood
  policy's open read into `Overtaken`.
- A terminal model's asks (owed, fresh, history and search reads) are
  entries tagged with the model's node and generation, which each model
  gets from a counter kept with the models (`Terminals`), so a model made
  again for the same node takes none of its predecessor's answers. Its
  questions are lookups (`model_asks`). An owed read asked while one is
  open turns the older to `Nothing`.
- `reduce` ends each transition by setting the trace of the queries made
  in it to the trace the shell runs their requests under: the key's inside
  `Effect::ForKey`, else the input's (`set_traces`).
- `held_nodes` takes the node a navigation starts from and the nodes of
  the caret keys', text commands' and held keys' entries; an entry with
  `Nothing` holds nothing.

What the shell and the outposts guarantee the reducer here, which its
generated test relies on and gives no stronger: the focus listener's
foreground stamps never decrease, though a focus may be stamped as late as
one; answers do not come in the order of their requests in general, and an
answer may come from another outpost than the one asked or come again,
both of which Core drops; every outpost generation of one application
reports its process; an outpost generation's id is never given again,
and once it has ended (`Input::OutpostEnded`) nothing more of it comes.
That Core never asks an ended generation is checked, not assumed.

Tests: the query table's unit tests, every sequence of up to three steps
from a text box, a held key's watch and a console, enumerated against the
table's invariants (`crates/verbatim-core/src/queries/tests.rs`), and a generated test of the
reducer's slice of queries, focus lifecycle and generations against a
reference model of sets and lists (`crates/verbatim-core/src/queries/generated.rs`): lists of actions run
through proptest's `TestRunner::run` under a fixed seed and 300 cases
(about 6.8 s in a debug build), each made valid against the model as it
is applied. The actions are focuses from the oldest or newest live
generation on a text box, a button or the terminal, with what the reader
read of the element it held, in the front's window or with no window
facts, observed now, before, or at no known time; terminal snapshots,
some with a ruler that changes on every such read, and the first
snapshots of 16 other terminals; the terminal's timer at its due time;
answers to the oldest or
newest open request, done, gone or failed, and at low weight to one
ended, to one never asked and from another outpost; caret keys, held keys
and their due times, typing and speech cut off; review commands; foreground
changes, now or at the newest focus's stamp; outposts' ends and new
generations. Focuses are generated only in the front's window or with
no window facts. After every input the enumeration's invariants are checked
against the wire's view of the requests, and the model's: every open
entry but Core's own names a live generation and no effect asks an ended
one; ids only rise and an ended id never returns; each new entry's trace
is its input's or its key's, and an older entry's is unchanged; the held
keys' awaited watch names an open entry; no held node is of an ended
generation; a focus is taken, its node alive and not left, or changes
nothing of the focus and says nothing; the focus is left only at a
foreground change and back only when a focus is taken; at
most 16 models, each generation new when made and none of an ended
outpost, one going only when evicted as the oldest for a new one, its
node's text forgotten or its outpost ended, the others in their order of
reading; and every memory entry a live model's. A foreground change gives
no effect and changes nothing but the foreground record and whether the
focus was left; the record then holds the change as the hook's view,
the front and attention, and keeps its entered window, object and
stamps; which focus is stale, a duplicate or left is left to the
reducer's hand-written examples;
an outpost's end asks nothing of it and leaves nothing of
it, and another outpost's entry ends then only by a withdrawal; an answer
to a model no longer live runs nothing; speech cut off asks nothing; the
terminal's timer, fired at the due time the state gives, is due later
than that or not at all. The
state round-trips through its serialisation after every action, each case
runs twice with the same effects, ends with every request answered and
only Core's own entries left, and the gate fails if any listed state is
never reached. Once a case is quiet, no field of the state names an
ended generation but those `ENDED_MAY_REMAIN` lists, each by its exact
path with why it is kept (the dead focus's copied data, the foreground
window's object, the newest focus stamp and the focus's formatting
cache): the state is serialised through a serializer that finds every
`OutpostId` by its type's name, in a node id, a tuple or a map's key
(`crates/verbatim-core/src/queries/ended.rs`), so a field added later is
checked with no change to the check. A failure's shrunk case is kept in
the test's `found()` list, as the failure prints it (`books/dev/src/reference/testing.md`,
"Exploration tests"), and so is a failing sequence of the query table's
enumeration in its own `found()`. Three fixed cases keep what the
generated cases reach too seldom: a model's request answered after a
newer model of the same terminal replaced it, a foreground change at the
newest focus's stamp, and the terminal's timer fired once a changing
row armed it.

## The request table

`RequestTable<A>` records every query the shell sends an outpost for its
own askers (a tree dump, focus-now, the fake focus, and the two shell
item queries), on the reducer thread, with a `RequestId` tagged
`Requester::Shell` and the outpost generation it went to, and holds its
asker `A` as a token it never calls. `verbatim-app` gives its `Asker` and
carries each outcome out (`requests::deliver`,
[verbatim-app](verbatim-app.md)).

- `begin` records a request. The table keeps no deadline: an outpost
  answers each query by its program's deadline, or the query's own
  `answer_by_us`, so a clock here would be a second path to its one
  outcome.
- Every ending hands the asker back with its outcome, so each asker leaves
  the table exactly once: `finish(id, outpost, outcome)` only for the
  generation asked and only the first time; `outpost_ended(outpost)`, each
  outstanding request answered "gone". `find(which)` names a request to
  withdraw from its outpost while it keeps its one outcome.
- Request numbers are given in increasing order and kept in a
  `BTreeMap`, so the outcomes `outpost_ended` returns together come in the
  order the requests were made.

## The flight recorder

The `flight_recorder` module is the always-on, bounded recording of recent
reducer history (architecture section 9). It runs on the reducer thread,
so it never blocks, never touches disk, and never grows past its bounds;
nothing is persisted until a crash handler or a user gesture takes a dump.

`FlightRecorder<T, S>` holds a window of recent entries of type `T`,
bounded both by count and by estimated bytes, with a checkpoint of type
`S` taken just before its oldest entry, because a window of inputs
replays only from the state it started in. The window is a list of
segments, each starting at a checkpoint:

- `new(max_entries, max_bytes, initial)` starts an empty window whose
  checkpoint is `initial`.
- `record(entry, bytes, checkpoint)` appends to the newest segment, starts
  a new segment, calling `checkpoint` for the state right after this
  entry, once the newest holds half of either bound, and drops the oldest
  segment whole while the window is over either bound. An entry larger
  than the whole byte bound is not kept.
- `checkpoint()` and `entries()` read the window; `len()`, `is_empty()`
  and `bytes()` measure it; `max_entries()` and `max_bytes()` give its
  bounds.

So the window always starts at a checkpoint, holds between half and all
of each bound, takes a checkpoint once per half window, and keeps at most
three alive. Their size is not counted against the byte bound; the
state's large parts that exist today (the ancestor chain, the text lines,
the terminal readers) are held behind `Arc`s, which a clone shares.

`ReducerRecorder` is `FlightRecorder<RecordedInput, SrState>`:
`with_default_bounds(initial)` uses the shell's bounds,
`DEFAULT_MAX_ENTRIES` (1,024 inputs) and `DEFAULT_MAX_BYTES` (8 MiB, which
ordinary inputs never reach, so text-bearing inputs are what it caps);
`record_input(input, effect_count, &state_after)` clones the state only
when a checkpoint is due; `dump_inputs()` gives the recorded inputs,
oldest first, ready to replay from the checkpoint.
`RecordedInput::estimated_bytes()` is the entry's inline size plus the
heap it owns, found by `HeapCounter`, a serde serializer that formats
nothing and adds each string's length and each element's inline size. It
is kept over counting the entry's JSON because its cost is constant per
string, where formatting JSON scans every byte of a terminal document on
the reducer's step before its speech.
`replay(initial, inputs)` re-runs a recorded sequence and returns the
effects per step; tests prove it deterministic, and prove that a window
that has dropped its start replays from its checkpoint to the effects the
live session produced. `RecordedInput` and `SrState` derive `Serialize`
and `Deserialize`.

## The dump format

`dump` is a versioned JSON-lines file: a header line (`DumpHeader`: format
version, the writing crate's version, the commit the writing build was
made from and whether its working tree had uncommitted changes, and a
caller-supplied timestamp string; the writer captures the commit when it
is built and the module never reads a clock), then the `SrState` the
inputs start from, then one compact-JSON `RecordedInput` per line.
`write_dump(writer, crate_version, commit, uncommitted_changes, timestamp,
base, inputs)` writes one; `read_dump(reader)` reads one back as
`DumpContents` (header, base state, the inputs that parsed completely, and
whether the file ended mid-line). It reads only `DUMP_FORMAT_VERSION`
(9), and keeps no fallbacks for dumps older code wrote: a dump either
loads or reports its own error.

`DumpReadError` distinguishes a missing header, a malformed header, an
unsupported format version, a base state line that is missing or cut off,
and a malformed complete line. A malformed final line with no trailing
newline is not an error: the writer always ends a complete line with a
newline, so an unterminated tail can only be a dump cut off mid-write, and
`read_dump` reports it as `DumpContents::truncated`, keeping every record
before the cut.

`tests/replay_fixture.rs` replays a committed dump of a scripted
focus-change, value-change and states-change session under
`tests/fixtures/` on every test run, asserting each step's exact effects,
that their counts match what the dump recorded, and that a second replay
is identical; it is the template for live-dump regressions. Its
`#[ignore]`d `regenerate_fixture` test regenerates the fixture after an
intentional change to the scripted shapes.

## The allocation invariant

`tests/alloc.rs` has its own global allocator counting the bytes each
thread requests. It asserts that a focus step, a navigation command and
its completion, caret and review steps, and a terminal read each allocate
the same against a small state and a large one (a previous focus with 10
ancestors and with 10,000). It holds because `reduce` changes the state
in place.

## Focus, events and objects

Each behavior below is specified in the parity entry cited; the reducer
code is in `reduce.rs` unless named.

- Cutting speech off (`docs/parity/focus-events.md`, "When speech is cut
  off, and cancellation of expired focus speech"). Speech is `Queued`
  except a selection in a list the focus controls and a notification
  whose processing hint asks to interrupt; a key press cancels speech in
  the hook ([verbatim-input](verbatim-input.md)). On every focus change
  the reducer first emits `Effect::DropExpiredSpeech` with a `FocusNow`
  built from the new focus, its ancestors, and the state's foreground
  node, then `Effect::StopSpeech` when the focus brings a new top-level
  window or enters a menu bar, menu, or menu item. Speech for the new
  focus follows, so these never cut it off.
- The focus announcement (`docs/nvda/speech.md`, "Which states are
  spoken, and in what order", "When the role is spoken", "Where the level
  goes", "What an object with text says"). Context first, then name, role,
  value, states, description, keyboard shortcut, position in set, and
  level, each absent when the backend reported nothing. `spoken_states`,
  `negated_states` and `STATE_ORDER` with a `StateReason` of focus, query,
  or change pick the states; `Reason` and `speaks_role` decide the role;
  `last_tree_level` puts a changed tree level first, and `cell_memory`
  leaves out a row or column unchanged within one table. An edit field,
  document, or terminal says its text at the caret from `FocusChanged`'s
  `text` as a second queued utterance (`editing::focus_text`). Per
  decision D12 the name travels as a `Label` span and the value as a
  `Value` span, and every utterance carries its source node's role and
  rectangle (`UtteranceSource`). Each entered container is its own
  utterance with a `FocusValidity` whose `had_focus` is false, which never
  expires (`FocusValidity::holds`); the focus's own utterance carries
  `had_focus` true and is dropped once the focus moves outside it.
- Focus-ancestry context (`docs/parity/focus-context.md`, "Focus-ancestry
  context" and "Top-level windows in the ancestry"). A `FocusChanged`
  carries the ancestor chain, outermost first, complete to the desktop
  but for the entries the readers leave out as not presentable, so it can
  be empty; the reducer joins it to the previous focus line where they
  meet (`focus_line`) and speaks the presentable containers
  (`is_presentable_container`) below the meeting point. The meeting point
  is the first entry from the focus outward with a node id, or a window
  (`NodeDetails::window`) when both carry one, of the previous line, so a
  Settings page's frame, the frame host's own focus and a pane above the
  page's content, is entered once, and two windows of one role and name
  are two. With no such entry, two lines whose window facts name one
  top-level window meet there, empty chains or not, and speech is not
  cut; lines in two top-level windows, or a focus whose containers a rule
  drops, cut speech and enter the whole chain, which is how a window is
  announced when its foreground event was dropped. Everything below the
  meeting point is entered: nothing is matched by role and name, the
  outpost giving an MSAA object reached by another path the id NVDA's
  equality finds it to have. A duplicate of the focus keeps the line,
  taking only its newer readings: a report of the node Core holds is one
  when its `earlier` evidence says the element its reader held under that
  node still has the focus, or, with no evidence, while the node is alive.
  A report whose evidence says that element lost the focus or is gone is
  no duplicate, and Core first forgets the node's caret, its waiting caret
  key and its terminal's model, so the new element's text is taken
  afresh; for one gone the lines meet above it, a new element, and for
  one that only lost the focus they meet at it, as UI Automation compares
  two elements by runtime id. `HeldNode` carries a window's handle, the
  backend and the stand-in mark as the snapshot does, so an outpost's own
  use of the rule, for a dialog's text, judges by `already_entered`.
- The window in front. `SrState::hook_foreground()`
  (`verbatim_model::HookForeground`) is decided from the window the focus
  listener last reported (`Input::Foreground`), one of Verbatim's own
  included, and published by the shell after each input: keys may be held
  only when the window's process integrity is known and no higher than
  Verbatim's, and a console host's keys are translated with the layout of
  the thread its event named (architecture section 5).
- Rules for particular applications' windows (`applications.rs`; decision
  D6 keeps them in Core until application modules run in the extension
  host). The shell's staging windows are taken as focus silently
  (`docs/parity/windows-and-processes.md`, "The shell's staging windows");
  a UIA focus on the task switcher's frame is refused whole, and its item
  reached with Alt held is taken with no containers
  (`applications::drops_ancestry`; "The Alt+Tab switcher's pane").
  `tests/reduce.rs` checks all three.
- The foreground change (`Input::Foreground`;
  [ADR 0001: Focus](../adr/0001-focus.md), "The foreground and the
  window title"): the focus listener's report that a window
  came to the foreground, the only one, applied at one point
  (`foreground_changed`). It gives the keyboard hook's view and, unless
  the window is one of Verbatim's own, attention, its process and window,
  and the record's front, its window, root owner and stamp. It applies
  to the last queued focus (below), the left mark's one home: Core keeps
  the change with that focus (`focus_queue::Change`), and a report Core
  judges after it, observed after it, finds that focus left for the
  change's window when the window is neither its top-level window nor of
  its root owner. A change observed before the last queued focus but
  reaching Core after it applies too, to the reports judged after it
  (Dickson's limit); a report already judged keeps its verdict. It speaks
  nothing, cuts nothing, sets no ordering stamp, and changes no node,
  focus group or navigator.
- A report judged left is executed as a focus from another top-level
  window is: no duplicate, a whole-window focus of its window is not
  refused, a replacement outpost's report is not taken silently, its line
  meets the previous line only at the desktop with speech cut, its spoken
  record comes from its reading, its caret and terminal speech state are
  forgotten, and the terminal model is blurred and focused anew, so the
  return from an application that never answered says, and leaves, what
  the same switch with no stall does (`tests/foreground.rs` compares both
  the speech and the whole state). A report from the window the focus was
  left for is not judged left, so the two lines are compared as they are.
- Event acceptance (`books/dev/src/explanation/architecture.md`, section 2, "Attention and
  acceptance (D14)"; `docs/parity/focus-events.md`, "Event acceptance").
  The state keeps an attention record, the application and window of the
  newest foreground change the listener reported. A focus with window
  facts, a whole-window focus among them, is judged against the record's
  front: dropped when the front was observed after it and is a window it
  is unrelated to (neither its top-level window nor its root owner is the
  front's window or the front's root owner), attended otherwise; a focus
  with no window facts by its application. Any other event is attended
  when its window facts share the attention window's top-level window or
  root owner, are topmost, or are related to the foreground, or, with no
  window facts, when it comes from attention's application or from that
  of Core's focus while that focus is in the attention window, as a
  Settings page's content is in `ApplicationFrameHost`'s frame. UIA
  notifications are filtered by the focus's application, except the snap
  results. Background kinds from elsewhere are spoken queued and never
  move focus or the navigator; everything else from elsewhere is dropped.
  With no attention yet, everything is accepted.
- Windows (`docs/parity/focus-context.md`, "Window announcement on
  switching applications"). A whole-window focus (`whole_window`), an
  outpost's report of a foreground change into a window it reads through
  MSAA or a held UIA application's title, makes its window the foreground
  object and is refused when the last queued focus is in its top-level
  window and the report is not judged left (handles, since node ids
  differ between outposts); a nameless
  one is taken silently. It never moves attention. The title rule
  (`enter_window`, `window_title`): a focus Core takes whose top-level
  window is the foreground window it was published with
  (`WindowFacts::foreground_window`), when that window is not the entered
  window or the report is judged left, enters it, making its chain's
  outermost entry the foreground object, and says the window's own entry,
  the outermost with its handle, with a whole-window focus's words, after
  the cut and before the other entered containers, unless its line
  already speaks it or it has no name. A whole-window focus taken enters
  its window too, so the focus after it says nothing more; a focus in a
  popup changes nothing. A focus in Program Manager or the taskbar
  (`WindowFacts::foreground_is_shell`) enters with no title, and a
  foreground change into one of Verbatim's own windows clears the
  entered window, so the return from Verbatim's menu says the title.
- The foreground record (`foreground.rs`): one `Foreground` with private
  fields, written only by `apply` with one `ForegroundFact` per writer
  (`Changed`, `Object`, `Entered`, `LineCut`, `Observed`) and
  read through accessors. It holds the attention record, the front, the
  entered window, the foreground object, the time of the newest
  foreground change, the listener's foreground window for the hook, and
  the ordering stamps (`focus_stamps`), the observation of each report Core accepted into its
  order, taken, waiting for a live re-read, a duplicate or a refused
  whole-window focus, with its generation, window and whether it was a
  whole-window report, which the stale rule reads, kept while a report
  Core can still accept could be stale by them (`Foreground::stale_floor`,
  the newest ordinary stamp). When the focus taken was observed, which a
  menu's end reads, is the focus object's own `observed_at_us` in the
  focus queue. `SrState` flattens it into the keys it held before.
- Repeats and staleness (`docs/parity/focus-events.md`, "Duplicate focus
  suppression" and "Stale focus events"). A focus is judged in this
  order: an older entry of a focus group whose newer report Core took is
  dropped (`FocusGroup`, keyed by the report's outpost generation and the
  group's number); an application's rule; the stale rule, which drops a
  focus from another outpost generation observed before the ordering
  stamps (`Foreground::stale`, `observed_at_us`, a tie being no
  earlier), one that a whole-window report of the focus's own top-level
  window set left out; two ordinary reports of one window shared by two
  processes, a frame and its content, follow the rule against each other;
  the front test (`classify`); then its ordering stamp is written, and
  once it reaches the queue's front it is judged against the last queued
  focus: a
  whole-window focus of its window is refused, and a focus that
  `repeats_focus` judges a repeat of it, unless the report is judged left
  or reached Core while the last queued focus still waited for its live
  re-read, is dropped, its newer reading kept unspoken on the focus object
  when it is the duplicate's node (`keep_duplicate`: its node and the
  ancestors it read live, matched as the lines are joined); what Core last spoke of
  the focus is left as it was, so a change the repeat carried in is
  spoken when its event comes. A focus dropped by the stale rule is never
  queued, so the events around it are judged as if it had not happened:
  focuses A, B, A from two generations in two top-level windows, reaching
  Core as A, the repeat of A, then B, say nothing after the first A and
  end on A, where NVDA says B then A.
- The focus queue (`focus_queue.rs`;
  [ADR 0001: Focus](../adr/0001-focus.md), "One ordered focus queue"):
  `FocusState` holds the focus object, the focus last executed, which
  commands, text, review and held nodes act on; the last queued focus,
  the newest report decided (`Last`): nothing yet, the focus object
  (`Executed`), or a report that ended unanswered, never spoken
  (`Unanswered`); the queue of what is not yet decided (`Queued`): focus
  reports, and the key commands, scripts and menu or switcher ends that
  reached Core while reports waited, handled in their turn
  (`reduce::waited_input`); and the foreground changes that can still
  apply to a report. Each report is queued as an `Entry` as it reaches Core,
  `Pending`, and decided at the queue's front (`reduce::run_queue`,
  `reduce::decide_report`), every report ahead of it decided: judged,
  planned against the focus object and executed, so its speech expires as
  NVDA's speech manager expires a focus overtaken, or set aside for its
  live re-read (`EntryState::Rereading`), which holds every report behind
  it, in the order they reached Core; once answered it is executed
  (`Answered`), or it ends unanswered and is the last queued focus
  (`Unanswered`). The queue counts the inputs it orders, reports,
  foreground changes and the endings of live re-reads, to know which
  change reached Core before which report and whether a repeat came while
  a wait was open; the count starts again whenever nothing waits.
- Background speech across a foreground change
  ([ADR 0001: Focus](../adr/0001-focus.md), "One input to Core, in
  arrival order"): a background event
  observed before the newest applied foreground change is dropped.
- Changes (`docs/parity/changes-and-selection.md`). Each focus keeps two
  things: the reading Core holds (`FocusContext::snapshot` and
  `ancestors`), which review reads and a repeat updates, and what Core
  last spoke of it (`FocusContext::spoken`, a `SpokenRecord`): the
  focus's name, value, description and states, and each ancestor's
  states. A repeat never moves the record. A focus taken moves it, and
  so does a report that speaks the focus's reading (Verbatim+Tab's
  answer of the focus, reporting the current object while the navigator
  holds the reading, and the navigator's return to the focus), whatever the reason
  it is spoken. A navigation that lands on the focus's node fetches a
  separate object, so reporting it moves the record only when it equals
  the reading. A name, value or description change on the focus moves it
  to the new reading even when nothing is spoken, as NVDA's property
  cache does, and so does a state change on the focus, except one without
  the focused state on a focus that reported itself focused, which the
  focus has left. A state change of an ancestor with an entry moves that
  entry.
  An ancestor the previous focus line held, the
  previous focus among them, keeps its entry, so an expansion of a combo
  box the focus has just left for its item, arriving after the item's
  report, is still spoken; an ancestor with no entry is compared with its
  reading. Every change is compared with the record. A name change speaks
  the new name on the focus; a description change the new description; a
  value change on the focus the bare value, unless unchanged, an edit
  field or document, or a role that never speaks a value. A states change
  on the focus or a spoken ancestor is diffed against the record: gained
  states, and lost ones spoken by their absence. A focus that reported
  itself focused (`FocusContext::reported_focused`) and loses the focused
  state keeps its states silently. An expansion carrying `child_count`
  adds "52 items" (`Phrase::Items`). A `Button` with the Toggle pattern is
  `Role::ToggleButton` with `Pressed` ([verbatim-uia](verbatim-uia.md)).
- Selections (same document, "Selection announcements" and "Selection in
  a list the focus controls"). A focus on a selection container carries
  its selected child, spoken after it; a `SelectionChanged` while focus
  stays on the container speaks each newly selected item once,
  deduplicated against the focus's own child. A `ControlledSelection` is
  spoken as a focus, interrupting, and moves the navigator, only while
  its controller is the live focus.
- Progress bars (`docs/nvda/object-model.md`, "How a progress bar reports
  its value"): `ProgressChanged` plays `Earcon::Progress` once the value
  moves a percent from the last one indicated at the same place on screen
  (`SrState::progress_reported`, the last 32 places).
- Notifications: a UIA `Notification` speaks its display string,
  interrupting for `MostRecent` and `ImportantMostRecent`, queued
  otherwise.
- Outpost replacement (`docs/parity/focus-events.md`, "Recovery after an
  outpost is replaced"). Node ids carry the outpost generation. On
  `Input::OutpostEnded` the focus and the waiting reports of that
  generation are marked dead, the reports waiting for its readers' live
  re-reads end unanswered and the queue runs on, and a navigator or
  pending navigation there is cleared; a later focus of another
  generation matching the kept copy (role, name, value, states, ancestors'
  names and roles), not judged left, takes its new ids silently, and one
  judged left is announced.

## The ancestry cache

A focus reader, the listener's reading thread for an application or the
outpost's own reads, keeps an ancestry cache: its last read's ancestors,
where a later walk stops and reuses the rest. With it it saves the window
text of each window entry, and before reusing it reads them again
(`verbatim_reader_rules::ancestry_cache::title_check`, local calls): a
changed title or a gone window makes the walk read live to the top or the
process boundary. Each report marks where the ancestors taken from the
cache begin, `first_cached_ancestor`, the index of its innermost cached
ancestor among the entries it emits, computed by the outpost after every
filter and rule (`focus_rules::first_cached_ancestor`), with the focus
reader that read it.

Core never trusts the cache. `plan_focus` computes, before anything is
written, the report joined to Core's focus ancestry
(`focus_rules::lowest_common_ancestor`), the entry the title rule would
speak, and `focus_rules::needs_live_ancestry`: whether a cached ancestor
would be spoken, as entered (below the lowest common ancestor) or as the
title. The line it is joined to is the focus object's, since the plan is
made only when the report reaches the queue's front. A report that would
speak a cached ancestor waits at the front for a live re-read, asked with
the input that let Core decide it, unless its outpost generation has
ended by then, when no re-read is asked and it ends unanswered, the last
queued focus, unspoken. Otherwise its focus group's rank is recorded
as taken, and `Effect::LiveReread` asks its focus reader to read the same
element again with no cache, carrying what the shell logs (the focus, the
cache join, the lowest common ancestor and the reason). The answer is a
report carrying `answers`, its reading and names replacing the report's,
executed in its turn with the report's verdict; it is judged by no rule
the report passed, and never needs another live re-read. A duplicate
report keeps the newer reading of its live ancestors only, never of its
cached ones; a window above its reader's process boundary, described from
Windows' state, is always live. A window entry is matched by its handle,
as the lines are joined.

`live_reread.rs` ends the waiting reports. Live re-reads are counted in
`SrState::live_re_reads`, which the control protocol's status reply
carries. A waiting report ends on one path: its answer, executed in its
turn, or refused by the foreground test when its window has been left;
its answer of gone (`Input::LiveRereadGone`); its outpost generation's
end; or, for the listener's, `Input::ListenerReplaced`. One that ends
unanswered is never spoken and stays the last queued focus, which the
next report is judged against, while that report's speech is executed
against the focus spoken before it. An answer to a report no longer
waiting says nothing. The waiting reports count as work in flight for the
idle barrier and as applied focuses for the fake focus's skip rule. A
repeat of its element that reaches Core while it waits is no duplicate
of it: it is decided as a new report when the wait ends, asking its own
live re-read or not, because a focus reader answers a live re-read only
for the read it holds last, and the repeat replaced it. The shell routes the live re-read to
the listener through the supervisor (`CoreToListener::LiveReread`) or to
the outpost (`CoreToOutpost::LiveReread`), and answers it gone at once
when it cannot be sent. Tests: `live_reread/tests.rs`, and the generated
test `live_reread/generated.rs`, which compares what Core's speech queue
holds and its focus object with a reference model of NVDA's one focus
queue, taking the reports in the order they reach Core, with Verbatim's
recorded decisions and limits, over a generated world of MSAA and UI
Automation windows, some owned across processes and some shared by a
frame and its content, whose names and
titles change while the user is away, with repeated focuses, foreground
changes with no focus, focuses before their foreground change and
reports delivered late, its focus readers answering as the real ones do,
and checks that no live re-read is answered gone for a repeat of its
element while its window is in front.

## Object navigation and review

(`docs/parity/review.md`.) `SrState` carries a navigator object and a
review cursor that follow focus. A `Command` runs against them:
report-object announces, spells, then copies on repeated presses; parent,
sibling and first-child moves emit a navigation `Fetch` whose completion
moves and announces the navigator, a newer navigation turning an older
one still pending to nothing; activate emits `Activate`, whose answer says
what was done; to-focus snaps back, turning a pending navigation to
nothing; report focus asks a live query ("Verbatim+Tab", below). With no
navigator a command says "No navigator object". An object with text is announced by
`announce_navigator` and `editing::navigator_text`: name, role and states,
then its text, read from the known caret or through a `PendingText` with
`NavigatorSelection` and `NavigatorLine` follow-ups, which a newer text
request supersedes.

### Verbatim+Tab

(`report_focus.rs`.) Each press is a live focus query,
`Effect::ReadFocus(FocusPurpose::Report)`, which the app routes to the
process of the foreground thread's focus window
([verbatim-app](verbatim-app.md)); nothing is said until it answers.
The answer comes back as an `Input::Event` whose `purpose` is `Report`,
under the press's trace, and is spoken only: never taken as Core's
focus, nor recorded as the last focus queued, so Core's focus ancestry,
its navigator and the outposts' followed chains are unchanged. The first
press says the object as a query announces it (`reduce::announce_query`),
then its text as the navigator's is read (`editing::reported_text`, whose
`PendingText::about` carries the answer, which is neither the navigator
object nor the focus); an answer of the focus Core holds moves what was
last spoken of it. A second press spells the name, "blank" for none, a
third and later with character descriptions.

The press waiting for its answer is `SrState::report_press`
(`WaitingPress`: its trace and the first and last repeat counts kept).
A press within the repeat interval while one waits is kept with it, so
the answer is spoken, then spelled, in order; a press after the interval
asks again and replaces it, the older query's answer then being late.
Its endings are its answer, `Input::ReadFocusUnanswered` (nothing to ask,
its outpost's end, `NotWatched` or an empty answer say "No focus";
abandoned at its deadline, or replaced in the app while it waited for an
outpost, nothing), a focus Core applies that was observed after the
press, in a later millisecond, or either time unknown (`record_taken`),
and a replacing press. A focus observed before the press and applied while
it waits leaves it waiting, as NVDA runs NVDA+Tab after the focus event
queued before it and speaks both. An answer for no waiting press says
nothing.

Completions are matched by the state's latest navigation query id, not by
navigator identity, so a focus event between a command and its completion
snaps the navigator without discarding the navigation, while a second move
or a to-focus supersedes it. `NoNeighbor` speaks the edge message; `Gone`
re-seeds the navigator from the focus; `Unanswered` says nothing.

An object with no text interface is reviewed as flat text, its value or
name, in the pure `review` module, by the same units as text: grapheme
clusters, and words from `text::words`, chosen from the text since flat
text carries no language.

## What the shell learns

Only the reducer speaks (`books/dev/src/explanation/architecture.md`, section 2), so what the
shell's threads learn comes in as inputs (`shell.rs`): `Input::Started`,
`ErrorLogged` play their sounds; `ApplicationNotResponding`, an
outpost's report that a window of the process it watches stopped
responding, sent once per hang, plays the not-responding sound only if
that process is attended as `window_is_attended` judges a process with no
window facts (`process_is_attended`): attention's own process (the
foreground record's `Attention`), or that of Core's focus while the focus
is in the attention window, as a Settings page's process is in
`ApplicationFrameHost`'s frame; so a hang met in the background is never
sounded, even after the user returns;
`Exiting` cancels speech, then plays the exit sound, unless a new
instance replaces this one, and sets `SrState::exiting`, after which
`reduce` does nothing for any input, so nothing is said after the exit
sound, as in NVDA (`docs/parity/system.md`, "Exiting"), and which
`SrState::exiting()` answers, so the shell plays that exit sound to be
waited on; `LockKeyToggled`
(`Phrase::LockKeyState`), `ClockRead` (time, then the date on a quick
second press), `ClipboardCopied` (`Phrase::CopiedToClipboard` or
`Message::UnableToCopy`) and `PreviewSpeech` speak, interrupting;
`ShellItemGone` and `ShellItemUnclickable` speak queued; `PreviewEarcon`
plays its event. Each speaks under its input's trace, so a later key press
fences a copy's confirmation off.

The scripts of the gestures that are not review commands
(`Input::Script`) ask the shell: `Script::ReportTime` emits
`Effect::ReadClock { repeat }`, answered by `ClockRead` under the
gesture's trace; `Script::ShowMenu` emits `Effect::Show(GuiRequest::Menu)`
and `Script::ShellList` `Effect::Show(GuiRequest::ShellList(kind))`, the
system tray on a first press and the taskbar on a quick second
(`shell_list_kind`). A lock key's press (`Input::ToggleKeyPressed`) adds
the key to `SrState::toggle_keys`, the announcements to come in press
order, each with its read due 30 ms after the press, which the shell reads
from the state (`SrState::toggle_key_read_due`); `LockKeyToggled` takes
the front one. Its utterance carries no key
number, so a key pressed within the 30 ms does not silence it, as NVDA's
announcement after its timer is heard; it never waits behind a held key,
since NVDA starts that timer when the key is pressed, outside its script
queue. `MenuOrSwitchEnded` asks for the
foreground application's focus (`Effect::ReadFocus(FocusPurpose::Fake)`,
NVDA's fake focus) unless a focus observed since has been applied, or is
waiting in the focus queue
(`docs/parity/backends.md`). A focus query's answer is marked with its
purpose (`Input::Event::purpose`), which decides the duplicate rule
(`reduce::duplicate_rule_applies`): a fake focus's answer is judged only
when it was read through MSAA, as NVDA's `processFocusNVDAEvent` tests
IAccessible objects only, so a UIA answer of the focus Core holds is
spoken again; the desktop's return's (`Again`) never; a plain query's as
any report.

The desktop's news (`desktop.rs`, `Input::Desktop`): the user's desktop
returned asks the application in front for its focus again
(`Effect::ReadFocus(FocusPurpose::Again)`, NVDA's `_handleUserDesktop`);
the virtual desktop's name is kept (`SrState::desktop_name`, the newest
only) until the next focus Core takes, which says it after the
foreground's title and before the entered containers, as NVDA's
`doPreGainFocus` calls `handlePossibleDesktopNameChange`, or, at its due
time (`DesktopNews::NameDue`), spoken as a queued message. `tests/shell.rs`
checks each rule.

## Text

The reducer's side of the text model, over the text protocol in
[verbatim-model](verbatim-model.md). Pure helpers over received text are
in `text.rs` (a line without its break, grapheme clusters, columns in
characters or cells, words by the text's language through
[verbatim-text](verbatim-text.md), and the spoken segments for
characters, words, lines and spelling); caret keys and typing echo in
`editing.rs`; the review cursor over text in `review_text.rs`; say-all in
`say_all.rs`.

The state keeps the focus's caret (`CaretContext`: its line and the
selection), the review cursor's position (`ReviewText`: unknown, flat, a
line and a position on it, at the caret it follows, or a point whose line
has not been read), the caret key being watched, the one review or text
request in flight, the start marker, say-all's queue, the word typed so
far (at most 256 bytes), and the focused terminal's output still to be
spoken. Text is held a line at a time, as a shared chunk of at most 64 KB.

Which objects have text: a focus or navigator whose role is an edit
field, a document, or a terminal, and any node the outpost sent a caret
report for. The first review command reads the line at the caret, and a
`NoText` answer makes the object flat text. Any other object is flat text
without asking.

### Held caret keys

(`books/dev/src/explanation/design/key-holding.md`, "Core".) An `Input::HeldKey`, a caret key
the hook held back because `holds_caret_keys` was true, asks the focus's
outpost to read the caret (`TextOp::ReadCaret`) and keeps the key
(`HeldKeys`). The answer becomes the focus's caret, and the key is
handled as an `Input::CaretKey` pressed one millisecond after the read
ended, then passed on (`Effect::PassHeldKey`) after its watch is asked
for, so the watch reaches the outpost before the key reaches the
application. Any other answer, or the read's deadline passing, passes the
key on with the hook's time; with no focus whose caret is read, it is
passed at once.

A held key that reaches Core while focus reports wait joins the focus
queue first and is taken in its turn, as NVDA runs its caret script in
its queue (ADR 0001, "One ordered focus queue"); that wait is bounded by
the live re-read ahead of it, by its reader's moved-on rule and read
deadline. Once taken, a held key has one deadline, which Core keeps for
the key named in `HeldKeys::bounded`: `KEY_AHEAD_BOUND_MS` (100 ms) after the key ahead was
passed on while the key waits for that key's watch, and
`HELD_READ_BOUND_MS` (150 ms) after its read is asked while the read is
in flight. `Effect::BoundHeldKey` sets the shell's one slot, or clears it
when the wait or the read ends sooner, and `Input::HeldKeyDue` enters
when it passes (`editing::held_key_due`): a key still waiting is read; a
key still being read is passed on without the read, and the read is
withdrawn (`Effect::Withdraw`), so it is no longer waited for and its
late answer finds nothing and is dropped.

Every key input that comes while a held key waits waits behind it
(`WaitingKey`) and is handled in order (`editing::next_held`): a held
key's read is asked for once the watch of the key ahead has been answered
(`HeldKeys::awaiting_watch`) or `KEY_AHEAD_BOUND_MS` (100 ms) after that
key was passed on, the held key's deadline. Other
keys are handled as the input they came as (`reduce::key_input`). The
watch of the key just passed is marked `later_key`, so its answer only
keeps the caret current and says its selection's change. Each waiting
key's effects carry its own trace (`Effect::ForKey`).

A review command or a script (`Input::Command`, `Input::Script`) pressed
while a held key waits, or while the watch of the held key last passed
on is open, waits behind it too (`editing::queue_command`,
`WaitingKey::Command`), as NVDA runs scripts one at a time in the order
queued: until the key ahead has had its caret read and been passed on,
and until that key's watch is answered or 100 ms after the pass,
whichever is first, the bound counted from the command's own press
(`pressed_at_ms`); `HeldKeyDue` with its trace ends the wait. The
key ahead, its watch marked `later_key`, says nothing, and the command
reads the line the key moved to, where NVDA reads the line before it
(`books/dev/src/explanation/design/ownership.md`).

## Key numbers

(`books/dev/src/explanation/design/ownership.md`.) Every user input carries the number the keyboard hook's thread
gave it (`verbatim_model::KeySeq`), as `key_seq` on all seven:
`CaretKey`, `HeldKey`, `CharacterTyped`, `ClearingKey`, `Command`,
`Script` and `ToggleKeyPressed`. `reduce` records the input's trace and
number in `SrState::key_origins` (`keys.rs`, the newest 1,024 pairs)
before dispatching it, and as its last step stamps every `Effect::Speak`,
inside `Effect::ForKey` included, with the number of the trace of the
innermost `ForKey` around it, or else of the input's trace
(`Input::trace_id`), failing that of the utterance's own trace. An answer
to a key's request comes back under the key's trace, so its speech
carries the key's number; speech of an input no key caused carries none.
The table is part of the state, so a flight recorder checkpoint carries
it. `KeySeq::NONE_YET` is not recorded.

### Caret keys

(`docs/nvda/editable-text-and-terminals.md`; `docs/parity/text.md`,
"TextInfo-equivalent layer".) An `Input::CaretKey` on a focus with text
asks its outpost to watch for evidence (`TextOp::AwaitCaret`) with the
caret Core last knew, when the key was pressed, the unit to report, the
character or word at the caret for Delete, and whether the key deletes.
The outpost answers once evidence comes, or `TextReply::WatchEnded`
without it, for which Core says nothing, as for a reply with `moved`
false. Backspace over a selection predicts the text before the
selection's start (`CaretReport::selection_line`; with the start in
neither line, `PendingCaret::before_unknown` says only the selection's
change).

The caret before the key comes from the history: Core keeps the focus's
last 8 caret reports (`CARET_HISTORY`, inline in the state) with their
observation times, and takes the newest observed strictly before the
key's `pressed_at_ms`, since the application's caret event can reach
Core before the key does. With no time for the key, or no timed report,
the caret Core holds stands in; a caret Core moved itself, as say-all
does, is kept at time 0. The caret Core holds is the newest by observed
time, not the last to arrive. A newer caret key or typed character
supersedes one still watched, and a focus change drops it.

The answer updates the caret and speaks, queued:

- Left and Right Arrow, Home and End: the character at the caret, each
  character of a line break its own (`text::characters`).
- Control with Left or Right Arrow: the provider's word; a one-character
  word by its name.
- Up and Down Arrow, the page keys, Control with Home or End: the line.
- Control with Up or Down Arrow: the paragraph, or the line.
- Backspace and Control+Backspace: over a selection, the text before the
  caret as the key found it, then the selection's change; otherwise what
  the key removed, from evidence (`editing::removal`), including the line
  break at a line's start, of the kind last seen.
- Delete and Control+Delete: the character or word now at the caret.
- Any other passed key that types nothing (`CaretMotion::Other`, from the
  hook's `KeyReport::Passed`, and the clearing keys): text it removed, a
  new line (`CaretReply::same_line`, answered for a watch marked
  `landing`), or by where the caret landed on an unchanged line. A key
  that removed a selection says only the selection's change
  (`editing::removes_selection`).
- With Shift, and Control+A: what became selected and unselected
  (`editing::selection_segments`), 512 characters or more as their count;
  a change in general terms (`content_changed`) speaks the whole new
  selection.
- A selection no key made arrives as a `CaretMoved`'s selection change
  and is spoken the same way (`editing::selection_changed`). While a caret
  key's answer is awaited for the node, its words are held with the key
  (`PendingCaret::heard`) and spoken after the key's; a superseded key's
  selection change is said with the later key's
  (`Then::SupersededCaret`, `editing::superseded_reply`). The
  superseded slot holds one key, so in a burst of three keys where the
  first answer arrives after the third press, the first answer's
  selection change is not said; speech is cut at each press, as with NVDA.

### Formatting

(`docs/nvda/document-formatting.md`.) A caret key's unit, and a new
focus's first line, carry the formatting the outpost read
(`TextChunk::formats`). The focus object keeps the formatting last
reported in its text (`FocusContext::reported_format`), as NVDA keeps its
per-object cache on the object, so each focus taken starts afresh, the
silent re-take after an outpost is replaced included, and a node other
than the focus has none; the unit is spoken with the formatting at
its start that differs from that (`text::format_changes`, in NVDA's
order), a list item's bullet when the unit is a line or say-all chunk,
then its text with each later change placed where it happens
(`text::formatted_segments`). A character or word also says an error's
end. Each change is a `Format` span, which the theme presents. Review
commands and say-all read no formatting.

### Typing echo

`Input::CharacterTyped` is echoed by the settings: a finished word first,
when a character that is not a word grapheme ends it
(`verbatim_text::is_word_grapheme`), then each printable character, each
in its own queued utterance. "Only in edit controls" means a focus that is
an edit field, a terminal, or a document that is not read-only. A
protected field echoes only "star". In a terminal, unless terminal
passwords are spoken, typing is held until its
`NormalizedEvent::TextChanged` arrives, and Enter drops what was held. A
caret key without Shift, or a focus change, ends the word. Interrupting
speech for typing is the hook's ([verbatim-input](verbatim-input.md)).

### The review cursor over text

(`docs/nvda/review-modes.md`; `docs/parity/review.md`, "Review cursor
line/word/character over object text" and "Review cursor columns".) While
the cursor follows the caret (`ReviewText::Caret`), a review command reads
the line at the caret as the outpost finds it (`TextPoint::Caret`), never
from Core's copy, because a caret event can reach Core after a review
command pressed later; say-all from the caret and the caret's location do
the same. Once moved, the cursor holds its line, so motion within it and
reading the current unit need no round trip; other lines, the document's
ends, the selection's ends and the start marker are read through the
protocol. Moving to another line keeps a cell column in a terminal and a
remembered character column elsewhere. A lost anchor starts again from
the caret.

Select then copy (`docs/parity/review.md`, "Select then copy from the
review cursor"): Verbatim+F9 marks the start, Verbatim+Shift+F9 moves the
review cursor there, and Verbatim+F10 asks the outpost to select from the
earlier end through the character at the later (Core sends both ends with
the points just past their characters, `TextThrough`, since only the
outpost can order them), and pressed twice reads and copies the range
(`Effect::CopyToClipboard`). The read carries the marker's generation, so
a marker set while it is in flight is kept.

Toggles and locations: Verbatim+6 toggles the review cursor following the
caret; Verbatim+2 and Verbatim+3 cycle character and word echo; each emits
`Effect::SettingsChanged`. The caret's and the review cursor's locations
are asked of the outpost and spoken "Positioned at x, y", or, in text
measured by character offsets, as the offset's percentage of the text's
length and the place, "93%, at x, y" (`Phrase::OffsetPositioned`, worded
by `verbatim-i18n`).

### Say-all

(`docs/nvda/speech.md`, "Say-all" and "Say-all speaks without pauses".)
From the caret or the review cursor, say-all reads by the "Say all reads
by" setting: a provider with no sentence unit reads by line, one whose
text NVDA splits itself answers with the paragraph, split by Unicode's
sentence rules, and a terminal always reads by line.

- Each read asks for a batch of 20 units (`TextOp::ReadAhead`), buffered
  in `SayAll::buffer`; the next batch is read once fewer than 10
  utterances (`LOW_WATER`) are left, so at most one read is in flight and
  the buffer holds under two batches. The costs are in
  `books/dev/src/reference/performance.md`, "Say-all, UIA".
- An utterance runs to the last sentence end in a unit
  (`verbatim_text::last_pause`); the rest is held back (`SayAll::held`) to
  open the next, and ten units with no sentence end (`MAX_HELD`) are
  spoken together. A space stands for the line break between two units.
- Each unit's start has an index mark and each utterance opens with one.
  Utterances are handed to speech two ahead of playback. Reaching a
  unit's mark moves the caret there (from the caret, taking it as Core's
  caret) or leaves the review cursor there; reaching an opening mark hands
  on the next utterance. Blank pieces are not spoken, and the display is
  kept on (`Effect::KeepDisplayOn`, by the setting).
- It ends after the last piece and stops on any command, caret key, typed
  text, `Input::SpeechCancelled`, `Input::UtteranceCancelled`, a focus
  change, or its outpost's end. `Input::UtteranceFailed` counts as heard
  but moves no cursor.

## Terminals

What a terminal says (`terminal.rs`; `books/dev/src/explanation/design/terminal-model.md`,
"Features and settings"; `docs/parity/terminals.md`). Core's terminal
model (below) gives each read to the reading rules, and what they say
(`Said`, `TerminalItem`s and new lines) is spoken here; speech of anything
but the focus is ignored. `TerminalItem` (output, a count of lines
skipped, or a key's answer), `TerminalUnit`, `Skipped` and
`TerminalKeyKind` are Core's own types (`terminal/items.rs`); none crosses
a pipe.

- The answers to keys are spoken at once, ahead of the output the same
  speech carries: typing the terminal showed (`TerminalItem::Echo`) by the
  echo settings, a key's answer in its unit, text the caret's line gained
  or lost, a row away from the caret that is a key's answer, and a
  suggestion. A caret key's watch's answer in a terminal only keeps
  Core's caret current.
- `caret_key_kind` and `typed_kind` say what a key is to the rules: the
  horizontal keys a character, with Control a word, the vertical and page
  keys a line, Delete as Delete, any other `Other`; text typed, `Enter`,
  `Forget` for Tab, `Other` for a control character.
- The review cursor follows the caret only when the rules say a key moved
  it within its window (`Said::followed`), since a program moves its
  cursor as it draws (`books/dev/src/explanation/design/terminal-model.md`, "The review
  cursor").
- Output is spoken queued, one line per utterance, each opening and
  closing with an index mark; a blank line counts and waits but is never
  spoken. Two utterances are handed to speech ahead of playback and the
  rest wait in `TerminalSpeechState`. Newer output never cancels older.
- The flood policy, by groups (`TerminalSpeechState::group`): the first
  "Lines spoken in full" lines of a burst are spoken whole. Only once the
  group's last line has been heard (its closing mark, never its start) is
  the backlog looked at: when more lines wait than that, everything but
  the newest "Last lines to speak" becomes one skip, spoken as
  `Phrase::SkippedLines`, `Phrase::SkippedUncountedLines`
  (`Skipped::Uncounted`), `Phrase::SkippedMoreThanLines`
  (`Skipped::MoreThan`, lines lost past the terminal's history) or
  `Phrase::SkippedAboutLines` (`Skipped::About`, counted from text after a
  lost read); `Skipped::plus` combines kinds, "more than" winning. A lost
  read sends a count of none of its kind ahead of its lines, each marked
  `Counted`. The next group follows, until everything waiting has been
  heard.
- Bounds: behind the group being spoken, the waiting queue keeps at most
  as many lines as either limit keeps, older ones folded into the count,
  and at most 10 MB (`MAX_WAITING_BYTES`); a single larger line is cut on
  a grapheme boundary and followed by `Message::TerminalLineCut`. A line
  and its growth (`TerminalItem::Growth`) are one unit, counted and
  dropped together.
- On-demand reading (`docs/parity/terminals.md`, "Reading a terminal on
  demand"). Once a group's worth of lines waits, Core tells the outpost to
  hold (`TextOp::TerminalHold`); when the group's last line is handed to
  speech, it asks for what is new (`TextOp::TerminalRead`), and the
  backlog is decided once both have happened
  (`TerminalSpeechState::deciding`). With nothing waiting, reading goes
  live again. Whether the outpost holds is the focused terminal's model's
  (`TerminalModel::held`), and every request goes to the focused terminal
  Core models, so when the outpost ends, its hold goes with the model and
  nothing more is asked of it; what Core had already read is still
  spoken, as NVDA speaks the lines its monitor read (the request out is
  answered gone, and the backlog decided then). A cut while held or with a request out sends
  `TextOp::TerminalCancel`; of reads before the cut
  (`Input::SpeechCancelled::at_ms` against the read's start) only the
  answers to keys are spoken, as for an overtaken request
  (`TerminalSpeechState::overtaken`).
- Anything that cuts speech drops the output waiting and handed to
  speech: `Input::SpeechCancelled`, a step whose effects stop speech or
  interrupt, and `Input::UtteranceCancelled`. `Input::UtteranceFailed`
  counts as heard. The output waiting is the focus's: a focus moving to
  another node forgets it, an outpost replaced under the same focus keeps
  it, and output utterances carry the focus's validity.
- Verbatim+5 (`ReviewCommand::ToggleReportNewOutput`) toggles "Report new
  output"; off drops what waits and speaks no output, but still the
  answers to keys.

The unit tests in `terminal/tests.rs` drive the flood policy and the
reducer with simulated playback and a simulated outpost that holds and
answers Core's requests as the real one does.

### Core's terminal model

The reading rules run in Core (`books/dev/src/explanation/design/terminal-model.md`, "Where it
runs"): `terminal_model.rs` and `terminal_model/review.rs` take each read
a terminal's outpost sends (`NormalizedEvent::TerminalSnapshot`) and the
answers to the requests they make.

- Public: `read_of(&TerminalSnapshot)`, the rules' `Read` of a snapshot
  (its text without its lines' padding, and the screen, padded again
  exactly); `snapshot_of(&Read, waited, colours)`, the snapshot of a read
  as an outpost sends it, for tests and measurements over the simulated
  terminal; and `read_at(&TerminalSnapshot)`, when a snapshot was taken as
  far as keys are concerned: when the read ended, or when it began for a
  snapshot that does not say when it ended.
- `SrState::terminals` (`Terminals`) holds a model per terminal node
  read, at most 16, least recently read forgotten first and an ended
  outpost's dropped, each only through `SrState::drop_models`: the rules
  (`TerminalReader`) behind an `Arc`, the model's generation, the keys not
  yet given to them, waiting review requests, history rows that came, the
  review places, the caret and screen shape, the shape told to the
  outpost, whether the outpost holds for the flood policy, and the
  quiet-period timer's due time. The reads it asked for
  are Core's queries, tagged with its node and generation ("Core's
  queries", above). A model is made with a terminal's first snapshot
  while it has the focus.
- Keys. Every key input while the focus is a modeled terminal is kept by
  its kind, a clearing key as `Forget` then `Other`. With each, Core asks
  for the screen's colours over the key's window, `KEY_WINDOW_MS` from its
  time (`TextOp::TerminalColours`, always answered `Done`). Before a read
  taken at S (`read_at`) is given to the rules, every kept key at or
  before S is given in order; before the timer at T, those up to T; past
  256 kept keys the oldest is given at once. Leaving the terminal tells
  the rules `blur` and drops the keys; returning tells them `focus`.
- A read's speech key is the newest key given whose window holds the
  read's time; its speech and effects go out as `Effect::ForKey` for that
  key. A read with no speech key, and the timer's speech, carry the
  input's trace.
- Reads. A live snapshot is taken only for the focused terminal. When the
  rules ask for the whole document or a first read with its history
  block, Core sends `TextOp::TerminalReadNow` (`TerminalScope::Whole` or
  `HistoryBlock { height }`) and drops live snapshots until it comes,
  since the outpost answers in order. `TextReply::TerminalUnchanged` says
  nothing. A read's caret report becomes Core's caret at the read's time.
  The read before a held key is answered with a snapshot (`before_key`)
  whose caret answers the read. A new screen shape is told with
  `TextOp::TerminalShape`. The rules' timer is the focused model's due
  time, which the shell reads after each step
  (`SrState::terminal_timer_due`) and answers with `Input::TerminalTimer`
  once it is past; another model's due time does not count, so a model
  dropped or left has no timer, and the timer's step moves the due time
  past the time it fired at (a debug assertion, and a postcondition of
  the generated test).
- Review commands. A request the memory answers is not sent: `reduce`
  takes it out of the step's effects, its query becoming one Core answers
  itself, never waited for, asks for a fresh read
  (`TerminalScope::Fresh`) and answers it from the memory, which ends its
  query (`queries::answer_from_memory`). Rows above the
  memory are asked for with `TextOp::TerminalHistory { above, whole }`, a
  screen more than needed, and placed only when they count from the read
  the memory last took (`review::placed_history`, sharing
  `review::start_of`); otherwise the request is answered `Unanswered`.
  The memory's rows carry anchors with the top bit set
  (`TERMINAL_ANCHORS`), a range no outpost mints from, with at most 64
  places beyond those Core holds. A waiting review is `Review::Text` or
  `Review::StatusBar` ("Report status bar",
  `terminal_model::report_status_bar`, reading `review::footer`), or
  `Message::NoStatusLineFound` outside a modeled terminal.
- Command marks. Enter on a caret line holding input
  (`TerminalReader::caret_line_input`) marks the line (`MarkLine`).
  Verbatim+Alt+Up and Down Arrow (`review_text::command_mark`,
  `Review::Mark`) try the marks beyond the review position, nearest
  first, by their line's hash in the memory and above it with
  `TextOp::TerminalFind`, whose matches are placed with `review::placed`;
  a press makes at most `review::MARK_SEARCHES` (3) searches.
- Settings: "report new output" and "review follows caret" are given to
  every model's rules whenever they change.

`tests/terminal_reading.rs` drives `reduce` with snapshots of the
simulated terminal (`tests/terminal_support`), whose outpost reads as the
real one does, and holds its reading while Core's flood policy asks it to;
`tests/terminal_floods.rs` drives the reading rules and the flood policy
together through floods at the harness's terminal size and history.
`tests/terminal_cost.rs` is a measurement, not a test
(`books/dev/src/reference/performance.md`, "A terminal's reads in Core").
