# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format. What each behavior says, and why, is in the
parity ledger (`docs/parity/`) and the NVDA prose (`docs/nvda/`); this
guide maps those behaviors onto the crate's types and modules.

## Public API

- `reduce(&mut state, &input)`: deterministic, no I/O, no clocks; advances
  the state in place, without copying it, and returns the effects to
  execute. Every `Speak` carries the triggering input's trace id, which
  is what makes end-to-end latency timelines possible.
- `SNAP_RESULTS_ACTIVITY`: the activity id of the shell's window-snap
  results notification, the one UIA notification spoken from an
  application without the focus; the supervisor starts an outpost for no
  other notification.
- `focus_rules`: the focus rules the reducer shares with the focus
  listener and the outposts, each defined once (`docs/architecture.md`,
  section 2, "Where the rules run (D14)"). The reducer applies them after
  an event is read; the listener and the outposts apply them where a
  judgment must come before an element is read, against the focus Core
  holds. The rules ported from NVDA that only the listener and the
  outposts make, the acceptance test and NVDA's foreground tests, are in
  [verbatim-outpost](verbatim-outpost.md), since ported code stays in the
  Windows-specific crates.
  - `ForegroundRelation`: how a window stands to the foreground window (in
    it, topmost, or a `Windows.UI.Core` window under the active window),
    which an outpost reports in an event's window facts, and which the
    reducer judges a focus by, `of_facts(facts).related()`.
  - `RepeatEvidence` and `repeats_focus(evidence)`: a focus repeats the
    held one when Core has judged every focus the judge sent, the event
    names the element of the focus Core holds, and that element still has
    the focus; Core, the listener and the outposts gather the evidence
    each by their own means.
  - `ChangeKind`, `ChangeTarget`, `change_target` and `change_candidates`:
    a name, description, or value change is spoken on the focus, a state
    change on the focus or an ancestor whose state changes are spoken.
    `SelectionTarget`, `selection_target` and `is_selection_container(role)`:
    a selection of the focus or an ancestor is a change of its states; any
    other is an item spoken only inside a focused list or tab control of
    the same outpost.
  - `already_entered(previous, same_window, node)` with `EnteredKey`:
    whether the focus was already inside a container of a new focus,
    which the reducer speaks newly entered and an outpost gathers a newly
    entered dialog's own text by.
  - `HeldFocus` and `HeldNode`: the focus Core holds as it sends it, the
    trace of the event it came from, its observation time, the focus and
    its ancestors (id, backend, role, name, states), its top-level window,
    and whether it is alive; `for_outpost(outpost)` keeps an outpost's own
    nodes, their ids unstamped, or gives `None` for another outpost's
    focus.
- `is_selection_container(role)`: re-exported from `focus_rules`.
- `SrState`, the reducer's state:
  - `new()`.
  - `focused()`, the live focus and its application; `focus_source()`, the
    application the focus belongs to; `focus_report()`, the focus's
    snapshot, its ancestors, and the navigator object, which the control
    plane's `DumpFocus` answers with.
  - `focuses_queued()`, how many focuses the reducer has taken, and
    `held_focus_owner()`, the outpost of the held focus and whether it is
    alive: what the host compares across an input to tell whether its
    focus changed, without copying the focus.
  - `held_focus()`, the focus Core holds, which the host tells the focus
    listener and the outposts concerned after every focus event the
    reducer judges and every change of its focus
    (`SupervisorToOutpost::CoreFocus`).
  - `latest_focus_observed_at()`, when the newest applied focus was
    observed, in microseconds, which lets the shell skip a fake focus
    after a menu closes when a later focus has been applied.
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
- `dump`: `DUMP_FORMAT_VERSION`, `DumpHeader`, `DumpContents`,
  `DumpReadError`, `write_dump` and `read_dump`; see "The dump format".
- `read_of(&TerminalSnapshot)`, `snapshot_of(&Read, waited, colours)` and
  `read_at(&TerminalSnapshot)`: see "Core's terminal model".

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
three alive. Their size is not counted against the byte bound, because
the state holds everything that grows behind shared pointers.

`ReducerRecorder` is `FlightRecorder<RecordedInput, SrState>`:
`with_default_bounds(initial)` uses the shell's bounds,
`DEFAULT_MAX_ENTRIES` (1,024 inputs) and `DEFAULT_MAX_BYTES` (8 MiB, which
ordinary inputs never reach, so text-bearing inputs are what it caps);
`record_input(input, effect_count, &state_after)` clones the state only
when a checkpoint is due; `dump_inputs()` gives the recorded inputs,
oldest first, ready to replay from the checkpoint.
`RecordedInput::estimated_bytes()` is the entry's inline size plus the
length of its compact JSON, counted without building the JSON.
`replay(initial, inputs)` re-runs a recorded sequence and returns the
effects per step; tests prove it deterministic, and prove that a window
that has dropped its start replays from its checkpoint to the effects the
live session produced. `RecordedInput` and `SrState` derive `Serialize`
and `Deserialize`.

## The dump format

`dump` is a versioned JSON-lines file: a header line (`DumpHeader`: format
version, the writing crate's version, and a caller-supplied timestamp
string, since the module never reads a clock), then the `SrState` the
inputs start from, then one compact-JSON `RecordedInput` per line.
`write_dump(writer, crate_version, timestamp, base, inputs)` writes one;
`read_dump(reader)` reads one back as `DumpContents` (header, base state,
the inputs that parsed completely, and whether the file ended mid-line).

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
ancestors and with 10,000), that a flight-recorder checkpoint allocates
under 1 KB for both, even with a 64 KB line in the state, and that the
theme's fetches are taken in without allocating. It holds because `reduce`
changes the state in place and `SrState` holds whatever grows with the
application (the ancestor chain, text, terminal models) behind `Arc`s;
anything added to the state that grows must be held the same way.

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
  `last_tree_level` puts a changed tree level first. An edit field,
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
  carries the ancestor chain, outermost first; the reducer speaks the
  presentable containers (`is_presentable_container`) not in the previous
  chain. Ancestors reported unknown announce nothing and keep the previous
  chain. Two UIA nodes of one outpost are the same container only when
  their ids are equal; otherwise, within the same top-level window, a
  previous node with the same role and name counts as already entered. A
  focus in a different top-level window treats the whole chain as newly
  entered, which is how a window is announced when its foreground event
  was dropped.
- The window in front. `SrState::hook_foreground()`
  (`verbatim_model::HookForeground`) is decided from the window the focus
  listener last reported (`Input::ForegroundWindow`) and published by the
  shell after each input: keys may be held only when the window's process
  integrity is known and no higher than Verbatim's, and a console host's
  keys are translated with the layout of the thread its event named
  (architecture section 5).
- Rules for particular applications' windows (`applications.rs`; decision
  D6 keeps them in Core until application modules run in the extension
  host). The shell's staging windows are taken as focus silently
  (`docs/parity/windows-and-processes.md`, "The shell's staging windows");
  a UIA focus on the task switcher's frame is refused whole, and its item
  reached with Alt held is taken with no containers
  (`applications::drops_ancestry`; "The Alt+Tab switcher's pane").
  `tests/reduce.rs` checks all three.
- Event acceptance (`docs/architecture.md`, section 2, "Attention and
  acceptance (D14)"; `docs/parity/focus-events.md`, "Event acceptance").
  The state keeps an attention record, the application and window facts
  of the most recent foreground change. A foreground change is always
  accepted; another focus is attended when
  `ForegroundRelation::related` holds for its window facts, or by its
  application when it has none; any other event when its window facts
  share the attention window's top-level window or root owner, are
  topmost, or are related to the foreground. UIA notifications are
  filtered by the focus's application, except the snap results.
  Background kinds from elsewhere are spoken queued and never move focus
  or the navigator; everything else from elsewhere is dropped. With no
  attention yet, everything is accepted.
- Windows (`docs/parity/focus-context.md`, "Window announcement on
  switching applications"). A foreground change is a focus on the window,
  ignored when its top-level window handle equals the current focus's
  (handles, since node ids differ between outposts); a nameless one moves
  focus and attention silently.
- Repeats and staleness (`docs/parity/focus-events.md`, "Duplicate focus
  suppression" and "Stale focus events"). A focus that `repeats_focus`
  judges a repeat is dropped before anything else judges it, its newer
  reading kept unspoken; what Core last spoke of the focus is left as it
  was, so a change the repeat carried in is spoken when its event comes. Across outposts, a focus observed before the
  applied one is dropped (`observed_at_us`, a tie being no earlier).
- Background speech across a foreground change
  (`docs/design/focus-pipeline.md`, section 7): a background event
  observed before the newest applied foreground change is dropped.
- Changes (`docs/parity/changes-and-selection.md`). Each focus keeps two
  things: the reading Core holds (`FocusContext::snapshot` and
  `ancestors`), which review reads and a repeat updates, and what Core
  last spoke of it (`FocusContext::spoken`, a `SpokenRecord`): the
  focus's name, value, description and states, and each ancestor's
  states. A repeat never moves the record. A focus taken moves it, and
  so does a report of the focus's object (the report-focus command,
  reporting the current object while the navigator is on the focus, and
  the navigator's return to the focus), whatever the reason it is spoken.
  A name, value, description or state change on the focus moves it to the
  new reading even when nothing is spoken, as NVDA's property cache does.
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
  outpost is replaced"). Node ids carry the outpost incarnation. On
  `Input::OutpostEnded` the focus is marked dead and a navigator or
  pending navigation there is cleared; a later focus matching the kept
  copy (role, name, value, states, ancestors' names and roles) takes its
  new ids silently.

## Object navigation and review

(`docs/parity/review.md`.) `SrState` carries a navigator object and a
review cursor that follow focus. A `Command` runs against them:
report-object announces, spells, then copies on repeated presses; parent,
sibling and first-child moves emit a navigation `Fetch` whose completion
moves and announces the navigator; activate emits `Activate`, answered by
`Input::ActivationCompleted`; to-focus snaps back; report focus
(`report_focus`) runs against the focus. With no navigator a command says
"No navigator object". An object with text is announced by
`announce_navigator` and `editing::navigator_text`: name, role and states,
then its text, read from the known caret or through a `PendingText` with
`NavigatorSelection` and `NavigatorLine` follow-ups, which a newer text
request supersedes.

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

Only the reducer speaks (`docs/architecture.md`, section 2), so what the
shell's threads learn comes in as inputs (`shell.rs`): `Input::Started`,
`Exiting` (unless a new instance replaces this one), `ErrorLogged` and
`ApplicationNotResponding` play their sounds; `LockKeyToggled`
(`Phrase::LockKeyState`), `ClockRead` (time, then the date on a quick
second press), `ClipboardCopied` (`Phrase::CopiedToClipboard` or
`Message::UnableToCopy`) and `PreviewSpeech` speak, interrupting;
`ShellItemGone` and `ShellItemUnclickable` speak queued; `PreviewEarcon`
plays its event. Each speaks under its input's trace, so a later key press
fences a copy's confirmation off. `MenuOrSwitchEnded` asks for the
foreground application's focus (`Effect::ReadForegroundFocus`, NVDA's
fake focus) unless a focus observed since has been applied
(`docs/parity/backends.md`). `ForegroundWindow` says nothing; it is kept
for `hook_foreground`. `tests/shell.rs` checks each rule.

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

(`docs/design/key-holding.md`, "Core".) An `Input::HeldKey`, a caret key
the hook held back because `holds_caret_keys` was true, asks the focus's
outpost to read the caret (`TextOp::ReadCaret`) and keeps the key
(`HeldKeys`). The answer becomes the focus's caret, and the key is
handled as an `Input::CaretKey` pressed one millisecond after the read
ended, then passed on (`Effect::PassHeldKey`) after its watch is asked
for, so the watch reaches the outpost before the key reaches the
application. Any other answer, or the shell's bound passing, passes the
key on with the hook's time; with no focus whose caret is read, it is
passed at once.

Every key input that comes while a held key waits waits behind it
(`WaitingKey`) and is handled in order (`editing::next_held`): a held
key's read is asked for once the watch of the key ahead has been answered
(`HeldKeys::awaiting_watch`) or `KEY_AHEAD_BOUND_MS` (100 ms) after that
key was passed on, which a waiting key asks the shell for once
(`Effect::BoundHeldKeyWait`, answered by `Input::HeldKeyWaitEnded`). Other
keys are handled as the input they came as (`reduce::key_input`). The
watch of the key just passed is marked `later_key`, so its answer only
keeps the caret current and says its selection's change. Each waiting
key's effects carry its own trace (`Effect::ForKey`).

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
  (`SrState::superseded_caret`, `editing::superseded_reply`). The
  superseded slot holds one key, so in a burst of three keys where the
  first answer arrives after the third press, the first answer's
  selection change is not said; speech is cut at each press, as with NVDA.

### Formatting

(`docs/nvda/document-formatting.md`.) A caret key's unit, and a new
focus's first line, carry the formatting the outpost read
(`TextChunk::formats`). The state keeps the formatting last reported and
its node (`reported_format`); the unit is spoken with the formatting at
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
are asked of the outpost and spoken "Positioned at x, y".

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
  `docs/performance.md`, "Say-all, UIA".
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

What a terminal says (`terminal.rs`; `docs/design/terminal-model.md`,
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
  cursor as it draws (`docs/design/terminal-model.md`, "The review
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
  live again. A cut while held or with a request out sends
  `TextOp::TerminalCancel`; of reads before the cut
  (`Input::SpeechCancelled::at_ms` against the read's start) only the
  answers to keys are spoken, as for an overtaken request
  (`TerminalSpeechState::overtaken`).
- Anything that cuts speech drops the output waiting and handed to
  speech: `Input::SpeechCancelled`, a step whose effects stop speech or
  interrupt, and `Input::UtteranceCancelled`. `Input::UtteranceFailed`
  counts as heard. A focus moving to another node forgets the output, and
  output utterances carry the terminal's focus validity.
- Verbatim+5 (`ReviewCommand::ToggleReportNewOutput`) toggles "Report new
  output"; off drops what waits and speaks no output, but still the
  answers to keys.

The unit tests in `terminal/tests.rs` drive the flood policy and the
reducer with simulated playback and a simulated outpost that holds and
answers Core's requests as the real one does.

### Core's terminal model

The reading rules run in Core (`docs/design/terminal-model.md`, "Where it
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
- `SrState::terminals` holds a model per terminal node read, at most 16,
  least recently read forgotten first and an ended outpost's dropped: the
  rules (`TerminalReader`) behind an `Arc`, the keys not yet given to
  them, a read owed, waiting review requests, history rows asked for, the
  review places, the caret and screen shape, the shape told to the
  outpost, and the quiet-period timer's due time. A model is made with a
  terminal's first snapshot while it has the focus.
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
  `TextOp::TerminalShape`; the rules' timer is `Effect::TerminalTimer`,
  answered by `Input::TerminalTimer`.
- Review commands. A request the memory answers is not sent: `reduce`
  takes it out of the step's effects, asks for a fresh read
  (`TerminalScope::Fresh`) and answers it from the memory. Rows above the
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
real one does. `tests/terminal_cost.rs` is a measurement, not a test
(`docs/performance.md`, "A terminal's reads in Core").
