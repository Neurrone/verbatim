# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format.

Public API:

- `reduce(&mut state, &input)` — deterministic, no I/O, no clocks;
  advances the state in place, without copying it, and returns the
  effects to execute.
- `SrState` — `new()`; `focused()`, the live focus and its application;
  `attention()`, the application holding attention; and `held_nodes()`,
  every node id the state refers to grouped by the outpost that issued it.
  The last two are the views the shell derives and sends out: attention to
  the supervisor, and each outpost's held nodes to that outpost.
- `FlightRecorder<T, S>` — a window of recent entries bounded both by
  count and by estimated bytes, kept with a checkpoint of type `S` taken
  just before its oldest entry, so the window always replays from its
  start. The window is a list of segments, each starting at a checkpoint:
  `record(entry, bytes, checkpoint)` appends to the newest segment, starts
  a new segment (calling `checkpoint` for the state right after this entry)
  once the newest holds half of either bound, and drops the oldest segment
  whole while the window is over either bound. So the window holds between
  half and all of each bound, a checkpoint is taken once per half window,
  and at most three are alive; an entry larger than the whole byte bound
  is not kept. `checkpoint()` and `entries()` read the window.
  `ReducerRecorder` is `FlightRecorder<RecordedInput, SrState>`:
  `with_default_bounds(initial)` uses the shell's bounds,
  `DEFAULT_MAX_ENTRIES` (1,024 inputs) and `DEFAULT_MAX_BYTES` (8 MiB,
  which ordinary inputs never reach, so text-bearing inputs are what it
  caps), and `record_input(input, effect_count, &state_after)` clones the
  state only when a checkpoint is due, which is cheap because the state
  shares everything that grows. `RecordedInput::estimated_bytes()` is the
  entry's inline size plus the length of its compact JSON, counted without
  building the JSON. `replay(initial, inputs)` re-runs a recorded sequence
  and returns the effects per step, proven deterministic by test, and a
  test proves a window that has dropped its start replays from its
  checkpoint to the effects the live session produced. `RecordedInput` and
  `SrState` derive `Serialize` and `Deserialize` so they survive the trip
  to disk.
- `dump` — the flight-recorder dump format (milestone M2): a versioned
  JSON-lines file, a header line (`DumpHeader`: format version, the writing
  crate's version, and a caller-supplied timestamp string — this module
  never reads a clock itself), then the `SrState` the inputs start from
  (format version 2), then one compact-JSON `RecordedInput` per line.
  `write_dump(writer, crate_version, timestamp, base, inputs)` writes one;
  `read_dump(reader)` reads one back as `DumpContents` (header, base state,
  the inputs that parsed completely, and whether the file ended mid-line).
  `DumpReadError` distinguishes a missing header, a malformed header, an
  unsupported format version, a base state line that is missing or cut
  off, and a malformed *complete* line — a
  malformed final line with no trailing newline is not an error: the
  writer always terminates a complete line with a newline, so an
  unterminated tail can only be a crash-time dump cut off mid-write, and
  `read_dump` reports it as `DumpContents::truncated` instead of failing,
  keeping every record parsed before the cut. `crates/verbatim-core/tests/
  replay_fixture.rs` commits a dump captured from a scripted focus-change,
  value-change, states-change session under `tests/fixtures/` and replays
  it on every test run, asserting the per-step effect counts match what
  was recorded and that a second replay is identical — the template every
  future live-dump regression follows; its `regenerate_fixture` test
  (`#[ignore]`d) is how the fixture was produced and how an intentional
  change to the scripted shapes regenerates it.
- `tests/alloc.rs` — Core's allocation invariant, a test binary with its
  own global allocator counting the bytes each thread requests. It asserts
  that a focus step, a navigation command, and its completion each
  allocate the same number of bytes against a state whose previous focus
  has 10 ancestors and one with 10,000, with the same input, and that
  cloning the state for a flight-recorder checkpoint allocates the same
  small amount (under 1 KB) for both. It holds because `reduce` changes the
  state in place and `SrState` holds the ancestor chain as a shared
  `Arc<[NodeSnapshot]>`; anything later added to the state that grows with
  the application must be held the same way.

Implementation notes, `reduce`:

- When speech is cut off (`docs/parity.md`, "When speech is cut off, and
  cancellation of expired focus speech"): focus, value, state, selection,
  object navigation, review, report-object, and activation speech is
  `Queued`, as NVDA queues it,
  rather than interrupting; only a selection in a list the focus controls
  and a notification whose processing hint asks for it interrupt. A key
  press cancels speech outside the reducer (the hook does it, see
  [verbatim-input](verbatim-input.md)). On every focus change the reducer
  first emits `Effect::DropExpiredSpeech` with a `FocusNow` built from the
  new focus, its ancestors, and `SrState`'s foreground node (the node of
  the window most recently reported as the foreground, which a foreground
  report updates), so the speech manager drops speech for a focus the user
  has left. It then emits `Effect::StopSpeech` when the focus brings a new
  foreground window, which is a foreground report or a focus whose
  top-level window differs from the previous focus's (NVDA's foreground
  event, run when the top of the focus ancestry changes), named or not,
  and when an entered ancestor is a menu bar, menu, or menu item. Speech
  for the new focus follows these effects, so they never cut it off.
- A focus change speaks, queued and in NVDA's property
  order: newly entered container context first (see below), then the
  node's name, role, value, states, then description, keyboard
  shortcut, position in set, and level — each detail simply absent when
  the backend reported nothing. Which states are spoken, which are
  spoken by their absence ("not checked"), and their order follow "Which
  states are spoken, and in what order" in `docs/nvda/speech.md`
  (`spoken_states`, `negated_states`, and `STATE_ORDER`, with a
  `StateReason` of focus, query, or change). The value is left out for a
  check box, radio button, link, menu item, or application, and a
  description equal to the name is dropped. The role is left out, as NVDA
  leaves it out, when the node has a name or a value and its role is one
  of the roles silent on focus (list item, menu item, tree item, pane,
  static text, unknown); this applies to focus changes, entered
  containers, selections in the focused list, toasts, and object
  navigation, while reporting the current object keeps the role (`Reason` and `speaks_role` in `reduce.rs`; the
  rule is under "When the role is spoken" in `docs/nvda/speech.md`). Per
  decision D12 the name
  travels as a `Label` span and the value as a `Value` span, never
  anonymous text, and every utterance carries its source node's role and
  rectangle (`UtteranceSource`) for presentation themes. Each entered
  container is its own utterance, carrying a `FocusValidity` with
  `had_focus` false, which never expires (`FocusValidity::holds`); the
  focus's own utterance (with a selection container's selected child)
  carries `had_focus` true, so it is dropped once the focus moves to
  something that is not it or below it.
- Focus-ancestry context (M3): a `FocusChanged` event carries the focused
  node's ancestor chain, outermost first, walked by the outpost before
  emitting. The reducer announces the presentable containers that were not
  in the previous focus's chain or the previous focus itself, before the
  control itself, so entering a dialog speaks the dialog and tabbing within
  it stays quiet about it. When the event says its ancestors are unknown
  (the outpost could not read them in time), no container is announced and
  the previous chain is kept for the next focus. Node ids compare only within one outpost, and
  one top-level window can hold elements of two processes (a Settings
  page's frame belongs to `ApplicationFrameHost.exe`), so when the new focus
  is in the same top-level window as the previous one, a previous node with
  the same role and name also counts as already entered. The
  presentable-container filter (`is_presentable_container`) is at NVDA parity
  with `_get_isPresentableFocusAncestor` over `_get_presentationType`: it is
  exclusion-based, not an allowlist. Item and editable-text roles (`TreeItem`,
  `ListItem`, `EditableText`) are never presented; the always-layout
  structural roles (`Unknown`, `Pane`) never; menu bars, menus, and menu
  items never (NVDA cancels speech and stays silent on entering them, and
  so does the reducer, through `Effect::StopSpeech`); `Window`, `Group`,
  and `PropertyPage` only when they carry a name or description, so a named
  window is announced on entry as NVDA does; `StaticText` only when it has
  real text; and every other role — dialogs, toolbars, an unnamed tree
  announcing as bare "tree view" — regardless of name, as NVDA treats them.
  A focus change into a different top-level window treats the whole chain as
  newly entered.
- Event acceptance (decision D14 as amended by the outpost redesign;
  `docs/parity.md`, "Event acceptance"): the state keeps an attention
  record, the application and window facts of the most recent foreground
  change, standing in for the system's foreground window. Every event is
  classified against it before anything else. A
  foreground change (a `FocusChanged` with `foreground` set) is always
  accepted, because its outpost has already checked that the window is
  still the system's foreground window. Any other focus is attended only
  when its window facts say the window was in the system's foreground
  window when its outpost read it, or is topmost, or is a
  `Windows.UI.Core` window under the input thread's active window, NVDA's
  test against the real foreground (D14 as amended on 2026-10-05); a focus
  with no window facts is judged by its application. Any other event is attended when
  its window facts share the attention window's top-level window or root
  owner, are topmost, report a `Windows.UI.Core` window under the input
  thread's active window, or say the window is in the system's foreground
  window; when either side has no window facts, the
  application decides. UIA notifications are filtered by the focus's
  application instead (the attended one before any focus), as NVDA
  filters them, except the shell's window-snap results, which are spoken
  from anywhere, always queued. A foreground change moves attention, and
  so does a focus in the system's foreground window that is unrelated to
  the attention window (Windows can raise no foreground event for a window
  given the foreground after its launch); a topmost popup menu taking focus
  without becoming the foreground leaves it where it was.
  Events from elsewhere are dropped, except background kinds, which are spoken
  queued and never move focus or the navigator. With no attention yet,
  everything is accepted.
- Windows follow NVDA (`docs/parity.md`, "Window announcement on switching
  applications"): a foreground change is a focus on the window, ignored
  when its top-level window handle equals the current focus's, which covers
  both a window that is already the focus and focus already inside it.
  Window handles are compared rather than node ids because node ids differ
  between outposts. A foreground change to a nameless window becomes the
  focus and moves attention without speaking, though it still cancels
  speech. A window nameless when focus
  enters it is not announced later: a name change speaks only on the focused node, where the new name
  alone is spoken, queued. Order within an outpost comes from its queue;
  across outposts, a focus observed before the focus already applied is
  dropped as stale (`observed_at_ms`, `docs/parity.md` "Stale focus
  events"), and `latest_focus_observed_at` lets the shell skip a fake
  focus after a menu closes when a later focus has been applied. The negated-state rules match NVDA's: negated checked for
  check boxes and radio buttons, and negated pressed ("not pressed") for a
  toggle button — a `Button` control that exposes the UIA Toggle pattern,
  which `verbatim-uia` reclassifies to `Role::ToggleButton` with the
  toggle-on state mapped to `Pressed` rather than `Checked`, exactly as
  NVDA does (this is what a Windows 11 Settings toggle announces as). A
  separate `Switch` role was considered and deliberately not added: the
  reference NVDA's UIA path has no such role and announces these as toggle
  buttons.
- Object navigation and the review cursor (M3): `SrState` carries a
  navigator object and a review cursor that follow focus by default (every
  focus change snaps them to the new focus). A `Command` input runs against
  them: report-object announces on the first press, spells its name and
  value on the second, and copies them on the third; parent, sibling,
  and first-child moves emit a navigation `Fetch` whose completion moves the
  navigator and announces it; activate emits `Activate`, whose outcome
  returns as `Input::ActivationCompleted` and is spoken as the action's
  name, "Activate" for an action without one, or "No action"; to-focus says "Move to focus" and snaps the navigator back; any
  command with no navigator says "No navigator object". An `Unanswered`
  completion (the application did not answer) leaves the navigator put and
  says nothing. The review commands' messages ("Top", "Bottom", "Left",
  "Right", "blank") and repeated presses follow "Reading commands built on
  review" in `docs/nvda/review-modes.md`. Completions are matched by `SrState`'s latest-navigation
  query id, not by navigator identity: an app-initiated focus event landing
  between the command and its completion still snaps the navigator (review
  follows focus) but never discards the user's in-flight navigation, while
  a rapid second move or an explicit to-focus does supersede it. A
  completion carries its query kind, so nothing else about the query is
  kept. A `NoNeighbor` completion is a genuine tree edge and speaks NVDA's
  edge message for its direction; a `Gone` completion means the navigator's
  node could not be reached, and re-seeds the navigator from the current
  focus, announcing it, so a dead node never presents as a command doing
  nothing. The review-cursor line, word, and character
  motions walk the navigator object's flat text (its value or name) in the
  pure `review` module — grapheme-cluster characters and word-boundary
  segmentation wait for M4's text model. All of this is pure and unit-tested
  in `verbatim-core`.
- Notification handling and focus-noise suppression (M3): a UIA
  `Notification` event speaks its display string when it carries one,
  interrupting for `MostRecent`/`ImportantMostRecent` processing and
  queuing otherwise (NVDA's `event_UIA_notification`; snap-layout hints are
  the motivating case), from the focus's application only. A
  focus event identical to the one already announced from the same
  application, back to back, is dropped — NVDA's already-the-focus early
  return, which removes the double-fire when the UIA callback and a
  foreground re-announcement both report one control; it never suppresses a
  genuine return to a window after visiting another, since that focuses a
  different control in between.
- A `ControlledSelection` (a search suggestion or result selected while
  the focus stays in the search box that controls the list) is spoken as
  a focus, interrupting, and moves the navigator to the item, but only
  while its controller is still the live focus; the focus does not move
  ("Selection in a list the focus controls" in `docs/nvda/events.md`).
- Selection announcements (M3): a focus event on a selection container (a
  list, a tab control) also carries the container's selected child, which
  is spoken right after the container; a `SelectionChanged` event is
  spoken while focus stays on the container — each newly selected item
  once, deduplicated against the focus event's own selected child and
  against repeats — and stays silent from other applications, on
  non-container focus, or for combo boxes (whose picks already arrive as
  value changes). Selected-state wording follows the state rules above:
  a focused list or tree item does not say "selected", a focusable one
  that is not selected says "not selected", and a change of selection
  on the focus says "selected".
- A value change on the currently focused node speaks just the bare value,
  queued — the slider-drag announcement — unless the value is
  unchanged, the node is an edit field or document (typing must not
  speak the whole field), or its role never speaks a value. Value
  changes elsewhere are ignored.
- A states change on the focused node is diffed against the stored
  snapshot: the gained states are spoken, and of the lost ones those
  spoken by their absence, so unchecking says "not checked" and leaving
  half checked without becoming checked says it too.
- Outpost replacement (`docs/parity.md`, "Recovery after an outpost is
  replaced"): node ids carry the outpost incarnation that issued them, so an
  id from a replaced outpost never names a node in its successor. On
  `Input::OutpostEnded` the focus keeps its copied data but is marked dead,
  and a navigator or pending navigation in that outpost is cleared;
  navigation, report object, activate, and to-focus then do nothing until
  focus is reported again. A focus reported while the focus is dead is
  compared with the kept copy (role, name, value, states, and the ancestors'
  names and roles): when they match, its new ids are taken silently;
  otherwise it is announced as usual.
- Every `Speak` carries the triggering input's trace ID, which is what
  makes end-to-end latency timelines possible.
