# verbatim-core

The deterministic reducer (architecture section 2), the flight recorder,
and its on-disk dump format.

Public API:

- `reduce(&mut state, &input)` — deterministic, no I/O, no clocks;
  advances the state in place, without copying it, and returns the
  effects to execute.
- `SrState` — `new()`; `focused()`, the live focus and its application;
  `attention()`, the application holding attention; `held_nodes()`,
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
  (`phase6-design.md`, "Themes: one model for verbosity, speech, and
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
  ("What an object with text says" in `docs/nvda/speech.md`): the state
  keeps a `FocusText` for it, and the outpost's first `CaretMoved` for the
  focus speaks the caret's line ("blank" when empty) as a second queued
  utterance with the focus's validity, or, when text is selected, asks for
  the selected text (`TextOp::ReadRange` between the reported selection's
  ends) and speaks "selected" with it, or its count at 512 characters or
  more. `NoText` in its place speaks the value. The name and role are never
  held back for it; a caret key or a focus change drops what is still
  waiting, and a protected field's text and value are never spoken. Object
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
  enters it is not announced later: a description change speaks the new
  description alone on the focused node, and a state change speaks on the
  focus or one of its ancestors, diffed against the ancestor's states as
  the focus was reported with them, as NVDA's base handlers do (a
  selection of an ancestor is such a state change); a name change speaks only on the focused node, where the new name
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
  pure `review` module — grapheme-cluster characters and word-boundary
  segmentation wait for M4's text model. All of this is pure and unit-tested
  in `verbatim-core`. Since M4 this flat walk serves only objects with no
  text interface (see "Text" below); its spelling goes by grapheme
  clusters, a punctuation character is spoken by its name, the current
  character pressed twice gives its description, and the current line or
  word pressed three times is spelled with descriptions.
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
  half checked without becoming checked says it too. A focus that
  reported itself focused when it became the focus
  (`FocusContext::reported_focused`) and whose new state set no longer
  includes focused has lost the focus before the next focus event
  arrived: its states are kept but nothing is spoken (`docs/parity.md`,
  "State changes after the focus has left"). When the change makes the
  focus expanded and the event carries a `child_count`, which the outpost
  reads only for a Win32 tree view item, "52 items" (`Phrase::Items`)
  follows as an utterance of its own (`docs/nvda/speech.md`, "How many
  items an expanded tree view item holds").
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

## Text (milestone M4)

The reducer's side of M4 items 1 and 3 to 6 (`phase6-design.md`, "The
autonomous run"), over the text protocol in `verbatim-model`. Pure helpers
over received text are in `text.rs` (a line without its break, grapheme
clusters, columns in characters or cells, words by the text's language
through `verbatim-text`, and the spoken segments for characters, words,
lines, and spelling); caret keys and typing echo in `editing.rs`; the
review cursor over text in `review_text.rs`; say-all in `say_all.rs`.

What the state keeps, within the bounds of "Core's state" in
`phase6-design.md`: the focus's caret (`CaretContext`: its line, with the
caret at the line's offset, and the selection), the review cursor's
position (`ReviewText`: unknown, flat, a line and a position on it, at
the caret it follows, or a point whose line has not been read), the caret key waiting for evidence,
the one review or text request in flight, the start marker, a say-all's
queue of spoken pieces, the word typed so far (at most 256 bytes), and
typing held for a terminal (at most 1 KB). Text is held only a line at a
time, as a shared chunk of at most 64 KB, so the state's clone, which the
flight recorder takes at each checkpoint, copies a pointer and not the
text; `tests/alloc.rs` asserts that caret and review steps allocate the
same over small and large states, and that a checkpoint with a 64 KB line
in the state allocates under 1 KB.

Which objects have text. A focus or navigator object whose role is an
edit field, a document, or a terminal may have text, and so does any node
the outpost sent a caret report for. For those, caret keys wait for
evidence and the review cursor reads lines through the protocol; the
first review command reads the line at the caret, and a `NoText` answer
makes the object flat text, reviewed by its value or name as in M3. Any
other object is flat text without asking.

Caret keys (`docs/nvda/editable-text-and-terminals.md`). An
`Input::CaretKey` on a focus with text asks its outpost to wait for
evidence (`TextOp::AwaitCaret`), with the caret Core last knew and when
the key was pressed, the unit
to report, for Delete the character or word at the caret (whose change is
evidence), for a selecting key the selection before it, and a wait three
times longer in a terminal. The caret, the Delete text, the selection,
and what Backspace deletes all come from the caret as the key found it:
Core keeps the focus's last 8 caret reports (`CARET_HISTORY`, held
inline in the state) with the time each was observed, a caret event's
`observed_at_ms` or a reply's `read_at_ms`, and takes the newest observed
strictly before the key's `pressed_at_ms`. The caret it holds when the
key arrives may already show what the key did, since the application's
caret event can reach Core before the key does. With no time for the
key, or no timed report (a report without a time empties the history),
the caret Core holds stands in; with timed reports but none from before
the key, the caret before the key is not known. The caret Core holds is
the newest by observed time, not the last to arrive: a timed report
observed before it, such as a caret key's reply read before a caret
event that reached Core first, only joins the history, and neither the
caret nor the review cursor following it moves back. A report without a
time is taken as it arrives. A newer caret key supersedes one still
waiting, and a focus change drops it, so a focus announcement wins and
speech never lags behind fast typing. The answer updates the caret,
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
- Backspace: the character before the caret before the key, worked out from
  Core's copy of the caret as the key found it, once the caret moved; Control+Backspace the
  text from the start of the word before the caret. At the start of a
  line, the line break it deleted, of the kind the text was last seen to
  use (the caret's line's own break, or the one remembered from an
  earlier line), a carriage return and line feed spoken as the line feed,
  as NVDA does. Nothing at the start of the text, or when no break has
  been seen.
- Delete and Control+Delete: the character or word now at the caret.
- Any of them with Shift, and Control+A: what became selected and
  unselected, NVDA's "hello selected" and "hello unselected", a single
  character by its name, 512 characters or more as their number. A
  movement without Shift that leaves a selection speaks its unit, then
  the text it unselected; a deletion does not. Text an object already has
  selected when it is announced puts the word first, "selected hello".

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
word first, when word echo applies and a character that is not a letter or
digit ends it, then each printable character (a tab included), each in
its own queued utterance. "Only in edit controls" means a focus that is an
edit field, a terminal, or a document that is not read-only. A protected
field echoes only the protected character, spoken "star", and no words.
In a terminal, unless the user asked for terminal passwords to be spoken,
typing is held until `NormalizedEvent::TextChanged` for it arrives, so a
password prompt that shows nothing speaks nothing; Enter drops what was
held. A caret key or a focus change ends the word being typed.
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
what the outpost reads after its wait. Say-all from the caret and the
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
this document". The column difference from NVDA (`docs/parity.md`, "Review
cursor columns"): moving to another line keeps the column, a cell column in
a terminal (where a column past a row's text is a blank cell and the cursor
can move across a padded row's blank cells) and a remembered character
column elsewhere (a shorter line puts the cursor on its last character, the
next longer line returns to the column). A lost anchor starts again from the
caret.

Select then copy. Verbatim+F9 marks the review cursor's position ("Start
marked"); Verbatim+Shift+F9 moves the review cursor there; Verbatim+F10
asks the outpost to select from the marker up to and including the
character at the review cursor, and pressed twice reads that text and
copies it (`Effect::CopyToClipboard`, so the shell's clipboard helper
confirms). With no marker it says "No start marker set", and with the
marker in another object, NVDA's "The start marker must reside within the
same object". Flat text cannot be selected, so there the first press says
"Not supported in this document" and the second copies.

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
asks the outpost to move the caret to the unit's start (and the review
cursor follows the caret as usual), say-all from the review cursor leaves
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
(`Input::SpeechCancelled`, from any key), a focus change (which also cuts
its speech off), or the end of its outpost, dropping the buffer and leaving
the cursor where reading got to.

## Terminals (milestone M4 item 9)

The reducer's side of terminal output (`terminal.rs`; `phase6-design.md`,
"Terminal output: notifications or diffing" and "The flood policy,
reconsidered"). The focused terminal's outpost diffs its text and sends
what is new as `NormalizedEvent::TerminalOutput`
([verbatim-model](verbatim-model.md)); output from anything but the focus
is ignored.

- Output is spoken queued, in order, one line per utterance, each starting
  with an index mark; blank lines are dropped. Two utterances are handed to
  speech ahead of playback (one playing, one ready behind it) and the rest
  wait in the state (`TerminalSpeech`), so the backlog not yet spoken is
  known. Each mark reached hands on the next. Newer output never cancels
  older output still waiting.
- The flood policy ("30 and 30"): when the lines waiting, with those
  handed to speech, are more than "Lines spoken in full", everything
  before the newest "Last lines to speak" becomes one "skipped N lines"
  (`Phrase::SkippedLines`), adding any count the outpost sent; a count the
  outpost could not make (`Skipped::Uncounted`) makes it "skipped lines"
  (`Phrase::SkippedUncountedLines`). Output under the limit is never
  touched, however many batches it arrives in. The waiting queue holds at
  most the limit's lines, each at most 4 KB, so the state stays bounded.
- The last line read, changed in place, speaks what changed; while an
  earlier version of that line is still waiting, the whole new line takes
  its place, so a progress bar rewritten quickly is spoken once, as it
  last was.
- Typing. With "speak passwords" off, typing into a terminal is held
  (`editing`) and echoed only when the terminal shows it: when the line
  grew by exactly what was typed (control characters, which never show,
  aside), those characters are echoed by the typing echo settings and not
  spoken again as output; what the terminal added beyond them (a tab
  completion) is output. When the line grew by something else (a password
  prompt's asterisks), what was held is dropped unspoken and what the
  terminal showed is spoken. A line rewritten while typing was held or
  echoed is taken as the typing showing: the typing is echoed and the
  rewrite is not spoken. With "speak passwords" on, typing is echoed at
  once and remembered until the terminal shows it, so it is not spoken
  twice. Enter forgets both. White space at the start of what the line
  gained that the line may already have had (`LineChange::uncertain`: a
  prompt's trailing space, which the outpost cannot tell from padding) is
  matched only as far as the typing itself starts with white space, so
  typing after "ready> " is echoed, and a typed space is still matched as
  typed.
- Anything that cuts speech off drops the output waiting and handed to
  speech: `Input::SpeechCancelled` (a key), and any step whose effects stop
  speech or speak an interrupting utterance (`reduce` checks every step's
  effects). A focus moving to another node forgets the terminal's output;
  utterances of output carry the terminal's focus validity, so the speech
  pipeline drops them once the focus leaves.
- Verbatim+5 (`ReviewCommand::ToggleReportNewOutput`) toggles "Report new
  output", says "report new output on" or "off", and emits
  `Effect::SettingsChanged`; off drops what is waiting and speaks no
  output, while held typing is still echoed when the terminal shows it.

`tests/terminal.rs` drives these with simulated playback, reaching each
utterance's mark as it starts; `tests/alloc.rs` checks that a terminal
line allocates the same whatever the size of the state.
