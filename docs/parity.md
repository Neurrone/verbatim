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
  fresh, never taken from NVDA's translations. **Different, deliberately**
  (Dickson, 2026-10-03): Caps Lock is a Verbatim key by default, with the
  two Insert keys, where NVDA's default modifiers are the Insert keys
  alone; and Verbatim's menu is on Verbatim+V, not NVDA+N. The laptop
  layout has the numpad bindings too, as NVDA binds them for every layout
  (since 2026-10-03; they had been desktop only).
- NVDA's global commands Verbatim has not built: **not yet**. Among them
  are the settings ring (NVDA+Control+arrows), speech modes, report focus
  (NVDA+Tab), the title (NVDA+T), the status bar (NVDA+End), read window
  (NVDA+B), sleep mode, quit (NVDA+Q), pass the next key through
  (NVDA+F2), and the desktop layout's current line (NVDA+UpArrow). As in
  NVDA, a Verbatim key combination with no command reaches the
  application as the bare key, so until each is built, pressing it types
  its letter or performs the key's own action.

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
    speaks with the focus reason, entered containers, which it speaks
    like a focus, and reporting the current object, which keeps the
    role.
  - Verbatim announced an MSAA window object above a control as an
    entered container ("Categories: window", "vbtest - File Explorer
    window vbtest - File Explorer"); NVDA never presents a window object
    it reaches through a control's parents, and reads a foreground
    window and window-level events through the window's client area.
    **Matched since 2026-10-02.**
  - Explorer's file items exposed their name again as their value, so
    Verbatim spoke it twice; NVDA's `UIItem` class reports no value.
    **Matched since 2026-10-02.**
  - NVDA speaks a tree item's level first ("level 1, System, 2 of
    12"); Verbatim speaks it last.

  A parity audit of the UIA and MSAA handling on 2026-10-02, checked
  against NVDA's source, found these differences in generic behavior.
  **Matched since 2026-10-02**, with reducer, mapping, and mockapp
  tests:
  - States: spoken in NVDA's fixed order (unavailable before checked,
    so "check box unavailable not checked"); "read only" only for edit
    fields and check boxes outside a query; "default" never, as NVDA
    has no such state; "submenu" dropped on a combo box and
    expanded/collapsed on a submenu item; "not selected" only for a
    focusable list item, tree view item, row, cell, header, or check
    box, and only on focus or a change on the focus; "not checked" also
    for any checkable item; positive "selected" kept on a tab and in a
    query; "focused" and "off screen" spoken in a query. The rules are
    "Which states are spoken, and in what order" in
    [Speech](nvda/speech.md).
  - Values and descriptions: an edit field or document no longer
    speaks the whole field after every keystroke; an unchanged value is
    not repeated; a check box, radio button, link, menu item, or
    application never speaks its value; a description equal to the
    name is dropped ("When values and descriptions are spoken" in
    [Speech](nvda/speech.md)).
  - UIA: a selected radio button is checked, not selected; a
    toggleable list or menu item is checkable; password, required,
    invalid entry, and read-only states are read (`ValueIsReadOnly`
    and `IsDataValidForForm` ignoring their defaults, as NVDA reads
    them); a control with only a `RangeValue` pattern has that value,
    rounded, and its changes are followed; the access key and the
    accelerator key are both spoken; dialogs are recognized by
    `IsDialog` and NVDA's dialog class names; and an ancestor counts as
    context only when UIA calls it both a control and content.
  - MSAA: protected is read; whitespace-only names and values count as
    absent; the edit field of a labelled combo box has no label of its
    own; list view and tree view items report their position ("1 of
    3").
  - Roles: both backends now map every role NVDA's tables give a
    counterpart for in Verbatim's vocabulary, including split button,
    graphic, progress bar, scroll bar, table, row, cell, headers, data
    grid, document, title bar, tool tip, and separator; a UIA document
    is a "document", not an "edit" (Notepad's text area is "Text editor
    document"). Progress bars and title bars are never focus context.

  **Different**, still, from the same audit:
  - On focus NVDA reads an edit field's selection or the line at the
    caret through its text interface; Verbatim, with no text interface
    yet, speaks the field's whole value.
  - A multi-column list view item (a report view, such as msinfo32's
    right pane) is named by NVDA from its column texts, with no value
    or description; Verbatim keeps MSAA's name and description, since
    reading column texts needs a cross-process read not yet written.
  - UIA read-only state from a text pattern's document range, which
    NVDA falls back to when `ValueIsReadOnly` is unsupported.
  - Events NVDA handles that Verbatim does not subscribe to:
    description changes (MSAA `EVENT_OBJECT_DESCRIPTIONCHANGE`, UIA
    `HelpText`), UIA live region changes and system alerts, and UIA
    elements added to or removed from a selection. NVDA also speaks
    state changes on the focus's ancestors, where Verbatim speaks them
    only on the focus.
  - Dialog text, which NVDA reads on entering a dialog (see the
    role-shaped behavior layer below).

- Focus-ancestry context: announce newly entered presentable
  containers before the control. NVDA: `focusEntered` +
  `isPresentableFocusAncestor` ([Event handling](nvda/events.md),
  [Object model](nvda/object-model.md)). Verbatim: **matched (unverified)** —
  filter claimed at parity with `_get_isPresentableFocusAncestor`
  (exclusion-based, same role exclusions). Since 2026-10-03 an entered
  container is spoken as NVDA speaks focus entered: as a focus, states and
  position included, without its value, level, or (except a list) keyboard
  shortcut; it had been spoken with only its name, role, and description.
  Except:
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
  attention without speaking (a bare "window" says nothing), though it
  cancels speech, as NVDA's foreground event does.
  **matched (unverified)**; the Start menu and window-switch scenarios
  verified the earlier, separate announcement.
- Duplicate focus suppression (same control announced once when two
  paths report it). NVDA: "already the focus" early return, comparing
  the focus object by identity only. Since 2026-10-02 Verbatim compares
  the node id too, not its states, name, or ancestors, which a second
  report can read mid-change (Notepad's edit control settling, File
  Explorer's title being filled in), and keeps the newer reading. Since
  2026-10-03 the selected item inside a list is not compared either (it
  had made a repeated focus with another selected item announce the whole
  focus again); a new selected item is announced by its selection event.
  Verbatim: **matched (unverified)** ([verbatim-core](crates/verbatim-core.md),
  M3 noise suppression). Unchanged by the outpost redesign: only Core
  sees focus across applications, so this stays in the reducer.
- Stale focus events: NVDA has no timestamp arbitration; it relies on
  queue-time freshness plus cancellable speech
  ([Event handling](nvda/events.md), [Speech](nvda/speech.md)), and its
  one queue handles every application's events in the order observed.
  Verbatim: snapshot versions are gone, and order within an application
  comes from its outpost's single queue and worker, as in NVDA. Across
  outposts, events can arrive out of the order they were observed in, so
  since 2026-10-02 the reducer drops a focus event from one outpost
  observed before the newest focus it applied from another (found live:
  a late focus from Notepad, then from Verbatim's closed menu, broke
  `notepad_and_verbatim_menu`). A UIA focus fact whose element is not the
  focused element is reported from the fact when its window is in the
  foreground, or with no window facts, judged by its application, when
  it has no window of its own. Since 2026-10-03 a focus read on request
  (at startup, after an outpost is replaced, or after a menu closes)
  carries the time its read began, so it is ordered with the events as
  NVDA's queue orders the focus it reads; it had carried no time, so it
  was never judged stale and did not count as the newest focus. A focus
  with no window facts is taken to be in the attended window, since only
  that let it be accepted, so a later foreground report for that window
  does not replace the control as the focus (NVDA always knows the
  focus's window). **matched (unverified)**; the
  `rapid_tabbing_in_settings` and `notepad_and_verbatim_menu` scenarios
  cover it.
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
  one cancels current speech (`Effect::StopSpeech`) and it is not
  announced. **matched (unverified)**. **Different:** NVDA cancels as it
  reaches the menu among the entered ancestors, outermost first, so a
  container entered outside the menu in the same focus change (a newly
  entered window, say) is cut off too; Verbatim cancels before any of the
  new focus's speech, so that container is still spoken.
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
  alerts and the shell's window-snap results, which are always queued,
  as NVDA's Explorer module queues them; other UIA notifications only
  from the focus's application, as NVDA drops notifications from any
  other (since 2026-10-03; Verbatim had judged them by the attended
  application, which differs in Settings, where ApplicationFrameHost
  holds attention and the focus is in SystemSettings). Accepted background events
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
- When speech is cut off, and cancellation of expired focus speech
  (focus left before speaking). NVDA: focus speech is queued, never
  interrupting; every key-down cancels speech before its gesture runs,
  bound or not, modifiers and typed characters included, except the
  volume keys and the unknown key `0xFF`; Shift alone pauses and resumes,
  its auto-repeat ignored; speaking while paused cancels first, and a
  cancel ends the pause ([Keyboard input](nvda/input.md), "What a key
  press does to speech"). A foreground change, which NVDA infers whenever
  the top of the focus ancestry changes, cancels speech, named window or
  not, and so does entering a menu bar, menu, or menu item. On each focus
  change, speech for a focus no longer current is culled
  (`FocusLossCancellableSpeechCommand`, which keeps speech for the
  focus, its ancestors, the foreground object, an object that never had
  the focus, and a menu item when the focus has moved to a popup menu;
  [Speech](nvda/speech.md), "Expired focus speech", and
  [Event handling](nvda/events.md)). Verbatim: **matched (unverified)**
  since 2026-10-04: focus, value, state, selection, navigation, and
  review speech is queued; the keyboard hook cancels, or pauses and
  resumes, on the key-down before the gesture is sent, with NVDA's
  exceptions, and a gesture injected through the control plane cancels
  too, while the keys Verbatim injects for itself (the Control tap that
  unlocks the foreground) leave speech alone, as NVDA ignores its own; a foreground report or a focus in another top-level window cancels,
  as does entering a menu; each focus announcement carries what it is
  about, entered containers are their own utterances, and on every focus
  change the speech manager stops what it has handed on when any of that
  has expired, and judges waiting focus speech when its turn comes, as
  NVDA does. A selection in a list the focus
  controls still interrupts, as NVDA cancels for it, and notifications
  keep their processing hint. Reducer, input, and speech pipeline tests
  cover the rules, and the end-to-end scenarios pass with them, but no
  scenario asserts where live speech is cut off yet.
  **Different:**
  - Audio cannot be taken out of the middle of the mixer's buffer, so
    when anything handed to the synthesizer or the mixer has expired,
    everything handed on is stopped; NVDA stops up to the newest expired
    utterance and keeps what was handed on after it.
  - The validity check has no clause for a menu item whose focus has
    moved to a popup menu (NVDA issues 12624 and 14550, MSAA only);
    `FocusValidity::holds` keeps speech for the focus, its ancestors, the
    foreground window, and a node that never had the focus.
  - The foreground node the check uses is the window most recently
    reported as the foreground; NVDA asks for the real foreground object
    on every focus change that changes the top of the ancestry, so a
    focus that reaches another window without a foreground report leaves
    Verbatim's foreground node on the old window until one arrives.
  - **Not yet:** NVDA's settings "Speech interrupt for typed characters"
    and "Speech interrupt for Enter" (both on by default) are not
    configurable; Verbatim always behaves as their defaults do. NVDA's
    advanced setting to turn the culling of expired focus speech off is
    not offered either.
- Menu popup announcements. NVDA: menu events with fake-focus
  fallback ([MSAA and winevent handling](nvda/msaa.md)). Verbatim:
  NVDA's menu rules run in the outpost's worker. Within a batch, focus
  events are handled first and the newest menu opening from each
  backend last; a menu opening is ignored if a focus in the batch
  already put focus on a menu or menu item, and otherwise becomes a
  focus on the popup menu, so the reducer receives only focus events
  for menus. A menu closing, menu mode ending, or the Alt+Tab
  switcher closing anywhere on the desktop is forwarded by the focus
  listener to Core 50 milliseconds later, with the time it closed; unless
  a focus observed after the close has been applied by then, Core asks
  the application in the foreground for its real focus, as NVDA's fake
  focus reads the focus from the foreground window when no focus followed
  the close (since 2026-10-03; the listener had checked only its own
  focus events). Focus usually returns to another application than the one
  that owned the menu, which a check inside the menu's own outpost
  could not see. **matched (unverified)**; the Start menu path was
  verified before the redesign.
- Toggle button role and "pressed"/"not pressed" wording (UIA Toggle
  pattern on a Button; no separate Switch role). NVDA: UIA
  detection. Verbatim: **matched (verified)** (cross-process test;
  commit c72afd8).
- Negated states: "not checked", "not pressed", and "not selected", by
  the rules in "Which states are spoken, and in what order" in
  [Speech](nvda/speech.md). Verbatim: **matched since 2026-10-02**
  against NVDA's source (the audit above); needs a live NVDA
  comparison across roles.
- Selection announcements (focused list's selected child; changes
  while focus stays on container; combo box exclusion). NVDA:
  selection events. Verbatim: **different, deliberately** (the roadmap's
  M3 "announce a focused list's selected item"): NVDA has no rule that
  speaks a newly selected item while focus rests on a list or tab
  control; its base `event_selection` speaks only a selection inside an
  element the focus controls, and every other selection is a change of
  state, spoken only for the focus or its ancestors. Selecting the focused
  item itself is such a change of state, and since 2026-10-03 Verbatim
  speaks it ("selected"), as NVDA does ([verbatim-core](crates/verbatim-core.md)).
- Selection in a list the focus controls (search suggestions and
  results). NVDA: when an item is selected inside an element the focus
  names in its UIA ControllerFor relation, NVDA reports that item as it
  reports a focus, without moving focus; this is how it reads the Start
  menu's search results and the Settings app's search suggestions as
  the user types and arrows ("Selection in a list the focus controls"
  in [Event handling](nvda/events.md)). Verbatim: **matched since
  2026-10-03** for UIA, verified live on the Settings app's search
  suggestions ("Display settings 1 of 25" and so on as the user arrows,
  with the focus staying in the search box), with reducer tests. Found
  on 2026-10-02, when Verbatim read neither while NVDA read both. Start's
  results are partly in a Chromium document NVDA reads through IA2,
  which belongs to the phase that implements IA2 (browsers).
- Value change on focused node speaks bare value (slider drag), not
  for an edit field or document, not when unchanged, and not for a role
  that never speaks its value. Verbatim: **matched since 2026-10-02**
  (source-checked). Background progress bar
  reporting (NVDA option): **not yet**.
- State-change diff announcements (gained states; lost states spoken
  by their absence, including half checked becoming "not checked").
  Verbatim: **matched since 2026-10-02** (source-checked).
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
  Verbatim: **not yet**: it has the alert role (UIA and MSAA map to it),
  but no alert events, so it reports no other alerts.
- Live regions (browsers). NVDA: in-process IA2 machinery
  ([IA2 usage](nvda/ia2.md)). Verbatim: **not yet (M6)**.

## Object navigation and review

- Navigation tree shape. NVDA default: simple review on (filtered
  tree, [Focus and the navigator](nvda/focus-and-navigator.md)). Verbatim: full tree,
  matching NVDA with simple review **off** — the project's stated
  baseline (maintainer decision; commit 6b7a519). **matched
  (verified)** against that baseline; simple-review-on filtering is
  **not planned** (revisit only if the baseline changes).
- The API at a window boundary. NVDA: an object reached by navigation, or
  as a focus ancestor, in a different window is read through that
  window's API (`correctAPIForRelation`): an MSAA object in a UIA window
  becomes the window's UIA element, and a UIA element that is another
  window's root, in an MSAA window, becomes that window's MSAA object.
  Verbatim: **matched since 2026-10-03**, for object navigation and for
  focus ancestry in both directions; the UIA-to-MSAA direction of focus
  ancestry was already matched. Checked live in Notepad's Save As dialog:
  the file name box's ancestry continues through the shell's UIA view,
  and navigating to its parents switches to UIA there. Until 2026-10-03
  an MSAA walk or navigation step stayed in MSAA across windows.
- Navigator follows focus; review follows navigator. NVDA coupling
  rules ([Review modes](nvda/review-modes.md)). Verbatim: **matched
  (unverified)** for the follow-focus default; `followCaret` /
  `followMouse` equivalents **not yet (M4+)**.
- Report current object: report / spell / copy on 1st/2nd/3rd press.
  NVDA: script repeat counting ([Keyboard input](nvda/input.md)), reading
  the object live. Verbatim: **matched since 2026-10-03**; since
  2026-10-03 a name, value, or state change on the focus also updates the
  navigator's copy while it rests there, so the report reads the object
  as it is now (it had read the copy taken when focus landed, saying "not
  checked" for a box just checked). The spelled text and the copy wording
  are covered under the messages entries below.
- Review and navigator messages and repeated presses (the list in
  [Review modes](nvda/review-modes.md), "Reading commands built on
  review"). Verbatim: **matched since 2026-10-03**: the edge messages
  ("Top", "Bottom", "Left", "Right") with the unit read again, character
  motions kept within the line, "blank", the current line or word spelled
  on a second press, the current character's code on a third, "space" in
  spelling, report current object spelling the name and value, "Move to
  focus", "No navigator object", and activation saying "Activate" or "No
  action" after walking up the parents. A copy says "Copied to clipboard:"
  with the text (its length from 1024 characters on) after reading the
  clipboard back, or "Unable to copy". **Not yet:** the current character's
  description on a second press and spelling with descriptions on a third,
  which wait for the character descriptions table (M4). Raised pitch for
  capitals is **matched since 2026-10-04** (unverified by ear): spelling
  and reading a single character speak an uppercase letter with the pitch
  setting raised by 30 and then restored, for every synthesizer, as NVDA
  does by default ([Speech](nvda/speech.md), "Capitals when spelling");
  the offset is not yet configurable, and saying "cap" or beeping for
  capitals is not offered. eSpeak NG and OneCore speak the pitch change
  within one synthesis, as SSML prosody written as NVDA's drivers write
  it, so a capital brings no pause; a synthesizer that cannot is given
  the change as a pitch setting between separate synthesis calls, which
  can leave a short pause.
- Toggle key announcements ("caps lock on", "num lock off", "scroll lock
  on") when a lock key reaches the operating system, including Caps Lock
  passed through by a double tap of the Verbatim key. NVDA:
  `KeyboardInputGesture.reportExtra`. Verbatim: **matched since
  2026-10-03**, verified live with Num Lock.
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
  (system_information_tree E2E; commits 809214d, 1078614).
- Navigator death recovery: NVDA reports failure and stays; Verbatim
  re-seeds navigator from focus on `Gone` and announces it —
  **different (documented in [verbatim-core](crates/verbatim-core.md))**; NVDA-side
  behavior in [Focus and the navigator](nvda/focus-and-navigator.md).
  A navigation the application did not answer (too slow, or the read
  failed) is not `Gone`: since 2026-10-03 the navigator stays where it is,
  as NVDA's stays when a call to a busy application is cancelled; it had
  jumped to the focus and announced it.
- Review cursor line/word/character over object text. NVDA: object
  review over TextInfo ([Review modes](nvda/review-modes.md)). Verbatim:
  **partial**: the motions, messages, and repeated presses match (the
  messages entry above); the text is the object's flat value or name
  rather than a text model, and the review position starts at offset 0
  rather than at the caret, until M4.
- Document review and screen review modes. Verbatim: **not yet
  (M6)**; screen review will be tree-projection **different (D11)**.
- Object activation (do default action). NVDA: the review position's
  or navigator's action, walking up the parents until one has an action,
  then its name spoken, or "No action". Verbatim: **matched since
  2026-10-03**: an MSAA object's default action is spoken by its own name
  ("Press"), a UIA element's Invoke as "invoke", and an action with no
  name as "Activate"; a UIA element is activated by Invoke, then Toggle,
  then selecting it, NVDA's order (Verbatim had tried MSAA's default
  action through UIA in place of selecting). NVDA first tries the review
  position's own activation, which the M4 text model brings.
- Move focus to navigator / caret routing. Verbatim: **not yet
  (M4)**.

## Backends

- Dual-stack MSAA+UIA with per-window arbitration. NVDA: the
  `isUIAWindow` referee with good/bad class lists
  ([The UIA client](nvda/uia.md)). Verbatim: **matched since
  2026-10-03**, in NVDA's order: the class name is normalized as NVDA
  normalizes it (its class map, and the Windows Forms and `ATL:` wrappers
  removed, so a Delphi `TEdit` or a Windows Forms edit counts as an
  `Edit`); NVDA's good classes are UIA; so is a Windows 11 shell window,
  recognized by its root ancestor's class, except the Start button; NVDA's
  bad classes are MSAA; otherwise the window is probed. A window with a
  provider is still read through MSAA when NVDA would not use the
  provider: a console whose text does not report formatting (older
  consoles; the Windows 11 console reports it and stays UIA, checked
  live), and a list view outside Windows Forms (a Windows Forms list view
  was recognized live). Until 2026-10-03 the class was not normalized and
  the shell rule tested the window's own class. **Not yet (M6):** NVDA's
  Word, Excel, and Chromium exceptions, which set UIA aside only when NVDA
  has injected its in-process helper; without it NVDA uses UIA for them,
  as Verbatim does, and they arrive with Verbatim's helper (decision D2).
  **Not carried:** the Office 2013 and older ribbon rule (`NetUIHWND`),
  for Office versions Microsoft no longer supports, and the rule against
  NVDA's own process, which guards against a freeze of UIA inside the
  screen reader's process that outposts cannot have. Application modules'
  own good and bad windows wait for extensions, except the Explorer shell
  rule above, which is core policy (roadmap M3). A probe
  that finds a UIA provider is kept for the window's lifetime, one that
  finds none for 500 ms, NVDA's cache period. Checked live on
  2026-10-02 across about 65 windows of Explorer, Settings, Start, and
  the desktop: no window's answer changed from a provider to none
  during its life. **Different from NVDA, since 2026-10-03:**
  `UiaHasServerSideProvider` reports no provider when the window does
  not answer in time (three seconds for Windows 11 Notepad's text control
  while Notepad was starting, five for a stalled mockapp window), and
  NVDA takes that as the answer, reading the focus through MSAA ("edit")
  until it moves. Verbatim counts only the window's own answer: a "no"
  slower than a second (real answers took 0 to 89 ms) makes the probe
  wait for the window to process messages and ask again. Found as the
  cause of `notepad_and_verbatim_menu` failing about one run in five; the
  dropped UIA focus had come from Notepad's own provider, and a check 18
  ms after the slow "no" answered "yes". The same failure had a second
  form: UIA itself gives up on a provider after two seconds by default
  (neither NVDA nor Verbatim had changed it), and reading Notepad's
  focused element then returned UIA's stand-in for the window, a
  nameless edit, instead of the "Text editor" document. Verbatim's UIA
  client waits ten seconds, the deadline its watchdog already holds each
  read to.
- How an outpost turns events into focus reports, from the audit of
  2026-10-03 (all **matched since 2026-10-03** unless marked otherwise):
  - A UIA focus is built from the event: its name, role, value, and
    states come from the properties the event delivered, as NVDA builds
    the focus object from the event's sender, and it is accepted only
    when those say the element has the keyboard focus
    (`shouldAllowUIAFocusEvent`). Until 2026-10-03 the outpost read the
    focused element live, took the focus from that read, and dropped the
    focus when the read failed; under a busy application the read blocked
    for more than ten seconds or returned UIA's stand-in for the window,
    so the focus was announced wrongly or not at all.
  - **Different, because of the outposts:** the event's element is in the
    listener's process and cannot cross to the outpost, which reads its
    own copy of it for the focus's ancestors and for navigation. That read
    waits at most a second; without it the focus is still reported, with
    its ancestors unknown (the reducer then announces no containers and
    keeps the previous chain), and a follow-up finds the element later so
    the focus's property changes are still followed. NVDA has the element
    from the event and never waits for it.
  - A focus's ancestors are read only until they meet the previous
    focus's chain, whose rest is reused, as NVDA's focus ancestry does.
    **Different:** reading the rest is limited to two seconds, after which
    the ancestors are reported unknown rather than the focus being held
    back; NVDA waits.
  - An MSAA focus is accepted only when the object or one of its
    ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`).
  - When the newest focus event of a batch cannot be reported (unreadable,
    destroyed, refused), the next older one is tried, up to three, as
    NVDA's event pump falls back.
  - The fake focus after a menu or the Alt+Tab switcher closes is skipped
    only when a focus observed after the close was actually applied; any
    focus fact used to cancel it, even one later dropped or unreadable.
  - NVDA's early `WinEvent` filters: object ids at or below `OBJID_ALERT`,
    a focus on a menu bar object itself, foreground events for Program
    Manager and the taskbar, and menu events from the IME candidate
    window are ignored. Selection add, remove, and within events are
    changes of state, not new selections, so a deselected item is no
    longer announced as selected.
  - Events are judged by their own application's window, never the
    system's focus window, which can belong to another application; UIA
    notifications are not arbitrated, as NVDA does not arbitrate them.
  - A provider probe stays within the outpost's deadline; a window that
    does not answer is read through MSAA for the event at hand and probed
    again next time, as NVDA treats a cancelled probe.
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
  speech ([Speech](nvda/speech.md)); almost all speech is NORMAL, and
  what cuts it off is a cancel, not a priority. Verbatim: **partial** —
  Queued/Next/Interrupt exist, and since 2026-10-04 announcements are
  `Queued` as NVDA's are, with key presses, foreground changes, menus,
  and expired focus speech cutting speech off (see "When speech is cut
  off" above); `Interrupt` remains for a selection in a list the focus
  controls, notifications that ask for it, and a few direct messages such
  as the time. NVDA's *resume of interrupted lower-priority speech* is
  **not yet**: Verbatim's Interrupt discards. Decide whether to match
  before M8 profiles work.
- Index marks driving callbacks at audible position. NVDA: manager
  indexing + WASAPI feed-end callbacks ([Audio output](nvda/audio.md)).
  Verbatim: **matched (unverified)** — the mixer reports each mark
  when the device has played it, for every synthesizer (D17); say-all
  (the main consumer) is **not yet (M4)**.
- Structured utterances vs flat strings. NVDA: command-laden flat
  sequences. Verbatim: **different (D12)** — typed spans flattened
  by a theme at the last stage.
- Speech settings model (driver settings, immediate application).
  NVDA: `SynthDriver.supportedSettings`. Verbatim: **matched
  (unverified)** — same descriptor-driven model. Starting a synthesizer,
  at startup or from the Select Synthesizer dialog, loads its own saved
  settings, voice first; a saved value it refuses, such as a voice no
  longer installed, is logged and the synthesizer keeps its own value
  rather than failing to start (docs/nvda/synth-drivers.md). When the
  configured synthesizer cannot start, Verbatim tries the others,
  eSpeak NG first, and logs which it used; a failed switch keeps the
  previous synthesizer, as NVDA does. **Different:** NVDA applies the
  refusal fallback to the voice only and fails the synthesizer on other
  refused settings, where Verbatim skips any refused setting. NVDA's last
  resort is a silent synthesizer, so it always starts; Verbatim has none
  and does not start when no synthesizer can. NVDA writes a corrected
  voice back to the config at once, where Verbatim saves it with the
  next commit of the settings dialog. A synthesizer used as a fallback
  has its settings saved but is not saved as the configured one, so the
  user's choice is tried again at the next start, as in NVDA; choosing a
  synthesizer in the dialog makes it the choice. The default voice is
  the synthesizer's own default, where NVDA picks one matching its or
  Windows' language. The settings
  dialog's sliders use wx's default steps rather than each setting's
  minimum and large steps, which gives the same steps for settings from 0
  to 100. Segments are joined with one space where NVDA joins chunks with
  two, which is not audible.
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
  **Different, deliberately:** Caps Lock is a Verbatim key by default (see
  "Spoken vocabulary and key layouts"). **Not yet:** NVDA treats Num Lock
  as a modifier of the numpad operator keys, so a binding of plain
  numpad plus does not take the plus sign from a user with Num Lock on;
  Verbatim ignores Num Lock, which matters once such a binding exists
  (say all, M4).
- Script repeat counting. NVDA: `scriptHandler` counts each run of the
  same script within the multi-press timeout, and forgets the last script
  when an unbound gesture comes between. Verbatim: **matched since
  2026-10-03** for the reset on an unbound key (it had carried the count
  across). **Different, deliberately:** auto-repeat of a held key is not
  counted as presses, so holding a key never turns into a double press;
  NVDA counts it.
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
