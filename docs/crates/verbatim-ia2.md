# verbatim-ia2

The minimal MSAA client (IA2 interface acquisition is an explicit seam
left for M7, landing with the browsers that motivate it; M3's backend
parity work covers MSAA and UIA only).

Public API:

- `WinEventHook::install(target_pid, kinds, callback)` — out-of-context
  WinEvent hooks for a caller-chosen set of event kinds, scoped to one
  process id (or global for pid zero). A thread holds one set at a time,
  since its hooks share one callback; a second install on the same thread
  fails. Two constant sets name the two
  callers (decision D13): `APP_SUBSCRIPTIONS`, what a per-application outpost
  installs — since stage A step 5 of phase 6, the application's focus,
  foreground, menu-popup opens, alerts, and tooltip windows shown, the
  same kinds as the listener's below but scoped to the one process, so
  every MSAA event of the application reaches its outpost by one route
  (`docs/design/focus-pipeline.md`, section 12.4); value, state, name,
  description, and selection changes
  (`EVENT_OBJECT_SELECTION` is `WinEventKind::Selection`; the selection add,
  remove, and within events are reported as `WinEventKind::StateChange`, as
  NVDA handles them), and since milestone M4 the caret
  (`WinEventKind::Caret`: `EVENT_OBJECT_LOCATIONCHANGE` or
  `EVENT_OBJECT_SHOW` on `OBJID_CARET`, the system caret, which NVDA
  handles as a caret event on its focus; every other object's location
  change, which windows raise constantly, is dropped at the hook, and a
  caret's show is never `WinEventKind::Show`)
  and text selection changes (`WinEventKind::TextSelectionChange`,
  `EVENT_OBJECT_TEXTSELECTIONCHANGED`); and
  `LISTENER_SUBSCRIPTIONS`, what the focus listener installs
  globally, forwarding an application's events only until its outpost's
  own hook is in (its console events are hooked from the install until
  the listener's start, and afterwards only while a console window in
  front has not named its thread; Program Manager's and the taskbar's
  foreground events come to it alone, as `WinEventKind::ShellForeground`,
  never a foreground change) — focus (`EVENT_OBJECT_FOCUS`), foreground
  (`EVENT_SYSTEM_FOREGROUND`, the `WinEventKind::Foreground` variant that
  absorbs Core's old foreground trigger), menu-popup opens
  (`EVENT_SYSTEM_MENUPOPUPSTART`), and the end of a menu or of the Alt+Tab
  switcher (`WinEventKind::MenuEnd` and `WinEventKind::SwitchEnd`), which
  is global because focus returns to whichever application is then in
  front, and a tooltip window shown (`WinEventKind::Show`:
  `EVENT_OBJECT_SHOW` from a `tooltips_class32` window on its client or a
  custom object, every other show event dropped at the hook, as NVDA
  accepts no others from a standard control). A popup menu opening announces the menu
  itself the moment it opens, NVDA's menu-start behavior — the app outpost
  emits it as focus on the menu's client object with no ancestry, the
  identical node its foreground-announce fallback produces for a menu-class
  window, so the reducer's duplicate-focus suppression drops whichever path
  announces second. Callbacks are delivered on the installing thread's
  message loop and must never make blocking calls into the target. The
  callback receives a `WinEvent`: the event's kind, its MSAA address, the
  thread that raised it, which is a console window's real thread in the
  console host where `GetWindowThreadProcessId` names the client's, the
  time Windows raised it (`dwmsEventTime`), and how long ago that was.
  Every hook that receives one event is given the same kind, address,
  thread, and time, which is how an outpost tells the listener's copy of
  an event from its own hook's. `event_clock()` reads the clock that time
  is on (`GetTickCount`). `WinEventKind` names the event; drop unhooks.
  NVDA's early filters run at the hook, some asking the window's class
  (Excel's `EXCEL7` child objects, a foreground change of the shell, a
  tooltip's show): a hook set that hears windows destroyed, as an
  outpost's does, keeps each window's class until its destroy event, so a
  flood from one window asks its class once; the listener's asks it once
  for each event. None of those filters applies to the console host's events
  (`ConsoleUpdate` and `ConsoleCaret`), whose object and child ids carry
  data, such as a scroll's horizontal distance, which is at or below
  `OBJID_ALERT` for ten or more columns to the left; NVDA hooks them
  apart (`winConsoleHandler.py`), where its filters do not run.
- `WinEventHook::install_with_sync_marker(target_pid, kinds, callback,
  on_marker)` — installs the same hooks and, as one more hook of the same
  set, a hook for this process's sync markers, and returns the set with a
  `SyncMarker`. `SyncMarker::raise(ticket)`, from any thread of the
  process, raises `EVENT_AIA_START + 0x5B1` on a message-only window the
  set made on its thread, the 64-bit ticket split across `idObject` and
  `idChild`; the hook thread hands the ticket to `on_marker`, never to
  `callback`. A thread message posted to the hook thread can be handled
  before a `WinEvent` raised before the post has reached that thread, but
  `WinEvent`s have been observed to arrive in the order raised (an
  observation, not an order Microsoft documents), so a marker's arrival
  is taken to show every event raised before it has arrived; the
  outpost's event thread
  waits for one in `Nudger::sync`. A marker raised with no window is
  never delivered, which is why the set makes one. The marker hook is
  scoped to this process, so another process's event with that id is not
  a marker, though another process's global hook for the id sees the
  markers; and a set takes only the markers raised on its own window, so
  another hook set's markers in the same process are never its own. `suspend_kinds` leaves the marker hook alone, and dropping the
  set removes it and destroys the window.
- `suspend_kinds(suspended)` — narrows the calling thread's hook set while
  it runs: every raw event of the kinds it was installed for is hooked
  except one whose installed kinds are all in `suspended`, which is
  unhooked, and an empty list hooks everything again. Each raw event's
  hook is set and removed on its own within the one set, never a second
  set on the thread, and it may be called from the thread's own callback,
  as the focus listener hooks and unhooks the console's events. The
  set's hooks are kept in a thread-local beside its callback, so dropping
  the `WinEventHook` removes whichever are hooked then. An outpost
  unhooks the change events of windows read through MSAA this way while
  its application's focus and foreground windows are read through UIA
  (`docs/crates/verbatim-outpost.md`, "Event flow").
- `WindowCache<T>` — what a thread keeps about each window by its handle
  until the window is destroyed (`forget`): `get_or_learn(hwnd, owner,
  learn)` learns an entry anew when none is kept or the handle is now
  owned by another thread than the one it was learned under, a reused
  handle whose destruction went unheard. The hook's class cache and the
  outpost's event thread use it.
- `dialog` — `DialogObject`, the objects a dialog's own text is gathered
  from (`docs/nvda/object-model.md`, "A dialog's own text"; the gathering
  is `verbatim-outpost`'s `dialog_text`). `DialogObject::of_node(node,
  registry)` is the kept object behind a dialog this outpost reported;
  `children()` reads its children in order (`accChildCount` and
  `AccessibleChildren`), standing in a window's client area for each child
  that is the window object of another window, as NVDA does, acquiring it
  the first time it is read, and knowing a hidden or disabled window from
  `IsWindowVisible` and `IsWindowEnabled`, so it is passed over without
  being acquired unless its role or name is asked for as the next sibling
  of a label; `role`, `states` (with multi-line from
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
  control's UTF-16 code units. The line messages' answers are read as
  the 32-bit `int` they are, the low half of the message's result: the
  classic edit control sign-extends `EM_LINEINDEX`'s -1 for a line past
  the last, but Common Controls version 6's, which Windows Forms text
  boxes are, zero-extends it, and read whole it was taken for an offset
  of 4294967295. Which message is sent follows the version,
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
  `IsWow64Process2`. A rich edit window that is not Unicode may still
  answer in UTF-16, so its whole text buffer is read back and decoded as
  NVDA decodes it (`decode_text_range`): UTF-16 when more than one
  character was copied and a byte past the copied count is not zero, and
  otherwise ANSI text converted from the system code page. Every message goes through `SendMessageTimeoutW` with
  `SMTO_ABORTIFHUNG` and half a second's wait, and counts as one window
  message; the memory calls are the kernel's and are not counted. A
  password field (`ES_PASSWORD`) reads as stars, as NVDA reads it.
- `acquire` — the query-pool side: `snapshot_from_event` (from
  `AccessibleObjectFromEvent` through name, role, value, state,
  description, keyboard-shortcut, and location reads to a `NodeSnapshot` —
  the `NodeDetails` half plain MSAA can express, with `top_level_client`
  set on a client object, child id 0, of a window with the `WS_SYSMENU`
  style, from one local style read, as NVDA's
  `isPresentableFocusAncestor` singles it out; position-in-set is
  counted for list-view and tree-view items, as described below, and is
  otherwise `None` until IA2's `groupPosition` (roadmap M7)),
  `tree_view_child_count` (a `SysTreeView32` item's number of children,
  its first child and each next sibling through `TVM_GETNEXTITEM`, as
  NVDA counts them; read by the outpost only with a state change that
  leaves the item expanded),
  `walk_tree` (the control plane's tree dump, from a window's client
  object, bounded by a depth and a node-count cap, each object's location
  and off-screen state read only when asked for, and otherwise left out
  of every node, the state word's bit too, so the dump shows them as not
  read; a failed
  `accChildCount` or `AccessibleChildren` fails the dump with its error,
  except on an object that is gone, answering `RPC_E_DISCONNECTED` or
  `CO_E_OBJNOTCONNECTED`, which ends that object's children there and
  marks the dump incomplete, as a cap does, so a menu closing during the
  dump costs only its own items; `E_FAIL`, which a server may answer for
  any failure, is not read as gone),
  `snapshot_from_focus_event` (the
  focus-specific entry the outpost uses for `EVENT_OBJECT_FOCUS` addresses,
  applying NVDA's `processFocusWinEvent` child-0-on-a-list redirect: when a
  focus event names a list on its own object — child id 0, MSAA role
  `ROLE_SYSTEM_LIST` — or the client of a `SysListView32` window, it reads
  `accFocus` and redirects to the named child when that child is real and
  different, so a container that fires focus on itself, like the wxWidgets
  generic list, announces the focused item rather than the container; a
  focus on any other object's own child id 0 is redirected to the child
  `accFocus` names by id when that child has the focused state, which is
  how a tree view or tab control raising focus on itself and then on its
  item announces the item, as NVDA does (`docs/parity/windows-and-processes.md`, "A control's
  own focus with a focused child"); the
  `accFocus` VARIANT is parsed in one shared place, `read_acc_focus`, which
  `focused_snapshot` also uses, so the child-id and child-object forms are
  handled once), `focus_candidate` (the same acquisition and redirect,
  reading no more than the redirect needs, so the outpost can make NVDA's
  checks before the read: `key()` is the address after the redirect, for
  NVDA's duplicate check; `has_focused_state(max_hops)` is NVDA's
  `shouldAllowIAccessibleFocusEvent`, the object's state and then each
  ancestor's read live through `accParent`; `read(registry)` reads it
  without reading again the role and state words the checks read;
  `snapshot_from_focus_event` is a candidate read at once),
  `event_object` (an event's object acquired and nothing read:
  `which_of(nodes, registry)` tells whether it is one of the given nodes by
  the address the node was issued for or by the same COM object and child
  id, reading no property; `role()` reads its role alone; `name`,
  `description`, `states`, and `value` read that one property alone, as
  a full read of an object of the given role would give it, for a change
  of the focus or an ancestor whose role is known; `progress` reads what
  a progress bar's report uses, its states, value, and location, into a
  `ProgressReading`, and nothing more for a busy indicator or an
  invisible bar; `read(registry, purpose)` reads it in full),
  `Purpose` (`Announce` or `Context`, passed to
  `snapshot_from_event` and `EventObject::read`: an object read for
  context, such as an ancestor or the object of a change, has no list view
  or tree view position counted, as NVDA counts it only when it speaks the
  object, and no location read, since a read of many objects, an ancestor
  chain, never reads one (`docs/design/uia-shapes.md`, decision 9) and a
  change reports none; ancestor walks read for context too),
  `focused_snapshot` ("what is focused right now" via `GetGUIThreadInfo`,
  for the synthetic focus event an outpost emits after a foreground
  change), and the node-relative operations, which take a `NodeId` and
  read through the object the registry kept for it (a node issued from
  local window data, which has no object, is acquired at its address).
  `navigate`, `activate`, and `ancestor_chain` answer `AcquireError::Gone`
  when the node is no longer kept, its window no longer exists, or its
  object has disconnected, and `selected_child` answers `None`:
  `ancestor_chain`, and `ancestor_chain_until` with `AncestorLimits`
  (stopping at a known ancestor, when the caller's `out_of_time` check
  says its time is up, or at a parent in a different
  window that `read_by_other_api` says is read through UIA, and saying
  which as `Walked`, whose `Crossed(hwnd)` lets the outpost continue the
  walk through UIA; a control's window object met on the way has as its
  next ancestor the group box enclosing it, a `Button` window with
  `BS_GROUPBOX` before it in z-order, as NVDA's `findGroupboxObject`
  finds it, and the walk goes on from the group box) (per-hop `accParent` walks, outermost first, with the simple-child
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
  comparison order (`docs/parity/focus-events.md`, "Held objects"): the same COM
  object (by its canonical `IUnknown`, confirmed against the kept object)
  with the same child id in the same window and the same role, else, for
  an object acquired at its address, a kept node acquired at the same
  address with the same role and, when both objects offer one, the same
  `IAccIdentity` string; anything else is a new node that keeps the
  object just read. Each snapshot is read with its provenance: objects
  acquired at an event or window address, and children by id on them,
  are at their address, and so is a window's own standard object reached
  through `accParent`, at the address its identity string names
  (`Accessible::address`, `IAccPropServices::DecomposeHwndIdentityString`),
  so a window keeps one node even when its title changes; other objects
  reached through `accParent`, through `accNavigate`, or as child objects
  are not, and never claim the address made up for them.

  Identity. A COM identity is never shared by two sightings of one object
  in another process: oleacc delivers every object wrapped in a new
  wrapper of its own, the Dynamic Annotation wrapper. Measured on
  2026-10-08 against mockapp, whose provider hands out one cached object
  per node: three sightings of one list, two through
  `AccessibleObjectFromEvent` and one as its item's `accParent`, all held
  at once, had three different `IUnknown` addresses. COM's own proxies
  could not do that, since unmarshalling one remote object into one
  apartment yields one proxy. All three objects' `IUnknown` and
  `IAccessible` tables lie in `oleacc.dll`; each answers
  `IServiceProvider`, which mockapp does not implement, and none is
  oleacc's proxy for a standard control (`QueryService` for
  `IIS_IsOleaccProxy` fails); none answers `IAccIdentity`. So identity
  matches only an object the outpost already holds, never a new
  sighting, and the address is what recognizes an object again. An
  object reached through `accParent` with no address of its own (a
  windowless ancestor) has neither, so an event naming it is never
  matched to it, as NVDA never matches it (`docs/parity/focus-announcement.md`, "Focus announcement content and property order"). Two seams
  mirror NVDA where plain MSAA navigation would mislead. A `SysTreeView32`
  item's navigation and ancestor chain route through the tree control's
  own `TVM_GETNEXTITEM` relations (with the accid-to-htreeitem mapping
  messages and their pre-v6 comctl32 fallback), because MSAA exposes every
  visible tree item as a flat sibling list under the control — parent,
  siblings, and first child would otherwise all answer the visible-order
  neighbor instead of the logical one; a tree item's `accValue` is its
  0-based indent depth, not a value, so `read_snapshot` reads it into the
  snapshot's level as it is, a root item at level 0, and leaves the value empty, again matching
  NVDA; and its state image (`TVM_GETITEMSTATE`) makes it checkable, and
  checked or half checked, as NVDA's tree view item reads a tree that
  draws its own check boxes. And a window-root object — the window face every windowed control
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
- `class` — `normalize_class_name(raw)`, a window class name as NVDA
  normalizes it before matching any class-based rule: a Windows Forms
  name (`WindowsForms10.SysTreeView32.app.0.…`) cut down to the control
  class it wraps, an `ATL:` prefix dropped, and the result looked up in
  NVDA's class map; and `normalized_class_of(hwnd)`, the same for a
  window. Every class-based rule in this crate compares the normalized
  name, so a Windows Forms tree view gets the `SysTreeView32` handling
  below and a Windows Forms list view the `SysListView32` position. The
  outpost's backend arbitration uses the same function.
- `map` — `role_from_msaa` and `states_from_msaa`, the tables from
  MSAA constants to the normalized vocabulary, following NVDA's MSAA
  role and state tables (so `STATE_SYSTEM_DEFAULT` is dropped and
  `STATE_SYSTEM_PROTECTED` kept, traversed is visited, and linked is
  linked), and `adjust_role_and_states`, NVDA's adjustment once mapped
  (a half-checked progress bar is a busy indicator), pinned by unit tests against raw state
  words captured from live controls. Reading a snapshot also treats a
  whitespace-only name or value as absent, drops the name of the edit
  field inside a labelled combo box, and gives a list view or tree view
  item its position, from `LVM_GETITEMCOUNT` or by counting its siblings
  with `TVM_GETNEXTITEM`, as NVDA does. A list view item has no value or
  description, and one of a list view showing columns (the report view or
  tiles) is named by its columns' texts, "content; Header: content", in
  the order shown, leaving out columns of zero width or no text
  (`list_view`, ported from NVDA's `sysListView32.py`: `LVM_GETVIEW`, the
  header's item count, `LVM_GETCOLUMNORDERARRAY`, `LVM_GETSUBITEMRECT`,
  `LVM_GETITEMTEXTW`, and `LVM_GETCOLUMNW`, their structures written into
  the list view's process through `edit`'s target process, with pointer
  fields sized for it). Every name, value, and description read loses its
  bidirectional formatting characters (`verbatim_text::strip_bidi_controls`),
  as NVDA strips the direction marks from list view items and the clock.
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
