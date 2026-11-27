# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format. What each behavior says, and why, is in the
parity ledger (`docs/parity/`) and the NVDA prose (`docs/nvda/`); this
guide maps those behaviors onto the crate's types and modules. Its API is
in its rustdoc (`cargo doc -p verbatim-core --no-deps --open`).

## The reducer and its state

- `Runtime`: the event queue's paused function, kept beside the state by
  whoever runs the reducer and never cloned or written to a dump; a new
  one, as after a clone or a restored dump, rebuilds the function from the
  state's checkpoint before the next input.
- `reduce(&mut state, &mut runtime, &arrival)`: deterministic, no I/O, no clocks; advances
  the state in place, without copying it, and returns the effects to
  execute. The input's `Arrival` carries when it arrived, stamped on the
  run's shared clock where it was made; Core takes that time once, as a
  `Now`, and every effect and stamp of the input's handling uses it,
  passed down by value. Core's state keeps no copy of it. The event
  queue's paused runtime holds it (`Shared::now`, read through
  `Cx::now`): it persists there between steps, and is overwritten as
  each step starts (`feed`) and by a rebuild (`rebuild`) before anything
  reads it. The one time the state keeps is the front entry's
  checkpoint's, the time its entry started, from which a rebuilt
  function polls. Every `Speak` carries the triggering input's trace id, which
  is what makes end-to-end latency timelines possible, and the number of
  the user input that caused it, if one did (below, "Key numbers").
- `SrState::announces_toggle_key`: whether a lock key's announcement is
  still to come, which `queue_idle` takes in and the shell's marks of
  handled input wait behind.
- `SNAP_RESULTS_ACTIVITY`: the activity id of the shell's window-snap
  results notification, the one UIA notification spoken from an
  application without the focus; the supervisor starts an outpost for no
  other notification.
- `batch`: Core's builder of batches and the parse of their answers
  (below, "Batches"), public so that `outside` is built from it.
- `outside`: the batches the shell asks from outside the event queue and
  the answers it builds from them: the control plane's tree dump
  (`dump`, `dumped`, a `DumpedTree`) and the notification area list's walks
  of the taskbar's windows (`shell`, `items`, `candidates`). A list is
  complete or nothing.
- `focus_rules`: the focus rules the reducer applies after an event is
  read, each defined once (`docs/parity/changes-and-selection.md` and
  `docs/parity/focus-context.md`).
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
- `SrState`, the reducer's state:
  - `focused()`, the live focus and its application; `focus_source()`, the
    application the focus belongs to; `focus_report()`, the focus's
    snapshot, its ancestors, and the navigator object, which the control
    plane's `DumpFocus` answers with.
  - `attention()`, the application holding attention, which the shell
    sends to the supervisor.
  - `held_nodes()` and `held_anchors()`: every node id and every text
    anchor the state refers to, grouped by the outpost that issued it
    (the anchors are the caret's line and selection, the review cursor's
    line or point, a start marker set outside flat text, and say-all's
    positions). The shell
    sends each outpost its own, which keeps them alive
    ([verbatim-model](verbatim-model.md), "The text protocol").
  - `held_told()`: the tokens of the caret tells' elements Core holds in
    each ready outpost, those its caret entries waiting or running name
    (`CaretTell::adopted`), sent with that outpost's held nodes and, a
    change of them alone, left for the next command, as anchors are.
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
    listener runs; a view names the held set's generation, and a listener
    started counts from 0 (`started`).
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
  - What the keyboard hook is told after each input (below, "The caret
    key on the event queue", and "Focus, events and objects", the window
    in front).
- `terminal::reader`: a terminal's reads as batches of the request list's
  generic text steps, and their answers read back as the reading rules'
  read; the module's documentation lists each batch's steps. They carry
  the focused terminal's full reads, its history reads and its command
  search (Dickson, 2026-11-26).
  - `WindowAsk` (`read`, `colours`, `starting`, `output`) with
    `WindowRead` (`First`, `From { start, height, count }`, a count read
    probing from the next read's start to the text's start so that the
    snapshot's `rows_above` counts the rows above it, and
    `HistoryBlock { height }`), and `HistoryAsk`
    (`Whole`, `Above { start, above, height }`): what Core asks, each read
    with only the facts it has.
  - `window_batch(node, &WindowAsk)` and `history_batch(node, &HistoryAsk)`:
    the batch and its steps, pure functions of their arguments.
  - `window_of(&Answers, &WindowSteps, &WindowAsk, (began_us, ended_us))`:
    the `WindowReply`, which is the `TerminalSnapshot`, the anchor held at
    the next read's start, when the read began and, for a starting read,
    the caret key's starting point; or the `TextFailure` a step ended
    with, a start no longer held being the terminal gone. The times are
    the answer's own. `history_of` gives a history read's padded text and
    its rows above the start.
  - `SearchAsk::new(start, needle, from, rows)`: a command search, for
    the rows that start with `needle` each way from the row `from` rows
    above the start an earlier read named, reading `rows` rows from each
    match, at least 1 and at most 255, a step's most.
    `search_batch(node, &SearchAsk)` is its batch: a move from the start
    to the text's end, a move to the row, taken wherever it lands, and a
    `FindText` toward the end and one toward the start, each to
    `SEARCH_MATCHES` (20) matches over `SEARCH_CALLS` (100) calls.
    `search_of` reads the answers as the matches, alternately the next
    toward the end and the next toward the start, at most 20, each in
    rows above the start (`TerminalMatch`); a search with a step not done
    answers its failure and no matches, the terminal gone when any step's
    node or start is not there, so that its caller drops no mark.
  - `asked(&Batch)`: the ask whose builder writes exactly that batch
    (`Asked::Window`, `Asked::History` or `Asked::Search`), `None` for
    any other batch; a caret key's stage tables and the tests tell a
    terminal's read by it.
- `supervisor`: Core's supervisor's pure decisions. The outpost
  generation record Core's reducer owns (`record`, crate-private, applied
  by the event queue's driver; below, "The outpost generation record")
  judges every
  application's generation; [verbatim-supervisor](verbatim-supervisor.md)'s
  owner only reports process facts and carries out the record's effects.
  `policy` decides when a child is wedged (`Liveness::wedged` under
  `WedgeLimits`, by its pongs alone, for the listener and the spares; an
  application's outpost is judged by Core's record), how long a failed outpost's
  replacement waits (`Backoff`, which Core's record and the spares use,
  each keeping its own wait's end, by `BACKOFF`'s times, written once
  with the ping interval and the pongs that may be missed,
  `PING_INTERVAL` and `MISSED_PONGS`, which the supervisor reads),
  and which spares are kept and given (`Spares`). Every time these take
  is in microseconds on the run's shared clock. `EndReason` is why a
  generation ended; `Received` is when verbatim.exe took in a request it
  relays.
## Core's requests

Every request Core makes of a reader is recorded in the event queue's
request table (`crates/verbatim-core/src/event_queue/table.rs`), from its
asking to its one ending, under the request number it goes out with. An entry holds the
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
  or when a later entry withdraws it. One function judges the ending
  before an answer (`Logic::ends`), for its three causes
  (`event_queue::Cause`): an entry arriving, judged before the limiter
  merges it; the foreground report; and the queue settling after Core's
  facts changed outside it, which `entries::unsettled` asks too.
- A request goes to the outpost of the process serving the window it
  names, which the reader that read the window decided
  (`verbatim_win32::provider::serving_process`): a node read's window
  (`NodeWindow`) and a tell's facets carry it as an `Option<Pid>`, `None`
  when none could be read (Claude, 2026-11-27). Core takes it once, at
  `RequestWindow::of_node` and `entries::request_window`: with none, a
  navigation leaves the navigator put and says nothing, a controlled
  selection's navigator names no window, a hidden dialog is not
  postponed, a hung window is not watched, a tell is taken as having no
  window to ask, and an asker outside the queue is answered "not there".
  The foreground record's process is the same `Option<Pid>`
  (`event_queue::Foreground`): with none, Core starts no outpost, holds
  no attention for it, and asks no reader (`Foreground::window` gives
  none), so no startup read, menu or switch check, or read on the input
  desktop's return is sent; the check and the return's read end as an
  untried read (`validity::untried_read`): the focus's caret and
  selection facts are forgotten and the readers' last focus's fence
  stays.
  A caret key's binding carries the committed focus's window as a
  `ServedWindow`, whose process is always known.
- A request no
  longer awaited that went out is kept by its number and instance only so
  that its late answer is dropped, and forgotten when that instance ends.
- Only the front entry may have a request out; its function is paused at
  the request and resumed with the outcome, decoded once as the request's
  reply type, so no paused function outlives its entry. A reply type
  holds every result a correct reader can give the request: a batch's is
  one outcome per step (`batch::Answers`), handled where the request is
  made. A result that still does not decode, a batch's answer that does not fit the batch sent, is a malformed
  answer: refused as undecodable and logged, the reader not replaced.
- The table also keeps the recovery set, the windows found hung, which the
  shell checks every half second until each answers again. A window
  joins it by a refusal or a stop as hung, or by a reader's report
  (`QueueInput::WindowHung`, `Input::WindowHung`, handled at once like a
  destroy, since it changes no entry), through one function,
  `Shared::hung_if_attended`: while the window's application holds
  attention, the not-responding sound plays when no window of that
  application is in the set yet (`Table::recovering`), and the window is
  watched (Claude, 2026-11-21).

Tests: the queue's unit tests (`crates/verbatim-core/src/event_queue/tests.rs`),
each state of a request and every ending, the recovery set's rounds, a
late answer dropped by number and an answer from an instance the request
did not go to dropped.

### The waiting entries

Where an arriving entry waits behind the front is decided in one place,
`event_queue/limiter.rs`, by the part the logic gives it (`Logic::place`;
ADR 0003, "Flood limiters and the merge"; `docs/parity/backends.md`, "MSAA
winevent flood control" and "MSAA menu events in one flush"). Only
waiting entries are merged, joined or counted, never the front.

- An entry of any other kind that the logic says merges replaces the
  newest waiting entry `Logic::merges` finds the same (a UI Automation
  focus of the same kind, element and thread, a property change of the
  same element, property and thread, a progress bar's or a caret tell's
  own rule), searching from
  the back and stopping at a focus-kind entry, which is itself reached;
  the newer joins the back.
- An MSAA focus or foreground tell joins the run waiting with no accepted
  MSAA tell after it, or starts one, replacing a candidate with its key; a
  foreground tell first removes the waiting focuses of its object from its
  thread. A run reaches its turn as one entry (`Logic::run_entry`).
- An MSAA menu start or end, popup start or end, or switch end takes the
  menu slot, replacing the one waiting; at its turn the slot moves once
  behind the runs waiting behind it, as NVDA's flush handles menus.
- An MSAA show removes a waiting hide of its object and a hide a waiting
  show; then it, like any other MSAA tell, replaces a waiting one of the
  same key (kind, window, ids and raising thread), searching back no
  further than a focus-kind entry.
- After each MSAA arrival NVDA's caps apply to the MSAA entries that
  arrived within one pump window (`PUMP_WINDOW_US`, 10 ms) of the newest,
  since NVDA flushes its limiter once a pump: the newest four run
  candidates (`FOCUS_CAP`), and the newest ten other entries of each
  raising thread (`THREAD_CAP`), foreground tells counted in both and
  dropped only when neither keeps them, and an entry of the readers' last
  focus's MSAA object kept but counted (`Logic::exempt_from_cap`), as
  NVDA always keeps the focused object's. The menu slot is outside both.

The foreground change ends the waiting entries it ends
(`Logic::ends_at_foreground`; "Focus, events and objects", "The
foreground change"). Tests: `src/event_queue/limiter_tests.rs`, one per
rule, and `tests/merge.rs`, which checks for every kind Core merges that
each short sequence between an older tell and a newer does what it does
without the older.

### The backend cache

Core's backend cache (`focus/backend.rs`) is the one keeper of whether a
window is read through UI Automation or MSAA (ADR 0004;
`docs/parity/backends.md`, "Dual-stack MSAA+UIA with per-window
arbitration"). A window is keyed by its handle and the thread that owns
it, so a reused handle is a new window. Its verdict is its class's when
the class, normalized, is on NVDA's good or bad list or the Explorer
shell rule names it, judged from each tell's classes and never kept;
otherwise the newest probe of the window begun no more than half a second
before the moment judged (verbatim-model's `PROBE_LIFE_US` and
`probe_stands`, NVDA's cache period, shared with the listener's probe
record), the
newest probe's start winning; otherwise none. A request that carried
"probe first" and ended with no answer for its probe counts as a probe
that timed out, begun when the request was sent, and a window's
destruction forgets its probes. An outpost's answer tells every probe
its request made that answered, of the request's window and of any
window its reads crossed, so the cache holds the newest verdict any
reader found (Dickson, 2026-11-27); an outpost tells no probe that timed
out.

A tell is judged at its turn by the verdict in force when its callback
received it: an MSAA tell, other than a caret one, of a window read
through UI Automation, and a UI Automation tell, other than a
notification or a toast, of a window read through MSAA, is dropped with
no read. A tell kept, and any request naming a window, carries the
verdict in force as the request is sent (`WindowVerdict`): `Uia`, `Msaa`,
or `ProbeFirst`, when the outpost probes the window before the read and
returns the probe with its answer, which the cache records. The outpost
takes the verdict given for every judgement of that window while it
answers the request, and keeps none ([verbatim-outpost](verbatim-outpost.md),
"The arbitrator"). Tests: the module's own, and the queue's unit tests of
the verdict a request carries and the probes requests answer.

### Batches

A function writes a batch with `batch::Builder` (`cx.batch(|| ())`) and
asks it as one request (`Sent::of`, the one `Request` a batch is); its
answer is parsed once against the batch sent into `batch::Answers`, each
step read by its handle (`Answers::get`, a `Got` of the step's kind), and
an answer that does not fit is undecodable. A handle (`Ref`) carries its
step's place, its result kind (`batch::kind`) and the batch's brand, the
type of the closure written where the batch is built, so a handle of
another call site or of a kind with no such point does not compile (the
`compile_fail` examples in `crates/verbatim-core/src/batch.rs`). The one
loop that builds batches numbers them (`cx.batch_numbered`): a handle of
another number is left off as an input, with every step naming it, and
read as failed, each logged as a defect (`QueueEffect::ForeignHandle`)
through the driver's ledger. A batch on Core's want list takes a brand
named for its one call site (`Builder::named`, a constant `Ref::named`
handle), and its answer is parsed against the batch held with the want.
A batch whose first step reads the focused window or the application's
focus alone takes what the shell read in front (`Answers::in_front`). A
batch whose answer is judged once as it is decoded, as a focus
candidate's is, is asked through `Asked::judged`, its function given the
judgement rather than the answers. Each text command builds its own batch
and judges each step's `Got` itself, a step not done through the one rule
of `text_request::got`, which reads it as refused, lost, gone or failed.

Every behaviour that crosses to a reader writes a batch from the list
and adds no request kind, reply, reader arm or decoder. Three are the
examples to copy:

- An object read: the MSAA run (`focus/msaa_run.rs`), each candidate's
  batch the hidden-dialog check, a `Test` of its focused state and
  identity, and the `FocusRead` unless the test matched, built in a loop
  with `cx.batch_numbered`.
- A caret key: Control+Backspace (`keys/caret_key.rs`, `starting_read`
  and `wait`), a starting batch of the caret, its line and the word
  around the character before the caret, then a wait batch of
  `WaitForCaret` with the starting texts and the caret, line and unit
  read after it.
- A text command: review's word move (`review_text.rs`, `TextRead::word`
  and `worded`), one batch a press of `ReadText` steps by the provider's
  word, judged into the word said.

Tests: `crates/verbatim-core/src/batch_tests.rs` and the queue's tests of
a batch asked, refused as undecodable, answered in front and numbered.

### A dialog read once it is shown

The behaviour is the focus design's
(`books/dev/src/explanation/design/focus.md`, "A dialog read once it is
shown"). A focus read that checks (an MSAA candidate at its turn, the
current focus, the menu check, a UI Automation focus's route or its read
afresh) begins its batch with the hidden-dialog check, a `Test` of its
window's top-level window through MSAA (`shown::check`): hidden, and a
dialog's role. The read after it runs unless it matched. `shown::judge`
takes the check's outcome: matched, the dialog's window; hidden with its
role unread, the read not there (the request list's design, section 6,
"Orders kept"). A hidden dialog goes to `shown::postpone_dialog`, the
one place a hidden answer is decided: it sets the one fact, `Versions::postponed`, a
`shown::Postponed` of the dialog's window and the front entry's speech
fence (`entries::front_fence`), keeping the older fence for the same
dialog, and returns `None`, ending the entry. `Versions::execute` and
`Versions::commit_own` end the fact, and so does the dialog's
destruction (`Logic::window_destroyed`); `Versions::respoken` leaves it.

A `Show` tell of a top-level window whose class is none of NVDA's show
classes, and whose own facts name that window in front, becomes a
current-focus entry of `Why::Shown` (`shown::of_show`), and the
foreground marker naming the postponed dialog joins one
(`shown::marker`); at its turn it reads only while the fact names its
window (`shown::shown_due`), and does not read the foreground record.
`retake::read_current` is the one place a read of the current focus is
judged, by its reason: `Startup` and `Returned` execute, `Retake`,
`Recovered` and `Shown` go through `retake()`; the shell's answer in
front goes to `plain::decide`, `Shown` as a re-take. A press other than
a caret key calls `shown::before_press` at its turn: with the dialog postponed and
named by the foreground record, which the keyboard hook's view is made from, it reads the current focus as it is through `read_current::<AsItIs>`,
whose decode refuses a hidden dialog, its speech stamped with the postponed fence, which
`Executed::deliver` takes for every caller. Tests: `focus::shown::tests`,
the design's sequences in Import's recorded order, the stand-in's marked;
`tests/replay_validity.rs` replays Import's flight dump.

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

(`supervisor/record.rs`, applied by the event queue's driver in
`event_queue/outposts.rs`, since a generation's start and end move the
requests that wait for it or are out to it.) Core's reducer owns one
record per application: launching, starting, ready or waiting out a
backoff, none being its absence. The supervisor's owner thread reports
the process facts and carries out the effects the record decides; it
judges no generation itself. The owner thread reports its process facts as
`QueueInput::Process` (`ProcessFact`): an outpost started, a launch
failed (with whether the application is gone), each pong, the
application's exit, the listener it is about to launch, the spares
waiting and Verbatim's own process; an
outpost's `Ready` and its end arrive as `ReaderReady` and `ReaderEnded`.
Core asks for an outpost (`QueueEffect::StartOutpost`) when a request
waits for an application with none, or the listener's foreground report
names one, and ends a generation (`QueueEffect::EndOutpost`) that passes
9 seconds with no pong, or whose pong reports `ABANDONED_WORKER_LIMIT`
workers or more abandoned to calls that never returned. Verbatim's own
process's outpost is launched at once (`ProcessFact::OwnProcess`). A
request waits through the launch and start and is sent at the
generation's `Ready`, in arrival order. A launch that fails, or a
generation that ends, before or after `Ready`, is replaced at once on a
first failure, a later one after its backoff
(`supervisor::policy::Backoff`), whose count starts again after an
outpost that stayed up long enough; a foreground report naming the
application resets the count, in any stage, and ends a backoff that
runs; a backoff that ends launches again only when a request waits or
the application holds attention. A ready generation's end answers its
requests out "reader ended", ends their entries and drops its held ids;
the committed focus's re-take follows as "The committed focus" (below)
says. The application's exit answers every request waiting or out "not
there", ends a starting generation, drops a ready one's held ids with no
re-take, and takes nothing again, with no backoff. An outpost is never ended
for being idle (Dickson, 2026-11-10 and 2026-11-16): it ends when its
application exits, when it fails or stops answering pings, or when
Verbatim exits. A frame of
an instance that is not its reader's current one, a tell of an ended
generation or of a listener other than the newest started, is dropped
(`QueueEffect::StaleFrame`). `SrState::readiness`, `outposts_starting`
and `ready_outposts` give the test harness's readiness, the shell's idle
conditions and the control plane's outpost list from the record. A process Core reads nothing from (`Logic::watched`) is
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
`record_input(arrival, effect_count, &state)` clones the state only
when a checkpoint is due; `dump_inputs()` gives the recorded inputs,
each with its arrival time, oldest first, ready to replay from the
checkpoint.
`RecordedInput::estimated_bytes()` is the entry's inline size plus the
heap it owns, found by `HeapCounter`, a serde serializer that formats
nothing and adds each string's length and each element's inline size. It
is kept over counting the entry's JSON because its cost is constant per
string, where formatting JSON scans every byte of a terminal document on
the reducer's step before its speech.
`replay(initial, arrivals)` re-runs a recorded sequence and returns the
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
(10), and keeps no fallbacks for dumps older code wrote: a dump either
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
  the hook ([verbatim-input](verbatim-input.md)). `reduce::write`, the
  one writer of a reader's focus, answers `Effect::DropExpiredSpeech`
  with a `FocusNow` built from the focus, its line, and the state's
  foreground node at every write, so the speech manager's view of the
  focus, which it judges waiting speech against at its turn, is fed by
  that writer alone. A spoken focus emits it first, then
  `Effect::StopSpeech` when the focus brings a new top-level window or
  enters a menu bar, menu, or menu item; speech for the new focus
  follows, so these never cut it off. A silent re-take
  (`reduce::take_silently`) emits it too, its `replaced` naming the
  nodes of the focus and line it replaced, the same objects under ended
  ids.
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
  utterance with no `FocusValidity`, which never expires; the focus's own
  utterance carries one and is dropped once the focus moves outside it
  (`FocusValidity::holds`).
- Focus-ancestry context (`docs/parity/focus-context.md`, "Focus-ancestry
  context" and "Top-level windows in the ancestry"). A `FocusChanged`
  carries the ancestor chain, outermost first, complete to just below the
  desktop, layout entries included, but for the MSAA reader's window
  objects, so it can be empty; each UI Automation ancestor carries
  whether UI Automation counts it a control and content
  (`FocusAncestor::uia_layout`, kept as `NodeDetails::uia_layout`) and
  whether it has a description whatever the theme (`described`), which
  the layout judgment reads. The
  reducer joins it to the previous focus line where they meet
  (`focus_line`), NVDA's focus difference level over the whole chain, and
  speaks the presentable containers (`is_presentable_container`, which
  alone judges presentation) below the meeting point. The meeting point
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
- The committed focus (`focus/versions.rs`; ADR 0003, "The two versions
  of the focus"): one record, `Versions::committed`, a `Committed` that is
  `Nothing` at startup, `Read(Box<Focus>)` for a reader's focus whose
  reader lives, `Ended(Box<Focus>)` once its outpost generation ended,
  its data kept for the silent re-take and its ids dead, or `Own`, the
  sleeping focus, which carries nothing. A `Focus` is what Core spoke of
  it (`context`, a `FocusContext`), what Core keeps of its focus answer
  (`kept`, a `Kept`: how it was read, its place, the window its read
  named, the foreground it entered under, its selection facts and whether
  it is a console host's terminal) and its line (`line`, `LineEntry`s
  outermost first, each an ancestor's snapshot with its runtime id and
  simple-child mark). The line is what the next focus is joined to and
  spoken against, and what the next focus request's known ancestors come
  from (`Versions::known_with_places`). `execute::ready` builds an
  answer's entries from its ancestors and, above the place the walk met,
  the committed focus's own line, the committed focus itself at place 0;
  an answer whose meeting point names no place Core sent ends unspoken.
  `reduce::execute_report` and `reduce::take_silently` write the record
  as their `Commit` says: `Commit::New` for an executed focus or a silent
  re-take, through `Versions::execute`, and `Commit::InPlace` for
  `execute::again`, a UI Automation menu opening on the committed focus
  when its reader lives, which carries that focus and writes it back
  with only what Core spoke of it replaced (`Versions::respoken`). Rules read it through
  `Versions::focus` (`Read` or `Ended`), `Versions::live` (`Read`),
  `Versions::caret_target` and `Versions::committed_node`; the readers'
  last focus (`LastFocus`) is a separate fact, empty at startup, with the
  speech fence of the entry that executed it (`LastFocus::since`).
  `Versions::execute` sets both versions and ends a postponed dialog;
  `Versions::commit_own` replaces only the committed focus and ends a
  postponed dialog; `respoken` replaces only the committed focus. A focus
  read tried at its turn and refused or answered not there, or whose reader
  ended, ends the last focus's fence (`Versions::tried_and_refused`), so
  no later focus is "with no key between" it. A foreground marker at the
  front empties the last focus when its window is a top-level window
  unrelated to the committed focus, and always while nothing or Core's own
  focus is committed, to which nothing is related
  (`Versions::foreground_marker`). The end of the outpost generation that
  minted the committed focus marks it `Ended` and always appends the
  re-take (`retake::reader_ended`), naming the foreground record's window
  when the focus's application holds attention and the window its read
  named otherwise; while it is `Ended`, no known ancestor that generation
  minted is sent. The listener's element token is held once, on the
  readers' last focus (`ReadBy<Option<ElementToken>>`, `HeldBy`); the
  committed focus keeps the identity alone (`Kept::read_by`, a
  `ReadBy`), and `LastFocus::names` compares identities. The listener's
  end marks nothing ended: it clears that token and re-takes the
  foreground window once,
  except for a plain focus, Core's own focus or none. The recovered read
  and the show's read are judged as the re-take is. A system caret tell
  has a target only while the last focus names the committed focus, its
  reader lives and its generation is its application's running one
  (`caret_tell::target`).
- The window in front. `SrState::hook_foreground()`
  (`verbatim_model::HookForeground`) is decided from the event queue's
  foreground record, the window the focus listener last reported
  (`QueueInput::Foreground`), the ghost window when one stands in for a
  hung window, and published by the shell after each input: keys may be held
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
- The foreground change (`QueueInput::Foreground`;
  [ADR 0001: Focus](../adr/0001-focus.md), "The foreground and the
  window title"): the focus listener's report that a window
  came to the foreground, the only one. The event queue takes it as its
  foreground (`Core::foreground`, `event_queue/inbox.rs`): it stops the
  front entry's request when the new foreground no longer holds the
  request's window, unless the request's own rule exempts it, and ends
  the waiting entries the change ends (`Logic::ends_at_foreground`), and
  the keyboard hook's view is read from it. It speaks nothing and cuts
  nothing. A change into one of Verbatim's own windows is like any other.
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
  (`WindowFacts::foreground_is_shell`) enters with no title. The return
  from Verbatim's menu says the title because the foreground marker for
  Verbatim's window empties the readers' last focus, so the return is
  judged left.
- The foreground record (`foreground.rs`): one `Foreground` with private
  fields, written only by `apply` with one `ForegroundFact` per writer
  (`Object`, `Entered`, `EnteredBy`) and read through accessors. It holds
  the foreground object, as NVDA's foreground object, and the entered
  window; the window in front is the event queue's alone.
- Repeats (`docs/parity/focus-events.md`, "Duplicate focus
  suppression"): a focus candidate is judged at its turn against the two
  versions of the focus (`focus/versions.rs`), the committed focus and
  the readers' last focus, by the one definition of a candidate's
  validity (below), whose duplicate tests (`focus/duplicate.rs`) need no
  read. A duplicate is valid, reads and says nothing, and ends a run. An
  MSAA focus a reader names with no event ids (`HeldIds::event` `None`,
  an object `accFocus` gave with no identity string naming its address) is
  kept as `ReadBy::Msaa { event: None, .. }`, which no event duplicates
  and no change or cap exemption names (Dickson, 2026-11-27).
- Background speech across a foreground change
  ([ADR 0001: Focus](../adr/0001-focus.md), "One input to Core, in
  arrival order"): a background event
  observed before the newest applied foreground change is dropped.
- Changes (`docs/parity/changes-and-selection.md`). Each focus keeps two
  things: the reading Core holds (`FocusContext::snapshot` and the line's
  snapshots), which review reads and a repeat updates, and what Core
  last spoke of it (`FocusContext::spoken`, a `SpokenRecord`): the
  focus's name, value, description and states, and each ancestor's
  states. A repeat never moves the record. A focus taken moves it, and
  so does a report that speaks the focus's reading (Verbatim+Tab's
  answer of the focus, reporting the current object while the navigator
  is on the focus, and the navigator's return to the focus), whatever the
  reason it is spoken. A navigation that lands on the focus's node
  fetches a separate object, so reporting it never moves the record. A
  live read of the focus that moves the record also writes the reading's
  value and the time its request was sent (`FocusContext::value_read_us`),
  and a UI Automation value change raised before then is stale and says
  nothing (`reduce::stale_value`). A name, value or description change on the focus moves it
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
  2026-11-17); a selection while focus
  stays on the container speaks each newly selected item once,
  deduplicated against the focus's own child. A selection in a list the
  focus controls is spoken as a focus, interrupting, and moves the navigator, only while
  its controller is the live focus.
- Progress bars (`progress.rs`; ADR 0003, "Progress bars";
  `docs/parity/changes-and-selection.md`, "Progress bars"): a bar's change
  is a tell judged at its turn, heard when its tell's own relation to the
  foreground window is equal or under, or with background progress bars
  reported. An MSAA value change, past acceptance and read through MSAA
  by its window's verdict, asks the listener for the object's role,
  value, states and place at once outside the queue (`msaa_changed`, run
  first in `changes::msaa`, before its early returns), as Core's facts'
  own request under a token, with Core's ordinary due time and endings.
  A bar has one read out at a time: `SrState::bar_reads` maps the bar's
  MSAA ids to `BarRead::Reading { token }`, or to `ReadingNewer { token,
  window }` once a change of the bar came while its read was out, which
  sends nothing; the read's ending, whichever it is, removes a `Reading`
  entry and turns a `ReadingNewer` one into a `Reading` of a new read, so
  the newest value is still heard. A flood of one bar's changes so keeps
  one read out, as NVDA's limiter keeps one waiting event per object and
  thread (`nvda/source/IAccessibleHandler/orderedWinEventLimiter.py` 79)
  and reads one at a time; one read per change had left thousands out,
  each step's work over the request table growing with them (Claude,
  2026-11-21). Its answer (`answered`, parsed once as a property
  set) plays for a progress bar not invisible or off screen whose value
  `Percent::parse` takes, at the centre of its rectangle, (0, 0) with
  none; every other ending says nothing, the hang check's "not
  responding" aside. A value change on the readers' last focus, its
  ids those its MSAA read carried and its role known and no progress bar,
  is not read (Claude, 2026-11-21), so typing costs no read. A UI
  Automation bar is `Entry::Progress` (`ProgressTell`, from a `UiaProgress`
  tell with `Payload::Progress`): accepted at arrival as a value change
  is, placed as a UI Automation change, merged with a waiting one of the
  same runtime id and process, asking nothing, and at its turn played
  from its tell when Core's backend cache does not drop it for UI
  Automation. A bar plays `Earcon::Progress` once the value moves a
  percent from the last one indicated at the same place on screen
  (`indicate_progress`, `SrState::progress_reported`, the last 32
  places).
- Core's facts' own requests (`wants.rs`): one want list and one token
  counter (`SrState::wants`) for every request Core's facts send outside
  the queue, the focused terminal's and a bar's, taken whole by
  `Logic::wanted` as the queue settles or by `Cx::send_wanted`;
  `Logic::facts_answered` routes an ending by its token to the bar
  (`bar_reads`) or the terminal.
- Announcements (`announcements/`; NVDA's behaviour is in
  `docs/parity/alerts.md` and `docs/parity/changes-and-selection.md`).
  Each kind is an event-queue entry, one `Announcement` with what its kind
  parsed from its tell, judged at its turn by its kind's async function;
  any outcome but a result drops it unspoken.
  - A UI Automation notification makes no request. It is spoken when it
    is from the committed focus's application, or, with no committed
    focus, the application in front; the shell's window-snap results are
    spoken from File Explorer whatever the focus, queued and never
    cutting, and take the desktop window. Any other notification whose
    check found no window, or whose text is empty, is dropped. One
    processed "most recent" or "important most recent" cuts speech, or
    during say-all is spoken at Next, after the line being read, until
    M6.3 gives speech an interrupt-and-resume priority; every other is
    queued.
  - A toast, a UI Automation window opened whose nearest window is a
    Windows Runtime core window (NVDA's toast test), is dropped when its
    element is not native or it repeats the toast spoken last within a
    second (ruled 2026-11-12), judged at its turn; else the listener reads
    its value, states, description and position, and it is announced by
    the name its tell's cached values carry, as NVDA's object for the
    event reads it and `Notification.event_alert` speaks the object,
    queued.
  - A UI Automation selection is dropped when not native, with no
    committed focus, or when refused, but for the Windows Forms list
    below. On the committed focus or one of its kept ancestors it is that
    node's state change, judged on the states its tell carries, with no
    request, and spoken only when its element was the committed focus as
    its tell joined the queue; an ancestor's states are compared with
    those last spoken of it. Otherwise the listener reads the item and the
    item's outpost completes it, under one due time, the completion
    reading the committed focus's controller-for relation live; the
    selection is dropped when the committed focus changed between the two
    reads. An item that is an object the focus controls, or inside one, is
    spoken as a focus. A list item without the keyboard focus whose parent
    is the committed focus gives that focus's value change, its value read
    live, when the focus is a combo box whose value is its selected item's,
    or has the `Value` pattern in a window whose class begins with
    `Windows.UI.Core`. A list item in a `ComboLBox` window of the focus's
    process, while the focus is a combo box, is taken whatever acceptance
    says once Core has asked the focus for its `ComboBoxList`, present only
    for the classic .NET Framework Windows Forms combo box, and gives the
    focus's value change. Anything else is a state change of the item,
    which says nothing.
  - An MSAA alert is moved once behind the entries waiting after it when a
    focus that counts as pending is among them, as NVDA runs its whole
    queue first, and its acceptance is checked. Its one batch asks
    whether it is one of the focus line's objects (`IsOneOf`), and else
    its read with its descendants (`Descendants` of the object and
    itself, at most 1,000).
    It is dropped when it is on the focus line, not of role alert, or has
    no name, description or child; else it is spoken, then each
    descendant NVDA speaks: one that is focusable, or, when the alert has
    no description, a list or list item. Queued. No application is known
    to reach this path.
  - A show asks a request only for `tooltips_class32` that passes
    acceptance, a snapshot judged by its role: a help balloon is spoken,
    from any application, an ordinary tooltip is not, as NVDA's tooltip
    reporting is off by default. NVDA's other five show classes wait for
    application support. Queued.
  - The virtual desktop's name, an MSAA name change on the desktop window,
    is read by the listener, the name alone, and kept until the next focus
    Core takes says it before its entered containers, or 250 ms later,
    when it is said alone (`desktop.rs`).

  A notification, a toast and a show are exempt from the foreground stop,
  and so are a selection, an alert and the desktop's name accepted other
  than by the foreground relation (`Announcement::exempt_from_foreground`).
  Neither notifications nor UI Automation selections merge a waiting
  entry: each is handled in arrival order (ADR 0003, "Flood limiters and
  the merge"). An MSAA selection, alert, show or desktop name replaces the
  older waiting entry of the same kind, ids and thread, searching back no
  further than a focus-kind item, and joins the back; a show also removes
  a waiting hide of its object, and a hide a waiting show.
- Outpost replacement (`docs/parity/focus-events.md`, "Recovery after an
  outpost is replaced"). Node ids carry the outpost generation. The end
  of a generation arrives as `ReaderEnded` (above, "The outpost
  generation record"), and a request out to it ends as "ended".

## Object navigation and review

(`docs/parity/review.md`.) `SrState` carries a navigator and a review
cursor that follow focus. The navigator is one of two places
(`state::NavigatorAt`): on the focus, where it holds no copy and its
object and window are the committed focus's record, read by
`Navigator::object`; or on an object a navigation or a controlled
selection reached, with the window its read found, `None` for a window
no process serves. Every focus spoken puts it on the focus; a silent
re-take keeps it where it is, putting it on the focus when there is
none and dropping a review position in text when the focus's node
changed. The navigator's commands are
event-queue entries (`navigator.rs`): each resolves the navigator object
at its turn and makes its reads as the queue's requests, naming the
object's window, so each holds the queue until its last answer.
Report-object announces, spells, then copies on repeated presses; parent,
sibling and first-child moves read the neighbour and announce it as the
navigator; activate says what was done, "No action" when every object
tried was there and none had one, and nothing when a call failed or
found an object gone; to-focus snaps back; report focus
is "Verbatim+Tab", below. With no navigator a command says "No navigator
object" (`navigator::no_navigator`). A review press is an event-queue
entry too (`review_text::begin`), so it reads live and runs in press
order with keys. An object with text is announced with its name, role
and states, then its text, read from the known caret or by the reads
`editing::navigator_text_start` asks: the selected text, judged by
`editing::navigator_selection_answered`, then the line, judged by
`editing::navigator_line_answered`.

### Verbatim+Tab

(`report_focus.rs`; NVDA's behaviour is in `docs/parity/review.md`,
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
focus. Over an ended focus or the sleeping focus it says nothing
([ADR 0001](../adr/0001-focus.md), "Report focus").

A press's answer ends it, or a follow-up read replaces its read (a line
read for the line, then a move, or an object's announcement and then its
text). A navigation that finds no neighbour or a dead object says the
direction's edge and the navigator stays (`docs/parity/review.md`, "Dead
objects"); `Unanswered` says nothing for the press. A dead object reviews
as empty flat text, one rule at the boundary (`review_text::answered` and
`review_text::run_next`); Report Current Object says nothing.

An object with no text interface is reviewed as flat text through the
same path as text, `execute` and `land`. Its flat text is NVDA's basic
text, `review_text::flat_text`: the name, value and description, each
that is not blank, joined by spaces. Each press reads the object live as
the queue's request (`review_text::Next::ReadObject`), the description
asked whatever the theme reports and kept on the navigator only where it
reports it, then runs on that text (`Next::Flat`): the review cursor is
`ReviewText::Flat(FlatPosition)`, a byte offset into the whole flat text
and a column, kept while the offset is a character boundary of the text
read and reset to the start otherwise. Core makes the line there as the
outpost would read it, its anchor the byte offset where the line starts
in the flat text, and answers every request the press makes from the
text itself (`review_text::flat_answer`): a line read at a point, the
start or the end, moving by line or not, and a range read for a copy. A
selection, a location or a selection point is answered unsupported, and
a page move unsupported by unit, since flat text has no screen place or
caret (Claude, 2026-11-21). Say-all from the review cursor in flat text
speaks the rest of the text at once. A `StartMarker` holds its places as
`MarkerEnds::Text`, positions at the outpost's anchors, or, set in flat
text, `MarkerEnds::Flat`, byte offsets into the whole flat text, so a
flat marker cannot reach `SrState::held_anchors` as anchors for the
outpost to keep (Claude, 2026-11-21). A press reads the marker back only
in the kind of text it was set in; in the other kind it is no start
marker.

## What the shell learns

Only the reducer speaks (`books/dev/src/explanation/architecture.md`, section 2), so what the
shell's threads learn comes in as inputs (`shell.rs`): `Input::Started`,
`ErrorLogged` play their sounds (the not-responding sound is the event
queue's, played for the first window of an application to join the
recovery set while the application holds attention, `hung_if_attended`
in `event_queue/driver.rs`);
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
plays its event. Each speaks under its input's trace; the time's speech
carries the gesture's key number, and the others none (below, "Key
numbers").

The scripts of the gestures that are not review commands
(`PressAction::Script`) ask the shell: `Script::ReportTime` emits
`Effect::ReadClock { repeat, key }`, answered by `ClockRead` with the
same key number; `Script::ShowMenu` emits `Effect::Show(GuiRequest::Menu)`
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
committed focus's node unchanged (`Versions::committed_node`), it asks the current
focus of the application whose window Windows named in front when the
check came due (`menus::InFront`, which the wake's `QueueInput::Due`
carries and `menus::due` gives every check due by that wake before the
wake's step joins it to the queue; no window in front names the
foreground record's window, and a window that could not be read ends the
check) with its focused state first (`retake::current_if_focused`: a
`Test` of the focus whose matcher holds of one without the focused
state, then its `FocusRead` unless it matched), the outpost reading an
MSAA focus's focused state over every ancestor up to the desktop; the answer is judged by the one definition's
menu-check rows, the focused state and the duplicate test for an MSAA
answer, as NVDA's `processFocusNVDAEvent` tests IAccessible objects
only, so a UI Automation answer of the focus Core holds is spoken again.
The check notes which end made it (`menus::CheckedEnd`), the first end's
for ends that share it.

A read of the focus Core makes itself (a batch whose first step reads
the focused window, `retake::read_current`) that the shell answered
unsent, its window hung or, for the check after an
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
in front. Watching and the sound are the entry's (`Cx::watch_if_attended`
for the ghost, `Cx::hung_if_attended` otherwise), each only while the
window's application holds attention, the sound only for the first
window of the application to join the recovery set.

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
activate; a node of an older generation is not there. A batch naming its
node is answered the same way, step by step under each step's gate: a
`Read` and a `Test` from its facts, no `Related` or `Descendants` node,
`IsOneOf` by its node, `Follow` applied, every text operation refused
"no text" and `Activate` refused "unsupported"; one that does not parse
is refused whole. When the ghost's
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
`Result<&CandidateFacts, M>`: with facts, `M` is uninhabited, so the
answer is `Settled`, which has no needed read.
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

Each read of a candidate is a batch built to its depth
(`msaa_run::TurnDepth`: `FullIfFocused`, `Full`, `UnlessSame`; a caret
key's confirmation reads the facts alone; and `retake::Depth` for a read
of the current focus), whose answer is judged once at its decode
(`Asked::judged`; `validity::Answered`: refused unread, `Unread`, or
found and read in full; a confirmation's facts alone; at `UnlessSame`,
`TurnAnswer::SameAsNamed` too). An answer that breaks its depth's rule,
a found candidate without its read or a confirmation with one, is
undecodable, logged as a malformed answer (`QueueEffect::Undecodable`),
and the entry ends.

The identity check (`docs/parity/focus-events.md`, "Duplicate focus
suppression"): the readers' last focus keeps the speech fence of the
entry that executed it (`LastFocus::since`), set by
`Versions::execute(focus, since)` from `Origin::fence`, which every path
names explicitly (a run's or menu start's executed candidate's own fence,
any other entry's own); a candidate tried and refused, not there or whose
reader ended clears it, by one function per answer: `validity::refused`
for a refusal by the rows (`Reason::tried`), and `validity::failed_read`
for every focus read's outcome other than a result, which also forgets
the caret and selection facts.
`validity::depth` gives an MSAA focus `TurnDepth::UnlessSame` when its
fence equals `since`, the last focus is held by a child id above 0, and
it is the committed focus; the `Identity` is made from the committed
snapshot then and kept nowhere. The candidate's `Test` then holds of an
object without the focused state or of the object at those ids reading
as that identity, in ADR 0004's order (`UnlessSame::matcher`); matched
with the focused state, it is `TurnAnswer::SameAsNamed`, which `validity::before_duplicate` makes a
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
without asking. A copy clears the start marker whatever it finds: no
marker can be set while its read is in flight, since a review press holds
the event queue until its last answer.

### The caret key on the event queue

(`books/dev/src/explanation/design/key-holding.md` gives the behaviour.)
A caret key's `Press` runs `keys/caret_key.rs` at its turn, on the node it
was bound to at its press (`CaretBinding`), even when the focus has moved
on. A held key asks its starting batch, then sends its stroke
(the pass) and asks its wait, then says its unit by its script; at the
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
confirmed by a batch of its facts alone (`msaa_run::facts`) naming the
candidate's window, its `spare` keeping the key's starting point held, its answer stored on
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

The key's places, each read from its request out or its stroke's status:
- Waiting, its stroke unsent. A stroke with a dropped mark ends the key
  as dropped. A key whose node is not held, a marked key, and a key with a
  focus that counts behind it pass with no read and end silent, except
  Backspace and Control+Backspace behind such a focus, which ask their
  starting read. A key on the terminal's path that is marked or behind a
  counting focus passes with no read, with no Backspace exception.
  Passed, the stroke goes at once when nothing unsent is ahead of it, and
  else with the stroke ahead. A foreground change that leaves its node's
  window ends it silent, its stroke sent in press order.
- Waiting, its stroke sent (released by its 150 ms timer, or never
  held). It says and asks nothing, stops say-all and keeps the typed
  word, Backspace dropping its last character at the end; a held key
  bound to the focused terminal is told to the model at its release
  time.
- Starting. A later key or the 150 ms release withdraws the read; a
  counting focus withdraws it but for Backspace's. At the answer the
  stroke goes and the watch is asked; on the terminal's path the read is
  taken and spoken before the stroke goes. A switch away from the user's
  desktop drops the stroke, so the read is unneeded and withdrawn as the
  queue settles.
- Confirming: a batch of the facts alone of an unconfirmed focus
  raised before the bound, which withdrew the watch. A later key
  withdraws it, and a marked sentence key then moves by sentence; a
  counting focus withdraws it, silent. Its facts, or an outpost ended or
  object gone stored as not there, are judged again; any other ending is
  silent.
- Watching. At the bound, selection keys, Enter and other keys say
  nothing, and a sentence key in text with a sentence unit moves by
  sentence, as it does after an effect its final read left unread. Enter
  at its effect keeps the word and types the carriage return.
- A terminal key's final read, which a later key or a counting focus
  withdraws; the terminal's reading rules speak it.
- Moving by sentence. A later key lets the move complete unspoken; a
  counting focus withdraws it.

A switch away from the user's desktop withdraws no watch or sentence
move: only a starting read becomes unneeded. A reset of the hook's line
(`QueueInput::StrokesReset`, below) acts as that switch does on the
stroke line; nothing sends it yet.

`keys/strokes.rs` is Core's stroke line (`SrState::strokes`): each held
stroke, from the press's arrival, and each deferred transition
(`QueueInput::Deferred`), in the hook's order. A held stroke goes when its
key passes, at its turn, by its 150 ms release from its press
(`Logic::due` and `Logic::tick`), or when a foreground change ends its
waiting entry (`Logic::ended_at_foreground`), with what is deferred behind
it up to the next unsent held stroke, as one `QueueEffect::SendStrokes`.
A held stroke whose key has passed while an unsent stroke is ahead stays
in the line, marked passed (`InLine::Held { passed: true }`), and goes
when the stroke ahead goes, in the same `SendInput`; a transition deferred
behind it joins behind it, and it no longer holds the line. A switch away
from the user's desktop (`StrokeLine::drop_unsent`) and a reset of the
hook's line (`QueueInput::StrokesReset`) drop what is unsent, leaving a
dropped mark for each held key that has not passed, and none for a
passed one, and withdraw the front's starting read; the key's pass
(`pass`), at its turn or once that read ends, reads the mark and forgets
it, and the key ends as dropped. At `QueueInput::Exit` Core names the
keys whose last transition in line is a release, a held stroke counting
as one (`QueueEffect::ReleaseAtExit`). The line is sent only from its
front, since releases fire in press order and keys reach the front in
press order; the shell keeps no copy. The line is also the one record of
the keys the focused terminal's model takes behind a held stroke: a held
key bound to the modelled terminal at its arrival carries its terminal key
(`keys::terminal::TerminalKey`), a key reported while the line holds joins
it as a marker that sends nothing (`InLine::Keys`), and each send
(`strokes::Sent`) names the terminal keys that went, which the model takes
in that order at the send's time (`keys::terminal::reached`), so a dropped
key never reaches it and a released key is never taken after the keys
behind it. A key is bound to the focused terminal at its arrival in any
state of its reading, a baseline out included, and taken at the send only
while that node is the modelled focus: a key whose terminal lost the focus,
or is still baselining, at the send is not taken. Limit: a terminal taken
again under a new node, as after its outpost restarts, and modelled before
the send, does not take a key bound to its old node; the echo of typed
text does not depend on it. The speech fence is not a state of
the line: `keys::entry::Arrivals` moves it on any press, and a
modifier's cut moves it and marks nothing.
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
property as a property change. Acceptance is judged once per tell: an
MSAA change and a UI Automation change store their verdict
(`MsaaChange::accepted`, `UiaChange::accepted`), judged as `Entry::of_tell`
makes the entry with the background setting as it stands at the tell's
arrival (`changes::msaa_of_tell`, `changes::uia_of_tell`); every other
tell is judged on its own facts by `acceptance::accepted`, with no
setting. `changes::passes` is the one test of acceptance and the backend
verdict for MSAA changes and announcements. An event accepted by
something other than the foreground relation
(`Accepted::exempt_from_foreground`), one accepted only because
background progress bars were reported among them, is exempt from the
foreground stop, so a foreground change neither stops its request out
nor ends it waiting (`entries::exempt_from_foreground`).
`AcceptanceFacts::in_foreground` is the one test of the foreground
relation, for acceptance and for the window facts a focus keeps.

### Caret tells and Core's text facts

(`text_facts.rs`, `caret_tell.rs`; NVDA's behaviour is in
`docs/parity/changes-and-selection.md`.) What Core keeps of the committed
focus (`Kept`) carries its `SelectionFacts`: the selection baseline, an
anchor its outpost generation holds of the selection Core last spoke
from, with that selection's text beside it, and the text-changed flag,
the focus's text having changed since the selection was last compared
(NVDA's `hasContentChangedSinceLastSelection`), so a new focus starts with
neither. A caret tell (`caret_tell.rs`), the system caret's
for the readers' last focus or a UI Automation text-selection change of
the committed focus's element, asks at its turn, with no focus pending,
one batch, a `Caret` (naming, for a UI Automation tell, the tell's
element for the outpost to adopt when the node's own has no text
pattern), the caret's line with its formatting, and a `CompareSelection`
of the caret's selection with the baseline, and takes the caret it reads
in arrival order. With no baseline named the answer only mints the next;
otherwise Core speaks the difference in general terms when the flag is
set or, in an edit control, when a value change of the focus waits in
the queue (taken out as counted), else in specific terms, then clears the
flag and takes the new baseline. An edit control's answer whose
selection offsets equal the last caret tell's (`SrState::edit_selection`)
changes nothing. An answer that the node has no text (the `Caret` refused
no text) leaves Core no caret and no selection for it, the baseline and the flag
cleared, nothing said (`text_facts::no_text`), as NVDA's caret event says
nothing for an object that is not editable text; any other failure says
nothing and keeps the baseline and the flag. An MSAA value change of the
committed focus's edit control, read through its window messages with
automatic selection detection, sets the flag at its turn when no caret
tell took it as counted, and says nothing; no other change sets it, and a
UI Automation value change of an element with a text pattern says
nothing. A `Caret` after a `Select` on the focused text, say-all's caret
move or review select, and a sentence move mint the next baseline.
Every focus request names the details its caret read fetches, the
line's formatting the settings report (`details::focus_details`), and a
focus answer's `FocusCaret` becomes Core's caret, its line, with its
formatting, or its selected text said with the focus: a caret read mints
the baseline and clears the flag, silently, a repeated focus's too, as
NVDA's `initAutoSelectDetection` does (`text_facts::focus_answered`;
Dickson, 2026-11-17); a focus answer with no caret,
or a failed focus read, clears the baseline and the flag. The end of the
focus's outpost generation drops the baseline and keeps the flag, and the
silent re-take, the recovery's included, takes the baseline its answer
minted and keeps the flag. A caret key's starting batch and the reads
after its wait compare no selection.

`SrState::held_anchors` and `held_nodes` name, besides the baseline,
every anchor and node a request of the event queue names
(`Batch::anchors`; a key's starting point while its wait or sentence
move is out), and the node a waiting caret key was bound to.

## The secure desktop and the lock state

(`input_desktop.rs`; NVDA's behaviour is in `docs/parity/focus-context.md`.)
The listener's desktop switch tell is handled at once in the inbox, never
queued (`entries::route`). Core's input desktop (`InputDesktop`) is the
user's, another, or a return pending. A switch from the user's desktop
or a pending return to an input desktop Verbatim cannot open cuts speech,
commits the sleeping focus (`Versions::commit_own`, which carries
nothing, so that the next focus is spoken from the top with its window's
title), leaves the focused terminal, sends `Follow` naming nothing to
every outpost followed, as Core's own requests outside the queue
(`event_queue::Input::Send`), drops the strokes Core holds, so the front's
starting read is withdrawn as the queue settles, and counts the switch in
the hook's view (`HookForeground::desktop_switches`), on which the hook
forgets its modifiers, but for an Alt still physically down, and its
line; a further such switch changes nothing. A switch whose desktop opens
arms a 200 ms timer (`InputDesktop::ReturnPending`), re-armed by each such
switch, and when it comes due (`Logic::tick`) the user's desktop is up
again and the read after the return joins the queue: the current focus of
the window in front, with none of the ancestors Core already knows and no
duplicate test (`retake::Why::Returned`), or nothing when no window is in
front. A switch away while the return is pending cancels it. Every tell
and answer is judged as usual meanwhile, so a focus on the user's
desktop replaces the sleeping focus; no re-take is made while it is
committed. The lock state is a fact beside the input desktop, recorded
from `QueueInput::Lock` and the startup query, unknown counting as
locked (`SrState::session_locked`), a Verbatim-only rule, security
mitigation B3 (`docs/design/security-model.md`); nothing judges it before
M8.7, which brings the rules for objects below the lock screen.

## Key numbers

(`books/dev/src/explanation/design/ownership.md`.) Every key arrives
numbered by the keyboard hook's thread (`verbatim_model::KeySeq`), as a
press's `key_seq`. A press runs at its turn as an event-queue entry
(`keys/entry.rs`): a caret key its own entry, a command with its own
function that function, and every other action its handling directly,
the four toggles, the scripts (`shell::script`), text typed, a control
character and a clearing key. Speech an entry says carries the entry's
fence (`keys::entry::stamp`). Two kinds of speech said outside an entry
carry a key's number too: the time, whose `Effect::ReadClock` and
`Input::ClockRead` carry the gesture's number; and terminal output handed
to speech by an input handled at once (an utterance's end or failure),
which carries the number the terminal's speech state took from the
speech entry that queued it (`TerminalSpeechState::key`), none for output
the quiet-period timer read. A lock key's announcement, and speech no key
caused, carry none.

### Caret keys

(`docs/nvda/editable-text-and-terminals.md`; `docs/parity/text.md`,
"TextInfo-equivalent layer".) A caret key is answered by its wait
batch: `WaitForCaret`, which compares the application's caret with the
key's starting point, judging the key's effect by `had_effect` on the
starting texts the batch carries, until its bound (`Waited`), then the
caret, its line and the key's unit read after it in the same poll.

Each focus object holds one caret (`FocusContext::caret`). Core takes
each report in arrival order, the order the event queue answers its
entries' requests in. A later key marked before an answer's read
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
  which the starting batch read (the caret moved back a character and
  expanded to the unit), spoken as a
  character when it is one and as text otherwise, as NVDA's Backspace
  scripts speak it; at a line's start the provider's unit is the line
  break before it, and at the document's start it is empty and nothing is
  said; over a selection, then the selection's change.
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
  a change in general terms (`editing::General`) speaks the whole new
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

Every text and node request names the details its read fetches, chosen
by `details.rs` from Core's settings (the details the active theme
reports, `Input::Reported`), the read's unit and its reason. The readers
keep no copy of the settings and fetch exactly what a request names, so a
detail whose setting is off is left unread and its call unmade.

The memory belongs to a focus record, the committed focus's context, told
apart by its node and the trace it was taken from. A caret key captures
its record and that record's memory at its press (`CaretBinding`,
`editing::focus_record`); when it speaks its unit it compares with its
copy, updates it, and writes it back only while the committed focus is
still that record (`editing::formatting_back`). A later focus taken, an
outpost replaced and the silent re-take each make a new record, which
starts empty, while a waiting key keeps its copy. So does the committed
focus executed again in place (`Commit::InPlace`): its memory is emptied
and its record changes, so a waiting key's write-back is dropped. The
focus's own caret line compares with the focus object's memory directly
(`editing::caret_line`).

### Typing echo

Text typed (`PressAction::Typed`, `editing::character_typed`) is echoed by the settings: a finished word first,
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
Backspace drops its last character at its end; a control character the
hook reports (`PressAction::Control`) (Control+C, Control+Enter; never
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
straight to the read that moves, so no press reads twice to move. A word
command reads the text's own word with no line read first, one batch a
press (`review_text.rs`, `TextRead::word`; `docs/parity/review.md`, "Review words by
the provider's word"), and a word move that crosses a line takes the
line it lands on from the same batch. Moving
to another line keeps a cell column in a terminal and a remembered
character column elsewhere, kept across a short line for every text. A
lost anchor starts again from the caret.

Select then copy (`docs/parity/review.md`, "Select then copy from the
review cursor"): Verbatim+F9 marks the start, Verbatim+Shift+F9 moves the
review cursor there, and Verbatim+F10 asks the outpost to select from the
earlier end through the character at the later (Core sends both ends with
the points just past their characters, a `Span`'s `through`, since only the
outpost can order them), one batch of the caret, the select, the caret
again and the two selections compared, and pressed twice reads and
copies the range
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

- Each read asks for 20 units (one `ReadText` of count 20), buffered
  in `SayAll::buffer`; the next 20 are read once fewer than 10
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

A caret key bound to a terminal takes the terminal's path ("The caret key
on the event queue"): its starting read is the terminal's full read,
which also mints the caret starting point, it is told to the model at its
pass, its watch polls with no event shortcut to its multiplied bound, it
says nothing itself, and the model takes its final read. Core drops a
system caret tell at its arrival and at its turn while the committed
focus is a console host's terminal (`Kept::console_terminal`), since the
console's caret event is its caret.

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
  (`Skipped::Uncounted`, after a lost read whose rows fit at more than one
  place, and for a lost read's "more than" count of none), `Phrase::SkippedMoreThanLines`
  (`Skipped::MoreThan`, lines lost past the terminal's history) or
  `Phrase::SkippedAboutLines` (`Skipped::About`, after a count read);
  `Skipped::plus` combines kinds, "more than" winning. A lost read sends a
  count of its kind ahead of its lines, of none or, after a count read, of
  the rows above it, each line marked `Counted`. The next group follows, until everything waiting has been
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
  the backlog is decided once the release read's speech has been taken
  and the last line heard. The pacing is one value,
  `TerminalSpeechState::pacing` (`Pacing`): `Idle` between bursts,
  `Speaking { left }` while a group is handed on, and `Deciding` once its
  last line is heard and a release is pending. Whether a release is
  pending is read from the reading alone (`reading::release_pending`), and
  is never copied. The group's end is judged in one place (`group_end`),
  at a closing mark and after each hand-on. A release ends at its speech
  entry's turn, when its read fails, at a cut, or when its reading is
  replaced (`committed`, `follow_ended`, `outpost_ended`), which runs the
  look waiting for it (`terminal::release_settled`) unless the commit's
  own speech cuts speech off. With nothing
  waiting, reading goes live again. When the outpost ends, the reading
  goes with its model and nothing more is asked of it; what Core had
  already read is still spoken, as NVDA speaks the lines its monitor
  read. A cut while holding or with a read out leaves that read
  silent: of reads begun before the cut
  (`TerminalSpeechState::cut_at_ms` against the read's start) only the
  answers to keys are spoken (`Mark::Silent`).
- Anything that cuts speech drops the output waiting and handed to
  speech, and names its kind (`terminal::Cut`): a key's cut
  (`QueueInput::SpeechCut`), the speech side's report of an utterance
  whose marks output handed to speech still waits for
  (`Input::UtteranceCancelled`), and a step whose effects stop speech or
  interrupt. Only the first two hold back the output of reads begun
  before them, by moving `cut_at_ms` to the input's time; a step's own
  cut moves nothing, so a read begun before it is spoken, as NVDA speaks
  a monitor read queued after its cancel (`docs/parity/terminals.md`,
  "Output", Dickson 2026-10-10). A key's cut drops what speech was
  handed, so the speech side's later report of it matches nothing; the
  report moves the fence only when it reaches Core before the key's own
  cut. `Input::UtteranceFailed`
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
read of a terminal (`terminal::reader::WindowReply`, the snapshot Core's
terminal reader reads back of its window batch's answer, with the anchor
held at the next read's start) and the answers to the requests they make;
`terminal/reading.rs` is the focused terminal's life and its reading.

- Public: `read_start(&WindowRead, &TerminalSnapshot, last)`, what a
  read Core asked as the `WindowRead` started from, parsed once from the
  request and the snapshot, `Kept` only when it named `last`, the start
  the model's memory placed; `read_of(&TerminalSnapshot, ReadStart)`, the
  rules' `Read` of a snapshot (its text without its lines' padding, and
  the screen, padded again exactly); `snapshot_of(&Read, waited, colours)`, the snapshot of a read
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
  policy's hold), `Releasing` (holding, with the release read `token`
  answered and its speech entry not yet at its turn) or `Stalled` (its
  window hung while a read was out). A speech entry names what it is the
  speech of (`Of`): a baseline's or an entry's read, or the output read
  by its token; only the entry of the read that
  `Releasing` names ends the release at its turn, so an owed read sent
  after the release keeps the look waiting for its own speech. A
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
  start anchor the next read names (the start of the last read whose
  screen showed text, or the first), the scope the rules owe (`Fresh`,
  `Count` or `HistoryBlock`, taken by the next full read sent and Core's
  alone: a read that is lost loses it harmlessly, since each answer is
  taken with the `WindowRead` it was asked as, `terminal_model::take`
  parsing its `ReadStart` once), when the
  newest read began, and `Given`, the read given before: `Focus`, none
  since the focus arrived, so the next is given whatever it shows, or
  `Last` with its hash, so an identical read that did not wait is not
  given. The caret, the screen's shape and whether the last read started
  at the document's start are the memory's, read through
  `TerminalModel::caret`, `width`, `height` and `at_document_start`, and
  the quiet-period timer's due time is the rules' (`TerminalReader::due`):
  the model keeps no copy of them. A model is made with the baseline while
  the terminal has the focus.
- Keys. Every key input while the focus is a modeled terminal is kept by
  its kind, a clearing key as `Forget` then `Other`, when it reaches the
  program: at its arrival, or, for a held key and the keys behind one, as
  the stroke line sends it ("The caret key on the event queue"). Whether a read takes
  the screen's colours is Core's decision from the newest key's window, as
  the rules judge it (`TerminalReader::key_window`, given the newest key
  kept), and the last read, sent in the read's
  request (`WindowAsk::colours`). Before a read
  taken at S (`read_at`) is given to the rules, every kept key at or
  before S is given in order; before the timer at T, those up to T; past
  256 kept keys the oldest is given at once. Leaving the terminal tells
  the rules `blur` and drops the keys; returning tells them `focus`. Each
  key is kept with its trace and number (`PendingKey`), which the rules
  are given as its `KeyTag`. A
  caret key in a terminal is Core's caret key entry with its watch, whose
  starting and final reads are full reads of the terminal.
- A read's speech key (`SpeechKey`) is what the rules say of the newest
  key given (`TerminalReader::read_key`): its speech and effects go out as
  `Effect::ForKey` for that key when its window holds the read's time, and
  every utterance of it, its answers to keys and the lines the flood policy
  or a release hands on at its entry's turn, carries the newer of the
  turn's fence and that key's number (`reading::emit_speech`), so that no
  cut by a key the read was given fences it off. A read with no speech
  key's trace, and the timer's speech, carry the input's trace.
- A read taken while a held key's stroke has not reached the program, or
  begun before the newest held stroke of a user's key was sent
  (`StrokeLine::held_reached_us`, written where the stroke line sends it,
  at its key's pass or its 150 ms release), speaks only its answers to
  keys: its output is that key's cut.
- Reads. A full read is the reader's window batch
  (`terminal::reader::window_batch`) of a `WindowAsk`: which read, by the
  scope the rules owe (from the start the model names with the screen's
  height, as a count read when the rules owe one, or a history block; the screen alone before a
  height is known), whether to take colours, whether it is a caret key's
  starting read, and whether it is an output read. The reading keeps the
  ask of a read it has out (`Reading::Out`, `Baselining`) and reads the
  answer back with it, at the times the reader's worker stamped on the
  answer (`Answer::began_us` and `ended_us`, which the event queue parses
  once into `Ran::Worker(Stamps)` for a reader's result and `Ran::NoWorker`
  for every other ending, Core's plain reader's results among them, and
  hands to `Request::decode` and `Logic::facts_answered`; a terminal read
  with no worker's run does not decode); a read
  that found the terminal's node or the start named gone ends as "not
  there". A
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
  memory are asked for with the reader's history batch (`history_batch` of
  `HistoryAsk::Above { start, above, height }`, or `Whole`), a
  screen more than needed, and placed from the start anchor Core named, which
  the outpost holds until the answer (`NodesHeld` lets it go); an anchor whose
  row left the memory ends the command silently, marks kept. The memory's rows carry anchors with
  the top bit set (`TERMINAL_ANCHORS`), a range no outpost mints from, with
  at most 64 places beyond those Core holds. "Report status bar"
  (`terminal::commands::status_bar`) reads the focused modelled
  terminal's footer (`review::footer`) after a fresh read, none when that
  read's screen shows no text (`terminal::commands::fresh` says so), or
  says `Message::NoStatusLineFound` outside one.
- Command marks. Enter on a caret line holding input
  (`TerminalReader::caret_line_input`) marks the line (`MarkLine`).
  Verbatim+Alt+Up and Down Arrow (`review_text::command_mark`, with
  `terminal::commands::command_mark`) try the marks beyond the review position, nearest
  first, by their line's hash in the memory and above it with the
  reader's search batch (`terminal::reader::search_batch`), whose matches
  are placed with `review::placed`;
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
Both assert what is heard and what Core asked for by its purpose alone
(`Asked`: a read, a count read, a first read again, a history read,
a search with its text, a caret key's watch), so that they hold whichever
way Core asks a terminal (Claude, 2026-11-26). The support's simulated
terminal answers each generic text step of a batch as the provider would
(`tests/terminal_support/steps.rs`), given the batches of Core's terminal
reader, which `terminal::reader::asked` tells apart, the command search
among them. The unit tests beside the rules state a read as the rules
take it and answer Core's batch with the outcomes that read back as it
(`terminal/reader_answers.rs`, each answer read back and compared before
it is given). `tests/terminal_answerers.rs` runs the batches of Core's
terminal reader (`terminal::reader`) through the support and compares what
the reader reads back with the simulated terminal's reference read, field
by field, after every chunk of every corpus stream and in the shapes the
terminal tests use, and pins the cases the reference read has none of: a
read that takes exactly the rows above its start, a provider that reports
no selection, a start no longer held and a start past the text's end; it
also compares the reader's command search with the rows of the simulated
terminal that start with the text sought. `tests/terminal_transcripts.rs` replays
every recording of the corpus through `reduce` in both terminals and
compares everything heard with the transcripts pinned from the reading
of 2026-11-26 in `tests/fixtures/terminal_transcripts/`; a transcript
that differs prints its section as it is now.
`tests/terminal_cost.rs` is a measurement, not a test
(`books/dev/src/reference/performance.md`, "A terminal's reads in Core").
