# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format.

Public API:

- `reduce(&mut state, &input)` — deterministic, no I/O, no clocks;
  advances the state in place, without copying it, and returns the
  effects to execute.
- `focus_rules` — the focus rules the reducer shares with the focus
  listener and the outposts, each defined once (decision D14, "Where the
  rules run"), written from `docs/parity`; the reducer applies them after
  an event is read and the listener and the outposts where a judgment
  must come before an element is read, against the focus Core holds. The
  rules ported from NVDA that only the listener and the outposts make, the
  acceptance test and NVDA's foreground tests, are defined once in
  `verbatim-outpost` instead, as ported code stays in the Windows-specific
  crates:
  - `ForegroundRelation`: how a window stands to the foreground window
    (in it, topmost, or a `Windows.UI.Core` window under the active
    window), which an outpost reports in an event's window facts, read by
    its acceptance test's own rule, and which the reducer judges a focus
    by, `of_facts(facts).related()`;
  - repeats: `RepeatEvidence` (Core has judged every focus the judge sent,
    the event names the element of the focus Core holds, and that element
    still has the focus) and `repeats_focus(evidence)`, which Core, the
    listener and the outposts gather each by their own means;
  - changes and selections: `ChangeKind`, `ChangeTarget`,
    `change_target` and `change_candidates` (a name, description, or value
    change spoken on the focus, a state change on the focus or an
    ancestor whose state changes are spoken), and `SelectionTarget`,
    `selection_target` and `is_selection_container(role)` (a selection of
    the focus or an ancestor is a change of its states; any other is an
    item spoken only inside a focused list or tab control of the same
    outpost);
  - entered containers: `already_entered(previous, same_window, node)`,
    whether the focus was already inside a container of a new focus, which
    the reducer speaks newly entered and an outpost gathers a newly
    entered dialog's own text by, with `EnteredKey`;
  - `HeldFocus` and `HeldNode`: the focus Core holds as it sends it, the
    trace of the event it came from, its observation time, the focus and
    its ancestors (id, backend, role, name, states), its top-level window,
    and whether it is alive; `for_outpost(outpost)` keeps an outpost's own
    nodes, their ids unstamped, or gives `None` for another outpost's
    focus.
- `is_selection_container(role)` — `focus_rules`' own, re-exported: whether
  a focused role is a list or a tab control, whose interior selections
  the reducer speaks.
- `SNAP_RESULTS_ACTIVITY` — the activity id of the shell's window-snap
  results notification, the one UIA notification spoken from an
  application without the focus; the supervisor starts an outpost for no
  other notification.
- `SrState` — `new()`; `focused()`, the live focus and its application;
  `focus_report()`, the focus's snapshot, its ancestors, and the navigator
  object, which the control plane's `DumpFocus` answers with for a test's
  artifacts; `focuses_queued()`, how many focuses the reducer has taken as
  the focus, which the host compares across an input to tell whether its
  focus changed; `held_focus()`, the focus Core holds as the host tells it
  to the focus listener and the outposts concerned after every focus event
  the reducer judges and every change of its focus
  (`SupervisorToOutpost::CoreFocus`), with the trace and observation time
  of the event it was taken from, which the state keeps with the focus; `attention()`, the application holding
  attention; `held_nodes()`,
  every node id the state refers to grouped by the outpost that issued it;
  and `held_anchors()` (milestone M4), every text anchor the state refers
  to, grouped the same way: the caret's line and selection, the review
  cursor's line or point, the start marker, and say-all's positions.
  These are the views the shell derives and sends out: attention to
  the supervisor, and each outpost's held nodes and anchors to that
  outpost, which keeps them alive (`docs/crates/verbatim-model.md`, "The
  text protocol"). `fetches()` (milestone M4) is another: the details the
  active theme wants fetched, as the last `Input::Fetches` set them
  (everything until then), which the shell gives every outpost so that a
  detail whose indication is off, such as descriptions, is never fetched
  (`docs/design/phase6-design.md`, "Themes: one model for verbosity, speech, and
  sounds"). It is the one place the reducer consults the theme; how
  everything else is presented is the speech pipeline's. `settings()`
  returns the reader settings as the reducer has them, toggle keys
  included, which the shell merges the settings dialog's Terminal page
  changes into.
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
  (format version 3), then one compact-JSON `RecordedInput` per line.
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
  it on every test run, asserting that each step's effects are exactly
  the ones the test states, that their counts match what the dump
  recorded, and that a second replay is identical — the template every
  future live-dump regression follows; its `regenerate_fixture` test
  (`#[ignore]`d) is how the fixture was produced and how an intentional
  change to the scripted shapes regenerates it.
- `tests/alloc.rs` — Core's allocation invariant, a test binary with its
  own global allocator counting the bytes each thread requests. It asserts
  that a focus step, a navigation command, and its completion each
  allocate the same number of bytes against a state whose previous focus
  has 10 ancestors and one with 10,000, with the same input, and that
  cloning the state for a flight-recorder checkpoint allocates the same
  small amount (under 1 KB) for both, and that the theme's fetches are
  taken in without allocating at all. It holds because `reduce` changes the
  state in place and `SrState` holds the ancestor chain as a shared
  `Arc<[NodeSnapshot]>`; anything later added to the state that grows with
  the application must be held the same way.

Implementation notes, `reduce`:

- When speech is cut off (`docs/parity/focus-events.md`, "When speech is cut off, and
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
  the backend reported nothing. A tree or list item's level goes first
  instead when it differs from `SrState`'s `last_tree_level`, the last level
  put first, which it then becomes, for every announcement of a node, not
  only a focus ("Where the level goes" in `docs/nvda/speech.md`). Which states are spoken, which are
  spoken by their absence ("not checked"), and their order follow "Which
  states are spoken, and in what order" in `docs/nvda/speech.md`
  (`spoken_states`, `negated_states`, and `STATE_ORDER`, with a
  `StateReason` of focus, query, or change). The value is left out for a
  check box, radio button, link, menu item, or application, and a
  description equal to the name is dropped. An edit field, document, or
  terminal leaves its value out too and says its text at the caret instead
  ("What an object with text says" in `docs/nvda/speech.md`): the focus's
  message carries what the outpost's caret read found (`FocusChanged`'s
  `text`), and the same reduction speaks the caret's line ("blank" when
  empty) as a second queued utterance with the focus's validity, or, when
  text is selected, "selected" with the selected text the outpost read
  with the caret, or its count at 512 characters or more
  (`editing::focus_text`). `NoText` speaks the value; `NotRead` speaks
  neither. Nothing is left waiting, so nothing from any source can come
  between the focus and its line. A protected (password) field's text and
  value are spoken as any field's, masked as the control gives them. Object
  navigation, to-focus, and the first press of report current object
  announce such an object the same way, with or without the focus
  (`announce_navigator` in `reduce.rs`, `navigator_text` in `editing.rs`):
  the name, role, and states without the value, then its text as a second
  queued utterance. The focus's known caret is read at once; otherwise the
  outpost is asked for the selected text (`ReadRange` from
  `SelectionStart` to `SelectionEnd`) and, when nothing is selected, for
  the line at the caret (`Read` at `Caret`, which a control with no caret
  answers from its first line). The requests are a `PendingText` with the
  `NavigatorSelection` and `NavigatorLine` follow-ups, so a newer text
  request supersedes them, and an answer for an object the navigator has
  left says nothing; `NoText` or `Unsupported` speaks the value. The role is left out, as NVDA
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
  the previous chain is kept for the next focus. Two UI Automation nodes
  from one outpost are the same container only when their ids are equal,
  as NVDA compares UI Automation elements by identity, so File Explorer's
  new items view list after a folder is opened is announced again. Node
  ids compare only within one outpost, and one top-level window can hold
  elements of two processes (a Settings page's frame belongs to
  `ApplicationFrameHost.exe`), and an MSAA object reached by another path
  can have another id, so for any other pair, when the new focus is in
  the same top-level window as the previous one, a previous node with the
  same role and name also counts as already entered. The
  presentable-container filter (`is_presentable_container`) is at NVDA parity
  with `_get_isPresentableFocusAncestor` over `_get_presentationType`: it is
  exclusion-based, not an allowlist. Item and editable-text roles (`TreeItem`,
  `ListItem`, `EditableText`) are never presented; the always-layout
  structural roles (`Unknown`, `Pane`) never, except a pane marked
  `NodeDetails::top_level_client`, the MSAA client area of a top-level
  window with a system menu, which stands for its window and is presented
  whatever its name, spoken by its title alone, as NVDA's
  `isPresentableFocusAncestor` presents it; menu bars, menus, and menu
  items never (NVDA cancels speech and stays silent on entering them, and
  so does the reducer, through `Effect::StopSpeech`); `Window`, `Group`,
  and `PropertyPage` only when they carry a name or description, so a named
  window is announced on entry as NVDA does; `StaticText` only when it has
  real text; and every other role — dialogs, toolbars, an unnamed tree
  announcing as bare "tree view" — regardless of name, as NVDA treats them.
  A focus change into a different top-level window treats the whole chain as
  newly entered: speech is cut off, then the entered presentable
  ancestors are spoken outermost first, the top-level window among them,
  then the focus, whether or not a foreground report came for the window.
  This is how a window is announced when its foreground event is dropped,
  as the outposts drop one whose window is not yet in front and every one
  for a window read through UI Automation (`docs/design/focus-pipeline.md`,
  section 16.4); there is no separate rule for an unannounced foreground
  window.
- `SrState::hook_foreground()`, the keyboard hook's view of the window
  in front (`verbatim_model::HookForeground`), decided from the window the
  focus listener last reported (`Input::ForegroundWindow`) and published
  by the shell after each input, as `holds_caret_keys()` is: keys may be
  held for the window only when its process's integrity is known and no
  higher than Verbatim's, and a console host window's keys are translated
  with the layout of the thread its event named (architecture section 5).
- Rules for particular applications' windows (`applications.rs`, decision
  D6: they live in Core until the extension host runs application
  modules, never in the listener or an outpost). A focus or foreground
  change on one of the shell's staging windows (`ForegroundStaging`,
  `LauncherTipWnd`, `ApplicationManager_DesktopShellWindow`, matched by
  the window class its window facts carry) is taken like any other
  focus, so the focus, the navigator and the foreground move to it, and
  speech is cut when the foreground window changed, as for any focus, but
  nothing is said for it
  (`docs/parity/windows-and-processes.md`, "The shell's staging
  windows"). A UI Automation focus, a menu opening's included but never a
  foreground change, on the task switcher's frame is refused whole before
  anything else judges it, moving nothing and cutting no speech: a node
  read through UI Automation whose own class (`NodeDetails::class_name`)
  and role are `MultitaskingViewFrame` and window, or
  `Windows.UI.Input.InputSite.WindowClass` and pane, in the application
  its window facts name `explorer` (`docs/parity/windows-and-processes.md`, "The
  Alt+Tab switcher's pane"). A UI Automation focus on the task
  switcher's item reached with Alt held is taken with no containers,
  known to be none, so none is spoken or kept: a list item in
  `explorer` whose window facts say Alt was held and whose parent's
  automation id (`NodeDetails::parent_automation_id`) is
  `SwitchItemListControl`, or whose window's class is
  `MultitaskingViewFrame` (`applications::drops_ancestry`, applied where
  the reducer takes a focus's ancestors, its duplicate's newer reading
  included). `tests/reduce.rs` checks all three.
- Event acceptance (decision D14 as amended by the outpost redesign;
  `docs/parity/focus-events.md`, "Event acceptance"): the state keeps an attention
  record, the application and window facts of the most recent foreground
  change, standing in for the system's foreground window. Every event is
  classified against it before anything else. A
  foreground change (a `FocusChanged` with `foreground` set) is always
  accepted, because its outpost has already checked that the window is
  still the system's foreground window. Any other focus is attended only
  when its window facts say the window was related to the system's
  foreground window when its outpost read it, by the acceptance test's own
  relation (`focus_rules::ForegroundRelation::related`: in the foreground
  window, topmost, or a `Windows.UI.Core` window under the input thread's
  active window), NVDA's test against the real foreground (D14 as amended
  on 2026-10-05); a focus with no window facts is judged by its
  application. Any other event is attended when
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
- Windows follow NVDA (`docs/parity/focus-context.md`, "Window announcement on switching
  applications"): a foreground change is a focus on the window, ignored
  when its top-level window handle equals the current focus's, which covers
  both a window that is already the focus and focus already inside it.
  Window handles are compared rather than node ids because node ids differ
  between outposts. A foreground change to a nameless window becomes the
  focus and moves attention without speaking, though it still cancels
  speech. A window nameless when focus
  enters it is not announced later: a description change speaks the new
  description alone on the focused node, and a state change speaks on the
  focus or one of its ancestors, diffed against the ancestor's states as
  the focus was reported with them, as NVDA's base handlers do (a
  selection of an ancestor is such a state change); a name change speaks only on the focused node, where the new name
  alone is spoken, queued. Order within an outpost comes from its queue;
  across outposts, a focus observed before the focus already applied is
  dropped as stale (`observed_at_ms`, and `observed_at_us` when both
  focuses carry it, so two focuses stamped in one millisecond are ordered
  too; `docs/parity/focus-events.md` "Stale focus events"), and `latest_focus_observed_at` lets the shell skip a fake
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
  command with no navigator says "No navigator object". Report focus
  (`report_focus`) runs against the focus instead, leaving the navigator
  where it is: the first press announces the focus as the first press of
  report-object announces the navigator, its text read the same way (a
  text answer is used while the object asked about is the navigator or the
  focus); the second spells its name alone, "blank" with none; the third and
  later spell the name with character descriptions; with no live focus it
  says "No focus". An `Unanswered`
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
  pure `review` module, by the same units as text with a text pattern:
  characters are grapheme clusters and words come from `text::words`
  (Unicode's word rules with dictionaries), so a Hindi vowel sign stays
  with its letter and a Thai name has words; a space read as a character
  is "space", and a one-character word is spoken by its name. Flat text
  carries no language, so the word rules are chosen from the text. All of
  this is pure and unit-tested
  in `verbatim-core`. Since M4 this flat walk serves only objects with no
  text interface (see "Text" below); its spelling goes by grapheme
  clusters, a punctuation character is spoken by its name, the current
  character pressed twice gives its description, and the current line or
  word pressed three times is spelled with descriptions.
- Notification handling and focus-noise suppression (M3): a UIA
  `Notification` event speaks its display string when it carries one,
  interrupting for `MostRecent`/`ImportantMostRecent` processing and
  queuing otherwise (NVDA's `event_UIA_notification`; snap-layout hints are
  the motivating case), from the focus's application only. The repeat
  rule (`focus_rules::repeats_focus`; `docs/design/focus-pipeline.md`,
  section 4.2): a focus on the same node as the live focus Core holds is
  dropped before anything else judges it, so it
  neither moves attention nor counts as the newest focus for the stale
  rule; its newer reading (states, name, ancestors read in time) is kept
  unspoken. The node id is the evidence of "same element, still focused",
  since an outpost reuses a node for a UIA focus only after reading live
  that its element still has the keyboard focus, and reports an MSAA focus
  only once the focused state is read live; a UIA focus whose element the
  outpost could not find keeps its runtime id's node without that read.
  The rule never suppresses a genuine return to a control after visiting
  another application, since that application's focus comes between.
- Background speech across a foreground change
  (`docs/design/focus-pipeline.md`, section 7): Core keeps the observation
  time of the newest foreground change it applied (a focus that brought a
  new foreground window and cut speech off), and an event accepted as
  background (a toast, the snap results) observed before it is dropped, as
  NVDA's foreground change cancels speech queued before it. Background
  events observed after it, events with no observation time, and attended
  events are unaffected.
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
- A progress bar's value change (`ProgressChanged`), focused or not,
  plays `Earcon::Progress` with the percentage once it moves by a percent
  from the last one indicated for a progress bar at the same place on the
  screen (`SrState::progress_reported`, the last 32 places), and its value
  is not spoken; an off-screen one, or a value that is no number, is an
  ordinary value change ("How a progress bar reports its value" in
  `docs/nvda/object-model.md`).
- A value change on the currently focused node speaks just the bare value,
  queued — the slider-drag announcement — unless the value is
  unchanged, the node is an edit field or document (typing must not
  speak the whole field), or its role never speaks a value. Value
  changes elsewhere are ignored.
- A states change on the focused node is diffed against the stored
  snapshot: the gained states are spoken, and of the lost ones those
  spoken by their absence, so unchecking says "not checked" and leaving
  half checked without becoming checked says it too. A focus that
  reported itself focused when it became the focus
  (`FocusContext::reported_focused`) and whose new state set no longer
  includes focused has lost the focus before the next focus event
  arrived: its states are kept but nothing is spoken (`docs/parity/changes-and-selection.md`,
  "State changes after the focus has left"). When the change makes the
  focus expanded and the event carries a `child_count`, which the outpost
  reads only for a Win32 tree view item, "52 items" (`Phrase::Items`)
  follows as an utterance of its own (`docs/nvda/speech.md`, "How many
  items an expanded tree view item holds").
- Outpost replacement (`docs/parity/focus-events.md`, "Recovery after an outpost is
  replaced"): node ids carry the outpost incarnation that issued them, so an
  id from a replaced outpost never names a node in its successor. On
  `Input::OutpostEnded` the focus keeps its copied data but is marked dead,
  and a navigator or pending navigation in that outpost is cleared;
  navigation, report object, activate, and to-focus then do nothing until
  focus is reported again. A focus reported while the focus is dead is
  compared with the kept copy (role, name, value, states, and the ancestors'
  names and roles, which a focus reported with its ancestors unknown
  leaves unchanged, since the focus then keeps its previous chain): when
  they match, its new ids are taken silently; otherwise it is announced
  as usual.
- Every `Speak` carries the triggering input's trace ID, which is what
  makes end-to-end latency timelines possible.

## Text (milestone M4)

The reducer's side of M4 items 1 and 3 to 6 (`docs/design/phase6-design.md`, "The
autonomous run"), over the text protocol in `verbatim-model`. Pure helpers
over received text are in `text.rs` (a line without its break, grapheme
clusters, columns in characters or cells, words by the text's language
through `verbatim-text`, and the spoken segments for characters, words,
lines, and spelling); caret keys and typing echo in `editing.rs`; the
review cursor over text in `review_text.rs`; say-all in `say_all.rs`.

What the state keeps, within the bounds of "Core's state" in
`docs/design/phase6-design.md`: the focus's caret (`CaretContext`: its line, with the
caret at the line's offset, and the selection), the review cursor's
position (`ReviewText`: unknown, flat, a line and a position on it, at
the caret it follows, or a point whose line has not been read), the caret key whose evidence is watched for,
the one review or text request in flight, the start marker, a say-all's
queue of spoken pieces, the word typed so far (at most 256 bytes), and
the focused terminal's output still to be spoken. Text is held only a line at a
time, as a shared chunk of at most 64 KB, so the state's clone, which the
flight recorder takes at each checkpoint, copies a pointer and not the
text; `tests/alloc.rs` asserts that caret and review steps allocate the
same over small and large states, and that a checkpoint with a 64 KB line
in the state allocates under 1 KB.

Which objects have text. A focus or navigator object whose role is an
edit field, a document, or a terminal may have text, and so does any node
the outpost sent a caret report for. For those, caret keys watch for
evidence and the review cursor reads lines through the protocol; the
first review command reads the line at the caret, and a `NoText` answer
makes the object flat text, reviewed by its value or name as in M3. Any
other object is flat text without asking.

Held caret keys (`docs/design/key-holding.md`). An `Input::HeldKey`, a
caret key the keyboard hook held back because `SrState::holds_caret_keys`
was true, asks the focus's outpost to read the caret
(`TextOp::ReadCaret`) and keeps the key (`HeldKeys`). The read's answer
becomes the focus's caret, as a report's does, and the key is handled as
an `Input::CaretKey` pressed one millisecond after the read ended, so the
caret before the key, here and in the outpost, is the one the read
found, and then passed on (`Effect::PassHeldKey`), after its watch is
asked for, so the watch reaches the outpost before the key reaches the
application. Any other answer, a failure or
the shell's bound passing, passes the key on with the hook's time. With
no focus whose caret is read, a held key is passed on at once.

Every key input that comes while a held key waits, a held key, an
`Input::CaretKey`, `Input::CharacterTyped` or `Input::ClearingKey`, waits
behind it (`WaitingKey`), since the hook deferred the key itself behind
the held one, and is handled in order once the keys ahead are passed on
(`editing::next_held`): a held key's read is asked for once the watch
of the held key passed on before it has been answered
(`HeldKeys::awaiting_watch`, which follows a newer watch a key handled in
between starts), or once `KEY_AHEAD_BOUND_MS` (100 ms) have passed since
that key was passed on: a held key that starts waiting asks the shell,
once, to end its wait then (`Effect::BoundHeldKeyWait`), and
`Input::HeldKeyWaitEnded` reads its caret if it still waits, leaving the
earlier watch to end as it would; a held key pressed after the bound is
read at once. Any
other is handled as the input it came as (`reduce::key_input`), a caret
key taken as pressed no earlier than the key ahead was passed on. The
watch of the key just passed on is marked as having a later key
(`later_key`), as the later key's press would have marked it had the
watch existed, so its answer only keeps the caret current and says its
selection's change. Each waiting key's effects carry its own trace
(`Effect::ForKey`), so its requests' answers and its speech are its own.

Caret keys (`docs/nvda/editable-text-and-terminals.md`). An
`Input::CaretKey` on a focus with text asks its outpost to watch for
evidence (`TextOp::AwaitCaret`), with the caret Core last knew and when
the key was pressed, the unit to report, for Delete the character or
word at the caret (whose change is evidence), and whether the key
deletes, for which a change of the caret's line is evidence too; every
key's answer, a
selecting key's too, compares with the selection the outpost kept for
the node, as a caret event's report does
(`docs/crates/verbatim-outpost.md`). Backspace and Control+Backspace over
a selection predict what comes before the selection's start, where
NVDA's caret is, whichever end the application's caret is: the report
gives the start as a position in the caret's line, or in the line it
starts in when that is an earlier one (`CaretReport::selection_line`),
and a start in neither has the key say only the selection's change
(`PendingCaret::before_unknown`). The outpost never waits for the evidence: it
answers once the evidence comes, and a watch that ends without it is
answered `TextReply::WatchEnded`, for which Core says nothing, as it
says nothing for a reply with `moved` false. A key that moves nothing,
such as Home at a line's start, is silent, where NVDA speaks the unit
after its 100 ms wait (`docs/parity/text.md`, "TextInfo-equivalent layer"). The caret, the Delete text, the selection,
and what Backspace deletes all come from the caret as the key found it:
Core keeps the focus's last 8 caret reports (`CARET_HISTORY`, held
inline in the state) with the time each was observed, a caret event's
`observed_at_ms` or a reply's `read_at_ms`, and takes the newest observed
strictly before the key's `pressed_at_ms`. The caret it holds when the
key arrives may already show what the key did, since the application's
caret event can reach Core before the key does. With no time for the
key, or no timed report (a report without a time empties the history),
the caret Core holds stands in; with timed reports but none from before
the key, the caret before the key is not known. A caret Core moved
itself, as say-all does, is kept at time 0, before any key, so it stays
the caret a key found when the application's report of the move was
observed no earlier than the key. The caret Core holds is
the newest by observed time, not the last to arrive: a timed report
observed before it, such as a caret key's reply read before a caret
event that reached Core first, only joins the history, and neither the
caret nor the review cursor following it moves back. A report without a
time is taken as it arrives. A newer caret key or a typed character
supersedes one still watched, and a focus change drops it, so a focus
announcement wins, speech never lags behind fast typing, and the caret
typing moves is not spoken as an earlier key's answer. The answer updates the caret,
then speaks, queued:

- Left and Right Arrow, Home, and End: the character at the caret, a
  punctuation character by its name, a capital raised in pitch, a line
  break by its name ("carriage return", "line feed"), and the end of the
  text as "blank". Each character of a line break is a character of its
  own, so End in a standard edit control lands on its carriage return
  (`text::characters`, which the review cursor also moves by).
- Control with Left or Right Arrow: the provider's word the outpost sent;
  a word of one character, such as the full stop Notepad counts as a word,
  as that character, by its name.
- Up and Down Arrow, Page Up and Page Down, Control with Home or End: the
  line at the caret, "blank" when it has nothing to read.
- Control with Up or Down Arrow: the provider's paragraph, or the line when
  the provider has no paragraphs.
- Backspace and Control+Backspace over a selection: the character, or
  the text from the start of the word, before the caret as the key found
  it, read from the caret's line before the key, "blank" when there is
  none, as NVDA's Backspace script reads it before passing the key; then
  the selection's change. A classic edit control's caret is the
  selection's start, as NVDA's; a UIA text's is the selection's active
  end, where NVDA uses its start (`docs/parity/text.md`,
  "TextInfo-equivalent layer").
- Backspace and Control+Backspace otherwise: what the key removed, from evidence
  (`docs/design/phase6-design.md`, "What a key did to the text"): the caret's line as
  the key found it compared with the line the reply read, text that ended
  at the caret before the key and is gone, with the caret now where it
  began (`editing::removal`; in a terminal the row reads as spaces up to a
  caret standing in its padding, so a typed space is found). Nothing is
  said when nothing was removed, as when the classic edit control's
  Control+Backspace inserts a DEL character. At the start of a
  line, the line break it deleted, of the kind the text was last seen to
  use (the caret's line's own break, or the one remembered from an
  earlier line), a carriage return and line feed spoken as the line feed,
  as NVDA does. Nothing at the start of the text, or when no break has
  been seen.
- Delete and Control+Delete: the character or word now at the caret,
  then, over a selection, the selection's change.
- Any other key passed to the application that types no text
  (`CaretMotion::Other`, from the hook's `KeyReport::Passed`, and the
  clearing keys): text it removed at the caret is spoken (Control+W, and
  Control+K, which removes what follows the caret); a caret now on another
  line (`CaretReply::same_line`, which the outpost answers for a watch
  marked `landing`) speaks that line; a caret that moved over an unchanged
  line speaks by where it landed: the character at the caret when it moved
  by exactly one character or landed at the line's start or end, the word
  when it landed at a word's start or just after its end, and inside a
  word the word and then the character at the caret (bash's Control+A and
  Control+E, Alt+B and Alt+F). Text a key added says nothing here; typing
  echo and terminal output speak it. A key that removed a selection,
  something selected when it was pressed and nothing in its answer
  (Control+X, Control+V), says none of this, only the selection's
  change, since NVDA has no script for such keys and speaks only its
  caret event (`editing::removes_selection`).
- Any of them with Shift, and Control+A: what became selected and
  unselected, NVDA's "hello selected" and "hello unselected", a single
  character by its name, 512 characters or more as their number. Any
  other key over a selection speaks its own words, then how the selection
  changed, as NVDA speaks its script's words and then its caret event's:
  a movement its unit then the text it unselected, a deletion its own
  words then "selection removed". A change with no text says
  nothing. A change in general terms, the text having changed since the
  selection compared with was reported (`content_changed`), speaks the
  whole new selection: "hello selected" when something was selected, then,
  when something was unselected, "hello selected instead", or "selection
  removed" when nothing is selected now (`editing::selection_segments`).
  Text an object already has
  selected when it is announced puts the word first, "selected hello".
- A selection no key made, the mouse's or the application's, arrives as
  the selection changes of a `CaretMoved` once the focus's announcement is
  complete, and is spoken with the same words (`editing::selection_changed`).
  While a caret key's answer is awaited for the node, nothing is spoken
  then: the key's answer compares with the selection the outpost kept,
  which the report already replaced, so the report's words are held with
  the key (`PendingCaret::heard`) and spoken after the key's own words, or
  alone when the watch ends with no answer worth speaking. A key that
  supersedes another still waiting on the node takes over its held words,
  and the superseded key's answer, whose caret and words are not spoken,
  has its selection change said with the later key's
  (`SrState::superseded_caret`, `editing::superseded_reply`); an answer
  read once a later key that is not a caret key was pressed says its
  selection change too. Each change is compared once, by whichever read
  comes first, so none is said twice and none is lost. The known limit:
  the superseded slot holds one key, so in a burst of three keys where the
  first key's answer arrives after the third press, the first answer's
  selection change is not said; speech is cut at each press, so the user
  hears the last caret event's change, as with NVDA.

Formatting (milestone M4 item 7, `docs/nvda/document-formatting.md`).
A caret key's character, word, or line, and a new focus's first line,
carry the formatting the outpost read for them (`TextChunk::formats`).
The state keeps the formatting last reported and the node it was in
(`reported_format`, NVDA's per-object cache); a new focus starts with
none. The unit is spoken with the formatting at its start that differs
from that (`text::format_changes`, in NVDA's order: font name, size, and
color when present and different, the background color after the color
when both change, "dark red on light grey", or alone, "light grey
background"; bold, italic, strikethrough, and underline starting, or
ending after having been on, a strikethrough's or an underline's kind
when it changes, and the underline by its kind in place of
"underlined" when the kind was read; a link starting or ending, at every
unit, as NVDA says it; a spelling or grammar error starting), then a
list item's bullet, when the unit is read as text (a line or a say-all
chunk, never a word or a character), at every line that has one, as NVDA
speaks a line prefix rather than as a change; then its text, with each
later change placed where it happens
(`text::formatted_segments`), and the formatting at its end becomes the
one reported. For a character or a word, the extra detail of NVDA's
review and caret units, an error's end is also said ("out of spelling
error"); a character, or a word of one character, says only the
formatting at its start. A word's trailing white space is not spoken,
but its formatting change is, so moving onto a misspelt word says
"spelling error", the word, and "out of spelling error". Each change is
a `Format` span, which the theme reports as words, a sound, both, or not
at all. Review commands and say-all read no formatting yet.

Typing echo. `Input::CharacterTyped` is echoed by the settings: a finished
word first, when word echo applies and a character that is not a letter,
mark, number, or zero-width joiner or non-joiner ends it
(`verbatim_text::is_word_grapheme`), then each printable character (a tab included), each in
its own queued utterance. "Only in edit controls" means a focus that is an
edit field, a terminal, or a document that is not read-only. A protected
field echoes only the protected character, spoken "star", and no words.
In a terminal, unless the user asked for terminal passwords to be spoken,
typing is held until `NormalizedEvent::TextChanged` for it arrives, so a
password prompt that shows nothing speaks nothing; Enter drops what was
held. A caret key without Shift, or a focus change, ends the word being
typed; a key that selects, or a key outside the caret table, does not.
Interruption of speech by typing and Enter is the keyboard hook's
(`verbatim-input`'s `interrupt_for_characters` and `interrupt_for_enter`).

The review cursor over text (`docs/nvda/review-modes.md`). While it
follows the caret and has not been moved away from it (`ReviewText::Caret`,
set by a caret key, a caret report, or a caret key's answer), a review
command reads the line at the caret as the outpost finds it
(`TextPoint::Caret`) and starts from there, never from Core's copy of the
caret: an application's caret event for a key can reach Core after a
review command pressed later, so Down Arrow followed at once by reading
the current line reads the line Down Arrow moved to, as a caret key speaks
what the outpost reads once its evidence comes. Say-all from the caret and the
caret's location ask about the caret the same way. Once moved, the cursor
holds its line, so character and word motion within the line and reading
the current unit need no round trip; another line, page, the document's ends,
the selection's ends, and the start marker are read through the protocol
and landed on. NVDA's messages and repeated presses hold as in M3, with
"Top" and "Bottom" known at once when the line is the document's first or
last and otherwise from a movement that did not move; the current character
pressed twice gives its description and three times its code; the current
line or word pressed twice is spelled and three times spelled with
descriptions; start and end of line speak the character there, the end
of a line being its last character with its line break included, so
"carriage return" in Windows 11 Notepad and "line feed" in a standard
edit control, as in NVDA, and moving by character crosses each character
of the break, outside a terminal, whose row has no break; previous
and next word cross lines, landing on the next line's first word or the
previous line's last; a word of one character is spoken by its name, as
the caret's word is; a unit the text does not have says "Not supported in
this document". The column difference from NVDA (`docs/parity/review.md`, "Review
cursor columns"): moving to another line keeps the column, a cell column in
a terminal (where a column past a row's text is a blank cell and the cursor
can move across a padded row's blank cells) and a remembered character
column elsewhere (a shorter line puts the cursor on its last character, the
next longer line returns to the column). A lost anchor starts again from the
caret.

Select then copy. Verbatim+F9 marks the review cursor's position ("Start
marked"); Verbatim+Shift+F9 moves the review cursor there; Verbatim+F10
asks the outpost to select from the earlier of the marker and the review
cursor through the character at the later, whichever that is, and speaks
the selection's change as a selecting key's is ("Name selected"), and
pressed twice reads the range the first press selected, wherever the
review cursor has gone since, and copies it (`Effect::CopyToClipboard`,
so the shell's clipboard helper confirms), clearing the marker whatever
the copy found; the read carries the marker's generation, so a marker set
while it is in flight is kept. A first press again before the copy says NVDA's "Press
twice to copy or reset the start marker" and selects nothing. Only the
outpost can tell which comes first, so Core sends both ends with the
points just past their characters (`TextThrough`), which it finds in the
lines it was sent: the marker keeps the point past its character as it
is set. With no marker, or with the marker in another object, which it
keeps, it says "No start marker set". Where the text cannot be selected, flat text among it, the first
press says NVDA's "Can't select text, press twice to copy" and the second
copies.

Toggles and locations. Verbatim+6 toggles the review cursor following the
caret ("caret moves review cursor", "caret doesn't move review cursor");
Verbatim+2 and Verbatim+3 cycle typed character and word echo through off,
only in edit controls, and always; each says the new value and emits
`Effect::SettingsChanged` for the shell to save. The caret's and the review
cursor's locations are asked of the outpost and spoken "Positioned at x,
y".

Say-all (`docs/nvda/speech.md`, "Say-all"). From the caret (the focus) or
the review cursor (the navigator), it reads by the "Say all reads by"
setting: asked for a sentence, a provider with no sentence unit (UIA)
answers that it has none and reading goes by line, and one whose text NVDA
splits itself answers with the paragraph, which is split by Unicode's
sentence rules; a terminal always reads by line. Each read asks for a batch
of 20 units (`TextOp::ReadAhead`), the first from the point onwards, every
later batch a unit on from the last chunk read, so a batch where the
provider runs remote operations is one round trip for tens of seconds of
speech. The batch's pieces wait in a buffer in the state
(`SayAll::buffer`, shared chunks, bounded by the outpost's limit on a
batch's text). Every unit read is spoken without pauses
(`docs/nvda/speech.md`, "Say-all speaks without pauses"): an utterance
runs to the last sentence end in a unit (`verbatim_text::last_pause`), and
the rest of the unit is held back (`SayAll::held`) to open the next one, so
a line holding the end of one sentence and the start of the next is
spoken in two utterances and a sentence that runs on to the next line in
one; ten units in a row with no sentence end (`MAX_HELD`, blank ones
counted) are spoken together, as is what is held back when the text ends.
Each unit's start has an index mark where its text starts in its
utterance, and each utterance opens with a mark, which for an utterance
opening with the rest of a unit moves nothing. Utterances are handed to
speech one at a time, two ahead of playback (the one playing and the
next). A space stands for the line break between two units' text in one
utterance. When playback reaches a unit's mark, say-all from the caret
asks the outpost to move the caret to the unit's start and takes the
unit, with the caret at its start, as Core's caret, so a caret key
pressed before the application reports the move starts its watch from
there (the review cursor follows the caret as usual); say-all from the review cursor leaves
the review cursor at that point, whose line the next review command
reads; when it reaches an utterance's opening mark, the next utterance is
handed on. The next batch is read once fewer than 10 utterances
(`LOW_WATER`) are left to speak, handed out and buffered together, so at
most one read is in flight and the buffer holds under two batches.
The choice and the measurements behind it are in `docs/performance.md`,
"Say-all".
Blank pieces are not spoken. The display is kept on while it reads
(`Effect::KeepDisplayOn`, by the setting). It ends after the last piece of
the text, known from a chunk marked as the last or a read that could not
move on, and stops on any command, caret key, typed text, cancelled speech
(`Input::SpeechCancelled`, from any key), one of its utterances cut off
however it was cut (`Input::UtteranceCancelled`, which the speech
pipeline reports with the utterance's marks), a focus change (which also cuts
its speech off), or the end of its outpost, dropping the buffer and leaving
the cursor where reading got to. An utterance that failed
(`Input::UtteranceFailed`) counts as heard: say-all goes on past it as if
playback had reached its last mark, handing on and reading on, but moves
no cursor to what was not heard.

## What the shell learns

Only the reducer speaks (`docs/architecture.md`, section 2), so what the
shell's threads learn comes in as inputs and the reducer decides what is
said (`shell.rs`): `Input::Started` plays the start sound, `Exiting` the
exit sound unless a new instance replaces this one, `ErrorLogged` the
error sound, and `ApplicationNotResponding` the not-responding sound;
`LockKeyToggled` says "caps lock on" (`Phrase::LockKeyState`), `ClockRead`
the time on a first press and the date on a quick second, as the
operating system formatted them, `ClipboardCopied` "Copied to clipboard:"
with the text (`Phrase::CopiedToClipboard`) or "Unable to copy"
(`Message::UnableToCopy`), and the theme page's `PreviewSpeech` its
sample, all interrupting; `ShellItemGone` says the list dialog's item is
no longer there and `ShellItemUnclickable` that it cannot be clicked,
both queued, and `PreviewEarcon` plays its event. Each speaks
under the trace its input carries, so a copy's confirmation is fenced off
by a later key press, as that key's other speech is.
`MenuOrSwitchEnded` asks for the application in front's focus
(`Effect::ReadForegroundFocus`, NVDA's fake focus) unless a focus
observed since the close has been applied
(`docs/parity/backends.md`). `ForegroundWindow` says nothing; it is kept
for `SrState::hook_foreground`. `tests/shell.rs` checks each rule.

## Terminals

What a terminal says (`terminal.rs`; `docs/design/terminal-model.md`,
"Features and settings", its "Core's flood policy"). The focused terminal's outpost reads it and sends
each read; Core's terminal model (below) gives the read to the reading
rules, and what they say (`Said`, `TerminalItem`s and the rules' own new
lines) is spoken here; speech of anything but the focus is ignored. The
types of what the rules say, `TerminalItem` (output, a count of lines
skipped, or a key's answer), `TerminalUnit`, `Skipped` and
`TerminalKeyKind`, are Core's own (`terminal/items.rs`): none crosses a
pipe.

- The answers to keys are spoken at once, in order, ahead of the output
  the same speech carries: typing the terminal showed (`TerminalItem::Echo`)
  by the typing echo settings (`editing::echo`), a key's answer at the
  caret in its unit (a character by its name or "blank", a word, or the
  line), text the caret's line gained or lost, a row away from the caret
  that is a key's answer, and a suggestion. With "speak passwords" on,
  typing is echoed as it is typed instead, and an echo the rules say
  says nothing more. A caret key's watch's answer in a terminal keeps
  Core's caret current and says nothing, the reading rules having answered
  the key.
- What a key is to the reading rules is worked out here
  (`caret_key_kind`: the arrows left and right, Home and End a character;
  Control with them a word; the arrows up and down, the page keys,
  Control with Home and End or the arrows up and down a line; Delete as
  Delete; any other `Other`; `typed_kind`: text typed, `Enter` for Enter,
  `Forget` for Tab, `Other` for a control character).
- The review cursor in a terminal follows the caret only when the
  reading rules say a key moved it within its window (`Said::followed`):
  the next review command then reads at the
  caret. A caret report alone, or a caret key's press, never moves it
  there, since a program moves its cursor as it draws
  (`docs/design/terminal-model.md`, "The review cursor").
- Output is spoken queued, in order, one line per utterance, each starting
  and ending with an index mark. Every line counts, blank ones included
  (Dickson, 2026-10-07, and the coherence review of 2026-10-09): a blank
  line waits and is counted when skipped, but is never spoken. Two
  utterances are handed to speech ahead of playback (one playing, one
  ready behind it) and the rest wait in the state (`TerminalSpeechState`),
  so the backlog not yet spoken is known. Each opening mark reached hands
  on the next. Newer output never cancels older output still waiting.
- The flood policy, by groups: the first "Lines spoken in full" lines of
  a burst of output are spoken whole, however fast the rest arrives
  (`TerminalSpeechState::group` counts the lines the group still hands to
  speech). Only once the group's last line has been heard (its closing
  mark) is the backlog looked at: when more lines wait than "Lines spoken
  in full" (any count already skipped included), everything but the
  newest "Last lines to speak" becomes one "skipped N lines"
  (`Phrase::SkippedLines`), adding any count the rules gave; a count the
  rules could not make (`Skipped::Uncounted`) makes it "skipped lines"
  (`Phrase::SkippedUncountedLines`), and lines that went by past the
  terminal's history (`Skipped::MoreThan`) make it "skipped more than N
  lines" (`Phrase::SkippedMoreThanLines`), N being the lines left
  unspoken after it; lines a lost read counted from text
  (`Skipped::About`) make it "skipped about N lines"
  (`Phrase::SkippedAboutLines`). A lost read sends a count of none ahead
  of its lines, and each of its lines waits marked with the read's kind
  (`Counted`), so any count that skips one of them, by this read or a
  later one, says "about" or "more than"; with lines of both kinds in one
  skip, "more than" wins, as `Skipped::plus` adds them. An "about" count of
  none is not kept, since it says nothing. A "more than" count of none
  waits ahead of its lines, and when it reaches speech, once, it is silent
  if the policy, with what waits then, will skip the backlog's first line
  and that line or count says "more than"; otherwise it says "skipped
  lines", and a later read that makes the policy skip its lines still
  makes that skip "skipped more than N lines". What waits is then
  the next group, and the decision repeats after it, until the output
  stops and everything waiting has been heard; the next output starts a
  burst of its own. A burst ends only once its last line has been heard,
  never when its last line starts playing: a flood's first line can start
  playing a moment before its second line arrives, and ending the burst
  then started a new group one line late (the intermittent
  `conhost_flood` failure of 2026-10-08). Output under the limit is never
  touched, however many batches it arrives in. Behind the group being
  spoken, the waiting queue keeps at most as many lines as either limit
  keeps, older ones folded into the skipped count as the output is
  queued, so a line past that bound never waits (a flood read whole hands
  the policy about 9,000 lines, of which it keeps a few), and everything
  waiting
  at most 10 MB (`MAX_WAITING_BYTES`, Dickson, 2026-10-07), the oldest
  waiting lines folded into the skipped count beyond it; a single line
  larger than that is cut on a grapheme boundary and followed by "line
  cut" (`Message::TerminalLineCut`). So the state stays bounded and every
  count stays exact: a line and its growth (`TerminalItem::Growth`) are
  one unit, counted once and dropped together, so a group never ends
  between them and a growth is never spoken without its line; a line after
  a "more than" or "about" count adds to it. The policy is the only one:
  the reading rules hand it every new line, and after a lost read a count
  of none of the lost read's kind (`Skipped::About(0)`,
  `Skipped::MoreThan(0)` or `Skipped::Uncounted`) ahead of its lines
  (`docs/design/terminal-model.md`, "Features and settings").
- On-demand reading. Once a group's worth of lines waits or is queued for
  speech, Core tells the outpost to hold (`TextOp::TerminalHold`): it only
  notes that the terminal changed. When the group's last line is handed
  to speech, Core asks for what is new (`TextOp::TerminalRead`, holding
  on), so the answer is in before that line has been heard; the backlog
  is looked at only once both have happened
  (`TerminalSpeechState::deciding`). When nothing is left waiting, Core
  asks once more and reading goes live again. A cut while the outpost
  holds, or with a request out, tells it to read to the end
  (`TextOp::TerminalCancel`), as NVDA drops what is pending; of speech
  read before a cut (`Input::SpeechCancelled::at_ms` against the time the
  read began, which the event carries), or before the cancel's answer,
  only the answers to keys are spoken. So is the answer to a request the
  cancel overtook (`TerminalSpeechState::overtaken`), which the outpost
  sends ahead of the cancel's: its read may be the one that saw the typing
  the key that cut speech typed.
- Anything that cuts speech off drops the output waiting and handed to
  speech: `Input::SpeechCancelled` (a key), any step whose effects stop
  speech or speak an interrupting utterance (`reduce` checks every step's
  effects), and an utterance of output cut off any other way, which the
  speech pipeline reports with its marks (`Input::UtteranceCancelled`), so
  Core never waits for marks playback will not reach. An utterance of
  output that failed counts as heard (`Input::UtteranceFailed`, its marks
  taken as reached), so output carries on past it, as NVDA's speech
  manager does when a later index arrives. A focus moving to another node forgets the terminal's output;
  utterances of output carry the terminal's focus validity, so the speech
  pipeline drops them once the focus leaves.
- Verbatim+5 (`ReviewCommand::ToggleReportNewOutput`) toggles "Report new
  output", says "report new output on" or "off", and emits
  `Effect::SettingsChanged`; off drops what is waiting (and cancels a hold)
  and speaks no output, while the answers to keys are still spoken.

The unit tests in `terminal/tests.rs` give the flood policy what the
reading rules said of each read, as the terminal model gives it, and drive
the rest of the reducer with its inputs, with simulated playback, reaching
each utterance's opening mark as it starts and its closing mark once
heard, and a simulated outpost that holds and answers Core's requests as
the real one does. They check the flood policy's groups and counts, the
answers to keys spoken at once, typing echoed by its settings, what a key
is to the rules, the review cursor reading at the caret once it followed
it, the answers alone spoken of what a read before a cut said, and output
cut off outside the reducer released by the pipeline's report of each cut
utterance; `tests/alloc.rs` checks that a read of a terminal showing a
line allocates the same whatever the size of the state.

### Core's terminal model

The reading rules run in Core (`docs/design/terminal-model.md`, "Where it
runs"; Dickson, 2026-10-24): `terminal_model.rs` and
`terminal_model/review.rs` take each read a terminal's outpost sends
(`NormalizedEvent::TerminalSnapshot`) and the answers to the requests
they make.

- Public: `read_of(&TerminalSnapshot)`, the reading rules' `Read` of a
  snapshot: its text as it came, without its lines' padding (the rules'
  `ReadText`), and the screen, the first row and the caret's row, a
  screen at most, padded again exactly; and `snapshot_of(&Read,
  waited, colours)`, the snapshot of a read as an outpost sends it, for
  the tests and measurements that read the simulated terminal as the
  outpost reads a real one.
- `SrState::terminals` holds a model per terminal node read, at most 16,
  the least recently read forgotten first, and those of an outpost that
  ended dropped: the reading rules (`TerminalReader`) behind an `Arc`, so
  a checkpoint's clone shares them and the first change after it copies
  them once; the keys taken in and not yet given to the rules; the newest
  key given; a read owed (the whole document, or a first read's history
  block); the fresh read before a review answer, the review requests
  waiting, the history read asked for and the read it was counted from,
  and the history rows that came last for the answer in progress; the
  review places; the caret, the screen's shape and whether the last read
  began at the document's start; the shape told to the outpost; the
  quiet-period timer's due time. A
  model is made with a terminal's first snapshot, while it has the focus.
- Keys. Every key input Core takes in while the focus is a terminal it
  models is kept (`key_input` and, for a held key, `editing::pass_held`):
  a caret key by its unit (`caret_key_kind`), Delete as Delete, any other
  key passed as `Other`, text typed by `typed_kind`, and a clearing key
  as `Forget` followed by the caret key the hook sends after it as
  `Other`; a key deferred behind a held key is kept when Core handles it,
  with its time taken as no earlier than the held key's pass. With each,
  Core asks the outpost for the screen's colours over the key's window,
  from its time to `KEY_WINDOW_MS` after it (`TextOp::TerminalColours`),
  for a held key before its watch and its pass; the answer is always
  `Done`, and a read the outpost takes at once for it comes as a live
  read. A read is taken as of when it ended (`read_at`): the terminal
  answered it at some moment before then, so a key whose effect it shows
  reached the program no later. Before a read taken at S is given to the
  rules, they are given every kept key whose time is at or before S, in
  the order Core took them in, stopping at the first later one; before
  the timer at T, those up to T. Past 256 kept keys the oldest is given at once. When the focus
  leaves the terminal, the rules are told (`blur`) and the kept keys
  dropped; when it returns, the rules are told (`focus`).
- A read's speech key: once the keys up to the read's time are given,
  the newest key given, when its window (`KEY_WINDOW_MS`) holds the
  read's time. What the read says is handed to the flood policy above
  under that key's trace, and every effect it causes goes out as
  `Effect::ForKey` for it, so a key deferred behind a held key, whose
  number fences off older speech, keeps the speech its read caused. A
  read with no speech key, and the timer's speech, carry the input's
  trace.
- A live snapshot is taken only for the focused terminal. When the rules
  ask for the whole document or for a first read again with its history
  block, Core asks for it (`TextOp::TerminalReadNow`, `TerminalScope::Whole`
  or `HistoryBlock { height }`) and, until it comes, drops the terminal's
  live snapshots and answers its own flood-policy reads with nothing: the
  outpost answers in order, so each began before the read owed, which
  holds all it showed. A snapshot answering the flood policy's
  `TerminalRead` or `TerminalCancel` is aligned the same way and its
  speech taken in by the policy as an answer;
  `TextReply::TerminalUnchanged` says nothing. A snapshot answering a
  colours request is taken as a live read. The read's caret report, when
  it carries one, becomes Core's caret, stamped with the read's time;
  output read before speech was cut off is judged by when the read
  began.
- The read before a held key (`TextOp::ReadCaret`) is answered in a
  terminal with a snapshot (`before_key`): it is aligned as a live read
  is, and its caret answers the read as `editing::held_read` takes it, as
  read at the read's time, so the key is taken as pressed a millisecond
  after the read ended.
  A new screen height or width is told to the outpost
  (`TextOp::TerminalShape`), and the timer the rules ask for is armed or
  disarmed for the focused terminal (`Effect::TerminalTimer`), which the
  shell answers with `Input::TerminalTimer` when it is due.
- Review commands. A request for a modeled terminal's text that the memory
  answers (a read, a read ahead or a range at the caret, the start, the
  end, or an anchor Core minted for it; a selection, a location or a caret
  move there, answered `Unsupported`) is not sent: `reduce` takes it out
  of the step's effects, asks for a fresh read (`TerminalScope::Fresh`),
  aligns it, and answers the request from the memory as the outpost did
  (`terminal_model/review.rs`, the outpost's review answers moved), its
  answer then handled as if the outpost had sent it, and any request that
  answer causes answered in turn. When an answer needs rows above the
  memory, Core asks for them (`TextOp::TerminalHistory { above, whole,
  read }`), counted from the start of the read its memory last took,
  and works the answer out again once they come, placed among the
  memory's rows as they are then (`review::placed_history`): the answer
  names the read its rows are counted from, and when the memory has
  taken later reads since, Core follows that read's start by place
  across them (`review::Counted`), each read adding how far its own
  start lies below the start it was counted from, so output that
  scrolled while the read was out moves nothing and the answer is used
  rather than asked for again. Rows counted from a read the memory did
  not take, or whose start it could not follow past a whole read, a
  refused start or a read it did not take, cannot be placed, and the
  request is answered `Unanswered`, saying nothing. Its rows carry text
  anchors with the top bit set (`TERMINAL_ANCHORS`), a range no outpost
  mints from, each the place the memory keeps for it; at most 64 places
  beyond those Core holds are kept. A request naming another anchor goes
  to the outpost as before. A waiting review is an enum,
  `Review::Text` for such a request or `Review::StatusBar` for "Report
  status bar" (`terminal_model::report_status_bar`), both answered after
  the same fresh read; the status bar's footer is
  `terminal_model/review.rs`'s `footer`, the lines below the caret's
  line, else the rows at the screen's bottom a program rewrote, else the
  last line with text, read, spelled or copied by the press's repeat
  count. Outside a modeled terminal it says `Message::NoStatusLineFound`,
  and with no text `Message::NoStatusBarInformation`.
- Command marks. Enter given to the rules on a caret's line that holds
  input (`TerminalReader::caret_line_input`) marks that line in the
  memory and notes its hash until the quiet period after the key ends;
  each read until then clears it when a mark with that hash survived,
  or marks the new row nearest the caret starting a line with it, as
  where Pi prints the input it cleared. Each mark's line, its first
  row's text and how many rows it held, is kept (`MarkLine`) for the
  search. `review_text::command_mark` runs Verbatim+Alt+Up and Down
  Arrow as a line move whose request the model answers
  (`Review::Mark`): after the fresh read, `review::mark` tries the marks
  beyond the review position, nearest first, finding each by its line's
  hash near its place in the memory, and above the memory with
  `TextOp::TerminalFind`, the outpost's search outward from the mark's
  place, whose matches it checks by their line's hash; a mark not found
  is dropped, and the answer is the row as `TextReply::Read`, landed as
  a line move lands, or nothing past the last mark. The model keeps the
  number of the read its memory last took, and places a search's
  matches among the memory's rows when the answer comes
  (`review::placed`, which shares `review::start_of` with the history's
  placement), counted from the start of the read the answer
  names: when that is the memory's last read, as it is whenever the
  memory took every read the outpost took before searching, output that
  scrolled the rows while the search was out moves nothing; otherwise
  the move ends, saying nothing, the mark kept. A press makes at most
  `review::MARK_SEARCHES` (3) searches, saying nothing once they are
  spent.
- Settings: "report new output" and "review follows caret" are given to
  every model's rules whenever they change, whichever way they changed.

`tests/terminal_reading.rs` drives `reduce` with snapshots of the
simulated terminal (`tests/terminal_support`), whose outpost reads as the
real outpost does: it keeps the start of the last read whose screen
showed text, reads at the height Core told, takes colours in the first
read from a key's time, and answers Core's reads, history and shape. It
checks new output spoken line by line; the review position moving with
its row as output scrolls it into the history, a place whose line left
its search keeping its place, a place above the memory found lower after
its history was printed again longer, moving above the memory's rows to
the document's start, a row ending at its text and a range joining lines,
and the review reading at the caret; a key whose time is after a read's
start given after that read; the echo of Up Arrow held and "x" deferred
behind it going out for "x"; a read after every window carrying no key;
live snapshots dropped while a whole read is owed; the timer giving the
keys that came first; a snapshot of every corpus read rebuilding its read
exactly, through its JSON, and the rules taking both alike; and a terminal
session replayed from a dump to the same effects. The module's unit tests
check the rules' speech mapped onto the flood policy's items and each key
input's kind. `tests/terminal_cost.rs` is a measurement, not a test:
the reducer's time for a terminal's costliest reads (`docs/performance.md`,
"A terminal's reads in Core").
