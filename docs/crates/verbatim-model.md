# verbatim-model

The normalized accessibility vocabulary (architecture section 3), the one
language every other crate speaks. No I/O; serde derives exist so the same
types travel over the Core-outpost pipe, the control plane, and the flight
recorder unchanged.

Public API:

- `TraceId` — correlates one observed OS event or keypress with everything
  it causes, through outpost, reducer, speech queue, synth, and audio.
  `mint()` allocates process-unique increasing IDs; `namespace(pid)` seeds
  the counter with the process id in the high 32 bits, called once at every
  process's startup so IDs minted in Core and in each outpost never collide
  when they meet in the latency ledger.
- `OutpostId`, `NodeId`, `Pid`, `QueryId` — small identity types. An
  `OutpostId` names one outpost process incarnation and is never reused. A
  `NodeId` is an outpost id plus a number that outpost issued: outposts mint
  ids with `NodeId::new(number)`, leaving the outpost `UNASSIGNED`, and Core
  stamps the real incarnation on everything arriving on that outpost's pipe
  (`assign_outpost` on snapshots, trees, events, and fetch results), so an
  id from a replaced outpost can never name a node in its successor.
- `WindowHandle` and `WindowFacts` — a native window handle as an opaque
  number, and the facts about an event's window its outpost reads with
  local calls: top-level window, root owner, whether it is topmost, and for
  `Windows.UI.Core` windows whether it is under the input thread's active
  window, and whether it is in the system's foreground window. The reducer
  classifies events with these, against its attention record or, for a
  focus, against the foreground window.
- `Backend` — `Uia` or `Msaa`; which client stack sourced a node or event.
  Diagnostics only above the outpost.
- `Role`, `State`, `StateSet` — the role vocabulary (window, dialog, menu
  item, button, check box, slider, and so on, with a counterpart for
  every role both backends' NVDA tables map that Verbatim speaks), the
  states (including protected, required, invalid entry, and checkable,
  but no "default", which NVDA does not have), and a bitmask state set with
  `contains`, `insert`, `remove`, `with`, and ordered `iter`. Both enums are
  non-exhaustive so later milestones can grow them without breaking
  matches.
- `NodeSnapshot` — everything needed to announce one node: id, backend,
  role, optional name and value, states, plus a `NodeDetails` group of
  optional properties (description, keyboard shortcut, position in set,
  set size, level, bounding `Rect`) that backends fill as they learn to
  fetch each one; everything in it defaults to "not reported", and its
  serde default keeps snapshots recorded before it existed deserializing
  unchanged.
- `TreeNode` — a `NodeSnapshot` plus its children in tree order: the
  shared vocabulary for a walked tree, carried unchanged by the outpost
  protocol's `DumpTree` reply and the control protocol's `DumpTree` reply,
  so a tree dump travels from the outpost through Core to
  `verbatim-inspect` without translation.
- `NormalizedEvent` — `FocusChanged` (carrying a full snapshot, a
  `foreground` flag set when the focus is a window that just became the
  system's foreground window, the node's ancestor chain, outermost first,
  with `ancestors_unknown` set when the outpost could not read it in time,
  and — for selection containers —
  the container's selected child, both gathered by the outpost on a query
  worker before emitting: deadline-guarded, degrading to empty on failure,
  so enrichment never blocks or loses a focus announcement),
  `PropertyChanged` (name, value, description, or the complete new
  `States` set, with a `child_count` the outpost reads only for a state
  change that newly expands the focus, a Win32 tree view item),
  `ValueChanged`, `ProgressChanged` (a visible progress bar's value
  changed, focused or not, with its snapshot, whose location the reducer
  remembers the last indication by), `SelectionChanged` (a node was selected within its
  container, carrying its snapshot), `ControlledSelection` (a node was
  selected inside an element the focus controls through UIA's
  ControllerFor relation, carrying the controlling focus's id and the
  node's snapshot), and `Notification` (UIA's
  app-initiated announcement channel, carrying a `Notification` payload of
  `NotificationKind`, `NotificationProcessing`, and optional display string
  and activity id). The last two are announced by the reducer since M3:
  selection under the focused selection container speaks the newly
  selected item, and notifications speak their display string at a
  priority chosen by the processing hint ([verbatim-core](verbatim-core.md)).
- `Input` and `Effect` — the reducer's contract. Inputs are events (each
  carrying its source pid, backend, optional `WindowFacts`, and the
  observation time, which is for the latency record only), fetch
  completions (echoing their `QueryKind`), `OutpostEnded` (an outpost
  incarnation ended, so its node ids are dead), timer ticks, and `Command`
  (a review or object-navigation gesture carrying a `ReviewCommand` and a
  press-repeat count, roadmap M3). Effects are `Speak`, `StopSpeech`
  (cancel current and queued speech), `DropExpiredSpeech` (the focus has
  changed: carrying a `FocusNow`, it asks the speech pipeline to drop
  focus speech whose `FocusValidity` no longer holds, see below),
  `Fetch` (a `Query` naming the node, whose outpost is the one asked, and
  a `QueryKind`: the navigation directions parent, next/previous sibling,
  first child, with a `NoNeighbor` `FetchResult` for a genuine tree edge and
  `Gone` for a node that could no longer be re-acquired — the outpost
  never conflates the two), `PlayEarcon`
  (an `Earcon` names an event reported at once, outside the speech queue,
  semantically: an application not responding, start, exit, an error,
  browse and focus mode, suggestions opened and closed, and progress with
  its percentage; the active theme decides whether it plays a sound, is
  spoken, both, or neither, see "Themes" below), `Activate` (invoke or default-action a
  node), and `CopyToClipboard` (routed through the shell's shared clipboard
  helper, so the reducer never touches the clipboard); menu and quit
  concerns never appear here. Milestone M4 adds the text protocol's
  inputs and effects, described under "The text protocol" below.
- `ActionName` — the name of the action an activation performed, carried
  in `Input::ActivationCompleted`: `Invoke` (UIA, spoken "invoke") or
  `Named` (an application's own name for a default action, spoken as is).
- `ReviewCommand` — the model-level review and object-navigation vocabulary
  (report object, parent, siblings, first child, to-focus, activate, and
  the review-cursor line/word/character motions; since M4 also previous
  and next page, the selection's start and end, say-all from the review
  cursor or the caret, the start marker and select then copy, the follow
  caret and typing echo toggles, and the caret's and review cursor's
  locations; and report focus) the keyboard layer's scripts map onto, so the reducer never
  depends on input-crate types.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `UtteranceSource`,
  `SpeechPriority` — structured speech per decision D12. Segments are
  semantic spans: literal text, `Label`, `Value`, `Description`, role and
  state tokens (including `NegatedState` for announcements like "not
  checked"), `SpelledCapital` (an uppercase letter spelled out, which a
  theme speaks at a raised pitch), `Position` (a "2 of 5" pair), `Level`,
  `Message` (a fixed
  reader message the reducer names — a navigation edge, for instance —
  rather than a property of any node, so it can say something without
  pre-flattening text), and, since M4, `Character` (one character spoken
  on its own, by its name from the character table, "comma", or raised in
  pitch when a capital), `CharacterDescription` ("Alfa" for a),
  `Mark` (a `SpeechMark`, an index mark reported back when playback
  reaches it), and `Phrase` (a reader message with values in it: `Selected`
  and `Unselected` with a `SelectionText` that is text, one character, or a
  count of characters; `Positioned` with screen coordinates; and the new
  values of the typing echo toggles, and `SkippedLines` with a count, for
  terminal output too much to read, or `SkippedUncountedLines`, "skipped
  lines", when the count is not known; and `Items` with a count, "52
  items", for a tree view item just expanded), and `Format` (a `TextFormat`: a
  spelling or grammar error starting or ending, bold, italic, or underline
  starting or ending, a font name, size, color, or background color as
  the application words it, the background after a color as "on light
  grey", strikethrough or an underline of a kind starting, changing, or
  ending (a `LineStyle`), a list item's bullet (a `BulletStyle`), or a
  link starting or ending, spoken as formatting changes). The pure reducer never touches localization; spans
  become words at the speech pipeline's presentation stage. An utterance optionally carries an
  `UtteranceSource` — the described node's role and screen rectangle — so
  M11 presentation themes can key earcons off the role and pan audio by
  position without a pipeline change. Focus speech also carries a
  `validity`, which is `None` on every other utterance. `say_all` marks
  an utterance say-all reads, so the theme's "play sounds during say all"
  setting applies to it; it is `false` on every other utterance.
- `FocusValidity` and `FocusNow` — what focus speech is about, for
  dropping it once the focus has moved on (`docs/nvda/speech.md`,
  "Cancellation"). A `FocusValidity` names the node the speech announces
  and whether that node was the focus when the speech was made
  (`had_focus`); a `FocusNow` names the focus, its ancestors, and the
  foreground window's node when known. `FocusValidity::holds(now)` is true
  when the node never had the focus (an entered container), or is the
  focus, an ancestor of the focus, or the foreground window.
- `UtteranceId` and `UtteranceEnding` (decision D17). An `UtteranceId`
  names one utterance from the moment the speech pipeline accepts it until
  its single ending and is never reused within a process, unlike a
  `TraceId`, which names the event behind speech and can be shared by
  several utterances. `UtteranceEnding` is `Completed` (the device played
  all of its audio; one with no audio completes when the audio before it
  has played), `Cancelled` (cut off or dropped before all of it was
  heard), or `Failed` with a reason.
- `ReaderSettings`, `TypingEcho`, `SayAllUnit` (milestone M4) — the
  settings the reducer reads, with NVDA's defaults: speak typed characters
  (only in edit controls) and words (off), each off, only in edit controls, or always;
  the review cursor following the caret (on); say-all reading by sentence
  where possible, by paragraph, or by line (sentence); keeping the display
  on during say-all (on); speaking terminal passwords (off); and, for M4
  item 9, reporting new terminal output (on, toggled with Verbatim+5 by
  `ReviewCommand::ToggleReportNewOutput`, spoken as
  `Message::ReportNewOutputOn` and `Off`) and the flood policy's two
  limits, "Lines spoken in full" and "Last lines to speak" (30 each,
  `DEFAULT_TERMINAL_LINES`), read through `full_lines` and `last_lines`,
  which keep them between 1 and `MAX_TERMINAL_LINES` (100);
  `terminal_read_lines` is how many of a change's newest lines an outpost
  reads. `TypingEcho::next` is the toggle key's cycle. `verbatim-config` stores
  them; the shell hands them to the reducer as `Input::Settings`.
- The theme model, described under "Themes" below: `Indication`,
  `IndicationCategory`, `Theme`, `IndicationSetting`, `Presentation`,
  `SoundSource`, `Tone`, `VoiceStyle`, `ThemeOptions`, `ThemeProblem`,
  `Fetches`, and `progress_frequency`. `Role::ALL` lists every role, the
  roles of the catalogue; `Role` and `State` are ordered, so indications
  are.
- `GestureId` — normalized gesture identifiers, NVDA's scheme.
- `CallKind` and `CallCounts` — how many cross-process calls a piece of
  work made, by kind: UIA calls, MSAA calls, and window messages
  (`docs/performance.md`). `CallCounts::record` counts one call,
  saturating rather than overflowing; `total` and `is_empty` read it, and
  counts add. The model only names the counts: `verbatim-uia` and
  `verbatim-ia2` count, the outpost sends them, and the latency ledger and
  the control plane carry them.

## The text protocol (milestone M4)

How an outpost sends text to Core, and how Core asks an outpost to read
text, wait for the caret, select, or move the caret
(`phase6-design.md`, "M4: text, editing, and terminals" and
"Internationalization in the text model"). The types are in `text.rs`;
everything is serde-serializable like the rest of the model, so it travels
the Core-outpost pipe and the flight recorder unchanged. This section is
the contract the Windows side implements.

### Positions

- Core never does arithmetic on provider positions. A `TextAnchor` is an
  opaque number an outpost mints for a position in one node's text (a UIA
  text range's start, an edit control's UTF-16 offset). A `TextPosition`
  is an anchor plus `offset`, a byte offset into the UTF-8 text of the
  chunk that started at that anchor, as the outpost sent it. The outpost
  resolves a position by reading forward from the anchor and converting
  the byte count into its own units; the offset is always a character
  boundary of that text.
- A `TextPoint` is where a request starts: `Caret`, `SelectionStart`,
  `SelectionEnd` (just past the last selected character), `Start`, `End`
  (after the last character, so a line read there is the last line), or
  `At(TextPosition)`. A node with text but no caret answers `Caret` from
  its start, as NVDA's object review falls back to the first position;
  with nothing selected, both selection points are the caret.
- Anchors are kept by the outpost. `SrState::held_anchors()` in
  `verbatim-core` lists every anchor Core holds, by outpost, alongside
  `held_nodes()`; the outpost keeps those, and may forget any other anchor
  once it has minted 64 newer ones for the node. A request naming a
  forgotten anchor is answered `AnchorLost`, and Core starts again from the
  caret. Anchors die with the outpost incarnation, as node ids do.

### Chunks

- A `TextChunk` is one unit of text: `unit` (`TextUnit`), `text` (UTF-8,
  line breaks included as the provider gives them), `start` (an anchor at
  its start), `offset` (the byte offset of the point the request was about:
  the caret in a caret report, the point reached in a read), `languages`
  (`LanguageRun`s: byte ranges of the text with a BCP 47 tag, in order and
  not overlapping; empty when the provider reports none), `first` and
  `last` (the chunk is known to be the document's first or last unit of its
  kind; false when not, or when the outpost cannot tell cheaply, and Core
  then asks), `truncated`, and `formats` (milestone M4 item 7): the
  formatting of the text, stretch by stretch, each a `FormatRun` (a byte
  range of the text on character boundaries and its `TextAttributes`), in
  order and not overlapping; empty when none was read. `TextAttributes`
  holds whether the text is a spelling or a grammar error, its font name,
  size, color, and background color as spoken ("Calibri", "11.0 pt",
  "dark red"), whether it is bold, italic, or underlined, the kind of its
  underline and of its strikethrough (`LineStyle`, UIA's text decoration
  line styles: none, single, double, wavy, and the rest), its list item's
  bullet (`BulletStyle`: none, round or square, hollow or filled, a dash,
  or another), and whether it is a link, each `None` (false for the
  errors and the link) when the provider does not expose it or its
  indication is off. The outpost sends
  formatting with a caret report after a focus (the line) and with a caret
  key's answer (the character, word, or line spoken), at most 64
  stretches per chunk, and with nothing else yet.
- A chunk's text is at most `MAX_CHUNK_BYTES` (64 KB), cut between whole
  characters (grapheme clusters), so one unit can never grow Core's state without bound.
- `TextUnit` is `Character` (a grapheme cluster, whatever the provider's
  own character unit says), `Word` (the provider's word), `Line` (the
  provider's line, soft-wrapped lines included; Core never splits text on
  line breaks to find lines), `Sentence`, `Paragraph`, `Page`, and
  `Document` (only for moving to the ends, never read as a chunk). In a
  terminal, paragraph, page, and document must not be read: line is the
  largest unit.

### Events the outpost sends

- `NormalizedEvent::CaretMoved { node_id, caret: CaretReport }`: the caret
  or selection moved in a node with text. A `CaretReport` is the line at
  the caret, with its `offset` at the caret, and the `Selection` (a start
  and an end `TextPosition`), `None` when nothing is selected. Send one
  when a node with text gains the focus, as soon after the focus event as
  possible, and on every later caret or selection change in the focus,
  coalesced so a burst sends only the latest. It keeps Core's copy of the
  caret current (Backspace knows what it deleted, the review cursor
  follows the caret). The first one after a focus ends the focus
  announcement with the selected text or the caret's line, in place of the
  value the announcement left out; any later one speaks nothing by itself.
- `NormalizedEvent::NoText { node_id }`: sent in place of that first
  `CaretMoved` when a focus whose role may have text (edit field, document,
  terminal) has no text the outpost can read, or its caret could not be
  read. Core then speaks the focus's value, as for any object without text.
- `NormalizedEvent::TextChanged { node_id }`: the node's text changed.
  Characters typed into a terminal wait for this before Core echoes them.
- `NormalizedEvent::TerminalOutput { node_id, output }` (M4 item 9): sent
  for a focused terminal in place of `TextChanged`, and only when its text
  really changed. A `TerminalOutput` (`terminal.rs`) is what the outpost's
  diff found, in the order it is spoken: `changed`, the last line read
  changed in place (a `LineChange`: the `text` to speak, the whole `line`
  as it now is, whether it was `appended` to, so `text` is exactly the
  characters added, and, for a line that grew, how many `uncertain` bytes
  of white space at the start of `text` the line may already have had,
  its padding or its own trailing spaces); `head`, when lines went by
  unread, the first new lines before them, so a flood's start is spoken in
  full, empty otherwise (`serde(default)`); `skipped`, lines that went by unread (`Skipped::Count`
  or `Skipped::Uncounted` when the scrollback overflowed past the anchor
  and the count is lost; `plus` adds two); and `lines`, the newest lines
  without padding, an empty string for a blank line, each at most
  `MAX_TERMINAL_LINE_BYTES` (4 KB). Core echoes typing it held when the
  terminal shows it at the end of the line, and speaks the rest by the
  flood policy ([verbatim-core](verbatim-core.md), "Terminals").
- `NormalizedEvent::ActiveTextPositionChanged { node_id, position }`: the
  place being read in a text focus moved without its caret (UIA's active
  text position changed event: the application scrolled to a place, such
  as an in-page link's target); `position` is a `TextPosition` at the
  start of the text now active, on an anchor the outpost minted for it.
  Core speaks nothing for it: it is browse mode's (milestone M6), which
  moves its own caret there, and outside browse mode NVDA does nothing
  with it either.

### Requests and replies

Core emits `Effect::Text(TextRequest { query_id, node_id, op })`; the
outpost owning `node_id` answers with `Input::TextCompleted { trace_id,
query_id, reply }`. A newer request for the same purpose supersedes an
older one, whose answer Core then drops. The operations (`TextOp`):

- `AwaitCaret(CaretWatch)`: Core has just passed a caret key to the
  application. Wait for evidence, then answer `TextReply::Caret` with a
  `CaretReply`. Evidence is any of: a caret event from the application;
  the caret no longer where it was when the key was pressed (Core's
  `since`, where Core last knew it, `None` when it did not, unless the
  outpost itself reported a caret from a read that finished before
  `pressed_at_ms`, the key's time on the clock of `observed_at_ms`, in
  which case its newest such report; a caret read at or after that time
  may already show the key's effect); the text of `unit` at the caret differing from `compare` (the
  character or word at the caret before a Delete); the selection no longer
  `previous_selection`. Wait up to 100 milliseconds for
  `CaretWait::Standard` and 300 for `CaretWait::Extended` (terminals), and
  answer when the wait runs out too, with `moved` false. The reply carries
  the caret as it now is, with `read_at_ms`, when the outpost read it on
  the clock of `observed_at_ms` (0 when unknown, which Core compares with
  a later key's `pressed_at_ms`), the requested `unit` at the caret as a chunk
  (`None` when the unit is `Line`, which the caret's line already is, or
  when the provider does not have the unit, in which case Core speaks the
  line), and `selection_changes` when the watch carried a
  `previous_selection` (a `PreviousSelection`, collapsed at the caret when
  nothing was selected). Each `SelectionChange` is `selected` (true for
  newly selected text, false for text no longer selected), its `text` cut
  to `MAX_SELECTION_TEXT_BYTES` (4 KB), and the full count of its
  `characters` (grapheme clusters). With the old selection from
  old start to old end and the new from new start to new end, compare
  endpoints: when the two neither overlap nor touch, the old text is
  unselected and then the new text selected; otherwise, first the start
  side (text between the new and the old start is selected when the start
  moved back, unselected when it moved forward), then the end side (text
  between the old and the new end is selected when the end moved forward,
  unselected when it moved back). Leave out empty changes.
- `Read(TextRead { at, movement, unit })`: start at `at`, move by
  `movement` (a `TextMovement`: a unit and a signed count) when there is
  one, landing on that unit's start, then read the `unit` containing the
  point reached. Answer `TextReply::Read { moved, chunk }`, where `moved`
  is how far the movement really went: less than asked at the document's
  ends, and zero when it could not move at all. Movement never wraps. The
  chunk's `offset` is the point reached. A unit the provider does not have
  is answered `UnsupportedUnit(unit)`, never approximated. For `Sentence`:
  a provider with no sentence unit (UIA) answers `UnsupportedUnit`, and
  Core reads by line; a provider whose text NVDA splits into sentences
  itself (the offset-based edit controls) answers with the paragraph
  containing the point (a chunk whose `unit` is `Paragraph`), which Core
  splits by Unicode's sentence rules, and is moved by `Paragraph` for the
  next read; a provider with a sentence unit of its own answers with the
  sentence.
- `ReadAhead(TextReadAhead { at, movement, unit, count })`: say-all's
  read. The first unit is read as `Read` reads it; then each next unit of
  the same kind (moved by one from the last one's start), up to `count`
  units in all (at most `MAX_READ_AHEAD`, 32), stopping early once the
  text read reaches `MAX_READ_AHEAD_TEXT` (32 K UTF-16 code units, as the
  providers count text). Answer `TextReply::Chunks { moved, chunks }`:
  `moved` as for `Read`, and the chunks in order, at least one, every one
  after the first with its `offset` at its start. When the outpost tried
  to move past the last chunk and could not, that chunk is marked `last`.
  A first unit that cannot be read is answered as `Read` would answer it
  (`UnsupportedUnit`, `AnchorLost`, and so on). Where the provider runs
  remote operations the whole batch is one round trip.
- `ReadRange { start, end }`: the text between two points, whichever comes
  first in the document, answered `Range { text, truncated }`, cut to
  `MAX_RANGE_BYTES` (1 MB). For a copy to the clipboard.
- `Select { start, end }`: select the text between two points and move the
  caret there where the application allows; answer `Done`, or
  `Unsupported` when the text cannot be selected.
- `MoveCaret(point)`: move the caret, as say-all from the caret does as it
  reads; answer `Done` or `Unsupported`. Core does not wait for the answer.
- `Location(point)`: the point's screen position, answered `Location { x,
  y }` in pixels, or `Unsupported`.

Any request can also be answered `NoText` (the node has no text interface
at all; Core then reviews its value or name as flat text, as NVDA's object
review falls back to), `AnchorLost`, `Gone` (the node no longer exists),
or `Unanswered` (the application did not answer in time, or the read
failed).

### Inputs from the shell

- `Input::CaretKey { trace_id, key: CaretKey, pressed_at_ms }`: a caret
  key the keyboard hook observed and passed to the application, with when
  the hook saw it, in milliseconds since the Unix epoch, the clock of an
  event's `observed_at_ms` (0 when unknown). A `CaretKey` is a
  `CaretMotion` (previous or next character, word, line, paragraph, or
  page; start or end of the line; top or bottom; Backspace and
  Control+Backspace; Delete and Control+Delete; Control+A) and whether
  Shift extends the selection. `CaretMotion::unit` is the unit spoken
  after it. `verbatim-input`'s `caret_bindings` maps gestures to these.
- `Input::CharacterTyped { trace_id, text }`: text typed into the focused
  application, one character or several at once (an input method's
  committed composition), a tab as a tab character and Enter as a
  carriage return. The platform side produces it, from the keyboard hook's
  translation of keys to text or from the application's text-edit events.
- `Input::MarkReached { mark }`: playback reached a `SpeechMark` the
  reducer placed, as the speech pipeline's `mark_reached` reports it.
- `Input::SpeechCancelled`: a key press cut speech off outside the reducer
  (the hook's `KeySpeechEffect::Cancel`); say-all stops.
- `Input::Settings(ReaderSettings)`: the reader settings, at startup and
  whenever they change.
- `Input::Fetches(Fetches)`: the details the active theme wants fetched
  (see "Themes" below), at startup and whenever the theme in use changes.

### Effects for the shell

- `Effect::Text(TextRequest)`, routed to the node's outpost.
- `Effect::KeepDisplayOn(bool)`: keep the display on while say-all reads,
  and let it go again; every true is followed by a false.
- `Effect::SettingsChanged(ReaderSettings)`: a toggle key changed a
  setting; save it.

The model also adds `Role::Terminal`, NVDA's terminal role, for Windows
Terminal's text control, the console host, and embedded terminals: the
review cursor keeps cell columns there, caret waits are longer, and typing
waits for the terminal to show it.

## Themes (milestone M4)

One model for verbosity, speech, and sounds (`phase6-design.md`,
"Themes: one model for verbosity, speech, and sounds"). The types are in
`theme.rs`; files and settings are `verbatim-config`'s, and presenting
with a theme is `verbatim-speech`'s.

- `Indication` — one entry of the catalogue of everything Verbatim can
  report, with a stable id (`id`, `from_id`) that theme files use, and a
  `category` (`IndicationCategory`: roles, states, properties, text
  formatting, structure, events). `Indication::catalogue()` lists all 95
  in display order: the 52 roles (`role-link`), the 13 states that are
  spoken (`state-checked`) and the 3 whose absence is
  (`state-not-checked`), the properties (`description`, `shortcut`,
  `position`, `level`), text formatting (`spelling-error`,
  `grammar-error`, `font-name`, `font-size`, `color`,
  `background-color`, `font-attributes` (bold, italic, and underline,
  NVDA's font attributes), `strikethrough`, `underline-style` (the kind of
  underline, which, when on, reports underlining with its kind in place
  of the font attributes' "underlined"), `bullet-style`, `link` (a link
  within text), and `capital`, a capital letter spoken on its own),
  structure (`blank`,
  `skipped-lines`), and events (`app-not-responding`, `start`, `exit`,
  `error`, `browse-mode`, `focus-mode`, `suggestions-opened`,
  `suggestions-closed`, `progress`). A role's or state's id is its variant
  name in kebab case, so variants are never renamed. `of_segment` finds
  the indication a span reports, `None` for content (labels, values,
  text, characters, marks, and messages other than "blank"), which is
  always spoken; `of_earcon` finds an event's.
- `Presentation` — how an indication is reported: `Off`, `Speech`,
  `Sound`, or `SpeechAndSound`; `speaks` and `sounds` read it.
- `IndicationSetting` — one indication in a theme: `report`, `sound` (a
  `SoundSource`, either a WAV file name or a generated `Tone` of a
  frequency and duration), `gain` in percent (100 as recorded, up to
  `MAX_GAIN`, 400), replacement `words`, and the name of a `voice` style.
- `VoiceStyle` — a theme's named change to the voice, relative pitch,
  rate, and volume; only the pitch is applied so far.
- `Theme` — id, name, author, description, version, overall gain, voice
  styles, and the indications where it differs from the default theme.
  `builtin_default()` is the complete built-in default theme, id
  `default`, each indication with `Indication::default_setting`;
  `setting(indication)` resolves an indication, falling back to the
  default theme for anything the theme does not mention (no chains of
  themes); `differs` says whether an indication is "changed";
  `problems` finds what is wrong without reading files (sound alone with
  no sound, an unknown voice style, a gain too high, a sound file name
  with a directory in it); `fetches` gives the reducer's `Fetches`.
- The default theme, matching NVDA's defaults: everything spoken as
  NVDA speaks it; sounds where NVDA plays them by default (browse and
  focus mode, suggestions opened and closed, errors, start and exit, NVDA's
  files in the top-level `sounds` directory), the spelling error sound
  with its words, Verbatim's own cues as tones (an application not
  responding, skipped terminal lines, progress bars rising three octaves
  from 220 Hz, `progress_frequency`), font name, size, color, background
  color, font attributes, and strikethrough off as in NVDA, links in text
  spoken as NVDA reports them by default, the kind of underline and bullet
  style (which NVDA does not report through UIA) off, and a capital raised
  in pitch.
- `ThemeProblem` — what loading a theme found wrong: an unknown
  indication id, sound alone with no sound, a sound missing or unreadable,
  a sound file name that is not a plain name, an unknown voice style, a
  gain too high. A theme with problems still loads.
- `ThemeOptions` — the settings that go with a theme but are not part of
  it: sound volume (0 to 100, relative to speech), sounds during say-all,
  and speaking indications that play a sound (for learning a theme).
- `Fetches` — the details the reducer fetches: description, shortcut,
  position, level, spelling and grammar errors, font name, font size,
  color, background color, font attributes, strikethrough, the kind of
  underline, bullet style, and links, each unless its indication is off. The shell gives them to the reducer as
  `Input::Fetches`.

Implementation note, `GestureId::parse`: splits `source:parts`, lowercases
everything, and sorts the plus-separated parts, exactly like NVDA's
`normalizeGestureIdentifier` — so `kb:Verbatim+V` and `kb:v+verbatim` are
one gesture and binding lookup is order- and case-insensitive.
Deserialization re-parses, so an identifier read from config or the wire is
always normalized.
