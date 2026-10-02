# NVDA parity ledger

This ledger tracks, per behavior, what NVDA does (linking the
`docs/nvda/` file that documents it) and where Verbatim stands. It is
the working checklist for parity reviews and the definition of done
for parity work: an "unverified" entry is a claim someone still has to
check against a live NVDA or against the cited NVDA source.

Statuses:

- **matched (verified)** — implemented and confirmed by a live E2E
  scenario or a cross-process test that asserts the NVDA-equivalent
  behavior.
- **matched (unverified)** — implemented to match the cited NVDA
  behavior, but no test or live comparison pins it yet. These are the
  entries most likely to hide inconsistency bugs.
- **different (Dn)** — deliberately divergent, recorded in
  [Architecture](architecture.md) decision Dn or noted inline in
  [the crate guides](crates/readme.md).
- **not yet (Mn)** — not implemented; scheduled milestone in
  parentheses.

Keep entries short; move discussion into the linked docs. When a
review confirms an entry, upgrade its status and note how it was
verified.

## Architecture-level stances

- Hang isolation. NVDA: one main thread plus watchdog cancellation
  ([Main loop and watchdog](nvda/main-loop-and-watchdog.md)). Verbatim: **different (D9,
  D13)** — per-app outpost processes; no output-producing thread may
  block cross-process. The evidence that NVDA's freezes come from
  synchronous cross-process calls is collected in that NVDA doc.
- Event intake. NVDA: single in-process winevent/UIA registration
  with foreground gating ([Event handling](nvda/events.md), [MSAA and winevent handling](nvda/msaa.md)).
  Verbatim: **different (D13)** — a dedicated no-cross-process-call
  focus-listener process plus per-app outposts.
- Extensions. NVDA: unrestricted Python add-ons
  ([App modules mechanism](nvda/app-modules-mechanism.md)). Verbatim: **not yet (M5)**,
  and **different (D6)** by design — sandboxed Wasm components.
- Injection. NVDA: universal lazy DLL injection
  ([Process injection](nvda/process-injection.md)). Verbatim: **not yet (M6)**, and
  **different (D2)**: injection only ever accelerates, never required
  for correctness.
- Display model. NVDA: GDI hooks power screen review
  ([The display model](nvda/display-model.md)). Verbatim: **different (D11)** —
  screen review will be a spatial projection of the tree (M6); GDI
  model deliberately last (M14, gated).
- Spoken vocabulary and key layouts. NVDA: the spoken role and state
  names, messages, synth setting labels, and the desktop and laptop
  gesture layouts ([Speech](nvda/speech.md), [Keyboard input](nvda/input.md)).
  Verbatim: **matched (verified)** by decision, not by accident: users'
  ears and hands already know them, so Verbatim speaks the same English
  words and binds the same keys wherever it implements the same command
  (E2E scenarios assert the wording; the gesture tables live in
  [verbatim-input](crates/verbatim-input.md)). Only the English catalogue
  is shared vocabulary; Verbatim's other-language resources are written
  fresh, never taken from NVDA's translations.

## Focus and announcements

- Focus announcement content and property order (name, role, value,
  states in fixed order, description, shortcut, position, level).
  NVDA: `speakObject` ordering ([Speech](nvda/speech.md)). Verbatim:
  **matched (verified)** for the M3 surface — E2E scenarios assert
  wording; per-property order transcribed in `verbatim-core`
  ([verbatim-core](crates/verbatim-core.md)). **Different**, found on
  2026-10-02 by running NVDA with its speech log through the same keys
  in Explorer, Settings, and Start:
  - NVDA speaks no role on focus for a set of roles, among them list
    item, menu item, tree view item, pane, static text, and unknown,
    when the object has a name or value; Verbatim spoke them ("alpha.txt
    list item" where NVDA says "alpha.txt"). **Matched since
    2026-10-02** ("When the role is spoken" in
    [Speech](nvda/speech.md)), including object navigation, which NVDA
    speaks with the focus reason, and reporting the current object,
    which keeps the role.
  - An Explorer file is spoken by Verbatim with a second copy of its
    name ("alpha.txt list item alpha.txt"), which NVDA does not speak;
    not yet traced to the property it comes from.
  - NVDA speaks a tree item's level first ("level 1, System, 2 of
    12"); Verbatim speaks it last.

- Focus-ancestry context: announce newly entered presentable
  containers before the control. NVDA: `focusEntered` +
  `isPresentableFocusAncestor` ([Event handling](nvda/events.md),
  [Object model](nvda/object-model.md)). Verbatim: **matched (unverified)** —
  filter claimed at parity with `_get_isPresentableFocusAncestor`
  (exclusion-based, same role exclusions), except:
- Top-level windows in the ancestry. NVDA presents an entered window
  like any other container when it has a name or a description, and
  treats an unnamed window as layout ([Event handling](nvda/events.md),
  "Foreground windows"). Verbatim: the window is one of the roles
  presentable only when named or described, as in NVDA, and there is
  no separate foreground announcement. **matched (unverified)**.
- Window announcement on switching applications. NVDA: a foreground
  event is a focus on the window; it is ignored when the latest focus
  is already in that window, and the window is otherwise announced as
  the focus. A window that is an ancestor of the new focus is spoken
  as an entered container. Nothing speaks a window just because it
  became the foreground ([Event handling](nvda/events.md), "Foreground
  windows"). Verbatim: a foreground fact arrives as a focus on the
  window, and the reducer ignores it when its window handle equals the
  current focus's top-level window handle. Window handles are compared
  rather than node ids because
  one window has different node ids in different outposts (a Settings
  page whose frame belongs to `ApplicationFrameHost.exe`), and an
  ancestor walk that timed out leaves no ancestors to compare. A
  window that has no name when focus enters it is not announced
  later, and a foreground change to a nameless window moves
  attention without speaking (a bare "window" says nothing).
  **matched (unverified)**; the Start menu and window-switch scenarios
  verified the earlier, separate announcement.
- Duplicate focus suppression (same control announced once when two
  paths report it). NVDA: "already the focus" early return.
  Verbatim: **matched (unverified)** ([verbatim-core](crates/verbatim-core.md),
  M3 noise suppression). Unchanged by the outpost redesign: only Core
  sees focus across applications, so this stays in the reducer.
- Stale focus events: NVDA has no timestamp arbitration; it relies on
  queue-time freshness plus cancellable speech
  ([Event handling](nvda/events.md), [Speech](nvda/speech.md)). Verbatim:
  snapshot versions and observation-time arbitration are gone; order
  comes from each outpost's single queue and worker, as in NVDA, and the
  observation time stays on inputs for latency records only. A UIA focus
  fact whose element is no longer the focused element when the worker
  reads it is dropped, since the newer fact follows. **matched
  (unverified)**; the `focus_churn` scenario covers it.
- Name change on the focus. NVDA: when the focused object's name
  changes, the new name alone is spoken, queued behind current
  speech; a name change on any other object, including an ancestor
  of the focus, is silent ([Event handling](nvda/events.md), "The focus
  gate" and "Foreground windows"). Verbatim: the new name alone is
  spoken at `Queued` priority, only for the focused node.
  **matched (unverified)**.
- Entered menus. NVDA: when focus newly enters a menu bar, a popup
  menu, or a menu item as an ancestor, speech is cancelled and the
  ancestor is not announced; the focused item is announced as usual
  ([Event handling](nvda/events.md), "The focus gate"). Verbatim: entering
  one interrupts current speech and it is not announced.
  **matched (unverified)**.
- Event acceptance. NVDA: every event except UIA notifications must
  come from a window related to the system's foreground window: a
  descendant of it, sharing its root owner, a topmost window or one
  whose root is topmost, or, for `Windows.UI.Core` windows, a
  descendant of the input thread's active window. Allowlisted tooltip
  and notification-bar windows, toast alerts, and (as an option)
  background progress bars are accepted from anywhere. UIA
  notifications are spoken only from the focus's application, with
  per-application opt-ins such as the shell's window-snap results
  ([Event handling](nvda/events.md), "Acceptance filtering"). Verbatim
  (D14, amended by the outpost redesign): the reducer keeps an attention
  record, the application and top-level window of the most recent
  foreground change, standing in for the system's foreground window
  NVDA compares against. A foreground fact is always accepted and
  moves attention; its outpost has already dropped it if the window
  was no longer the system's foreground when the outpost handled it.
  Before handling a batch that holds a foreground fact, the outpost
  waits up to 250 ms for that window to become the foreground window,
  as NVDA holds back event handling after a foreground event (issue
  3831); a starting application's focus event, which can come just
  before its foreground event, is then judged against the real
  foreground. The bound was measured live on 2026-10-02: over 245 such
  events, the window arrived 5 to 100 ms after its event. Without the
  wait, msinfo32 was sometimes never announced. Every other event, focus
  events included, is classified against the record from window
  facts its outpost attached: top-level window, root owner, topmost,
  for `Windows.UI.Core` windows whether the window is under the
  input thread's active window, and whether the window is in the
  system's foreground window when the outpost read the event (NVDA's
  live test: its top-level window is the foreground window, or its root
  owner is the foreground window or that window's root owner). Such an
  event is attended. Windows can raise a window's foreground event while
  its foreground lock keeps another window in front, and raises none
  when the window is given the foreground later (found live), so a
  focus in the system's foreground window that is unrelated to the
  attention window moves attention, as NVDA takes the foreground from
  the focus's ancestry. Nothing else moves attention, so a topmost
  popup menu that takes focus without becoming the foreground leaves
  attention where it was, and focus returning from it is still
  attended. Accepted from anywhere as background: toast
  alerts and the shell's window-snap results; other UIA notifications
  only from the attention application. Accepted background events
  never move focus or the navigator and are spoken queued.
  **matched (unverified)** for these; the tooltip and notification-bar
  windows, background progress bars, and a per-source cap on
  background events are **not yet**.
- Recovery after an outpost is replaced. NVDA has no equivalent: it
  is one process, and after an application crash it re-queries the
  real focus ([Focus and the navigator](nvda/focus-and-navigator.md)).
  Verbatim (**different (D9, D13)**): when an outpost ends, the reducer
  keeps the focus's copied data but treats its node ids as dead, and
  clears the navigator if it belonged to that outpost. If the
  application holds attention, a replacement starts at once and is
  asked for its current focus; when the reply arrives, it is compared
  with the kept copy (role, name, value, states, and the ancestors'
  names and roles), and if all match, the new node ids are taken
  silently, otherwise the focus is announced as usual. The same
  question is asked after the focus listener is replaced.
  **unverified**.
- Held objects. NVDA: an object lives as long as something refers to
  it, and an MSAA event, which names its object only by window, object
  id, and child id, is directed to the existing focus object when the
  two compare equal ([Event handling](nvda/events.md)). The comparison
  requires equal child ids; the same COM object is equal; two
  `IAccessible2` objects in the same window compare by unique id;
  otherwise differing event addresses are unequal, and the MSAA
  identity strings, location, role, and name must all match.
  Verbatim: each outpost keeps its nodes, with the live UIA element or
  MSAA object behind each where it has one (a UIA node reported from a
  listener fact carries only cached properties), for every node Core
  still holds, plus every node reported after the last message Core has
  acknowledged, and releases the rest; a query for a released node
  answers "gone". Navigation, activation, and ancestor reads of an MSAA
  node use the object that was announced. A new MSAA sighting is the
  same node when it is the same COM object with the same child id in the
  same window and has the same role, or when it was acquired at the same
  address as the node, has the same role, and, if both objects offer
  one, the same identity string. An object reached through `accParent`
  or as a child object has no address of its own (other objects in its
  window share the one Verbatim would make up), so it is matched only as
  the same COM object. `IAccessible2` unique ids are
  not read yet, and location and name are not compared, because
  Verbatim would compare them with values read when the node was
  issued rather than a fresh read of both. A window's nodes are
  dropped when the window is destroyed. Positional child ids in simple
  list controls remain a known limitation that NVDA shares.
  **different (unverified)**: NVDA releases an object when nothing
  refers to it, Verbatim when Core reports it no longer holds the node.
- Cancellation of expired focus speech (focus left before speaking).
  NVDA: `_CancellableSpeechCommand`. Verbatim: **not yet** — no
  equivalent validity check in the speech queue; Interrupt priority
  masks most cases. Candidate gap for fast-typing scenarios.
- Menu popup announcements. NVDA: menu events with fake-focus
  fallback ([MSAA and winevent handling](nvda/msaa.md)). Verbatim:
  NVDA's menu rules run in the outpost's worker. Within a batch, focus
  events are handled first and the newest menu opening from each
  backend last; a menu opening is ignored if a focus in the batch
  already put focus on a menu or menu item, and otherwise becomes a
  focus on the popup menu, so the reducer receives only focus events
  for menus. A menu closing, menu mode ending, or the Alt+Tab
  switcher closing anywhere on the desktop is forwarded by the focus
  listener to Core; if Core's focus has not changed 50 milliseconds
  later, Core asks the application then in the foreground for its
  real focus, as NVDA's fake focus reads the focus from the foreground
  window. Focus usually returns to another application than the one
  that owned the menu, which a check inside the menu's own outpost
  could not see. **matched (unverified)**; the Start menu path was
  verified before the redesign.
- Toggle button role and "pressed"/"not pressed" wording (UIA Toggle
  pattern on a Button; no separate Switch role). NVDA: UIA
  detection. Verbatim: **matched (verified)** (cross-process test;
  commit c72afd8).
- Negated states: "not checked" for unchecked check box/radio, "not
  pressed" for toggle, "not selected" for selectable-unselected;
  positive "selected" never on node announcement. Verbatim:
  **matched (unverified)** — transcribed rules in reducer; needs a
  live NVDA comparison across roles.
- Selection announcements (focused list's selected child; changes
  while focus stays on container; combo box exclusion). NVDA:
  selection events. Verbatim: **matched (unverified)** ([verbatim-core](crates/verbatim-core.md), M3).
- Selection in a list the focus controls (search suggestions and
  results). NVDA: when an item is selected inside an element the focus
  names in its UIA ControllerFor relation, NVDA reports that item as it
  reports a focus, without moving focus; this is how it reads the Start
  menu's search results and the Settings app's search suggestions as
  the user types and arrows. Verbatim: **not yet**; found on
  2026-10-02, when Verbatim read neither while NVDA read both. Start's
  results are also partly in a Chromium document NVDA reads through
  IA2.
- Value change on focused node speaks bare value (slider drag).
  Verbatim: **matched (unverified)**. Background progress bar
  reporting (NVDA option): **not yet**.
- State-change diff announcements (gained states; checked-loss
  negation). Verbatim: **matched (unverified)**.
- UIA notification events (snap layout hints etc.), incl.
  interrupt-vs-queue by processing hint. NVDA:
  `event_UIA_notification` ([The UIA client](nvda/uia.md)). Verbatim:
  **matched (unverified)**, foreground-gated in shell. NVDA speaks
  notifications only from the focus's application, plus
  per-application opt-ins; see "Event acceptance" above.
- Toasts. NVDA: a toast's `alert` event is accepted from any application
  ([Event handling](nvda/events.md), "Acceptance filtering"), and the
  `Notification` behavior speaks the object. Verbatim: the listener hooks
  `EVENT_SYSTEM_ALERT` desktop-wide; the outpost reports an alert only from
  a window whose parent has the class `ToastChildWindowClass`, and the
  reducer speaks the object queued from anywhere, never moving focus.
  **matched (unverified)**; the setting to turn toast reporting off is
  **not yet**.
- Other alerts. NVDA speaks an alert at once when the object's role is
  alert, it has a name, description, or children, and it is not already
  among the focus's ancestors (`event_alert` on IAccessible objects).
  Verbatim: **not yet**; it has no alert role, and reports no other alerts.
- Live regions (browsers). NVDA: in-process IA2 machinery
  ([IA2 usage](nvda/ia2.md)). Verbatim: **not yet (M6)**.

## Object navigation and review

- Navigation tree shape. NVDA default: simple review on (filtered
  tree, [Focus and the navigator](nvda/focus-and-navigator.md)). Verbatim: full tree,
  matching NVDA with simple review **off** — the project's stated
  baseline (maintainer decision; commit 6b7a519). **matched
  (verified)** against that baseline; simple-review-on filtering is
  **not planned** (revisit only if the baseline changes).
- Navigator follows focus; review follows navigator. NVDA coupling
  rules ([Review modes](nvda/review-modes.md)). Verbatim: **matched
  (unverified)** for the follow-focus default; `followCaret` /
  `followMouse` equivalents **not yet (M4+)**.
- Report current object: report / spell / copy on 1st/2nd/3rd press.
  NVDA: script repeat counting ([Keyboard input](nvda/input.md)). Verbatim:
  **matched (verified)** (multi-press machinery in verbatim-input;
  copy via the shared clipboard helper).
- Parent/next/previous/first-child moves with edge reporting ("no
  parent" etc. spoken, not silence). Verbatim: **matched (verified)**
  (commits 9bd6bec, 0fe39f0); NVDA wording comparison still
  worthwhile.
- Sibling navigation resolving back to self reported as edge
  (MSAA `accNavigate` quirk). NVDA: fallback heuristics
  ([MSAA and winevent handling](nvda/msaa.md)). Verbatim: **matched (unverified)**
  (commit 0fe39f0).
- Windowed MSAA children navigated through the window hierarchy.
  NVDA: same strategy. Verbatim: **matched (verified)** (msinfo32
  E2E; commit d3e13b7).
- SysTreeView32 via TVM messages, Tree/TreeItem roles. NVDA:
  control-specific overlay. Verbatim: **matched (verified)**
  (tree_navigation E2E; commits 809214d, 1078614).
- Navigator death recovery: NVDA reports failure and stays; Verbatim
  re-seeds navigator from focus on `Gone` and announces it —
  **different (documented in [verbatim-core](crates/verbatim-core.md))**; NVDA-side
  behavior in [Focus and the navigator](nvda/focus-and-navigator.md).
- Review cursor line/word/character over object text. NVDA: object
  review over TextInfo ([Review modes](nvda/review-modes.md)). Verbatim:
  **matched (unverified)** at M3 fidelity (flat value/name text;
  grapheme/word segmentation deferred to M4 — a known divergence
  until then).
- Document review and screen review modes. Verbatim: **not yet
  (M6)**; screen review will be tree-projection **different (D11)**.
- Object activation (do default action). Verbatim: **matched
  (unverified)**.
- Move focus to navigator / caret routing. Verbatim: **not yet
  (M4)**.

## Backends

- Dual-stack MSAA+UIA with per-window arbitration. NVDA: the
  `isUIAWindow` referee with good/bad class lists
  ([The UIA client](nvda/uia.md)). Verbatim: **matched in architecture (D1)**;
  the arbitration probe exists, but the per-class scar-tissue lists
  are **not yet** transcribed — expect per-app fidelity differences
  until each is triaged (tracked per app-family as they land). A probe
  that finds a UIA provider is kept for the window's lifetime, one that
  finds none for 500 ms, NVDA's cache period. Checked live on
  2026-10-02 across about 65 windows of Explorer, Settings, Start, and
  the desktop: no window's answer changed from a provider to none
  during its life, but a window can answer "none" just after it is
  created and gain its provider within seconds (Explorer's file list,
  Windows 11 Notepad's edit control). Until the re-probe, that first
  focus is read through MSAA, as in NVDA.
- UIA caching discipline (cache requests on events and walks). NVDA:
  `baseCacheRequest` pattern. Verbatim: **matched (unverified)** —
  cached elements + scoped search landed in 918e5b8/4563bff after a
  desktop-wide re-find bug; audit against NVDA's per-event cache
  contents still open.
- UIA event registration scope (global group vs focus-local
  high-frequency events). Verbatim: **partial** — focus/property
  registrations exist; the local-group re-registration pattern for
  text events is **not yet (M4)**.
- MSAA winevent flood control (per-thread caps, focus coalescing,
  latest-menu-only). NVDA: `OrderedWinEventLimiter`
  ([MSAA and winevent handling](nvda/msaa.md)). Verbatim: **not yet** — outposts rely on
  per-app isolation to bound damage, but no equivalent coalescing
  exists inside an outpost; flagged as a review question for busy-app
  scenarios.
- Event acceptance filtering (foreground gating, show/hide rules).
  NVDA: `shouldAcceptEvent`. Verbatim: **partial, different (D13)** —
  foreground gating lives in the shell/focus-listener design rather
  than per-event heuristics; background-app opt-ins (progress bars)
  **not yet (M8)**.
- IA2 upgrade of MSAA objects. NVDA: `normalizeIAccessible`
  ([IA2 usage](nvda/ia2.md)). Verbatim: **matched (unverified)** —
  verbatim-ia2 QueryService path exists ([verbatim-ia2](crates/verbatim-ia2.md));
  IA2 text/hypertext consumption **not yet (M4/M6)**.
- JAB. Verbatim: **not yet (M13)**.
- Remoted UIA trees (Application Guard-style: valid UIA, locally
  invalid window handles and process identity). NVDA: the WDAG
  pathway ([The UIA client](nvda/uia.md)). Verbatim: **not planned**
  — MDAG is deprecated and removed in Windows 11 24H2; noted because
  Verbatim's window-handle assumptions (arbitration, hierarchy
  navigation) would need a treat-as-remote pathway if a successor
  technology with the same shape appears.

## Speech and audio

- Priority lanes. NVDA: NORMAL/NEXT/NOW with resume of interrupted
  speech ([Speech](nvda/speech.md)). Verbatim: **partial** —
  Queued/Next/Interrupt exist; NVDA's *resume of interrupted
  lower-priority speech* is **not yet**: Verbatim's Interrupt
  discards. Decide whether to match before M8 profiles work.
- Index marks driving callbacks at audible position. NVDA: manager
  indexing + WASAPI feed-end callbacks ([Audio output](nvda/audio.md)).
  Verbatim: **matched (unverified)** — index-mark echoes exist in
  the synth contract; say-all (the main consumer) is **not yet
  (M4)**.
- Structured utterances vs flat strings. NVDA: command-laden flat
  sequences. Verbatim: **different (D12)** — typed spans flattened
  by a theme at the last stage.
- Speech settings model (driver settings, immediate application).
  NVDA: `SynthDriver.supportedSettings`. Verbatim: **matched
  (unverified)** — same descriptor-driven model.
- Instant cancel (stop + reset, audible immediately). NVDA:
  `WavePlayer.stop`. Verbatim: **matched (verified)** — WASAPI
  stop+reset; latency measured in the E2E ledger.
- Rate boost via sonic, audio ducking, sound split, tones/earcons:
  **not yet** (M8 ducking/config breadth, M11 earcons).
- Sonification (spelling-error sounds, mode-switch sounds, progress
  beeps, indentation tones) and its scheduling split between
  in-stream playback-synchronized commands and immediate
  fire-and-forget sounds. NVDA: [Sonification](nvda/sonification.md).
  Verbatim: **not yet (M11)**, and **different (D12)** by intent —
  themes should let one semantic event render as word, earcon, or
  parameter change; NVDA hard-codes the choice per feature. The
  inventory is the parity floor M11 must cover.
- Synth isolation. NVDA: in-process drivers (crash = NVDA crash),
  one out-of-process precedent ([Synth drivers](nvda/synth-drivers.md)).
  Verbatim: **different (D6)** — native synth host out of process
  (M7).
- Symbol/punctuation processing, speech dictionaries, character
  descriptions. NVDA:
  [Symbols, dictionaries, and character processing](nvda/symbols-and-dictionaries.md).
  Verbatim: **not yet (M8)** — D12 plans this over typed spans; the
  dictionaries-before-symbols ordering and the symbol preserve rules
  are the parity-critical details.
- Automatic language switching. NVDA: strip-or-pass of language
  commands per config ([Speech](nvda/speech.md)). Verbatim: **not
  yet** (unscheduled; needs backend language attributes first).

## Input

- NVDA-modifier semantics: swallow, double-tap passthrough,
  trapped-key release swallowing, unbound-chord fallthrough.
  NVDA: `keyboardHandler` ([Keyboard input](nvda/input.md)). Verbatim:
  **matched (verified by unit tests)** — semantics transcribed
  ([verbatim-input](crates/verbatim-input.md)); live side-by-side with NVDA still
  worth one session (sticky/locked modifier states **not yet**).
- Script repeat counting incl. auto-repeat exclusion. Verbatim:
  **matched (verified)**.
- Gesture map / rebindable input, input help mode. Verbatim: map
  exists; user rebinding UI and input help **not yet (M8/M9)**.
- Typed-character echo. NVDA: in-process reports; UIA textEdit
  events where applicable. Verbatim: **not yet (M4)** — decide the
  no-injection echo source (UIA events + polling?) explicitly
  against [Keyboard input](nvda/input.md).
- IME/composition reporting. Verbatim: **not yet** (unscheduled;
  needs a decision — NVDA's implementation is injection-dependent).
- Mouse tracking (text-unit speech, audio coordinates, injection
  filtering) and touch interaction. NVDA:
  [Mouse and touch](nvda/mouse-and-touch.md). Verbatim: **not yet**
  (unscheduled; mouse tracking becomes cheap once point-to-node and
  point-to-text resolution exist).

## Text, documents, terminals (M4/M6 previews)

- TextInfo-equivalent layer, caret-key reporting via
  wait-for-evidence, selection deltas, terminal diffing: all **not
  yet (M4)**; the NVDA references to design against are
  [TextInfo](nvda/text-infos.md) and
  [Editable text and terminals](nvda/editable-text-and-terminals.md).
- Word and character segmentation (Uniscribe grapheme clusters and
  word stops) and the three-way paragraph-style setting. NVDA:
  [TextInfo](nvda/text-infos.md). Verbatim: **not yet (M4)** — the
  review module's flat-text walk explicitly defers both.
- Browse mode, quick nav, pass-through rules, virtual-buffer
  equivalent: **not yet (M6)**; references
  [Browse mode](nvda/browse-mode.md), [Virtual buffers](nvda/virtual-buffers.md).
- Office object-model reading: **not yet** (app-module track);
  reference [Office through COM](nvda/office-com.md) before deciding whether
  Verbatim replicates the OM path or bets on modern-Office UIA.
- Document formatting reporting (the option vocabulary and the
  cache-and-diff announcement model). NVDA:
  [Document formatting reporting](nvda/document-formatting.md).
  Verbatim: **not yet (M4)** — the diff-not-per-run behavior is the
  parity-critical core.
- The role-shaped behavior layer (progress bars, dialog text
  harvesting, suggestion sounds, fake table rows, tooltips/toasts).
  NVDA: the behavior mixins ([Object model](nvda/object-model.md)).
  Verbatim: **partial** — selection and notification handling exist
  in the reducer; the rest lands per feature. Architectural question
  for the review: where is Verbatim's home for this layer?
- ARIA vocabulary, annotations/details, compound documents. NVDA:
  [ARIA, annotations, and compound documents](nvda/aria-and-annotations.md).
  Verbatim: **not yet (M6)**.
- Math (MathML pipeline, provider seam, interactive navigation).
  NVDA: [Math](nvda/math.md). Verbatim: **not yet** (in scope,
  unscheduled — maintainer decision 2026-07-17).

## System integration

- Vision framework (focus highlight, screen curtain, magnifier),
  OCR, secure screens, remote access: all **not yet** (M8 for OCR
  and secure desktop, M12 remote); references [The vision framework](nvda/vision.md),
  [OCR and content recognition](nvda/ocr-and-content-recognition.md),
  [Secure mode](nvda/secure-mode.md), [Remote access](nvda/remote-access.md).
- Elevated applications on the user desktop (admin consoles,
  elevated installers): reading them requires uiAccess — signed
  binaries in a trusted install location. NVDA:
  [Secure mode](nvda/secure-mode.md). Verbatim: **not yet** —
  unsigned dev builds cannot reach elevated windows at all; the
  signing/packaging work has no milestone, and M8's secure-desktop
  work depends on it. Recognize the symptom now: "Verbatim goes
  quiet in the admin prompt" is this, not a bug.
- Braille: **not yet (M15, D7)**; the inputs braille needs preserved
  are listed at the end of [Braille](nvda/braille.md).
- Configuration profiles and triggers (app, say-all, manual;
  queue-synchronized application to speech). NVDA:
  [Configuration and profiles](nvda/config-and-profiles.md).
  Verbatim: **not yet (M8)** — `verbatim-config` covers the base
  store only; the layering and trigger semantics are undesigned.
- Settings GUI generated from driver descriptors. NVDA:
  `AutoSettingsMixin` ([NVDA's GUI and the settings framework](nvda/gui-and-settings.md)).
  Verbatim: **matched (unverified)** in pattern — `SettingsHost`
  feeds the GUI from `SettingDescriptor`s; NVDA's panel framework
  and accessibility glue are the reference as the GUI grows.
- Logging and the log viewer. NVDA: [Logging](nvda/logging.md).
  Verbatim: **not yet (M9)** — tracing exists (flight recorder,
  latency ledger); user-facing logging is unbuilt.
- Developer console. NVDA: [The Python console](nvda/python-console.md).
  Verbatim: **not yet (M10)** — `verbatim-inspect` covers part of
  the inspection role today.
- Installation, portable copies, self-update, COM registration
  repair. NVDA: [Installation, portable copies, updates, and COM fixes](nvda/installation-and-updates.md).
  Verbatim: **not yet** (no packaging milestone yet).
