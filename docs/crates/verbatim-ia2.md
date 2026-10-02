# verbatim-ia2

The minimal MSAA client (IA2 interface acquisition is an explicit seam
left for M6, landing with the browsers that motivate it; M3's backend
parity work covers MSAA and UIA only).

Public API:

- `WinEventHook::install(target_pid, kinds, callback)` — out-of-context
  WinEvent hooks for a caller-chosen set of event kinds, scoped to one
  process id (or global for pid zero). Two constant sets name the two
  callers (decision D13): `APP_SUBSCRIPTIONS`, what a per-application outpost
  installs — value, state, name, and selection changes (the four
  `EVENT_OBJECT_SELECTION*` events collapse to one `WinEventKind::Selection`,
  since all four report "the selection within a container changed" and
  acquisition reads the affected node from the event's own address either
  way); and `LISTENER_SUBSCRIPTIONS`, what the focus listener installs
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
- `acquire` — the query-pool side: `snapshot_from_event` (from
  `AccessibleObjectFromEvent` through name, role, value, state,
  description, keyboard-shortcut, and location reads to a `NodeSnapshot` —
  the `NodeDetails` half plain MSAA can express; position-in-set and level
  stay `None` on this backend until IA2's `groupPosition` lands in M6,
  never faked by counting siblings), `snapshot_from_focus_event` (the
  focus-specific entry the outpost uses for `EVENT_OBJECT_FOCUS` addresses,
  applying NVDA's `processFocusWinEvent` child-0-on-a-list redirect: when a
  focus event names a list on its own object — child id 0, MSAA role
  `ROLE_SYSTEM_LIST` — or the client of a `SysListView32` window, it reads
  `accFocus` and redirects to the named child when that child is real and
  different, so a container that fires focus on itself, like the wxWidgets
  generic list, announces the focused item rather than the container; the
  `accFocus` VARIANT is parsed in one shared place, `read_acc_focus`, which
  `focused_snapshot` also uses, so the child-id and child-object forms are
  handled once), `resnapshot` for re-reading whatever is at an address,
  `focused_snapshot` ("what is focused right now" via `GetGUIThreadInfo`,
  for the synthetic focus event an outpost emits after a foreground
  change), and the node-relative operations, which take a `NodeId` and
  read through the object the registry kept for it (a node issued from
  local window data, which has no object, is acquired at its address).
  `navigate`, `activate`, and `ancestor_chain` answer `AcquireError::Gone`
  when the node is no longer kept, its window no longer exists, or its
  object has disconnected, and `selected_child` answers `None`:
  `ancestor_chain`
  (per-hop `accParent` walks, outermost first, with the simple-child
  special case its doc explains — a bare child id has no `accParent` of
  its own, so its first hop is the object it is a child of; MSAA has no
  remote-ops analog, so unlike UIA's equivalent this stays the permanent
  implementation), `navigate` (parent via `accParent`, siblings and first
  child via `accNavigate`; returns `Ok(None)` for a genuine edge and
  `Err(AcquireError::Gone)` when the source node itself is no longer
  reachable, so the outpost can report `Gone` rather than a fake edge),
  `selected_child`, and `activate` (`accDoDefaultAction`, MSAA's only
  activation primitive). Every snapshot is minted through `node_for`,
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
  snapshot's one-based level and leaves the value empty, again matching
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
- `NodeIdRegistry` — the nodes the outpost has issued, each with its
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
