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
- `NodeId`, `Pid`, `SnapshotVersion`, `QueryId` — small identity types.
- `Backend` — `Uia` or `Msaa`; which client stack sourced a node or event.
  Diagnostics only above the outpost.
- `Role`, `State`, `StateSet` — the role vocabulary (window, dialog, menu
  item, button, check box, slider, and so on) and a bitmask state set with
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
- `NormalizedEvent` — `FocusChanged` (carrying a full snapshot plus the
  node's ancestor chain, outermost first, and — for selection containers —
  the container's selected child, both gathered by the outpost on a query
  worker before emitting: deadline-guarded, degrading to empty on failure,
  so enrichment never blocks or loses a focus announcement),
  `PropertyChanged` (name, value, or the complete new `States` set),
  `ValueChanged`, `SelectionChanged` (a node was selected within its
  container, carrying its snapshot), and `Notification` (UIA's
  app-initiated announcement channel, carrying a `Notification` payload of
  `NotificationKind`, `NotificationProcessing`, and optional display string
  and activity id). The last two are emitted by outposts but deliberately
  not yet announced: the reducer's wildcard arm drops them until M3's
  selection-announcement policy work lands.
- `Input` and `Effect` — the reducer's contract. Inputs are events, fetch
  completions, timer ticks, and `Command` (a review or object-navigation
  gesture carrying a `ReviewCommand` and a press-repeat count, roadmap M3).
  Effects are `Speak`, `StopSpeech`, `Fetch` (whose `QueryKind` now also
  names the navigation directions parent, next/previous sibling, first
  child, with a `NoNeighbor` `FetchResult` for a genuine tree edge and
  `Gone` for a node that could no longer be re-acquired — the outpost
  never conflates the two), `PlayEarcon`
  (an `Earcon` names a sound semantically — `AppNotResponding` first — and
  themes decide what it sounds like), `Activate` (invoke or default-action a
  node), and `CopyToClipboard` (routed through the shell's shared clipboard
  helper, so the reducer never touches the clipboard); menu and quit
  concerns never appear here.
- `ReviewCommand` — the model-level review and object-navigation vocabulary
  (report object, parent, siblings, first child, to-focus, activate, and
  the review-cursor line/word/character motions) the keyboard layer's
  scripts map onto, so the reducer never depends on input-crate types.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `UtteranceSource`,
  `SpeechPriority` — structured speech per decision D12. Segments are
  semantic spans: literal text, `Label`, `Value`, `Description`, role and
  state tokens (including `NegatedState` for announcements like "not
  checked"), `Position` (a "2 of 5" pair), `Level`, and `Message` (a fixed
  reader message the reducer names — a navigation edge, for instance —
  rather than a property of any node, so it can say something without
  pre-flattening text). The pure reducer never touches localization; spans
  become words at the speech pipeline's presentation stage. An utterance optionally carries an
  `UtteranceSource` — the described node's role and screen rectangle — so
  M11 presentation themes can key earcons off the role and pan audio by
  position without a pipeline change.
- `GestureId` — normalized gesture identifiers, NVDA's scheme.

Implementation note, `GestureId::parse`: splits `source:parts`, lowercases
everything, and sorts the plus-separated parts, exactly like NVDA's
`normalizeGestureIdentifier` — so `kb:Verbatim+V` and `kb:v+verbatim` are
one gesture and binding lookup is order- and case-insensitive.
Deserialization re-parses, so an identifier read from config or the wire is
always normalized.
