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
  window, and whether it is in the system's foreground window. The reducer classifies events against its attention record with
  these.
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
  `PropertyChanged` (name, value, or the complete new `States` set),
  `ValueChanged`, `SelectionChanged` (a node was selected within its
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
  (an `Earcon` names a sound semantically — `AppNotResponding` first — and
  themes decide what it sounds like), `Activate` (invoke or default-action a
  node), and `CopyToClipboard` (routed through the shell's shared clipboard
  helper, so the reducer never touches the clipboard); menu and quit
  concerns never appear here.
- `ActionName` — the name of the action an activation performed, carried
  in `Input::ActivationCompleted`: `Invoke` (UIA, spoken "invoke") or
  `Named` (an application's own name for a default action, spoken as is).
- `ReviewCommand` — the model-level review and object-navigation vocabulary
  (report object, parent, siblings, first child, to-focus, activate, and
  the review-cursor line/word/character motions) the keyboard layer's
  scripts map onto, so the reducer never depends on input-crate types.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `UtteranceSource`,
  `SpeechPriority` — structured speech per decision D12. Segments are
  semantic spans: literal text, `Label`, `Value`, `Description`, role and
  state tokens (including `NegatedState` for announcements like "not
  checked"), `SpelledCapital` (an uppercase letter spelled out, which a
  theme speaks at a raised pitch), `Position` (a "2 of 5" pair), `Level`,
  and `Message` (a fixed
  reader message the reducer names — a navigation edge, for instance —
  rather than a property of any node, so it can say something without
  pre-flattening text). The pure reducer never touches localization; spans
  become words at the speech pipeline's presentation stage. An utterance optionally carries an
  `UtteranceSource` — the described node's role and screen rectangle — so
  M11 presentation themes can key earcons off the role and pan audio by
  position without a pipeline change. Focus speech also carries a
  `validity`, which is `None` on every other utterance.
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
- `GestureId` — normalized gesture identifiers, NVDA's scheme.

Implementation note, `GestureId::parse`: splits `source:parts`, lowercases
everything, and sorts the plus-separated parts, exactly like NVDA's
`normalizeGestureIdentifier` — so `kb:Verbatim+V` and `kb:v+verbatim` are
one gesture and binding lookup is order- and case-insensitive.
Deserialization re-parses, so an identifier read from config or the wire is
always normalized.
