# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format. What each behavior says, and why, is in the
parity ledger (`docs/parity/`) and the NVDA prose (`docs/nvda/`); this
guide maps those behaviors onto the crate's types and modules.

## Public API

- `Runtime`: the event queue's paused function, kept beside the state by
  whoever runs the reducer and never cloned or written to a dump; a new
  one, as after a clone or a restored dump, rebuilds the function from the
  state's checkpoint before the next input.
- `reduce(&mut state, &mut runtime, &input)`: deterministic, no I/O, no clocks; advances
  the state in place, without copying it, and returns the effects to
  execute. Every `Speak` carries the triggering input's trace id, which
  is what makes end-to-end latency timelines possible, and the number of
  the user input that caused it, if one did (below, "Key numbers").
- `SrState::announces_toggle_key`: whether a lock key's announcement is
  still to come, which the shell's idle conditions read.
- `SNAP_RESULTS_ACTIVITY`: the activity id of the shell's window-snap
  results notification, the one UIA notification spoken from an
  application without the focus; the supervisor starts an outpost for no
  other notification.
- `focus_rules`: the focus rules the reducer applies after an event is
  read, each defined once (`docs/parity/focus-events.md`,
  `docs/parity/changes-and-selection.md` and
  `docs/parity/focus-context.md`). The rules ported from NVDA that the
  readers make, the acceptance test and NVDA's foreground tests, are in
  `verbatim-reader-rules`, since ported code stays in the GPL-tier crates.
  - `ChangeKind`, `ChangeTarget` and `change_target`: a name,
    description, or value change is spoken on the focus, a state change
    on the focus or an ancestor whose state changes are spoken.
  - `is_sensitive_slider(node)`: whether a node is a UI Automation slider
    of class `SensitiveSlider`, which gives the focus to its thumb; the
    reducer speaks it entered as a focus and its value changes while it is
    the focus's parent (`docs/parity/focus-context.md`, "A slider that
    focuses its thumb").
  - `same_object(old, new)` with `EnteredKey`, which gives an entry's
    `ObjectKey`: whether an entry of a previous focus line and one of a
    new line are the same object by `ObjectKey::is_same_object`, one node
    id, or one window's object, of one backend or a stand-in above the
    process boundary, when both are windows.
    `lowest_common_ancestor((previous, previous_focus), (new, new_focus))`
    gives where a new focus line meets the previous one, as NVDA's
    `api.setFocusObject` finds it, by `same_object` alone, the indexes of
    the two entries, or `None` when the lines share no object; the reducer
    keeps the previous line to that entry and enters the new entries below
    it. Neither allocates.
  - The module also exports items no code in the workspace calls:
    `ForegroundRelation`, `HeldNode`, `RepeatEvidence` with
    `repeats_focus`, `change_candidates`, `SelectionTarget` with
    `selection_target` and `is_selection_container`, and
    `already_entered_ancestors`.
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
  - `held_tokens()`: the tokens of the running listener's told elements
    Core holds (`HeldTokens`): those its waiting entries, its front entry
    and the entries it appends later name (`Logic::told_token`: a UI
    Automation focus or menu opening, a toast candidate, a UI Automation
    selection), and the readers' last focus's; `None` while no listener
    runs. A token is held from its tell while its entry waits or runs,
    and afterwards only while it is the readers' last focus; the set never
    takes back a token it dropped. `ListenerView` is the shell's record
    of the views it sends (`held_tokens`, the module): it counts the
    listener's messages carrying tokens as they are handed to Core
    (`handled`), and after each pass answers `ViewSend::Now` when a token
    handled, or one the last view held, is not held now, `Leave` when only
    additions or the position moved, and `Nothing` otherwise, as when no
    listener runs; a held set of another generation than the one counted
    is an error (`GenerationMismatch`), a logic bug, and sends nothing; a
    listener generation started counts from 0 (`started`).
  - The details the active theme reports, as the last `Input::Reported`
    set them (everything until then), kept in the state a dump restores.
    Core chooses each request's details from them (`details.rs`): a node
    read's property set is its base set less the properties of each node
    detail off (`NodeDetailSet::choose`), and a caret key's text reads
    take the text details reported, less spelling and grammar errors for
    a paragraph, as NVDA leaves them out for speed
    ([verbatim-model](verbatim-model.md), "Themes"). It is the one place
    the reducer consults the theme, and no reader holds a copy.
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
- `supervisor`: Core's supervisor's pure decisions. The outpost
  generation record Core's reducer owns (`record`, crate-private, applied
  by the event queue's driver; Table 5 of
  `docs/design/readers-tell-core-asks-build1.md`) judges every
  application's generation; [verbatim-supervisor](verbatim-supervisor.md)'s
  owner only reports process facts and carries out the record's effects.
  `policy` decides when a child is wedged (`Liveness::wedged` under
  `WedgeLimits`, by its pongs alone, for the listener and the spares; an
  application's outpost is judged by Core's record), how long a failed outpost's
  replacement waits (`Backoff`, which Core's record and the spares use),
  and which spares are kept and given (`Spares`). Every time these take
  is in microseconds on the run's shared clock. `EndReason` is why a
  generation ended; `Received` is when verbatim.exe took in a request it
  relays.
## Core's requests

Every request Core makes of a reader is recorded in the event queue's
request table (`crates/verbatim-core/src/event_queue/table.rs`; Table 2 of
`docs/design/readers-tell-core-asks-build1.md`), from its asking to its one
ending, under the request number it goes out with. An entry holds the
reader asked, the window it names, when Core stops waiting for it (13
seconds for a focus read's whole chain, 11 for any other), the request and
its owner: the front entry's function, an asker outside the queue (a tree
dump, the shell's item lists), Core itself (`Follow`), or Core's facts by
a token (a terminal's output reads and baseline).

- A request is waiting for its reader, out to one instance of it, or no
  longer awaited. An answered request leaves the table, so a later answer
  naming it finds nothing and is dropped.
- Every request begins in one place (`Shared::begin`), whether an entry,
  an asker outside the queue or Core itself asks it: a request on a plain
  node (`Issuer::Plain`) is routed to Core's plain reader (`Route::Plain`,
  `Logic::plain_answer`), which answers it at once from Core's facts with
  nothing sent; its ending is delivered once the asking function has
  paused in its ask, or as the step settles, and a function rebuilt from
  its checkpoint takes it from the checkpoint's record.
- One ending each, handed back with its owner: its reader's answer, taken
  only from the instance it went to, or the plain reader's; "not
  responding" when the shell refused to send it to a hung window; for a
  read of an application's current focus so refused, what the shell read
  in front (`Ending::InFront`), which only that read's reply decodes
  (`Request::decode_in_front`) and its entry judges, the inbox playing no
  sound and watching nothing for it; "ended" when its reader ended; or
  no longer awaited, at its due time, when the shell's live check finds
  its window hung, at a foreground change that leaves its window behind,
  or when a later entry withdraws it. A request no
  longer awaited that went out is kept by its number and instance only so
  that its late answer is dropped, and forgotten when that instance ends.
- Only the front entry may have a request out; its function is paused at
  the request and resumed with the outcome, decoded once as the request's
  reply type, so no paused function outlives its entry. A reply type
  holds every result a correct reader can give the request: a text
  request's is the op's own reply or a `TextFailure`
  (`text_request::decode_text`), each handled where the request is made.
  A result that still does not decode, one of another request's kind, is
  a malformed answer: refused as undecodable and logged, the reader not
  replaced.
- The table also keeps the recovery set, the windows found hung, which the
  shell checks every half second until each answers again.

Tests: the queue's unit tests (`crates/verbatim-core/src/event_queue/tests.rs`),
each rule of Table 2 and every ending, a late answer dropped by number and
an answer from an instance the request did not go to dropped.

### A dialog read once it is shown

A focus read whose window's top-level window is a hidden dialog is
answered with that window (`Reply::HiddenDialog`), parsed into
`focus::shown::Hidden` by the read's own reply type. Every focus read
that may meet one goes through `focus::shown::read_shown`: an MSAA
candidate at its turn (`msaa_run::read_in_full`), the current focus
(`retake::run`, the menu check, a UI Automation focus read afresh) and a
UI Automation focus's completion. On a hidden dialog the front asks the
listener `AwaitShown` under the read's own due time, and, shown, reads it
again from its start. The ending condition (`shown::ending`) is a pure
function of the dialog's window, the foreground record and the entries
waiting behind the front: the user is elsewhere when the record names
another window, a menu or popup start outside the dialog from the
application holding attention waits, or a focus outside it, a UI
Automation menu opening among them, that the one definition's rows
needing no read accept waits; otherwise a key pressed since the front
joined ends it as a key, by the speech fence's one rule
(`Arrivals::fence` above the front's fence), a modifier alone included,
whose cut settles the queue. The queue judges it as the request begins, ending one
that holds already unsent (`Shared::stopped_at_once`), and after every
entry that joins and every foreground report while it waits
(`Logic::stops`); the foreground's tree-and-owner stop does not apply to
it (`Logic::own_foreground_rule`). The outcome alone tells the function
which ending held, so a function rebuilt from its checkpoint decides
alike: elsewhere (`LeftHiddenDialog`), the read ends, nothing said, and
the oldest such focus waiting carries the dialog (`ended_wait`, kept
through a merge or the limiter's cap by `Logic::merged`); a key (withdrawn), it is read again
at once with `HiddenRoot::ReadAnyway`. The application's exit ends the wait not
there and cancels it to the listener (`Table::answer_cancelling`). A run
or UI Automation focus carrying `ended_wait` that executes nothing while
the record still names the dialog joins a re-take of the foreground
window's current focus (`shown::after_ended_wait`, by `Cx::join`). Tests:
`focus::shown::tests`, one per cell of the design's table, with the
message box, the dialog closed by hiding and the tray menu each fed in
both orders.

### A window's destruction

The listener's destroy arrives as `QueueInput::WindowDestroyed`, with the
process of the thread that raised it or none, and `route` drops it from a
listener that is not the current one. The inbox handles it at once, ahead
of the waiting entries, whose reads in the window answer "not there" at
their turn: it takes the window out of the recovery set and clears its
probe verdicts, and `Logic::window_destroyed` marks a plain ghost at the
window destroyed (`PlainReader::window_destroyed`, below) and ends a
followed node in it (`Follows::window_destroyed`), returning the `Follow`
naming nothing for its generation, which is always the ready one, since
only a ready generation's answer is committed and its end names nothing,
so the request never starts an outpost. The inbox sends `Effect::WindowDestroyed` to
the ready outpost of the raising process, if any, then begins the
`Follow` as Core's own request (`Owner::Core`, whose answer no rule
reads), so it runs after the outpost's forget. A destroy with no process
clears the same records and sends nothing to an outpost. Cost per
destroy: one pipe message from the listener and one wake of Core, which
a destroy with a process already paid when it was a tell, and removals
from maps; with the raising process's outpost running, one effect and one
pipe write to it; one forget program per drain of the outpost's mailbox
that holds destroys; one `Follow` only when the followed node's window
is destroyed; no call into any application (`books/dev/src/reference/performance.md`,
"A window's destruction").

Tests: `src/window_destroyed_tests.rs`, fed as the listener sends, the
recovery set's and the probe verdicts' in the queue's unit tests, and the
recorded settings dialog's three destroys in `tests/replay_validity.rs`.

## The outpost generation record

(`docs/design/readers-tell-core-asks-build1.md`, "The outpost generation
record", Table 5; `supervisor/record.rs`, applied by the event queue's
driver in `event_queue/outposts.rs`.) Core's reducer owns one record per
application: launching, starting, ready or waiting out a backoff, none
being its absence. The owner thread reports its process facts as
`QueueInput::Process` (`ProcessFact`): an outpost started, a launch
failed (with whether the application is gone), each pong, the
application's exit, the listener it is about to launch, the spares
waiting and Verbatim's own process; an
outpost's `Ready` and its end arrive as `ReaderReady` and `ReaderEnded`.
Core asks for an outpost (`QueueEffect::StartOutpost`) when a request
waits for an application with none, or the listener's foreground report
names one, and ends a generation (`QueueEffect::EndOutpost`) that passes
9 seconds with no pong. A request waits through the
launch and start and is sent at the generation's `Ready`; a first
failure is replaced at once, a later one after today's backoff, which a
foreground report resets and ends; a backoff that ends launches again
only when a request waits or the application holds attention. The
application's exit answers every request waiting or out "not there",
drops its held ids and takes nothing again. An outpost is never ended
for being idle (Dickson, 2026-11-10 and 2026-11-16): it ends when its
application exits, when it fails or stops answering pings, or when
Verbatim exits. A frame of
an instance that is not its reader's current one, a tell of an ended
generation or of a listener other than the newest started, is dropped
(`QueueEffect::StaleFrame`). `SrState::readiness` and `ready_outposts`
give the test harness's readiness and the control plane's outpost list
from the record. A process Core reads nothing from (`Logic::watched`) is
never started, and a request for it is answered "not there".

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
- `snapshot()` returns a `FlightSnapshot<T, S>`, the window as it stands
  (`checkpoint()`, `entries()`, `len()`, `is_empty()`), sharing rather
  than copying it, for a dump taken on another thread.

So the window always starts at a checkpoint, holds between half and all
of each bound, takes a checkpoint once per half window, and keeps at most
three alive. Their size is not counted against the byte bound; the
state's large parts that exist today (the ancestor chain, the text lines,
the terminal readers) are held behind `Arc`s, which a clone shares.

A dump must never hold up the reducer, which takes the recorder's lock
for every input, so nothing is copied under that lock. Each segment's
entries are a chain of immutable links behind `Arc`, each new entry
pointing at the one before it, and its checkpoint is behind an `Arc`
too; `snapshot()` clones one `Arc` per segment, at most three, whatever
the entries hold. The recorder goes on appending links and dropping old
segments, which the snapshot keeps alive until it is dropped, and the
dump walks the chains after the lock is released. A link's `Drop` frees
the chain behind it in a loop, stopping at a link a snapshot still
holds, so dropping a long segment never recurses once per entry.

`ReducerRecorder` is `FlightRecorder<RecordedInput, SrState>`:
`with_default_bounds(initial)` uses the shell's bounds,
`DEFAULT_MAX_ENTRIES` (1,024 inputs) and `DEFAULT_MAX_BYTES` (8 MiB, which
ordinary inputs never reach, so text-bearing inputs are what it caps);
`record_input(input, effect_count, &state)` clones the state only
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
thread requests. It asserts that caret and review steps and a terminal
read each allocate the same against a small state and a large one (a
previous focus with 10 ancestors and with 10,000), because `reduce`
changes the state in place. A focus step, whose request carries the
ancestors Core already knows, allocates no more than in proportion to the
chain: with 10,000 ancestors, at most twice the depths' ratio times what
it allocates with 1,000, the factor of two being a vector's doubling.

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
  two elements by runtime id.
- The window in front. `SrState::hook_foreground()`
  (`verbatim_model::HookForeground`) is decided from the window the focus
  listener last reported (`Input::Foreground`), one of Verbatim's own
  included, and published by the shell after each input: keys may be held
  only when the window's process integrity is known and no higher than
  Verbatim's, and a console host's keys are translated with the layout of
  the thread its event named (architecture section 5). Its desktop part
  counts the switches away from the user's input desktop Core has handled
  (below, "The secure desktop and the lock state").
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
  came to the foreground, the only one. The event queue takes it as its
  foreground (`Core::foreground`, `event_queue/inbox.rs`): it stops the
  front entry's request when the new foreground no longer holds the
  request's window, unless the request's own rule exempts it, and ends
  the waiting entries the change ends (`Logic::ends_at_foreground`). The
  reducer writes it to the foreground record (`foreground_changed`) for
  the keyboard hook's view. It speaks nothing and cuts nothing.
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
  (`Changed`, `Object`, `Entered`, `EnteredBy`) and read through
  accessors. It holds the foreground object, the window the listener last
  reported in front, which the keyboard hook's view is made from, and the
  entered window.
- Repeats (`docs/parity/focus-events.md`, "Duplicate focus
  suppression"): a focus candidate is judged at its turn against the two
  versions of the focus (`focus/versions.rs`), the committed focus and
  the readers' last focus, by the one definition of a candidate's
  validity (below), whose duplicate tests (`focus/duplicate.rs`) need no
  read. A duplicate is valid, reads and says nothing, and ends a run.
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
  cache does, and so does a state change on the focus, except an
  outpost's UI Automation change without the focused state on a focus
  that reported itself focused, which the focus had left when it was
  raised. A state change of an ancestor with an entry moves that
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
  state in an outpost's UI Automation change keeps its states silently
  (`reduce::outpost_states_changed`); an MSAA change or a selection is
  spoken whatever its focused state, sharing the listener's pipe with the
  focus in the order they were raised. A UI Automation change or
  selection records, as it joins the event queue, whether its element was
  on the focus line (`changes::AtArrival`): for an outpost's change the
  committed focus, a kept ancestor, or the UI Automation focus whose
  completion is out, the front's (`Logic::arrival` is given the front and
  its request); for the listener's selection the committed focus. Its
  state and description speech is given only when it was on the line and
  is still the committed focus or a kept ancestor at its turn
  (`docs/parity/changes-and-selection.md`, "State changes and the focus
  line"). A description change is spoken even when it repeats the name.
  A suggestions change off the focus plays the closed sound only for a
  suggestions field, by the text pattern and automation id its tell
  carries. An expansion carrying `child_count`
  adds "52 items" (`Phrase::Items`). A `Button` with the Toggle pattern is
  `Role::ToggleButton` with `Pressed` ([verbatim-uia](verbatim-uia.md)).
- Selections (same document, "Selection announcements" and "Selection in
  a list the focus controls"). A focus on a selection container is
  spoken without its selected item, as NVDA speaks it (Dickson,
  2026-11-17); a `SelectionChanged` while focus
  stays on the container speaks each newly selected item once,
  deduplicated against the focus's own child. A `ControlledSelection` is
  spoken as a focus, interrupting, and moves the navigator, only while
  its controller is the live focus.
- Progress bars (`docs/nvda/object-model.md`, "How a progress bar reports
  its value"): the listener's `Input::Progress` plays `Earcon::Progress`
  once the value moves a percent from the last one indicated at the same
  place on screen (`SrState::progress_reported`, the last 32 places).
- Notifications: a UIA `Notification` speaks its display string,
  interrupting for `MostRecent` and `ImportantMostRecent`, at Next for
  them during say-all, queued otherwise. Neither notifications nor UI
  Automation selections merge a waiting entry: each is handled in
  arrival order (ADR 0003, "Flood limiters and the merge").
- Outpost replacement (`docs/parity/focus-events.md`, "Recovery after an
  outpost is replaced"). Node ids carry the outpost generation. The end
  of a generation arrives as `ReaderEnded` (above, "The outpost
  generation record"), and a request out to it ends as "ended".

## Object navigation and review

(`docs/parity/review.md`.) `SrState` carries a navigator object and a
review cursor that follow focus. The navigator's commands are
event-queue entries (`navigator.rs`): each resolves the navigator object
at its turn and makes its reads as the queue's requests, naming the
object's window, so each holds the queue until its last answer.
Report-object announces, spells, then copies on repeated presses; parent,
sibling and first-child moves read the neighbour and announce it as the
navigator; activate says what was done; to-focus snaps back; report focus
is "Verbatim+Tab", below. With no navigator a command says "No navigator
object" (`navigator::no_navigator`). A review press is an event-queue
entry too (`review_text::begin`), so it reads live and runs in press
order with keys. An object with text is announced with its name, role
and states, then its text, read from the known caret or by the reads
`editing::navigator_text_start` asks and `editing::navigator_text_answered`
takes, each in turn.

### Verbatim+Tab

(`report_focus.rs`; `docs/design/readers-tell-core-asks-build1.md`,
"Report focus".) Each press is an event-queue entry that, at its turn,
reads the committed focus node's properties live in one request
(`ANNOUNCED_SET`) and speaks the answer only: never taken as Core's
focus, so Core's focus ancestry, its navigator and the outposts'
followed chains are unchanged. The first press says the object as its
announcement does, then its text from the answer's summary
(`TextSummary`: the selection, or the line); a second press spells the name, "blank" for
none, a third and later with character descriptions. With no committed
focus the press says "No focus" and asks nothing; any outcome but a
result says nothing (a request refused unsent or stopped because the
window is hung has already played the not-responding sound in the queue,
if the window is attended). Each press waits its turn, so two presses
give two announcements, and a press ahead of a focus reads the old
focus.

A press's answer ends it, or a follow-up read replaces its read (a line
read for the line, then a move, or an object's announcement and then its
text). A navigation that finds no neighbour or a dead object says the
direction's edge and the navigator stays (`docs/parity/review.md`, "Dead
objects"); `Unanswered` says nothing for the press. A dead object reviews
as empty flat text, one rule at the boundary (`review_text::answered` and
`review_text::object_read`); Report Current Object says nothing.

An object with no text interface is reviewed as flat text, its value or
name, read live as the queue's request (`review_text::Next::ReadObject`),
in the pure `review` module, by the same units as text: grapheme clusters, and words
from `text::words`, chosen from the text since flat text carries no
language.

## What the shell learns

Only the reducer speaks (`books/dev/src/explanation/architecture.md`, section 2), so what the
shell's threads learn comes in as inputs (`shell.rs`): `Input::Started`,
`ErrorLogged` play their sounds (the not-responding sound is the event
queue's, played for a request's hung window only while its application
holds attention, `sound_if_attended` in `event_queue/driver.rs`);
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
queue. A menu, popup or switcher end whose run executed no focus makes
the check entry 50 ms later (`focus::menus`, NVDA's fake focus), one for
the ends of one pump that noted the same committed focus
(`event_queue::PUMP_WINDOW_US`): with the
committed focus unchanged, it asks the foreground application's current
focus at depth `FullIfFocused` (`Request::CurrentFocus`), the outpost
reading an MSAA focus's focused state over every ancestor up to the
desktop as a fact; the answer is judged by the one definition's
menu-check rows, the focused state and the duplicate test for an MSAA
answer, as NVDA's `processFocusNVDAEvent` tests IAccessible objects
only, so a UI Automation answer of the focus Core holds is spoken again.
The check notes which end made it (`menus::CheckedEnd`), the first end's
for ends that share it.

A read of the focus Core makes itself (`retake::CurrentFocusRead`) that
the shell answered unsent, its window hung or, for the check after an
end, no window in front, is answered with what is in front
(`retake::Focused::InFront`), and its entry decides by `focus::plain::decide`
(Dickson, 2026-11-19). For the reads NVDA makes itself, the menu or
popup end's check, the read at startup and the read after the return
from another input desktop, a ghost in front is committed as the plain
object and spoken, its hung window watched with no sound; the desktop
window, with no window in front, is committed as the plain desktop
(`PlainKind::Desktop`) and says "Desktop window", for the switcher's
check too, with nothing watched and no sound; a responsive window or one
gone says nothing and watches nothing; a hung window with no ghost plays
the sound and is watched. Every other such read, the
switcher's check, the re-take, the recovery read and a UI Automation
focus's read afresh, plays the sound and watches the window whatever is
in front. Watching and the sound are the entry's (`Cx::watch_if_attended`,
`Cx::sound_if_attended`), each only while the window's application holds
attention.

The plain object is an ordinary object in Core's model: its node is
issued by Core's plain reader (`SrState::plain`, one `PlainObject` at a
time, each commit a new generation, held until the next plain commit),
its `Kept` reads `ReadBy::Plain` and names the application of the hung
window the ghost stands for, never the ghost's own nor, for another
application's ghost in front, the asked one, and its place names the ghost as top-level
window and root owner. It is committed by the ordinary `execute`, a read
of one node with no ancestors below the desktop, no text, and itself as
the foreground object, so the committed focus, the readers' last focus,
the focus line, the navigator, the foreground object and the entered
window all follow, the old `Follow`s stop and a focused terminal is
left. The plain reader answers every request on its node at once with
what NVDA's plain `Window` gives (`focus::plain::answer`): its
properties (name, the role window, the unavailable state from its
style), no children, an edge for every move, no text, "No action" for
activate; a node of an older generation is not there. When the ghost's
window is destroyed the object stays held and becomes
`PlainKind::Destroyed`, NVDA's plain `Window` of the dead handle: no
name, the role window and the invisible state alone, so Insert+Tab and
report navigator object, queries, say "window invisible" (Dickson,
2026-11-19); the desktop's plain object is never destroyed. Every command
therefore reads it through its own path. A plain focus is never a duplicate and nothing is a duplicate
of it; the listener's end re-takes nothing under it.

### One definition of a candidate's validity

`focus/validity.rs` is the one place a focus candidate is judged: a
focus-kind tell (an MSAA focus, foreground or menu start, a UI
Automation focus or menu opening), the menu check's read, or a recovered
read (`Subject`). `judge(subject, view, facts)` runs its rows in
order against Core's facts now (`View`: the two versions of the focus,
the foreground record, the backend cache, and whether the run before a
menu start executed a valid focus) and what an outpost read of it, if
anything (`CandidateFacts`), the same rows for its own turn and for
whether it is pending, and answers `Validity`: valid (read in full, or the
committed focus executed again), a duplicate (valid, nothing read or
said, ending a run), refused with a `Reason`, or needing a read (`Need`:
the focused state or a menu start's role). Its rows take the facts as
`Result<&CandidateFacts, M>`: with facts, `judge_read` instantiates `M`
uninhabited, so its answer is `Settled`, which has no needed read.
`depth` says how far the turn's read goes. `pending` maps an answer to
`Pending` (counts, unconfirmed, no).

`judge_run` is the one walk of a run, newest first, each candidate judged
with the facts stored on it: it answers the refusals on the way and the
first candidate not refused (`Found`, with the older candidates, where the
walk goes on). The run's turn (`focus/msaa_run.rs`, one request per
candidate, strictly newest first across applications) steps through it,
reading the found candidate and going on past one its read refuses;
`pending_behind` walks each waiting run through it, the first valid
candidate counting even behind a newer one needing a read, only the
candidates `Within` names judged: with a caret key's bound, one needing a
read raised at or after it is not pending; for a UI Automation menu
opening's drop, only what was raised before the opening, the front
included. So the turn, the pending judgement and the opening's drop
agree by construction. The other callers are the UI Automation focus
(`focus/uia_focus.rs`), the menu start and check (`focus/menus.rs`), the
recovered read (`focus/retake.rs`), the caret key, the caret tell and the
alert. `duplicate.rs`'s tests are its private primitives.

Each read of a candidate is a request typed by its depth
(`msaa_run::RunRequest<D>`: `FactsOnly`, `FullIfFocused`, `Full`,
`UnlessSame`; and `retake::CurrentFocusRead<D>`), whose answer is parsed
at its decode (`validity::Answered`: refused unread, `Unread`, or found
and read in full; a confirmation's facts alone; at `UnlessSame`,
`TurnAnswer::SameAsNamed` too). An answer that breaks its depth's
rule, a found candidate without its read or a confirmation with one, is
undecodable, logged as a malformed answer (`QueueEffect::Undecodable`),
and the entry ends.

The identity check (`docs/parity/focus-events.md`, "Duplicate focus
suppression"): the readers' last focus keeps the speech fence of the
entry that executed it (`LastFocus::since`), set by
`Versions::execute(kept, since)` from `Origin::fence`, which every path
names explicitly (a run's or menu start's executed candidate's own fence,
any other entry's own); a candidate tried and refused, not there or whose
reader ended clears it (`Reason::tried`, `Versions::tried_and_refused`).
`validity::depth` gives an MSAA focus `TurnDepth::UnlessSame` when its
fence equals `since`, the last focus is held by a child id above 0, and
it is the committed focus; the `Identity` is made from the committed
snapshot then and kept nowhere. The outpost's `Reply::CandidateSame`
parses to `TurnAnswer::SameAsNamed`, which `validity::judge_same` makes a
duplicate unless a row before the duplicate test now refuses the
candidate. A focus read asks the position in the set and the set's size
whatever the settings (`details::focus_set`), and speech leaves them out
with position reporting off. A candidate on the desktop window is refused
(`Reason::NoReader`), so Core starts no outpost for its process.

A candidate's `Confirmation` is written only by `store_facts`, from a
caret key's confirmation, and read by the rows as its facts: each
candidate is read at most once while it waits, and a stored refusal also
serves its turn, which asks nothing.

The desktop's news (`desktop.rs`): the user's input desktop returned
asks the application in front for its focus again (`retake::returned`,
NVDA's `_handleUserDesktop`); the virtual desktop's name, an MSAA name
change on the desktop window that the listener reads
(`announcements/desktop_name.rs`), is kept (`SrState::desktop_name`, the
newest only) until the next focus Core takes, which says it after the
foreground's title and before the entered containers, as NVDA's
`doPreGainFocus` calls `handlePossibleDesktopNameChange`, or, at its due
time 250 ms later (`desktop::tick`), spoken as a queued message.
`tests/shell.rs` checks the name's two rules.

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

### The caret key on the event queue

(`docs/design/readers-tell-core-asks-build1.md`, Tables 7 and 13.) A
caret key's `Press` runs `keys/caret_key.rs` at its turn, on the node it
was bound to at its press (`CaretBinding`), even when the focus has moved
on. A held key asks its starting read, then sends its stroke
(the pass) and asks the watch, then says its unit by its script; at the
bound a sentence key in text with a sentence unit moves by sentence from
its starting point. The key picks its path once at its turn, the edit
field's or the terminal's, and each path's stage (`CaretStage`,
`TerminalStage`) is derived from the request out, never stored; each has
its stage table, `withdrawn_by` one arm per cell, judging an `Arrival` (a
key, or a focus judged alone by the one definition, with when it was
raised). A later key withdraws the watch; a focus that counts withdraws
the starting read (but Backspace's), the watch and the sentence move; an
unconfirmed MSAA focus raised before the bound withdraws the watch. After
the pass the key judges what waits behind it with `pending_behind` up to
its bound: a focus that counts leaves it silent; an unconfirmed one is
confirmed by an `MsaaRun` at depth `Facts` naming the candidate's window,
its `spare` keeping the key's starting point held, its answer stored on
the waiting candidate and judged again; with neither, the watch runs for
the rest of the bound. Silent, Backspace and Control+Backspace still say
what they deleted from the starting read. Whether a key is marked is read
from `SrState::arrivals`, never stored. At its end a key that was not
dropped at a desktop switch applies the character it typed to the word
being typed (`typed_at_end`): Backspace's drops the word's last
character. Enter's carriage return ends the word and speaks it only when
the watch saw the caret move (`new_line_typed`); otherwise the word is
kept for a focus change to forget.
Enter held for a terminal, by a hook view behind the focus, is passed
with no read, the carriage return the terminal's model takes.

`keys/strokes.rs` is Core's stroke line (`SrState::strokes`): each held
stroke, from the press's arrival, and each deferred transition
(`QueueInput::Deferred`), in the hook's order. A held stroke goes when its
key passes, at its turn, by its 150 ms release from its press
(`Logic::due` and `Logic::tick`), or when a foreground change ends its
waiting entry (`Logic::ended_at_foreground`), with what is deferred behind
it up to the next unsent held stroke, as one `QueueEffect::SendStrokes`.
A reset of the hook's line (`QueueInput::StrokesReset`) drops what is
unsent, leaving a dropped mark for each held key's turn, and withdraws
the front's starting read; at `QueueInput::Exit` Core names the keys
whose last transition in line is a release (`QueueEffect::ReleaseAtExit`).
Beside the line, `SrState::alt_down` is whether Alt is down, set by the
hook's `QueueInput::Alt` at once in the inbox; a UI Automation focus tell
takes it at its arrival (`UiaFocus::alt_down`), and its execution judges
File Explorer's switcher rule (`applications::drops_ancestry`) with it and
the parent's automation id its tell carries. A UI Automation property
change, the listener's or a followed node's, passes acceptance on its
tell's facts before Core judges it (`changes::uia`, `changes::uia_entry`);
a value change is judged as a value change, which background progress
bars reported accepts from any window, as NVDA maps a UI Automation value
or range value change to its value change event, so the focus's value is
spoken while the taskbar is in front with the setting on, and every other
property as a property change. An event accepted only because background
progress bars are reported is accepted apart from the foreground, so a
foreground change neither stops its request out nor ends it waiting
(`accepted_apart_from_foreground`, judged with Core's facts, which
`Logic::exempt_from_foreground` now takes). `verbatim_core::accepts` is
the acceptance test's answer alone, public so that the listener judges
with Core's own function whether to read a value change while background
progress bars are not reported (Dickson, 2026-11-19). Every
`Input::Settings`, the first included and whatever its value, emits
`Effect::ListenerConfiguration`, so the listener's configuration and the
supervisor's launch copy follow Core's setting.

### Caret tells and Core's text facts

(`docs/design/readers-tell-core-asks-build1.md`, "Selection and the
caret's text facts", "Held anchors", Table 17.) The committed focus's
record (`Kept`) carries its `SelectionFacts` (`text_facts.rs`): the
selection baseline, an anchor its outpost generation holds, and the
text-changed flag. A caret tell (`caret_tell.rs`), the system caret's
for the readers' last focus or a UI Automation text-selection change of
the committed focus's element, asks at its turn, with no focus pending,
one `ReadSelection` naming the baseline and, for a UI Automation tell,
the tell's element for the outpost to adopt when the node's own has no
text pattern, and takes the caret it reads in arrival order. With no baseline named the answer only mints the next;
otherwise Core speaks the difference in general terms when the flag is
set or, in an edit control, when a value change of the focus waits in
the queue (taken out as counted), else in specific terms, then clears the
flag and takes the new baseline. An edit control's answer whose
selection offsets equal the last caret tell's (`SrState::edit_selection`)
changes nothing. An answer that the node has no text (`TextFailure::NoText`)
leaves Core no caret and no selection for it, the baseline and the flag
cleared, nothing said (`text_facts::no_text`), as NVDA's caret event says
nothing for an object that is not editable text; any other failure says
nothing and keeps the baseline and the flag. A value change of an edit control that is the committed
focus sets the flag and says nothing; a UI Automation value change of an
element with a text pattern says nothing. Answers to `MoveCaret`, a
sentence move and `Select` on the focused text mint the next baseline.
Every focus request names the details its caret read fetches, the
line's formatting the settings report (`details::focus_details`), and a
focus answer's `FocusCaret` becomes Core's caret, its line, with its
formatting, or its selected text said with the focus: a caret read mints
the baseline and clears the flag, silently, a repeated focus's too, as
NVDA's `initAutoSelectDetection` does (`text_facts::focus_answered`;
Dickson, 2026-11-17); a focus answer with no caret,
or a failed focus read, clears the baseline and the flag. The end of the
focus's outpost generation drops the baseline, and the silent re-take
takes the baseline its answer minted and keeps the flag. A caret key's starting and final reads compare no
selection.

`SrState::held_anchors` and `held_nodes` name, besides the baseline,
every anchor and node a request of the event queue names (a key's
starting point while its watch or sentence move is out), and the node a
waiting caret key was bound to.

## The secure desktop and the lock state

(`docs/design/readers-tell-core-asks-build1.md`, "The secure desktop and
the lock state", Table 8; `input_desktop.rs`.) The listener's desktop
switch tell is handled at once in the inbox, never queued
(`entries::route`). A switch to an input desktop Verbatim cannot open
cuts speech, commits the sleeping focus (`Versions::commit_own` with no
source, the focus context emptied so that the next focus is spoken from
the top with its window's title), sends `Follow` naming nothing to every
outpost followed, as Core's own requests outside the queue
(`event_queue::Input::Send`), drops the strokes Core holds, and counts
the switch in the hook's view (`HookForeground::desktop_switches`), on
which the hook forgets its modifiers and its line. A switch whose
desktop opens arms a 200 ms timer (`InputDesktop::ReturnPending`), and
when it comes due (`Logic::tick`) the read after the return joins the
queue: the front application's current focus with none of the ancestors
Core already knows and no duplicate test (`retake::Why::Returned`). A
switch away while the return is pending cancels it. No re-take is made
while the sleeping focus is committed. The lock state is recorded from
`QueueInput::Lock`, unknown counting as locked
(`SrState::session_locked`); nothing judges it before M8.7.

## Key numbers

(`books/dev/src/explanation/design/ownership.md`.) Every user input carries the number the keyboard hook's thread
gave it (`verbatim_model::KeySeq`), as `key_seq` on all seven:
`CaretKey`, `HeldKey`, `CharacterTyped`, `ClearingKey`, `EditingChord`, `Command`,
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
"TextInfo-equivalent layer".) A report carrying
`ReadFacts` that ended at or after the key was passed on, on the key's
node, and shows the key's effect by `had_effect` is the key's evidence; a
report with no effect, or one of another node, is ordinary, and its
selection changes are held for after the key's words. At its bound Core
asks one final read: if it shows an effect that is the evidence too, and
otherwise the read is the bound answer, which says the key's unit as NVDA
does for a key that moved nothing. The previous line and selection the
key's words need travel in `Change::Compared`.

Each focus object holds one caret (`FocusContext::caret`). Core takes
every report as the newest caret by when its read began
(`CaretReport::read_at_us`), so a key's answer and an ordinary report
need no comparison of arrival. A later key marked before an answer's read
began leaves that answer's words unsaid, and the caret and the selection
changes are still taken. A bound answer with any later key says nothing.

The answer updates the caret and speaks, queued:

- Left and Right Arrow, Home and End: the character at the caret, each
  character of a line break its own (`text::characters`).
- Control with Left or Right Arrow: the provider's word; a one-character
  word by its name.
- Up and Down Arrow, the page keys, Control with Home or End: the line.
- Control with Up or Down Arrow: the paragraph, or the line.
- Backspace and Control+Backspace: the provider's character or whole word
  around the character before the caret, or before a selection's start,
  which the starting read read (`StartingUnit::BeforeCaret`), spoken as a
  character when it is one and as text otherwise, as NVDA's Backspace
  scripts speak it; at a line's start the line break, of the kind last
  seen; over a selection, then the selection's change.
- Delete and Control+Delete: the character or word now at the caret.
- Enter in a text field other than a terminal (`CaretMotion::NewLine`):
  when the caret moved, the new line's text from its start up to the
  caret, nothing where it is blank, and where it is empty only the
  formatting at the caret's character (`editing::new_line_segments`), as
  NVDA's new-line script; nothing at the bound.
- Any other passed key that types nothing (`CaretMotion::Other`, from the
  hook's `KeyReport::Passed`, and the clearing keys): nothing in a text
  field, as NVDA; in a terminal its model judges it by its evidence
  (`docs/parity/text.md`, "Keys outside the caret table").
- With Shift, and Control+A: every part that became selected, then every
  part that became unselected, as NVDA orders them
  (`editing::selection_segments`), 512 characters or more as their count;
  a change in general terms (`content_changed`) speaks the whole new
  selection.
- A selection no key made arrives as a caret tell, an event-queue entry
  (`caret_tell.rs`): at its turn on the committed focus, with no focus
  pending, it asks one caret read naming the selection baseline, and the
  difference is spoken the same way, in general terms when the text
  changed.

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
an edit field, a document or a terminal that is not read-only
(`editing::is_editable`), as NVDA excludes a read-only focus of every
role; the model has no editable state, which NVDA also counts, until the
IAccessible2, MSHTML and Java Access Bridge backends bring it. A
protected field echoes only "star". In a terminal, unless terminal
passwords are spoken, typing is held until its
`NormalizedEvent::TextChanged` arrives, and Enter drops what was held.
The word is forgotten when a caret key's script speaks (not Enter's, whose
carriage return ends it after when the caret moved), and when the focus changes; a held
Backspace drops its last character at its end; `Input::ControlTyped`, a
control character the hook reports (Control+C, Control+Enter; never
Escape), ends it and speaks it, spelling nothing (`editing::control_typed`).
Interrupting speech for typing is the hook's
([verbatim-input](verbatim-input.md)).

### The review cursor over text

(`docs/nvda/review-modes.md`; `docs/parity/review.md`, "Review cursor
line/word/character over object text" and "Review cursor columns".) While
the cursor follows the caret (`ReviewText::Caret`), a review command reads
the line at the caret as the outpost finds it (`TextPoint::Caret`), never
from Core's copy, because a caret event can reach Core after a review
command pressed later; say-all from the caret and the caret's location do
the same. Once moved, a command that answers from the cursor's line first reads
the line where it is now (one round trip, `review_text::begin`); a move goes
straight to the read that moves, so no press reads twice to move. Moving
to another line keeps a cell column in a terminal and a remembered
character column elsewhere, kept across a short line for every text. A
lost anchor starts again from the caret.

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
- From the caret on a focus with no text, by its role or a caret Core
  holds, it says nothing and starts nothing, as NVDA's says nothing where
  it can make no caret position (Dickson, 2026-11-19).
- It ends after the last piece and stops on any command, caret key, typed
  text, a key's cut of speech (`QueueInput::SpeechCut`),
  `Input::UtteranceCancelled`, a focus
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
- Reading on demand (`docs/parity/terminals.md`, "Reading a terminal on
  demand"; `docs/design/readers-tell-core-asks-build3.md`). The outpost
  tells Core of every change signal; Core alone keeps "told, not yet
  read" and sends the output read (`terminal/reading.rs`, "Core's
  terminal model" below). Once a group's worth of lines waits, the
  reading holds: a change told is kept, not read. When the group's last
  line is handed to speech, one waited read is sent as the release, and
  the backlog is decided once the release read's answer and the last
  line have both happened (`TerminalSpeechState::deciding`). With nothing
  waiting, reading goes live again. When the outpost ends, the reading
  goes with its model and nothing more is asked of it; what Core had
  already read is still spoken, as NVDA speaks the lines its monitor
  read. A cut while holding or with a read out leaves that read
  silent: of reads begun before the cut
  (`TerminalSpeechState::cut_at_ms` against the read's start) only the
  answers to keys are spoken (`Mark::Silent`).
- Anything that cuts speech drops the output waiting and handed to
  speech: a key's cut (`QueueInput::SpeechCut`), a step whose effects stop speech or
  interrupt, and `Input::UtteranceCancelled`. `Input::UtteranceFailed`
  counts as heard. The output waiting is the focus's: a focus moving to
  another node forgets it, an outpost replaced under the same focus keeps
  it, and output utterances carry the focus's validity.
- Verbatim+5 (`ReviewCommand::ToggleReportNewOutput`) toggles "Report new
  output"; off drops what waits and speaks no output, but still the
  answers to keys.

The unit tests in `terminal/tests.rs` drive the flood policy and the
reducer with simulated playback and a simulated outpost that tells a
change per write and answers Core's reads from the start anchors it names,
as the real one does.

### Core's terminal model

The reading rules run in Core (`books/dev/src/explanation/design/terminal-model.md`, "Where it
runs"): `terminal_model.rs` and `terminal_model/review.rs` take each full
read of a terminal (`TextReply::TerminalRead`, the snapshot with the start
anchor it minted) and the answers to the requests they make;
`terminal/reading.rs` is the focused terminal's life and its reading.

- Public: `read_of(&TerminalSnapshot)`, the rules' `Read` of a snapshot
  (its text without its lines' padding, and the screen, padded again
  exactly); `snapshot_of(&Read, waited, colours)`, the snapshot of a read
  as an outpost sends it, for tests and measurements over the simulated
  terminal; and `read_at(&TerminalSnapshot)`, when a snapshot was taken as
  far as keys are concerned: when the read ended, or when it began for a
  snapshot that does not say when it ended.
- The focused terminal is one value, `FocusedTerminal` (`terminal/reading.rs`):
  `None`; `Baselining`, its `Follow` and baseline read out (the baseline
  carries Core's parameters; a failed baseline is sent at the next
  change); `WaitingForWindow`, its window hung before the baseline
  answered, which the recovery set re-asks; or `Modelled` with its
  `Reading`. A `Reading` is `Quiet`, `Behind` (a change told while an
  entry's full read is out), `Out` (an output read out, with its token,
  its `Mark` and the change told meanwhile), `Holding` (the flood
  policy's hold) or `Stalled` (its window hung while a read was out). A
  read's `Mark` says what its speech does: `Spoken`, `Waited` (the
  release at a group's last line or not) or `Silent`. A change that the
  last read, begun after it was observed, covers needs no read; every
  other change sends one output read at once, outside the event queue,
  as Core's own request awaited by its facts. Every full read's answer is
  taken by the model at once, in the order the outpost's worker made the
  reads, and its speech joins the event queue as a `SpeechEntry`, judged
  at its turn (a cut that came after the read began, a changed focus or
  model generation, and its reader's end silence it). A late answer is
  dropped by its number.
- `SrState::terminals` (`Terminals`) holds a model per terminal node
  read, at most 16, least recently read forgotten first and an ended
  outpost's dropped, each only through `SrState::drop_models`: the rules
  (`TerminalReader`) behind an `Arc`, the model's generation, the keys not
  yet given to them, the history rows that came, the review places, the
  caret and screen shape, the start anchor the next read names (the start
  of the last read whose screen showed text, or the first), the scope the
  rules owe (`Fresh`, `Whole` or `HistoryBlock`, taken by the next full
  read sent), when the newest read began, and the quiet-period timer's due
  time. A model is made with the baseline while the terminal has the
  focus.
- Keys. Every key input while the focus is a modeled terminal is kept by
  its kind, a clearing key as `Forget` then `Other`. Whether a read takes
  the screen's colours is Core's decision from the keys kept
  (`KEY_WINDOW_MS` from a key's time) and the last read, sent in the read's
  request (`TerminalRead::colours`). Before a read
  taken at S (`read_at`) is given to the rules, every kept key at or
  before S is given in order; before the timer at T, those up to T; past
  256 kept keys the oldest is given at once. Leaving the terminal tells
  the rules `blur` and drops the keys; returning tells them `focus`. A
  caret key in a terminal is Core's caret key entry with its watch, whose
  starting and final reads are full reads of the terminal.
- A read's speech key is the newest key given whose window holds the
  read's time; its speech and effects go out as `Effect::ForKey` for that
  key. A read with no speech key, and the timer's speech, carry the
  input's trace.
- Reads. A full read is a `TextOp::ReadTerminal(TerminalRead)`: the start
  anchor the model names, the scope, the screen's shape from the model,
  whether to take colours, and whether the window is a console host's. A
  read identical to the last one given to the rules, with its times,
  number, start and caret left out, is skipped unless it is the first
  after the focus arrived. A read's caret report becomes Core's caret at
  the read's time. The rules' timer is the focused model's due
  time, which the shell reads after each step
  (`SrState::terminal_timer_due`) and answers with `Input::TerminalTimer`
  once it is past; another model's due time does not count, so a model
  dropped or left has no timer, and the timer's step moves the due time
  past the time it fired at (a debug assertion, and a postcondition of
  the generated test).
- Review commands. A review command in a focused modelled terminal is one
  entry on the event queue (`terminal/commands.rs`): it sends one fresh full
  read and answers from the memory the model then holds. Rows above the
  memory are asked for with `TextOp::TerminalHistory { above, whole, start, height }`, a
  screen more than needed, and placed from the start anchor Core named, which
  the outpost holds until the answer (`NodesHeld` lets it go); an anchor whose
  row left the memory ends the command silently, marks kept. The memory's rows carry anchors with
  the top bit set (`TERMINAL_ANCHORS`), a range no outpost mints from, with
  at most 64 places beyond those Core holds. "Report status bar"
  (`terminal::commands::status_bar`) reads the focused modelled
  terminal's footer (`review::footer`) after a fresh read, or says
  `Message::NoStatusLineFound` outside one.
- Command marks. Enter on a caret line holding input
  (`TerminalReader::caret_line_input`) marks the line (`MarkLine`).
  Verbatim+Alt+Up and Down Arrow (`review_text::command_mark`, with
  `terminal::commands::command_mark`) try the marks beyond the review position, nearest
  first, by their line's hash in the memory and above it with
  `TextOp::TerminalFind`, whose matches are placed with `review::placed`;
  a press makes at most `review::MARK_SEARCHES` (3) searches. Output
  written just before Control+C during a mark search can be spoken, a
  limit Dickson accepted (2026-11-16).
- Settings: "report new output" and "review follows caret" are given to
  every model's rules whenever they change.

`tests/terminal_reading.rs` drives `reduce` with snapshots of the
simulated terminal (`tests/terminal_support`), whose outpost reads as the
real one does, and holds its reading while Core's flood policy asks it to;
`tests/terminal_floods.rs` drives the reading rules and the flood policy
together through floods at the harness's terminal size and history.
`tests/terminal_cost.rs` is a measurement, not a test
(`books/dev/src/reference/performance.md`, "A terminal's reads in Core").
