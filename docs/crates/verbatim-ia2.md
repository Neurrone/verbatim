# verbatim-ia2

The minimal MSAA client (IA2 interface acquisition is an explicit seam
left for M6, landing with the browsers that motivate it; M3's backend
parity work covers MSAA and UIA only).

Public API:

- `WinEventHook::install(target_pid, kinds, callback)` — out-of-context
  WinEvent hooks for a caller-chosen set of event kinds, scoped to one
  process id (or global for pid zero). A thread holds one set at a time,
  since its hooks share one callback; a second install on the same thread
  fails. Two constant sets name the two
  callers (decision D13): `APP_SUBSCRIPTIONS`, what a per-application outpost
  installs — value, state, name, and selection changes
  (`EVENT_OBJECT_SELECTION` is `WinEventKind::Selection`; the selection add,
  remove, and within events are reported as `WinEventKind::StateChange`, as
  NVDA handles them), and since milestone M4 the caret
  (`WinEventKind::Caret`: `EVENT_OBJECT_LOCATIONCHANGE` on `OBJID_CARET`,
  the system caret NVDA follows in edit controls; every other object's
  location change, which windows raise constantly, is dropped at the hook)
  and text selection changes (`WinEventKind::TextSelectionChange`,
  `EVENT_OBJECT_TEXTSELECTIONCHANGED`); and `LISTENER_SUBSCRIPTIONS`, what the focus listener installs
  globally — focus (`EVENT_OBJECT_FOCUS`), foreground
  (`EVENT_SYSTEM_FOREGROUND`, the `WinEventKind::Foreground` variant that
  absorbs Core's old foreground trigger), menu-popup opens
  (`EVENT_SYSTEM_MENUPOPUPSTART`), and the end of a menu or of the Alt+Tab
  switcher (`WinEventKind::MenuEnd` and `WinEventKind::SwitchEnd`), which
  is global because focus returns to whichever application is then in
  front. A popup menu opening announces the menu
  itself the moment it opens, NVDA's menu-start behavior — the app outpost
  emits it as focus on the menu's client object with no ancestry, the
  identical node its foreground-announce fallback produces for a menu-class
  window, so the reducer's duplicate-focus suppression drops whichever path
  announces second. Callbacks are delivered on the installing thread's
  message loop and must never make blocking calls into the target.
  `WinEventKind` names the event; drop unhooks.
- `dialog` — `DialogObject`, the objects a dialog's own text is gathered
  from (`docs/nvda/object-model.md`, "A dialog's own text"; the gathering
  is `verbatim-outpost`'s `dialog_text`). `DialogObject::of_node(node,
  registry)` is the kept object behind a dialog this outpost reported;
  `children()` reads its children in order (`accChildCount` and
  `AccessibleChildren`), standing in a window's client area for each child
  that is the window object of another window, as NVDA does, and knowing a
  hidden or disabled window from `IsWindowVisible` and `IsWindowEnabled`
  without acquiring its client area; `role`, `states` (with multi-line from
  an edit control's window style), `invisible`, `name`, `value`, and
  `description` each read their property the first time they are asked,
  and the first three keep it, since the gathering looks at a child's
  neighbors too.
- `edit` (milestone M4) — the standard Win32 edit and rich edit controls'
  text through their window messages, ported from NVDA's `EditTextInfo`
  (this crate is GPL like NVDA). `edit_api_version(normalized_class)`
  gives NVDA's edit API version for a class name normalized by NVDA's
  class map: 0 for `Edit`, 1 for `RichEdit`, 2 for `RichEdit20` and
  `REComboBox20W`, 5 for `RICHEDIT50W`. `EditControl::new(hwnd, version)`
  then answers `selection`, `set_selection`, `line_from_offset`,
  `line_start`, `line_length`, `line_count`, `text_length`, `line_text`,
  `text_range`, `find_word_break`, `position_of` (screen coordinates), and
  `is_password`, each an `EditResult` whose `EditError` is `Gone` when the
  window no longer exists and `Failed` otherwise. Offsets are the
  control's UTF-16 code units. Which message is sent follows the version,
  as in NVDA: a plain edit control answers `EM_GETSEL` (two `DWORD`
  pointers Windows marshals across processes), `EM_SETSEL`,
  `EM_LINEFROMCHAR`, `EM_GETLINE` (a buffer Windows marshals, its size in
  the first word), and, for a range, `WM_GETTEXT` of the whole text cut
  down, as NVDA reads it; a rich edit control answers `EM_EXGETSEL`,
  `EM_EXSETSEL`, and `EM_EXLINEFROMCHAR`, and from version 2
  `EM_GETTEXTRANGE`, `EM_GETTEXTLENGTHEX`, and `EM_FINDWORDBREAK`, with
  `EM_POSFROMCHAR` taking a point structure from version 1 and from 3 on.
  Windows does not marshal those structures (`CHARRANGE`, `TEXTRANGEW`,
  `GETTEXTLENGTHEX`, `POINTL`), so they are written into memory allocated
  in the control's process (`VirtualAllocEx`, `WriteProcessMemory`,
  `ReadProcessMemory`, freed on drop), with the text range's pointer field
  sized for that process: four bytes for a 32-bit (WOW64) process, by
  `IsWow64Process2`. An ANSI rich edit window's text is converted from the
  system code page. Every message goes through `SendMessageTimeoutW` with
  `SMTO_ABORTIFHUNG` and half a second's wait, and counts as one window
  message; the memory calls are the kernel's and are not counted. A
  password field (`ES_PASSWORD`) reads as stars, as NVDA reads it.
- `acquire` — the query-pool side: `snapshot_from_event` (from
  `AccessibleObjectFromEvent` through name, role, value, state,
  description, keyboard-shortcut, and location reads to a `NodeSnapshot` —
  the `NodeDetails` half plain MSAA can express; position-in-set is
  counted for list-view and tree-view items, as described below, and is
  otherwise `None` until IA2's `groupPosition` (roadmap M6)),
  `snapshot_from_focus_event` (the
  focus-specific entry the outpost uses for `EVENT_OBJECT_FOCUS` addresses,
  applying NVDA's `processFocusWinEvent` child-0-on-a-list redirect: when a
  focus event names a list on its own object — child id 0, MSAA role
  `ROLE_SYSTEM_LIST` — or the client of a `SysListView32` window, it reads
  `accFocus` and redirects to the named child when that child is real and
  different, so a container that fires focus on itself, like the wxWidgets
  generic list, announces the focused item rather than the container; the
  `accFocus` VARIANT is parsed in one shared place, `read_acc_focus`, which
  `focused_snapshot` also uses, so the child-id and child-object forms are
  handled once), `focused_snapshot` ("what is focused right now" via `GetGUIThreadInfo`,
  for the synthetic focus event an outpost emits after a foreground
  change), and the node-relative operations, which take a `NodeId` and
  read through the object the registry kept for it (a node issued from
  local window data, which has no object, is acquired at its address).
  `navigate`, `activate`, and `ancestor_chain` answer `AcquireError::Gone`
  when the node is no longer kept, its window no longer exists, or its
  object has disconnected, and `selected_child` answers `None`:
  `ancestor_chain`, and `ancestor_chain_until` with `AncestorLimits`
  (stopping at a known ancestor, a deadline, or a parent in a different
  window that `read_by_other_api` says is read through UIA, and saying
  which as `Walked`, whose `Crossed(hwnd)` lets the outpost continue the
  walk through UIA) (per-hop `accParent` walks, outermost first, with the simple-child
  special case its doc explains — a bare child id has no `accParent` of
  its own, so its first hop is the object it is a child of; MSAA has no
  remote-ops analog, so unlike UIA's equivalent this stays the permanent
  implementation), `navigate` (parent via `accParent`, siblings and first
  child via `accNavigate`, where a result that is the same COM object
  counts only when its child id moves the right way and another object
  taken as a first child must be in the object's own window or one inside
  it, with `AccessibleChildren` asked when `accNavigate` finds no first
  child, as NVDA does;
  returns `Ok(None)` for a genuine edge and
  `Err(AcquireError::Gone)` when the source node itself is no longer
  reachable, so the outpost can report `Gone` rather than a fake edge),
  `selected_child`, and `activate` (`accDoDefaultAction`, MSAA's only
  activation primitive, answering the `accDefaultAction` name read first). Every snapshot is minted through `node_for`,
  which matches a new sighting against the kept nodes in NVDA's
  comparison order (`docs/parity.md`, "Held objects"): the same COM
  object (by its canonical `IUnknown`, confirmed against the kept object)
  with the same child id in the same window and the same role, else, for
  an object acquired at its address, a kept node acquired at the same
  address with the same role and, when both objects offer one, the same
  `IAccIdentity` string; anything else is a new node that keeps the
  object just read. Each snapshot is read with its provenance: objects
  acquired at an event or window address, and children by id on them,
  are at their address; objects reached through `accParent`, through
  `accNavigate`, or as child objects are not, and never claim the address
  made up for them. Two seams
  mirror NVDA where plain MSAA navigation would mislead. A `SysTreeView32`
  item's navigation and ancestor chain route through the tree control's
  own `TVM_GETNEXTITEM` relations (with the accid-to-htreeitem mapping
  messages and their pre-v6 comctl32 fallback), because MSAA exposes every
  visible tree item as a flat sibling list under the control — parent,
  siblings, and first child would otherwise all answer the visible-order
  neighbor instead of the logical one; a tree item's `accValue` is its
  0-based indent depth, not a value, so `read_snapshot` reads it into the
  snapshot's level as it is, a root item at level 0, and leaves the value empty, again matching
  NVDA. And a window-root object — the window face every windowed control
  exposes alongside its client object, keyed under `OBJID_WINDOW` (not
  `OBJID_CLIENT`, so the two faces of one hwnd get distinct node ids
  rather than colliding) — navigates the Win32 window hierarchy rather
  than `accNavigate`/`accParent`: parent is `GA_PARENT`, siblings are the
  next and previous visible top-level child windows
  (`GW_HWNDNEXT`/`GW_HWNDPREV`), and the first child is the first visible
  child window or, for a leaf control, its own client object. This is
  NVDA's `Window`/`WindowRoot` navigation, and it is what makes
  parent-then-sibling-then-child navigation between the controls of a
  dialog work: MSAA's own answers there are the control's scroll-bar and
  client pieces, not the sibling controls.
- `map` — `role_from_msaa` and `states_from_msaa`, the tables from
  MSAA constants to the normalized vocabulary, following NVDA's MSAA
  role and state tables (so `STATE_SYSTEM_DEFAULT` is dropped and
  `STATE_SYSTEM_PROTECTED` kept), pinned by unit tests against raw state
  words captured from live controls. Reading a snapshot also treats a
  whitespace-only name or value as absent, drops the name of the edit
  field inside a labelled combo box, and gives a list view or tree view
  item its position, from `LVM_GETITEMCOUNT` or by counting its siblings
  with `TVM_GETNEXTITEM`, as NVDA does.
- `calls` — the count of the cross-process calls `acquire` makes, kept per
  thread like `verbatim-uia`'s: `calls::count(kind)` and `calls::take()`.
  Every `IAccessible` method, `IAccIdentity`'s identity string, the
  acquisitions (`AccessibleObjectFromEvent`, `AccessibleObjectFromWindow`,
  `AccessibleChildren`, `WindowFromAccessibleObject`), and a
  `QueryInterface` for any interface but `IUnknown` on an object from the
  application count as MSAA calls, one per API call however many round
  trips it makes inside; the list view and tree view messages count as
  window messages. A snapshot read is seven calls (six properties and the
  location), plus the role and identity reads that match a sighting to a
  kept node. Local window functions on the application's handles are not
  counted (`docs/performance.md`, "What counts as a call").
- `accessible` and `window` (private) — the safe wrappers `acquire` is
  written against, so that `acquire` holds no `unsafe` code. `unsafe`
  lives only in these two modules and in `hook`, the WinEvent hook's install,
  removal, and callback; each block carries a `SAFETY` comment. `Accessible` is an
  `IAccessible` with the child id it is read at, and each of its methods
  holds one `IAccessible` or `oleacc` call and counts it as one MSAA call,
  so a caller never counts anything itself:
  - `window` (`WindowFromAccessibleObject`) answers `None` when the call
    fails or finds no window, which it does for an object none of whose
    ancestors names its window (mockapp's); callers then keep the window
    they are in, rather than an address in window 0.
  - `from_event` (`AccessibleObjectFromEvent`) and `client_of_window`
    (`AccessibleObjectFromWindow` for `OBJID_CLIENT`) acquire one;
    `new`, `with_child`, and `child` build and inspect one without a call.
  - `name`, `value`, `description`, `keyboard_shortcut`, and
    `default_action` read the text properties; `role` and `state` the raw
    role number and state word; `location` the screen rectangle; and
    `do_default_action` activates.
  - `parent` (`accParent`, then a counted `QueryInterface` for
    `IAccessible`), `child_count`, `children` (`AccessibleChildren`),
    `navigate` (`accNavigate`), `focus` (`accFocus`), `selection`
    (`accSelection`), `window` (`WindowFromAccessibleObject`), and
    `identity_string` (a counted `QueryInterface` for `IAccIdentity`, then
    `GetIdentityString`).
  - `canonical` (the `IUnknown` identity) and `agile` (an agile reference
    for the registry) are local and not counted.

  The methods that name another object answer a `Related`: nothing, a
  child id, an object, or another form. Its `object` method asks an
  object for `IAccessible`, a counted `QueryInterface`, only when the
  caller uses it, so walking `AccessibleChildren`'s entries casts only the
  entries it visits. The `VARIANT` union reads behind `Related` stay in
  `accessible`. `window` wraps the local window functions (`IsWindow`,
  `GetClassNameW`, `GetAncestor`, `GetWindow`, `GetTopWindow`, `IsChild`,
  `IsWindowVisible`, `GetDesktopWindow`, `GetGUIThreadInfo`), which are not
  counted, and sends the list view and tree view messages
  (`LVM_GETITEMCOUNT`, `TVM_GETNEXTITEM`, and the two child-id mapping
  messages), each counted as a window message; it offers only messages
  whose parameters are plain integers.
- `NodeIdRegistry` — also carries the details a read includes
  (`fetches` and `set_fetches`, a `verbatim_model::Fetches`, everything by
  default): a snapshot skips `accDescription`, `accKeyboardShortcut`, the
  list and tree position reads, and a tree item's level when the active
  theme reports them as off. And the nodes the outpost has issued, each with its
  address (window handle, object id, and child id), the role read when it
  was issued, and the accessible object it was read from, kept as an agile
  reference; it shares the outpost-wide counter with the UIA registry. It
  only stores and looks up, and never makes a COM call under its lock;
  `acquire` makes the comparisons. `retain` releases the nodes the outpost
  no longer needs (objects are dropped after the lock is released, since
  releasing one can call into its process), `forget_window` drops a
  destroyed window's nodes so a reused handle never inherits them, and
  `take_touched` reports the nodes issued or looked up since the last
  call, so the outpost can record which message reported them.
