# verbatim-core

The pure reducer (architecture section 2), the flight recorder, and its
on-disk dump format.

Public API:

- `reduce(state, input)` — the frozen signature: pure, no I/O, no clocks;
  returns the next state and the effects to execute.
- `SrState` — `new()`; `focused()`, the live focus and its application;
  `attention()`, the application holding attention; and `held_nodes()`,
  every node id the state refers to grouped by the outpost that issued it.
  The last two are the views the shell derives and sends out: attention to
  the supervisor, and each outpost's held nodes to that outpost.
- `FlightRecorder<T>` — a bounded ring buffer; `ReducerRecorder` and
  `RecordedInput` specialize it for reducer inputs; `replay(initial,
  inputs)` re-runs a recorded sequence and returns the effects per step,
  proven deterministic by test. `RecordedInput` derives `Serialize` and
  `Deserialize` so it survives the trip to disk.
- `dump` — the flight-recorder dump format (milestone M2): a versioned
  JSON-lines file, a header line (`DumpHeader`: format version, the writing
  crate's version, and a caller-supplied timestamp string — this module
  never reads a clock itself) followed by one compact-JSON `RecordedInput`
  per line. `write_dump(writer, crate_version, timestamp, inputs)` writes
  one; `read_dump(reader)` reads one back as `DumpContents` (header, the
  inputs that parsed completely, and whether the file ended mid-line).
  `DumpReadError` distinguishes a missing header, a malformed header, an
  unsupported format version, and a malformed *complete* line — a
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

Implementation notes, `reduce`:

- A focus change speaks, at Interrupt priority and in NVDA's property
  order: newly entered container context first (see below), then the
  node's name, role, value, states in a fixed order (checked or its
  negation first, then mixed, pressed, selected, expanded, collapsed,
  has-popup, default, read-only, disabled, busy), then description,
  keyboard shortcut, position in set, and level — each detail simply
  absent when the backend reported nothing. Per decision D12 the name
  travels as a `Label` span and the value as a `Value` span, never
  anonymous text, and every utterance carries its source node's role and
  rectangle (`UtteranceSource`) for presentation themes.
- Focus-ancestry context (M3): a `FocusChanged` event carries the focused
  node's ancestor chain, outermost first, walked by the outpost before
  emitting. The reducer announces the presentable containers that were not
  in the previous focus's chain or the previous focus itself, before the
  control itself, so entering a dialog speaks the dialog and tabbing within
  it stays quiet about it. Node ids compare only within one outpost, and
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
  the focus announcement that follows interrupts anyway); `Window`, `Group`,
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
  still the system's foreground window. Any other event is attended when
  its window facts share the attention window's top-level window or root
  owner, are topmost, or report a `Windows.UI.Core` window under the input
  thread's active window; when either side has no window facts, the
  application decides. UIA notifications are filtered by application
  instead, as NVDA filters them, except the shell's window-snap results,
  which are spoken from anywhere. Only a foreground change moves
  attention, so a topmost popup menu taking focus leaves it where it was.
  Events from elsewhere are dropped, except background kinds, which are spoken
  queued and never move focus or the navigator. With no attention yet,
  everything is accepted.
- Windows follow NVDA (`docs/parity.md`, "Window announcement on switching
  applications"): a foreground change is a focus on the window, ignored
  when its top-level window handle equals the current focus's, which covers
  both a window that is already the focus and focus already inside it.
  Window handles are compared rather than node ids because node ids differ
  between outposts. A foreground change to a nameless window becomes the
  focus and moves attention without speaking. A window nameless when focus
  enters it is not announced later: a name change speaks only on the focused node, where the new name
  alone is spoken, queued. There is no ordering check of any kind:
  `observed_at_ms` is carried for the latency record only, and order comes
  from each outpost's queue. The negated-state rules match NVDA's: negated checked for
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
  them: report-object announces on the first press, spells its review text
  on the second, and copies name-and-value on the third; parent, sibling,
  and first-child moves emit a navigation `Fetch` whose completion moves the
  navigator and announces it; activate emits `Activate`; to-focus snaps the
  navigator back. Completions are matched by `SrState`'s latest-navigation
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
  the motivating case), from the attention application only. A
  focus event identical to the one already announced from the same
  application, back to back, is dropped — NVDA's already-the-focus early
  return, which removes the double-fire when the UIA callback and a
  foreground re-announcement both report one control; it never suppresses a
  genuine return to a window after visiting another, since that focuses a
  different control in between.
- Selection announcements (M3): a focus event on a selection container (a
  list, a tab control) also carries the container's selected child, which
  is spoken right after the container; a `SelectionChanged` event is
  spoken while focus stays on the container — each newly selected item
  once, deduplicated against the focus event's own selected child and
  against repeats — and stays silent from other applications, on
  non-container focus, or for combo boxes (whose picks already arrive as
  value changes). Selected-state wording matches NVDA: positive "selected"
  is never spoken on a node announcement, and a selectable node that is
  not selected says "not selected"; selection-state *changes* still say
  "selected" through the state-change diff. The negated-checked rule: a `CheckBox` or `RadioButton`
  carrying neither Checked nor Mixed announces "not checked". Focus-related
  states are never announced.
- A value change on the currently focused node speaks just the bare value,
  Interrupt — the slider-drag announcement. Value changes elsewhere are
  ignored in M1.
- A states change on the focused node is diffed against the stored
  snapshot: newly gained announceable states are spoken, and losing Checked
  on a check box or radio button announces the negation — the
  spacebar-toggle-off case, which has no newly gained state to catch it.
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
