# verbatim-model

The normalized accessibility vocabulary (architecture section 3), the one
language every other crate speaks. No I/O; serde derives exist so the same
types travel over the Core-outpost pipe, the control plane, and the flight
recorder unchanged. What the reducer does with each type is in
[verbatim-core](verbatim-core.md).

## Public API

- `TraceId`: correlates one observed OS event or keypress with everything
  it causes, through outpost, reducer, speech queue, synth, and audio.
  `mint()` allocates process-unique increasing ids; `namespace(pid)`,
  called once at every process's startup, seeds the counter with the
  process id in the high 32 bits, so ids minted in Core and in each
  outpost never collide in the latency ledger.
- `OutpostId`, `NodeId`, `Pid`, `QueryId`: small identity types. An
  `OutpostId` names one outpost process incarnation and is never reused. A
  `NodeId` is an outpost id plus a number that outpost issued: outposts
  mint ids with `NodeId::new(number)`, leaving the outpost `UNASSIGNED`,
  and Core stamps the real incarnation on everything arriving on that
  outpost's pipe (`assign_outpost` on snapshots, trees, events, and fetch
  results), so an id from a replaced outpost never names a node in its
  successor.
- `HIDDEN_FRAME_WINDOW_PROP`: the window property
  [verbatim-gui](verbatim-gui.md) stamps on Core's hidden main frame
  (decision D9), which every outpost checks, with the window's owning
  process, before emitting a `FocusChanged`, so the frame is never
  announced.
- `WindowHandle` and `WindowFacts`: a native window handle as an opaque
  number, and the facts about an event's window its outpost reads with
  local calls: top-level window, root owner, whether it is topmost,
  whether a `Windows.UI.Core` window is under the input thread's active
  window, whether it is in the system's foreground window, and its class;
  on a focus change also the application's name as NVDA names an
  application module (`explorer`) and whether Alt was held (`alt_held`).
  The reducer classifies events with these
  (`docs/architecture.md`, section 2, "Attention and acceptance (D14)").
- `ForegroundWindow` and `HookForeground`: the window the focus listener
  reported in front (its handle, the thread its event named, whether it is
  a console host window, its process's integrity level and Verbatim's
  own), carried in `Input::ForegroundWindow`, and what Core decides from it
  for the keyboard hook: whether keys may be held back for that window and
  which thread's keyboard layout translates its keys (architecture
  section 5).
- `Backend`: `Uia` or `Msaa`, which client stack sourced a node or event;
  diagnostics only above the outpost.
- `Role`, `State`, `StateSet`: the role vocabulary, with a counterpart for
  every role both backends' NVDA tables map that Verbatim speaks
  (`Role::ALL` lists them; `Role::Terminal` is NVDA's terminal role), the
  states (no "default", which NVDA does not have), and a bitmask state set
  with `contains`, `insert`, `remove`, `with`, and ordered `iter`. Both
  enums are non-exhaustive and ordered.
- `NodeSnapshot`, `NodeDetails`, `Rect`: everything needed to announce one
  node: id, backend, role, optional name and value, states, and a
  `NodeDetails` group of optional properties: description, keyboard
  shortcut, position in set, set size, level, bounding `Rect`,
  `top_level_client` (the MSAA client area of a top-level window with a
  system menu, presented as that window), `class_name` and
  `parent_automation_id` (only on a UIA focus's snapshot, for Core's rules
  for an application's elements), `row` and `column` (a grid item's,
  read only for a node read alone), and `window` (the `WindowHandle` of
  the window the node is, carried only by a plain window, a UI Automation
  element that is its window's own, and an MSAA window or client object,
  never a nearest window, and left out of the message when absent).
  Everything in `NodeDetails` defaults
  to "not reported", and its serde default keeps older recordings
  deserializing.
- `TreeNode`: a `NodeSnapshot` plus its children in tree order, carried
  unchanged by the outpost protocol's and the control protocol's
  `DumpTree` replies, so a tree dump travels to `verbatim-inspect` without
  translation.
- `ShellItemKind`, `ShellItem`, `ShellCandidate`, `ShellCandidates`: the
  system tray or taskbar surface and one of its buttons (a name and a UIA
  runtime id), for the outpost protocol's `ShellItems` and
  `ShellItemPosition` queries; a `ShellCandidate` is a button with the
  clicked item's runtime id or name, whether it is off screen, and its
  rectangle, and `ShellCandidates` says whether the walk read every
  button.
- `NormalizedEvent`, `PropertyChange`, `ProgressReading`, `Notification`,
  `NotificationKind`, `NotificationProcessing`: the events an outpost
  sends.
  - `FocusChanged`: a full snapshot; `foreground`, set when the focus is a
    window that just became the system's foreground window; the ancestor
    chain, outermost first, with `ancestors_unknown` when the outpost
    could not read it in time; a selection container's selected child;
    and, for a focus that may have text, `text` (see "The text protocol").
    The enrichment is deadline-guarded and degrades to empty, so it never
    blocks or loses a focus.
  - `PropertyChanged` with a `PropertyChange`: name, value, description,
    or the complete new `States` set, with a `child_count` read only for a
    state change that newly expands a Win32 tree view item.
  - `ValueChanged`; `ProgressChanged` with a `ProgressReading` (node, new
    value, whether off screen, location; only what is read, so a property
    not read is never taken for an empty one).
  - `SelectionChanged` (a node selected within its container) and
    `ControlledSelection` (a node selected inside an element the focus
    controls through UIA's ControllerFor relation, with the controlling
    focus's id).
  - `Notification`: UIA's application-initiated announcement, with its
    kind, processing hint, and optional display string and activity id.
  - The text events, under "The text protocol".
- `Input` and `Effect`: the reducer's contract.
  - Inputs are events (each with its source pid, backend, optional
    `WindowFacts`, and the observation time on the run's shared clock in
    microseconds, one clock read for every event kind), fetch completions (with
    their `QueryKind`), `OutpostEnded` (an incarnation ended, so its node
    ids are dead), timer ticks, `Command` (a `ReviewCommand` and a
    press-repeat count), and what the shell's threads learn, since only
    the reducer speaks: `Started`, `Exiting { replaced }`, `ErrorLogged`,
    `ApplicationNotResponding`, `LockKeyToggled` (a `LockKey` and its
    state), `ClockRead`, `ClipboardCopied`, `ShellItemGone`,
    `ShellItemUnclickable`, `PreviewSpeech`, `PreviewEarcon`,
    `MenuOrSwitchEnded`, and `ForegroundWindow`. The text inputs are under
    "The text protocol".
  - Effects are `Speak`; `StopSpeech`; `DropExpiredSpeech` (carrying a
    `FocusNow`); `Fetch` (a `Query` naming the node and a `QueryKind`:
    parent, next or previous sibling, first child, answered by a
    `FetchResult` that tells a tree edge, `NoNeighbor`, from a node that
    could not be re-acquired, `Gone`); `PlayEarcon`; `Activate`;
    `CopyToClipboard` (routed through the shell's clipboard helper, its
    result back as `Input::ClipboardCopied`); and `ReadForegroundFocus`
    (NVDA's fake focus). The text effects are under "The text protocol".
- `Earcon`: an event reported at once, outside the speech queue: an
  application not responding, start, exit, an error, browse and focus
  mode, suggestions opened and closed, and progress with its percentage.
  The theme decides how each is presented.
- `ActionName`: the action an activation performed, in
  `Input::ActivationCompleted`: `Invoke` (spoken "invoke") or `Named` (an
  application's own name, spoken as is).
- `ReviewCommand`: the review and object-navigation vocabulary the
  keyboard layer's scripts map onto, so the reducer never depends on
  input-crate types: report object and focus, parent, siblings, first
  child, to-focus, activate, review-cursor motions by character, word,
  line and page, the selection's ends, say-all, the start marker and
  select then copy, the follow-caret, typing echo and new-output toggles,
  the caret's and review cursor's locations, report status bar, and the
  previous and next command mark.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `UtteranceSource`,
  `SpeechPriority`, `SpeechMark`, `Message`, `Phrase`, `SelectionText`,
  `TextFormat`: structured speech per decision D12. Segments are semantic
  spans: literal text, `Label`, `Value`, `Description`, role and state
  tokens (`NegatedState` for "not checked"), `SpelledCapital`, `Position`
  ("2 of 5"), `Level`, `Row` and `Column`, `Message` (a fixed reader
  message), `Character` (one character spoken on its own),
  `CharacterDescription` ("Alfa"), `Mark` (a `SpeechMark` reported back
  when playback reaches it), `Phrase` (a reader message with values in
  it, such as "hello selected", "skipped 30 lines", or "52 items"), and
  `Format` (a `TextFormat`, a formatting change). The reducer never
  touches localization; spans become words at the speech pipeline's
  presentation stage ([verbatim-speech](verbatim-speech.md)). An
  utterance optionally carries an `UtteranceSource`, the described node's
  role and screen rectangle, for presentation themes; focus speech carries
  a `validity`; `say_all` marks an utterance say-all reads.
- `FocusValidity` and `FocusNow`: what focus speech is about, for dropping
  it once the focus has moved on (`docs/parity/focus-events.md`, "When
  speech is cut off, and cancellation of expired focus speech").
  A `FocusValidity` names the node the speech announces and whether it was
  the focus when the speech was made (`had_focus`); a `FocusNow` names the
  focus, its ancestors, and the foreground window's node when known.
  `FocusValidity::holds(now)` is true when the node never had the focus,
  or is the focus, an ancestor of it, or the foreground window.
- `UtteranceId` and `UtteranceEnding` (decision D17). An `UtteranceId`
  names one utterance from acceptance to its single ending and is never
  reused within a process, unlike a `TraceId`, which several utterances
  can share. `UtteranceEnding` is `Completed`, `Cancelled`, or `Failed`
  with a reason.
- `ReaderSettings`, `TypingEcho`, `SayAllUnit`, `DEFAULT_TERMINAL_LINES`,
  `MAX_TERMINAL_LINES`: the settings the reducer reads, with NVDA's
  defaults: typed character echo (only in edit controls) and word echo
  (off); the review cursor following the caret (on); say-all by sentence,
  paragraph, or line (sentence); keeping the display on during say-all
  (on); speaking terminal passwords (off); reporting new terminal output
  (on); and the flood policy's "Lines spoken in full" and "Last lines to
  speak" (30 each), read through `full_lines` and `last_lines`, which keep
  them between 1 and `MAX_TERMINAL_LINES`. `TypingEcho::next` is the
  toggle key's cycle. [verbatim-config](verbatim-config.md) stores them.
- The theme model, under "Themes".
- `GestureId` and `GestureParseError`: normalized gesture identifiers,
  NVDA's scheme. `GestureId::parse` splits `source:parts`, lowercases
  everything, and sorts the plus-separated parts (`docs/nvda/input.md`,
  "Gestures and mapping"), so `kb:Verbatim+V` and `kb:v+verbatim` are one
  gesture; deserialization re-parses, so an identifier from config or the
  wire is always normalized.
- `CallKind` and `CallCounts`: how many cross-process calls a piece of
  work made, by kind (UIA calls, MSAA calls, window messages), and how
  many of those connected to a window by its handle (`connections`, which
  `total` leaves out). `record` and `record_connection` saturate; `total`
  and `is_empty` read; counts add. The model only names the counts;
  [verbatim-uia](verbatim-uia.md) and [verbatim-ia2](verbatim-ia2.md)
  count them.

## The text protocol

How an outpost sends text to Core, and how Core asks an outpost to read
text, wait for the caret, select, or move the caret. The types are in
`text.rs` and `terminal.rs`; this section is the contract the Windows side
implements. What each answer says is in `docs/parity/text.md`,
"TextInfo-equivalent layer".

### Positions

- Core never does arithmetic on provider positions. A `TextAnchor` is an
  opaque number an outpost mints for a position in one node's text. A
  `TextPosition` is an anchor plus `offset`, a byte offset into the UTF-8
  text of the chunk that started at that anchor, always on a character
  boundary; the outpost resolves it by reading forward from the anchor.
- A `TextPoint` is where a request starts: `Caret`, `SelectionStart`,
  `SelectionEnd` (just past the last selected character), `Start`, `End`
  (after the last character), or `At(TextPosition)`. A node with text but
  no caret answers `Caret` from its start; with nothing selected, both
  selection points are the caret.
- Anchors are kept by the outpost. `HeldAnchors` is the anchors Core
  holds, by outpost, which the shell sends with the held nodes; the
  outpost keeps those and may forget any other once it has minted 64
  newer ones for the node. A request naming a forgotten anchor is
  answered `AnchorLost`. Anchors die with the outpost incarnation.

### Chunks

- A `TextChunk` is one unit of text: `unit`, `text` (UTF-8, line breaks
  as the provider gives them), `start` (an anchor at its start), `offset`
  (the point the request was about), `languages` (`LanguageRun`s, byte
  ranges with a BCP 47 tag), `first` and `last` (known to be the
  document's first or last unit of its kind; false when not or when not
  cheap to tell), `truncated`, and `formats`.
- `formats` are `FormatRun`s, byte ranges with their `TextAttributes`:
  spelling and grammar errors, font name, size, color, and background
  color as spoken, bold, italic, underline, the kind of underline and of
  strikethrough (`LineStyle`), a list item's bullet (`BulletStyle`), and
  whether it is a link, each `None` or false when the provider does not
  expose it or its indication is off. The outpost sends formatting only
  with a caret report after a focus and with a caret key's answer, at
  most 64 runs per chunk.
- A chunk's text is at most `MAX_CHUNK_BYTES` (64 KB), cut between
  grapheme clusters, so one unit can never grow Core's state without
  bound.
- `TextUnit` is `Character` (a grapheme cluster), `Word` (the provider's),
  `Line` (the provider's, soft-wrapped lines included; Core never splits
  text on line breaks to find lines), `Sentence`, `Paragraph`, `Page`, and
  `Document` (only for moving to the ends). In a terminal, line is the
  largest unit read.

### Events the outpost sends

- `FocusChanged`'s `text`, a `FocusText`, read while the outpost handled
  the focus so the caret's line comes in the focus's own message:
  `Caret { caret, selected_text }`, `NoText` (Core speaks the value), or
  `NotRead` (Core speaks neither).
- `CaretMoved { node_id, caret, selection_changes, content_changed,
  new_selection }`: the caret or selection moved in a node with text. A
  `CaretReport` is the line at the caret, with its `offset` at the caret,
  and the `Selection` (`None` when nothing is selected), whose start is in
  that line or in `selection_line`, the earlier line it starts in. Sent on
  every caret or selection change in the focus, coalesced so a burst
  sends only the latest with the selection changes of every report it
  replaces. `selection_changes` is how the selection changed from the one
  in the outpost's last report for the node, by the rule of `CaretReply`'s;
  after Verbatim selected text or moved the caret, the comparison is with
  that. A key's own caret event is taken by its watch.
- `TextChanged { node_id }`: the node's text changed; Core says nothing of
  it.
- `TerminalSnapshot { node_id, snapshot }`: one read of a focused
  terminal, for Core's terminal model ([verbatim-core](verbatim-core.md),
  "Core's terminal model"), sent in place of `TextChanged` unless
  identical to the last read sent. A `TerminalSnapshot` is whole by
  itself: its `text` from the read's start to the document's end, the
  `screen`, the document's `first_row` and the `caret_line`, each a
  `TerminalText`; the flags `from_start`, `shrank`, `at_document_start`
  and `alternate`; `caret_rows_to_end`, `caret_column`, and `above`;
  `started_ms` and `ended_ms` on the shared clock (`ended_ms` 0 when not
  known); `waited`; `read`, its number among the terminal's reads from 1;
  the screen's `colours` when taken, rows of `TerminalColourRun`; and the
  `caret`. A `TerminalText` is text without the spaces that pad its lines
  and each line's padded length as runs (`lengths`): `unpadded` makes one
  and `padded` gives the text back exactly. The screen is sent beside the
  text, read as the provider's first visible range in a call of its own,
  so it is where the user scrolled the view.
- `ActiveTextPositionChanged { node_id, position }`: the place being read
  moved without the caret (UIA's active text position changed event).
  Core speaks nothing for it; it is browse mode's.

### Requests and replies

Core emits `Effect::Text(TextRequest { query_id, node_id, op })`; the
outpost owning `node_id` answers with `Input::TextCompleted { trace_id,
query_id, reply }` (a `TextReply`). A newer request for the same purpose
supersedes an older one, whose answer Core drops. The operations
(`TextOp`):

- `AwaitCaret(CaretWatch)`: Core has just passed a caret key. Watch for
  evidence without waiting for it, and answer `TextReply::Caret` with a
  `CaretReply` once it comes (`moved` true). Evidence is a caret event; the
  caret no longer where it was when the key was pressed (the outpost's
  newest caret read before `pressed_at_ms`, else Core's `since`); the text
  of `unit` at the caret differing from `compare` (for Delete); for a
  watch marked `deletes`, the caret's line differing from the line the
  outpost last reported before the key; or the selection no longer the
  one the outpost kept. The outpost checks when the request arrives and
  whenever the application reports a caret, text, or selection change. A
  watch marked `landing` is answered with `same_line`; in a terminal it
  may carry `removed`, text removed from a typed line that had wrapped. A
  watch that ends with no evidence (replaced by the next watch, the focus
  moving, or a bound on its age passing) is answered
  `TextReply::WatchEnded`. The reply carries the caret, `read_at_ms` in
  milliseconds on the clock of `observed_at_us` (0 when unknown), the
  requested `unit` at the caret (`None` for `Line` or a unit the provider
  lacks), and `selection_changes`.
- Selection changes: each `SelectionChange` is `selected` or not, its
  `text` cut to `MAX_SELECTION_TEXT_BYTES` (4 KB), and its full count of
  `characters`. When the old and new selections neither overlap nor
  touch, or either is empty, the old text is unselected then the new
  selected, each whole; otherwise the start side is compared, then the
  end side. Empty changes are left out. When the node's text changed since
  the compared selection was reported, `content_changed` is set, the
  changes carry no text, and `new_selection` carries the whole new
  selection's text.
- `ReadCaret`: the caret before a held caret key
  (`docs/design/key-holding.md`). Answered `TextReply::Caret` with `moved`
  false, the caret's line, selection and `selection_line`, no unit, no
  changes, and `read_at_ms`; the read joins the node's caret reports, and
  the kept selection is left as it was. In a focused terminal it is
  answered with a `TerminalSnapshot`, whose caret answers it.
- `Read(TextRead { at, movement, unit })`: start at `at`, move by
  `movement` (a `TextMovement`: a unit and a signed count), landing on
  that unit's start, then read the `unit` containing the point reached.
  Answered `TextReply::Read { moved, chunk }`, `moved` being how far the
  movement really went; movement never wraps. A unit the provider lacks is
  `UnsupportedUnit(unit)`, never approximated. For `Sentence`, a provider
  with no sentence unit (UIA) answers `UnsupportedUnit`; one whose text
  NVDA splits itself (the offset-based edit controls) answers with the
  paragraph, which Core splits; one with its own sentence unit answers
  with the sentence.
- `ReadAhead(TextReadAhead { at, movement, unit, count })`: say-all's
  read. The first unit as `Read`, then each next unit of the same kind, up
  to `count` (at most `MAX_READ_AHEAD`, 32), stopping once the text
  reaches `MAX_READ_AHEAD_TEXT` (32 K UTF-16 code units). Answered
  `TextReply::Chunks { moved, chunks }`, at least one, the last marked
  `last` when the outpost could not move past it. Where the provider runs
  remote operations the batch is one round trip.
- `ReadRange { start, end, through }`: the text between two points,
  whichever comes first, answered `Range { text, truncated }`, cut to
  `MAX_RANGE_BYTES` (1 MB). With `through` (`TextThrough`, the points just
  past the characters at each end), the text runs through the character
  at the later end, whichever way round: the review copy's range.
- `Select { start, end, through }`: select between two points and move
  the caret there where allowed; answered `Selected(changes)` by the rule
  of a caret reply, or `Unsupported`.
- `MoveCaret(point)`: answered `Done` or `Unsupported`; Core does not wait
  for it.
- `Location(point)`: answered `Location { x, y }` in pixels, or
  `Unsupported`.
- `TerminalHold`, `TerminalRead { hold }`, `TerminalCancel`: a focused
  terminal's on-demand reading (`docs/parity/terminals.md`, "Reading a
  terminal on demand"). Hold: only note changes (`Done`). Read: what the
  terminal says now, then hold or read each change again
  (`TerminalSnapshot`, or `TerminalUnchanged`). Cancel: speech was cut, so
  read to the end and go live again.
- `TerminalReadNow(TerminalScope)`: read at once (`TerminalSnapshot`):
  `Fresh`, before a review command Core answers from its memory; `Whole`,
  the whole document after a lost read; `HistoryBlock { height }`, a first
  read again.
- `TerminalHistory { above, whole }`: the text from `above` rows above the
  next read's start to the document's end, or the whole document,
  answered `TerminalHistory { text, above, read }`.
- `TerminalFind { needle, from, rows }`: a command mark's line sought by
  its first row's text outward from `from` rows above the next read's
  start, at most 20 matches, `rows` rows read from each, answered
  `TerminalFound { matches, read }` with a `TerminalMatch` for each.
- `TerminalColours { from_ms, until_ms }`: every read that begins by the
  latest such `until_ms` takes the screen's colours; when the last read
  ended at or after `from_ms` without them, one is taken at once and sent
  as a live read (`Done`).
- `TerminalShape { height, width }`: the screen's shape for the outpost's
  next reads (`Done`).

Any request can also be answered `NoText` (no text interface; Core reviews
the value or name as flat text), `AnchorLost`, `Gone` (the node no longer
exists), or `Unanswered` (no answer in time, or the read failed).

### Inputs from the shell

- `Input::CaretKey { trace_id, key, pressed_at_ms }`: a caret key the hook
  passed to the application, with when it saw it, in Unix milliseconds
  (0 when unknown). A `CaretKey` is a `CaretMotion` (character, word,
  line, paragraph, sentence, or page; the line's start or end; top or
  bottom; Backspace and Delete, alone or with Control; Control+A; or
  `Other`, any other key that types no text) and whether Shift extends
  the selection; `CaretMotion::unit` is the unit spoken after it.
  [verbatim-input](verbatim-input.md)'s `caret_bindings` maps gestures to
  these.
- `Input::HeldKey`, with the same fields: a caret key the hook held back.
  `Input::HeldKeyWaitEnded { trace_id }` ends a held key's wait.
- `Input::CharacterTyped { trace_id, text, at_ms }`: text typed into the
  focused application, one character or an input method's committed
  composition, a tab as a tab and Enter as a carriage return.
- `Input::ClearingKey { trace_id, at_ms }`: Escape, Control+C, Control+D,
  or Control+Break passed to the focused application.
- `Input::MarkReached { mark }`, `Input::UtteranceCancelled { marks,
  at_ms }`, `Input::UtteranceFailed { marks }`: what the speech pipeline
  reports of the marks the reducer placed.
- `Input::SpeechCancelled { at_ms }`: a key press cut speech off outside
  the reducer.
- `Input::Settings(ReaderSettings)` and `Input::Fetches(Fetches)`: at
  startup and whenever they change.
- `Input::TerminalTimer { trace_id, at_ms }`: the focused terminal's
  quiet-period timer came due.

### Effects for the shell

- `Effect::Text(TextRequest)`, routed to the node's outpost.
- `Effect::PassHeldKey(trace_id)`: pass a held key on.
- `Effect::ForKey(trace_id, effect)`: an effect executed on another key's
  trace.
- `Effect::BoundHeldKeyWait { key, within_ms }`: send
  `Input::HeldKeyWaitEnded` once `within_ms` have passed, a later one
  replacing it.
- `Effect::KeepDisplayOn(bool)`: keep the display on while say-all reads;
  every true is followed by a false.
- `Effect::SettingsChanged(ReaderSettings)`: a toggle key changed a
  setting; save it.
- `Effect::TerminalTimer { due_ms }`: arm the focused terminal's
  quiet-period timer, replacing any armed before, or disarm it (`None`).

## Themes

One model for verbosity, speech, and sounds: a theme says, for every kind
of thing Verbatim can report, whether it is reported and how. The reducer
puts every fact into the utterance as a typed span and consults the theme
only to skip fetching what is off; the presentation stage turns each span
into words, a sound, both, or nothing. The types are in `theme.rs`; files
and settings are [verbatim-config](verbatim-config.md)'s, and presenting
is [verbatim-speech](verbatim-speech.md)'s.

- `Indication`: one entry of the catalogue of everything Verbatim can
  report, with a stable id (`id`, `from_id`) that theme files use, and a
  `category` (`IndicationCategory`: roles, states, properties, text
  formatting, structure, events). `Indication::catalogue()` lists all 96
  in display order: the 52 roles (`role-link`), the 13 spoken states
  (`state-checked`) and the 3 spoken by their absence
  (`state-not-checked`), the properties (`description`, `shortcut`,
  `position`, `level`, `cell-coordinates`), text formatting
  (`spelling-error`, `grammar-error`, `font-name`, `font-size`, `color`,
  `background-color`, `font-attributes`, `strikethrough`,
  `underline-style`, `bullet-style`, `link`, `capital`), structure
  (`blank`, `skipped-lines`), and events (`app-not-responding`, `start`,
  `exit`, `error`, `browse-mode`, `focus-mode`, `suggestions-opened`,
  `suggestions-closed`, `progress`). A role's or state's id is its variant
  name in kebab case, so variants are never renamed. `of_segment` finds
  the indication a span reports, `None` for content, which is always
  spoken; `of_earcon` finds an event's.
- `Presentation`: `Off`, `Speech`, `Sound`, or `SpeechAndSound`; `speaks`
  and `sounds` read it.
- `IndicationSetting`: `report`, `sound` (a `SoundSource`, a WAV file name
  or a generated `Tone` of a frequency and duration), `gain` in percent
  (`DEFAULT_GAIN` 100, up to `MAX_GAIN` 400), replacement `words`, and a
  `voice` style's name.
- `VoiceStyle`: a named change of relative pitch, rate, and volume; only
  the pitch is applied.
- `Theme`: id, name, author, description, version, overall gain, voice
  styles, and the indications where it differs from the default.
  `builtin_default()` is the built-in default theme, id `default`, each
  indication with `Indication::default_setting`; `setting(indication)`
  falls back to the default for anything not mentioned (no chains);
  `differs` says whether an indication is changed; `problems` finds what
  is wrong without reading files; `fetches` gives the reducer's `Fetches`.
- The default theme matches NVDA's defaults: everything spoken as NVDA
  speaks it; sounds where NVDA plays them by default (NVDA's files in the
  top-level `sounds` directory); Verbatim's own cues as tones (an
  application not responding, skipped terminal lines, and progress bars
  rising three octaves from 220 Hz, `progress_frequency`); font name,
  size, color, background color, font attributes, and strikethrough off;
  links in text spoken; the kind of underline and bullet style off; and a
  capital raised in pitch.
- `ThemeProblem` and `is_plain_file_name`: what loading a theme found
  wrong (an unknown indication id, sound alone with no sound, a sound
  missing or unreadable, a sound file name that is not a plain name, an
  unknown voice style, a gain too high); a theme with problems still
  loads.
- `ThemeOptions`: the settings that go with a theme: sound volume (0 to
  100, relative to speech), sounds during say-all, and speaking
  indications that play a sound.
- `Fetches`: the details the reducer fetches (description, shortcut,
  position, level, a cell's row and column, and each text attribute),
  each unless its indication is off.
