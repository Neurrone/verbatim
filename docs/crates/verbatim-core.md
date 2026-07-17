# verbatim-core

The pure reducer (architecture section 2), the flight recorder, and its
on-disk dump format.

Public API:

- `reduce(state, input)` — the frozen signature: pure, no I/O, no clocks;
  returns the next state and the effects to execute.
- `SrState` — `new()`, `focused()`, `last_seen_version(source)`,
  `pending_fetch_count()`.
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
  in the previous focus's chain, before the control itself, so entering a
  dialog speaks the dialog and tabbing within it stays quiet about it. The
  presentable-container filter (`is_presentable_container`) is at NVDA parity
  with `_get_isPresentableFocusAncestor` over `_get_presentationType`: it is
  exclusion-based, not an allowlist. Item and editable-text roles (`TreeItem`,
  `ListItem`, `EditableText`) are never presented; the always-layout
  structural roles (`Unknown`, `Pane`) never; top-level windows never (the
  foreground announcement owns those, a documented divergence — NVDA would
  present a named window); `Group` and `PropertyPage` only when they carry a
  name or description; `StaticText` only when it has real text; and every
  other role — dialogs, toolbars, an unnamed tree announcing as bare "tree
  view" — regardless of name, as NVDA treats them. A focus change from a
  different application treats the whole chain as newly entered.
- Focus is last-observation-wins: `Input::Event` carries the
  `observed_at_ms` the outpost stamped, and the focus context keeps it. A
  `FocusChanged` from the same application observed strictly earlier than the
  focus currently held does not move focus or the navigator — the
  earlier-observed one is not the real focus — but it is not always dropped. A
  window, or an ancestor of the current focus, is still *spoken* (the same
  utterance a fresh window focus produces, no entered-container replay), because
  a window announcement is foreground context the maintainer requires never
  lost: the app outpost's announce lane emits the window before the control it
  precedes when it can, but a window still nameless when the lane had to move on
  is read in the background and arrives late, after the control (see the
  announce lane below). Any other stale event — a stale *control* focus — is
  dropped entirely, spoken to nobody, since it is noise and a navigator hazard.
  A zero timestamp (a flight-recorder stream recorded before the field existed)
  can never be strictly earlier, so it always proceeds and replay stays
  deterministic; a different application is unaffected, its staleness being the
  shell's cross-app foreground gate. The negated-state rules match NVDA's: negated checked for
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
  `NoNeighbor` completion is a genuine tree edge and stays silent (the M11
  earcon's slot); a `Gone` completion means the navigator's node could not
  be re-acquired, and re-seeds the navigator from the current focus,
  announcing it, so a dead node never presents as a command doing nothing. The review-cursor line, word, and character
  motions walk the navigator object's flat text (its value or name) in the
  pure `review` module — grapheme-cluster characters and word-boundary
  segmentation wait for M4's text model. All of this is pure and unit-tested
  in `verbatim-core`.
- Notification handling and focus-noise suppression (M3): a UIA
  `Notification` event speaks its display string when it carries one,
  interrupting for `MostRecent`/`ImportantMostRecent` processing and
  queuing otherwise (NVDA's `event_UIA_notification`; snap-layout hints are
  the motivating case), with foreground gating already done in the shell so
  only the foreground application's notifications reach the reducer. A
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
- Staleness: an event whose snapshot version is lower than the last seen
  for its source is never trusted; the reducer instead emits a `Fetch` for
  the focused node. The completion is compared against what was last
  actually announced (tracked separately from the live snapshot, since
  silent updates move one but not the other) and speaks only on a real
  difference. A completion arriving after focus moved on is dropped.
- Every `Speak` carries the triggering input's trace ID, which is what
  makes end-to-end latency timelines possible.
