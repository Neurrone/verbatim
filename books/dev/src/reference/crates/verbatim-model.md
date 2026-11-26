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
  outpost never collide in the latency ledger. Only the processes that
  observe events mint; a reducer takes every trace id with the input that
  caused the work. `from_raw(u64)` gives a fixed id, for tests and samples.
- `OutpostId`, `NodeId`, `Issuer`, `Pid`: small identity types. An
  `OutpostId` names one outpost process generation and is never reused
  (`Instance::outpost_id` gives a request's instance's). A
  `NodeId` is its issuer plus a number that issuer gave: the issuer is an
  outpost generation (`Issuer::Outpost`) or Core's plain reader
  (`Issuer::Plain`, by the generation of the plain window it holds), so a
  plain node never compares equal to an outpost's. Outposts mint ids with
  `NodeId::new(number)`, leaving the outpost `UNASSIGNED`, and Core stamps
  the real generation on everything arriving on that outpost's pipe
  (`assign_outpost` on answers and tells), so an id from a replaced
  outpost never names a node in its successor. `NodeId::outpost` is
  `None` for a plain node, and the stamp `with_outpost` keeps a plain id
  plain; in a message an outpost's id is `{"outpost": n, "number": m}`
  and a plain one `{"plain": n, "number": m}`, written and read by hand
  with no buffering.
- `HIDDEN_FRAME_WINDOW_PROP`: the window property
  [verbatim-gui](verbatim-gui.md) stamps on Core's hidden main frame
  (decision D9), which a reader reads into a window's facts
  (`verbatim_win32::window`) so that Core never announces the frame.
- `WindowHandle` and `WindowFacts`: a native window handle as an opaque
  number, and the facts about an event's window its outpost reads with
  local calls: top-level window, root owner, whether it is topmost,
  whether a `Windows.UI.Core` window is under the input thread's active
  window, whether it is in the system's foreground window, that foreground
  window itself, from the same read, by which Core's title rule decides
  whether a focus entered the window in front, whether that window is
  Program Manager's or the taskbar's (`foreground_is_shell`), which say
  no title, and its class;
  on a focus change also the application's name as NVDA names an
  application module (`explorer`) and whether Alt was held (`alt_held`).
  The reducer classifies events with these
  (`books/dev/src/explanation/architecture.md`, section 2, "Attention and acceptance (D14)").
- `HookForeground`: what Core decides for the keyboard hook from the
  listener's foreground report (`QueueInput::Foreground`, below), with
  its desktop part (`desktop_switches`, the switches away from the user's input desktop Core has handled): whether
  keys may be held back for that window, what text its focus has
  (`FocusTextKind`: none, a field's, whose Enter is held too, or a
  terminal's), and which thread's keyboard layout translates its keys
  (architecture section 5).
- `Backend`: `Uia` or `Msaa`, which client stack sourced a node or event;
  diagnostics only above the outpost.
- `class`: NVDA's window class lists and its class name normalization,
  written from `docs/parity/backends.md` ("The class lists the
  per-window verdict reads") for Core and both readers. `normalize(class)`
  replaces a name found in the class map (`CLASS_MAP`) by its target;
  otherwise a Windows Forms name (`WindowsForms`, any digits, a dot, the
  inner name up to the last `.app.`) or an `ATL:` name is reduced to its
  inner name and looked up in the map; otherwise the name is kept.
  `by_class(class, root_class)` is the verdict the lists give: a
  normalized class in `GOOD` is UI Automation, then a window under one of
  `SHELL_ROOTS` other than the Start button, then a normalized class in
  `BAD` is MSAA, otherwise none.
- `Role`, `State`, `StateSet`: the role vocabulary, with a counterpart for
  every role both backends' NVDA tables map that Verbatim speaks
  (`Role::ALL` lists them; `Role::Terminal` is NVDA's terminal role), the
  states (no "default", which NVDA does not have; `Invisible`, an MSAA
  object's or a destroyed ghost's plain window's, spoken only for a query,
  makes an ancestor unpresentable), and a bitmask
  state set
  with `contains`, `insert`, `remove`, `with`, and ordered `iter`. Both
  enums are non-exhaustive and ordered.
- `NodeSnapshot`, `NodeDetails`, `Rect`: everything needed to announce one
  node: id, backend, role, optional name and value, states, and a
  `NodeDetails` group of optional properties: description, keyboard
  shortcut, position in set, set size, level, bounding `Rect`,
  `top_level_client` (the MSAA client area of a top-level window with a
  system menu, presented as that window), `class_name` (on a UIA focus's
  snapshot, for Core's rules for an application's elements, and on each
  UIA ancestor, for the slider that focuses its thumb) and
  `parent_automation_id` (only on a UIA focus's snapshot), `row` and
  `column` (a grid item's,
  read only for a node read alone), and `window` (the `WindowHandle` of
  the window the node is, carried only by a plain window, a UI Automation
  element that is its window's own, and an MSAA window or client object,
  never a nearest window, and left out of the message when absent), and
  `uia_layout` (a UI Automation ancestor that UI Automation does not
  count both a control and content, which Core does not present as focus
  context; carried to Core on `FocusAncestor::uia_layout`) and
  `described` (such an ancestor has a description whatever the theme,
  which Core's layout judgment reads; on `FocusAncestor::described`).
  Everything in `NodeDetails` defaults
  to "not reported", and its serde default keeps older recordings
  deserializing.
- `TreeNode`: a `NodeSnapshot` plus its children in tree order, which
  Core builds from a tree dump's `Descendants` answer
  (`verbatim_core::outside`) and the control protocol's `DumpTree` reply
  carries unchanged to `verbatim-inspect`.
- `ShellItemKind`, `ShellItem`, `ShellCandidate`, `ShellCandidates`: the
  system tray or taskbar surface and one of its buttons (a name and a UIA
  runtime id), which Core builds from the walks of the taskbar's windows
  (`verbatim_core::outside`); a `ShellCandidate` is a button with the
  clicked item's runtime id or name, whether it is off screen, and its
  rectangle, and `ShellCandidates` says whether the walk read every
  button.
- `Percent`: a progress bar's percentage, parsed at the boundary so
  nothing receiving one checks it again: held between 0 and 100 in
  hundredths, made only from a UI Automation bar's range or a bar's text.
  A UI Automation bar's tell carries it (`tell::Payload::Progress`); Core
  parses a classic bar's from the value its read answers.
- `NormalizedEvent`, `PropertyChange` and `EarlierElement`: an event as
  Core took it, which `Effect::Observed` carries to the control plane's
  event stream, its protocol version 5 event: `FocusChanged` (a full
  snapshot with its ancestor chain, outermost first), `PropertyChanged`
  with a `PropertyChange` (name, value, description, or the complete new
  `States` set), `ValueChanged`, and the text events under "The text
  protocol". The readers send none: they tell (`tell`) and answer
  requests (`request`).
- `NotificationKind` and `NotificationProcessing`: a UI Automation
  notification's kind and processing, as Core's announcement of it takes
  them.
- `ToastId`: a toast's identity, its runtime id, which Core compares
  with the last toast's to drop a repeat.
- `KeySeq`: the number of a user input, ordered by press, which fences
  speech (`books/dev/src/explanation/design/ownership.md`). Only the keyboard hook's
  thread mints one (`KeySeq::after`), a review rule since the type is in
  this platform-neutral crate; tests construct them with `KeySeq::new`.
  Every press carries it as `key_seq`.
  `KeySeq::NONE_YET` (zero) is the hook's start and what every key field
  of an input from an old flight dump reads as (`KeySeq::none_yet`, the
  fields' serde default).
- `Input` and `Effect`: the reducer's contract. `Input::trace_id` is the
  trace an input carries, for those that carry one: the trace the shell
  executes the input's effects under.
  - Inputs are `Queue` (an input to Core's event queue, ADR 0003, a `QueueInput`
    below, with the trace it runs under and its time on the run's shared
    clock; every key and command arrives this way, as a press),
    `ToggleKeyPressed` (a lock key reached the operating system, with its
    press time), timer ticks, the speech inputs under "The text protocol",
    `Settings` and `Reported` (the reader settings, and the details the
    active theme reports, from which Core chooses each read's details),
    and what the
    shell's threads learn, since only the reducer speaks: `Started`,
    `Exiting { replaced }`, `ErrorLogged`, `LockKeyToggled` (a `LockKey`
    and its state), `ClockRead` (with the gesture's key number),
    `ClipboardCopied`, `ShellItemGone`,
    `ShellItemUnclickable`, `PreviewSpeech` and `PreviewEarcon`.
  - Effects are `Speak` (an utterance and the number of the key that
    caused it: an event-queue entry's fence, or the number the time's
    read and a terminal's output handed on carry); `StopSpeech`; `Queue` (what Core's event
    queue asks of the shell, a `QueueEffect` below); `DropExpiredSpeech` (carrying a `FocusNow`); `PlayEarcon`;
    `CopyToClipboard` (routed through the shell's clipboard helper, its
    result back as `Input::ClipboardCopied`); `KeepDisplayOn` (while
    say-all reads); `SettingsChanged` (a toggle key changed a reader
    setting, which the shell saves); `ForKey` (an effect on behalf of
    another key's trace); `ReadClock { repeat, key }` (read the time and
    date into `Input::ClockRead`, with the gesture's key number); `Show(GuiRequest)` (the menu, or the list of
    a `ShellItemKind`'s items); and `Observed` (an event Core took, with
    its trace, process, backend and window facts, for the control plane's
    event stream). A lock key's read and the terminal's quiet-period timer
    are not effects: the shell reads their due times from Core's state
    after each step.
- `Earcon`: an event reported at once, outside the speech queue: an
  application not responding, start, exit, an error, browse and focus
  mode, suggestions opened and closed, and progress with its percentage.
  The theme decides how each is presented.
- `ActionName`: the action an activation performed, in its answer
  (`Done::Activate`): `Invoke` (spoken "invoke") or `Named` (an
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
  ("2 of 5"), `Level`, `Row` and `Column`, `Phrase::TableCounts` (a table's
  row and column counts, Report Current Object's), `Message` (a fixed reader
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
  A `FocusValidity` names the node the speech announces and the window
  that node is when it is one; speech for a node that never had the focus
  carries none. A `FocusNow` names the focus, its ancestors, and the
  foreground object when known, each an `ObjectKey`.
  `FocusValidity::holds(now)` is true when the node is the same object as
  the focus, an ancestor of it, or the
  foreground object.
- `ObjectKey`: an object's identity as Core and the speech manager compare
  it, a node id and the window the node is when it is one, a
  `WindowIdentity` (`ObjectKey::of` a snapshot): the window's handle, the
  backend of the object, and whether it is a stand-in, a window above the
  process boundary described from Windows' state
  (`NodeDetails::above_boundary`). `is_same_object` is the one identity
  rule (`docs/parity/focus-context.md`, "Focus-ancestry context"): the
  same node, or both windows with one handle and either one backend, as
  NVDA never equals objects of two types (`NVDAObject.__eq__`), or one a
  stand-in, which is whatever object its own application's API gives
  (`WindowIdentity::is_same_window`).
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
  speak" (30 each), each a `TerminalLines`, brought between 1 and
  `MAX_TERMINAL_LINES` as it is made or read from a file, so no reader
  clamps it, and read as counts through `full_lines` and `last_lines`.
  `TypingEcho::next` is the toggle key's cycle. [verbatim-config](verbatim-config.md) stores them.
- `VerbatimKeys` and `KeyboardLayout`: the Verbatim modifier keys and
  the gesture-binding layout, declared once for the configuration, which
  stores them, and the keyboard hook, which reads them.
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

## Tells, requests and the tables

The types of [ADR 0003](../adr/0003-readers-tell-core-asks.md), "Readers
tell, Core asks", which the protocol below carries.

- `property`: the property table. `PropertyId` names each property once,
  and `PropertyId::entry` gives its `PropertyEntry`: its name, its value
  type (`ValueKind`), how UI Automation reads it (`UiaRead`: the property
  ids cached and their `UiaConversion`), how MSAA reads it (`MsaaRead`),
  how `IAccessible2` reads it (`Ia2Read`, empty until M7), and its `Cost`
  for the bulk-read gate. `PropertyId::named` finds an entry by name, for
  extensions. A read names a set fixed at compile time: the
  `property_set!` macro declares a struct with one field per property,
  typed by the property's marker in `props`, and its `PropertySet`
  implementation, whose `parse` turns an answer's `PropertyValues` into
  the struct once, at the boundary, refusing (`SetMismatch`) a missing,
  repeated, unasked or mistyped property.
- `event_kind`: the event-kind table. `EventKind::entry` gives each kind's
  `EventKindEntry`: its name, its `Teller` (the listener's hook, focus
  handler or global group, an outpost's `Follow` group, or the console
  host), its `Source` (the `WinEvent` constant, the UI Automation event
  id, or for a progress bar's range value change, `UiaProgress`, the one
  property's id) and the properties its tell carries from the event's
  cache.
- `tell`: `Event`, the one tell for both readers: its kind, process,
  raising thread, callback time, object (`EventObject`, a kept element's
  `ElementToken` with its runtime id, the runtime id alone of an element
  its reader keeps nothing for, a followed element Core names back by its
  committed node or a progress bar, or MSAA ids), cached properties, a
  kind's `Payload` (a property's new value, a notification, a desktop
  switch, or a progress bar's percentage and place), the window it is judged by (`TellWindow`, found with
  its `AcceptanceFacts`, none, or failed; the facts mark Verbatim's
  hidden frame and a window it owns through its owner chain, and
  `AcceptanceFacts::in_foreground` is the one test of their relation to
  the foreground) and what the
  listener's check thread found at delivery (`Checked`).
- `batch`: the request list, closed (ADR 0003, "A new command adds no
  kind of request"). A `Batch` is at most sixteen `Step`s, each an `Op`
  of twenty-one (`OpKind`) with an optional `Condition`: a `Test` of one
  earlier step (`Check::Matched` of a `Test` operation, `Check::Found`),
  or of two points of the step's own text (`Test::NotAfter`, whether one
  lies at or before the other, which only an outpost runs; Dickson,
  2026-11-26), either one negated or not
  (`Condition::gate` gives the condition table: a skipped step skips its
  dependants, a refused, missing or failed one makes the test false; for
  two points it answers `Gate::Compare` once the steps they name are
  done, and the executor that makes the calls compares them, a failed
  comparison false and an anchor no longer held the step's own outcome;
  `Step::gate` adds that every step it takes a result from,
  `Step::references`, is done, the one gate each reader runs, and
  `Subject::earlier_nodes` the nodes an earlier result names, by
  `Done::nodes`),
  its `Placement` in the reader's order and a `spare` node and anchor
  held while it is out. An operation names its object by a `Subject`: a
  node, a listener's `Kept` element with its `keep` mark, an `Msaa`
  object with or without NVDA's redirect, a `Route`, a `Window` by
  `WindowOf` with its backend and `visible`, the application's `Focus`,
  or an earlier step's `Result` or `Item` by `Place`; a text operation
  starts at a `Point`, one Core names or `Point::Of` an earlier caret or
  chunk start. Each step answers a `StepOutcome`: `Done` with its kind's
  result (`Done`: today's `NodeRead`, `FocusRead` and `SentenceMoved`
  among them, and `Tested`, `Ancestry`, `Descended`, `CaretAt`, `Chunks`,
  `RangeText`, `SelectionCompared`, `Selected`, `Located`, `Waited`),
  `Refused` (`StepRefusal`), `NotThere` (`Missing`), `Failed` or
  `Skipped`. `parse` turns a batch as it arrives into a `Parsed` one for
  a `ReaderKind`, or refuses it whole with a `Malformed` reason: more than
  `MAX_STEPS` steps, a reference not to an earlier step or to one of the
  wrong kind, a `Bound` broken (depth, node cap, limit, count, poll, wait),
  a caret wait not first, a matcher value of another type, two `keep`
  marks on one element, an operation the reader does not run (the
  listener reads only `LISTENER_MSAA_SET` of an MSAA object, and runs
  its other operations only on kept elements), a subject
  the operation does not take, or an anchor on a subject that is no node.
  `Batch::visit_nodes` and `visit_outcomes` are the one walk each over the
  node ids (and the outcomes' anchors) Core stamps and an outpost keeps;
  `Batch::anchors` lists each text step's node with the anchors it names,
  the spare's last; `Batch::reads_focus_first` is the shell's test for the
  answer in front.
- `matcher`: `Matcher`, a test on one node's properties (a value, absent,
  any of a list, a state, `All`, `Any`, `Not`), and its one evaluator,
  `Matcher::evaluate`, which reads through a callback lazily in the
  matcher's order, each property once, stopping at the first value that
  decides. The property table holds the four a batch's reads add:
  `FocusedWithin`, `MsaaAddress` (`PropertyValue::MsaaAddress`, a
  `HeldIds`), `WindowShown` and `IsContent`.
- `request`: the shared request list's envelope. `Ask` is a request as
  Core sends it, numbered (`RequestNumber`) and naming its window,
  asking a `Question`, which is a `Batch`, answered `Reply::Steps`, one
  outcome per step, and refused `Refusal::Malformed` when it does not
  parse. The
  focus reads are batches: a `FocusRead` of an MSAA candidate, the
  application's focus or a `Route`, the outpost's own copy of an element
  the listener read, which carries the route's steps, where its walk
  ended and what the listener read of each element (`Route::read`).
  Each answers a `FocusRead`, which carries what Core keeps with the
  committed focus and its `FocusCaret`: no text, text not read, or the
  caret read (`FocusCaretRead`), its line with the formatting the
  read's `details` name, and the selection baseline the outpost minted
  from it, which a focus read becomes silently. A `Follow` step names
  the committed focus's node to its outpost (`Followed`, with its
  `FollowGroup` and an extra object), or nothing. Each `Ask` carries its
  window's `WindowVerdict`, "probe first" when Core has none in force,
  and each `Answer` the `Probe`s the request made. `Answer` carries one
  of the five outcomes (`Outcome`: a `Reply`, `Refused` with a
  `Refusal`, not there, not responding, or the reader ended), a
  diagnostic detail, and when the reader's worker began and ended the
  request (`began_us`, `ended_us`, both 0 when no worker ran it). `Cancel` carries a `CancelReason`: foreground, hung,
  due time or withdrawn; only the first abandons a running call.
- The event queue's inputs and effects (`QueueInput`, `QueueEffect`). A
  key or command arrives as a `Press`, its action chosen at the press
  (`PressAction`: a command, a script, a caret key, text typed, a
  clearing key, an editing chord or a control character); a
  caret key's `PressAction::CaretKey` carries the `HeldStroke` the hook
  held, if it held one, and the `CaretBinding` it was bound to: the
  committed focus's node, window, role and `TextProvider`;
  `PressAction::Control` is a control character a key typed, after the
  key's other actions. A transition
  the hook deferred arrives as `QueueInput::Deferred`; Core names what to
  send in `QueueEffect::SendStrokes` (each a `SentTransition`) and, at
  `QueueInput::Exit`, the keys to release in `QueueEffect::ReleaseAtExit`.
  A window that came to the foreground arrives once, as
  `QueueInput::Foreground`, a `ForegroundReport`: the window attention
  and the foreground test judge, the ghost window in front when one
  stands in for that hung window (the keyboard hook's window), its root
  owner, process, thread and class, whether it is a console host window,
  and its process's and Verbatim's integrity levels.
  The session's lock state arrives as `QueueInput::Lock`, a
  `SessionLock`: unlocked, locked or unknown. The supervisor's owner
  thread reports its process facts for Core's outpost generation record
  as `QueueInput::Process`, a `ProcessFact`, and carries out
  `QueueEffect::StartOutpost` and `QueueEffect::EndOutpost`;
  `QueueEffect::StaleFrame` logs a frame of an instance that is not its
  reader's current one. A listener's window destroyed arrives as
  `QueueInput::WindowDestroyed { from, window, process }`, its process
  `None` when the listener could not open the raising thread, and Core
  passes it on in `QueueEffect::WindowDestroyed` to the outpost of that
  process when one is ready. A reader's report of a hung window arrives
  as `QueueInput::WindowHung { from, hung }`, admitted only from the
  reader's current instance.
- `tell::Hung` is that report: the hung window's `WindowFacet`, its
  top-level window's and its root owner's, and when the reader observed
  the event it found the window hung at. `hung::Marks` is the pure
  reducer both readers keep of the threads they have reported hung (ADR
  0003, "A frozen window"): `step` takes an event's window as read, a
  `hung::WindowState` (`Responsive { thread }`, thread 0 for no window,
  or `Hung(Hung)`), and returns the report to send when the thread was
  not marked, marking it, while a responsive window's thread is
  unmarked; `foreground` clears every mark. So a stall of one thread is
  reported once, however many of its windows' events arrive.
  The `stroke` module holds `Stroke`, `HeldStroke` with its `Modifier`s,
  and `SentTransition`.
- The text types a batch's text operations carry (`text`): a step names
  its `TextDetails`, the format attributes to read (a `FormatSet` of
  `FormatAttribute`s, one per attribute the theme switches), spelling
  and grammar errors, and links, which the outpost reads exactly; a read
  that names none fetches nothing beyond the text. A `WaitForCaret`
  carries the starting texts and the effect test's two parts
  (`batch::EffectTest`), which `had_effect` judges against what a check
  found (`Compared`). `SentenceMoved` is `MoveBySentence`'s result,
  `SelectionDifference` `CompareSelection`'s, and `EditSelection` an edit
  control's selection offsets.

## Time limits

The limits a slow application is waited for by, in one place for the
outposts, their logic crate, the focus listener and Core, which all
depend on this crate under the tier rules: a slow application is waited
for, as NVDA's watchdog waits (`docs/nvda/main-loop-and-watchdog.md`),
rather than cut off. Each limit is longer than everything inside it plus a
margin of a second, and a compile-time assertion states each nesting.

- `UIA_TRANSACTION_TIMEOUT`, 10 seconds: one UIA request, set once per
  process (re-exported by `verbatim-uia`).
- `HANDLING_DEADLINE`, 11 seconds: one task, an outpost's program, the
  request and the margin. It bounds MSAA calls and window messages too,
  which have no limit of their own: a window found hung while one waits is
  told of and waited for, never abandoned for its hang, so the deadline is
  their only bound. A task of several slow calls can still pass it; only
  one request is promised to end by its own timeout.
- `FOCUS_READ_DEADLINE`, 13 seconds: `HANDLING_DEADLINE` and two seconds
  for the checks around the walk, the deadline of a focus read: an
  outpost's focus read (`Query::ReadFocus`), its report of a listener
  read, and the listener's read from its start. Past it the read's thread
  is abandoned and the focus lost; no read has a time budget or bound of
  Verbatim's own within it.

## The wire protocol

`protocol` holds the messages between verbatim.exe and its children, the
focus listener and the outposts, and the types they carry
(`books/dev/src/explanation/design/ownership.md`). They are pure data; the framing
that carries them on the pipes is
[verbatim-process](verbatim-process.md)'s.

- One enum per direction and role, so a receiver cannot be sent what it
  does not take: a frame of another direction fails to decode, and its
  child is ended as failed.
  - `CoreToOutpost`: `Ping`, `NodesHeld` (the nodes, text anchors and
    caret tells' tokens Core holds, and the position acknowledged), `IdleQuery`, `Shutdown`,
    `Assign`, `Request` (an `Ask` of the shared list, its node ids
    stamped with the outpost generation, which the outpost clears with
    `Ask::unstamp`), `Cancel` (a request's `Cancel` with its reason) and
    `WindowDestroyed` (a window of the outpost's application destroyed:
    the outpost answers "not there" the queued reads whose MSAA ids name
    it and the watches waiting on it, and forgets its MSAA nodes, text
    facts and classic-read mark before any later work).
  - `CoreToListener`: `ApplicationExited` (answered by `ExitHandled`),
    `Ping`, `IdleQuery`, `Shutdown`, `Request`, `Cancel` and
    `ElementsHeld { generation, tokens, acknowledged }`, the told elements
    Core holds, with the position of the last message carrying tokens it
    handled; a batch names a kept element by the subject
    `Subject::Kept { listener, token, keep }`, the listener's generation
    beside its token, since each listener mints its tokens from 1.
  - `OutpostToCore`: `Ready`, `Pong` (with `parked_count`, the abandoned
    workers not yet returned), `Fault`, `TargetExited`, `SpareReady`, `IdleAnswer`, `Tell` (a tell its
    `Follow` registration or a console host's hook delivered, with the
    value it carried) and `Answer` (one per `Request`; its node ids are
    stamped by `Answer::assign_outpost`, named by `Answer::named`, and it
    counts toward `NodesHeld`'s positions when it carries a result) and
    `Hung` (a window of its application read hung as one of its events
    arrived, sent before that event's tell).
  - `ListenerToCore`: `Ready`, `ExitHandled`, `Foreground`,
    `Tell` (a tell, `tell::Event`, which the supervisor hands to Core),
    `Destroyed { window, process, at_us }` (a window destroyed, with the
    raising thread's process or none, which the supervisor hands to Core
    too), `Hung` (a window whose event it dropped as hung, handed to Core
    likewise), `Answer`, `Pong`, `Fault` and `IdleAnswer`.
- The module denies wildcard matches over its enums, and `samples` gives
  one sample of each `OutpostToCore` variant, from a chain whose matches
  name every variant, for the tests that must cover each.
- `ListenerToCore::carried_token` names the kept element's token a
  listener tell carries (`carries_tokens`), and the listener and
  verbatim.exe count the positions `ElementsHeld` acknowledges with this
  one function.
- `OutpostToCore::carries_nodes` says which messages carry what the
  outpost keeps for Core, an answer with a result and a tell naming an
  element by its token, and the outpost and verbatim.exe count
  the positions `NodesHeld` acknowledges with this one function.
  `assign_outpost` stamps the arriving generation on every node id, and
  `named` lists the node ids and text anchors a message names (`Named`),
  which an outpost records as carried by it; `written` stamps the write.
  `Urgent` is what an outpost sends ahead of its queue (`Pong`, `Ready`,
  `TargetExited`, `SpareReady`), none of which carries a node.
- `Front`, `GhostFront` and `DesktopFront`: what the shell read in front
  when it answered a read of an application's current focus unsent, as
  `QueueInput::InFront { number, front }`, its window hung or, for the
  check after a menu, popup or switcher end, no window in front: the
  desktop window with its owner; a ghost window with its text, the hung
  window it stands for with that window's application and thread, and
  its disabled style; a hung window with no ghost; a responsive one; or
  one gone. Every other request the shell refuses as hung is
  `QueueInput::RefusedHung`.
- `EventTiming` records each point of an event's way through the
  listener and the outpost, in microseconds; `hop_us` is the time between
  two stamps.
- `Responder<T>` answers one in-process request once: `answer` consumes
  it, and its `Drop` answers `None`, "gone", so a request lost anywhere is
  still answered. Across processes a killed child runs no `Drop`, so there
  Core's request table answers.

The completion's types live here too: `UiaSnapshotFact`, what a reader
read of an element, and `DumpedTree`.

## The text protocol

How an outpost sends text to Core, and how Core asks an outpost to read
text, wait for the caret, select, or move the caret, by the request
list's text operations (`batch`). The types are in
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
  newer ones for the node. A step naming a forgotten anchor is answered
  `NotThere(Missing::Anchor)`. Anchors die with the outpost generation.

### Chunks

- A `TextChunk` is one unit of text: `unit`, `text` (UTF-8, line breaks
  as the provider gives them), `start` (an anchor at its start), `offset`
  (the point the request was about), `languages` (`LanguageRun`s, byte
  ranges with a BCP 47 tag), `first` and `last` (known to be the
  document's first or last unit of its kind; false when not or when not
  cheap to tell), `truncated`, and `formats`. A chunk decodes only when
  its offset and every language and formatting range are character
  boundaries of its text, so Core takes them with no clamp (Claude,
  2026-11-21).
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

### Text events Core takes

These are `NormalizedEvent`'s, which `Effect::Observed` carries to the
control plane's event stream; no reader sends them.

- `FocusChanged`'s `text`, a `FocusText`, read while the outpost handled
  with the focus (`FocusCaret`) so the caret's line comes with it:
  `Caret { caret, selected_text }`, `NoText` (Core speaks the value), or
  `NotRead` (Core speaks neither).
- `CaretMoved { node_id, caret, selection_changes }`: the caret or selection moved in a node with text. A
  `CaretReport` is the line at the caret, with its `offset` at the caret,
  the `CaretSelection` (`None` when nothing is selected), whose start is
  the caret, in that line. Core takes it in arrival order and gives it
  for each caret tell's read it takes; `selection_changes` is how the
  selection changed from Core's selection baseline, by the rule below. A
  key's own caret event is taken by its watch.
- `TextChanged { node_id }`: an edit control's text changed; Core says
  nothing of it.
- `TerminalSnapshot { node_id, snapshot }`: one full read of a focused
  terminal, the control plane's protocol version 5 event the test harness
  waits on ([verbatim-core](verbatim-core.md), "Core's terminal model"),
  which Core gives as `Effect::Observed` for each read it takes; no
  outpost sends it. A `TerminalSnapshot` is whole by itself: its `text` from the read's start to the document's end, the
  `screen`, the document's `first_row` and the `caret_line`, each a
  `TerminalText`; the flags `from_start`, `shrank`, `at_document_start`
  and `alternate`; `caret_rows_to_end`, `caret_column`, and `above`;
  `started_ms` and `ended_ms` on the shared clock (`ended_ms` 0 when not
  known); `waited`; the screen's `colours` when taken, rows of `TerminalColourRun`; and the
  `caret`. A `TerminalText` is text without the spaces that pad its lines
  and each line's padded length as runs (`lengths`): `unpadded` makes one
  and `padded` gives the text back exactly. The screen is sent beside the
  text, read as the provider's first visible range in a call of its own,
  so it is where the user scrolled the view.

### Requests and replies

A text request is a batch of the request list's text operations,
which Core's event queue sends to the node's outpost
(`QueueEffect::Send`); its answer reaches Core as `QueueInput::Answer`
carrying one outcome per step. A terminal's reads, history reads and
command search are batches too (`verbatim_core::terminal::reader`). The
operations:

- Selection changes: each `SelectionChange` is `selected` or not, its
  `text` cut to `MAX_SELECTION_TEXT_BYTES` (4 KB), and its full count of
  `characters`. When the old and new selections neither overlap nor
  touch, or either is empty, the old text is unselected then the new
  selected, each whole; otherwise the start side is compared, then the
  end side. A change whose text is empty, its text deleted, is listed
  too, as NVDA lists it. The list's order is where each part lies, not
  the order of speech: Core says every selected part first, then every
  unselected one, as NVDA does. A `SelectionDifference` carries both the
  changes and the whole new selection's text (`new_selection`), so Core
  speaks the change in general terms when the node's text changed since
  the compared selection was read.
- Every other read or move of text is a batch's text operation
  (`verbatim_model::batch`): `ReadText` reads units from a point after a
  movement, landing on the unit's start and answering how far it really
  went, a unit the provider lacks refused, never approximated (for
  `Sentence`, a provider with no sentence unit, UI Automation, refuses;
  the offset-based edit controls answer the paragraph, which Core
  splits), several units at once up to `MAX_READ_AHEAD` and
  `MAX_READ_AHEAD_TEXT` for say-all; `ReadRange` the text between two
  points whichever comes first, through the character at the later when
  the points past both are given, cut to `MAX_RANGE_BYTES`; `Select`
  selects, two equal points moving the caret, and a `Caret` after it holds
  the next baseline; `Locate` a point's place on the screen, with its
  offset and the text's length in text measured by character offsets.
- The generic text operations added for a terminal, which Core's
  terminal reader reads and searches one by
  (`verbatim_core::terminal::reader`): `Visible`, the first visible range,
  its ends held, whether it starts at the text's start, and its text
  trimmed; `Move`, a point moved by units from the point itself, its
  start and landing held, refused (`StepRefusal::OutsideText`) for a held
  point past the text's end or one the provider will not compare; a
  whole `ReadText`, units read as one span from the point, up to
  `MAX_WHOLE_UNITS`; `trim` on `ReadText` and `ReadRange`, each line
  without its padding and the padded lengths answered (`padded`), a
  trimmed `ReadRange` read whole with no cut, bounded only by its
  message's frame (Claude, 2026-11-26); a `Caret`'s `reported`, false
  when the provider gave no selection at all and the caret stands in at
  the text's start (Claude, 2026-11-26);
  `FindText`, a needle sought from a point to the text's end or back to
  its start, at most `MAX_FIND_MATCHES` matches over at most
  `MAX_FIND_CALLS` calls, each with its units to the text's end and its
  text (`FoundText`), which Core's reader reads into a `TerminalMatch`
  for each, in rows above the start; and `ReadColours`, the one operation for terminals
  alone, each row's colour runs (`TerminalColourRun`) from a point, for
  the rows of the screen a `Visible` step read, as wide as the line at its top, since the terminals answer
  a stretch whose last cell differs as one colour (Dickson, 2026-11-22).
A batch's text step answers its refusals and failures as its own
outcome (`StepOutcome`). The three `TextFailure`s, `NoText` (no text
interface), `Gone` (the node no longer exists) and `Unanswered` (no
answer in time, or the read failed), say why a node's text could not be
opened, which the outpost and the MSAA text arms turn into a step's
outcome, and are what Core's terminal reader answers for a read or
search whose steps did not all run.

### Inputs from the shell

- A caret key is a `CaretKey`: a `CaretMotion` (character, word, line,
  paragraph, sentence, or page; the line's start or end; top or bottom;
  Backspace and Delete, alone or with Control; Control+A; Enter in a text
  field, `NewLine`; or `Other`, any other key that types no text) and
  whether Shift extends the selection; `CaretMotion::unit` is the unit
  spoken after it. [verbatim-input](verbatim-input.md)'s `caret_bindings`
  maps gestures to these.
- `Input::UtteranceCancelled { marks, at_ms }`,
  `Input::UtteranceFailed { marks }`: what the speech pipeline reports of
  the marks the reducer placed.
- `Input::Settings(ReaderSettings)` and
  `Input::Reported(ReportedDetails)`: at startup and whenever they
  change.
- `Input::TerminalTimer { trace_id, at_ms }`: the focused terminal's
  quiet-period timer came due, at the due time Core's state gave.
- The shell sends the speech side's reports and its timers as queue
  inputs (`Input::Queue`), handled at once by Core:
  `QueueInput::SpeechCut { key }` (the press of key `key`, a modifier
  included, cut speech off),
  `QueueInput::UtteranceCancelled` and `QueueInput::UtteranceFailed`,
  `QueueInput::Due` (the time Core last asked to be woken at came, with
  the window Windows names in front, `LiveFront`, while a check after a
  menu, popup or switcher end waits),
  `QueueInput::TerminalDue { at_ms }` for `TerminalTimer`, and
  `QueueInput::LockKeyRead { key, on }` for `LockKeyToggled`; a mark
  reached is `QueueInput::MarkReached`, which joins the event queue.

### Effects for the shell

- `Effect::Queue(QueueEffect)`: a text request sent to the node's
  outpost (`Send`) or cancelled (`Cancel`), a held key's stroke sent
  (`SendStrokes`), and the wake Core asks for (`WakeAt`).
- `Effect::ForKey(trace_id, effect)`: an effect executed on another key's
  trace.
- `Effect::KeepDisplayOn(bool)`: keep the display on while say-all reads;
  every true is followed by a false.
- `Effect::SettingsChanged(ReaderSettings)`: a toggle key changed a
  setting; save it.

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
  is wrong without reading files; `reported` gives the details it reports
  (`ReportedDetails`).
- The default theme matches NVDA's defaults: everything spoken as NVDA
  speaks it; sounds where NVDA plays them by default (NVDA's files in the
  top-level `sounds` directory); Verbatim's own cues as tones (an
  application not responding, skipped terminal lines, and progress bars
  rising three octaves from 220 Hz, the pitch `verbatim-speech` computes);
  font name, size, color, background color, font attributes, and
  strikethrough off;
  links in text spoken; the kind of underline and bullet style off; and a
  capital raised in pitch.
- `ThemeProblem` and `is_plain_file_name`: what loading a theme found
  wrong (an unknown indication id, sound alone with no sound, a sound
  missing or unreadable, a sound file name that is not a plain name, an
  unknown voice style, a gain too high); a theme with problems still
  loads.
- `ThemeOptions`: the settings that go with a theme: sound volume (a
  `SoundVolume`, 0 to 100 relative to speech, 100 when read past it),
  sounds during say-all, and speaking indications that play a sound.
  `sound_gain(theme, setting, options)` is the gain a sound plays at: the
  theme's, the indication's and the sound volume together, for speech and
  the Theme page alike.
- `ReportedDetails`: the details the theme reports, each unless its
  indication is off: the node details (a `NodeDetailSet` of description,
  keyboard shortcut, position, level and a cell's row and column) and the
  text details (`TextDetails`). Core keeps it and chooses each request's
  details from it; no reader holds a copy. The property table names the
  setting that switches each property (`PropertyEntry::detail`), so a
  property set names its details (`NodeDetailSet::named_by`) and the
  settings choose a set (`NodeDetailSet::choose`).
