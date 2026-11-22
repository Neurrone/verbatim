# verbatim-ia2

The minimal MSAA client (IA2 interface acquisition is an explicit seam
left for M7, landing with the browsers that motivate it; M3's backend
parity work covers MSAA and UIA only).

Public API:

- `batch`, the MSAA executor (see "The MSAA executor" below): `run`
  answers one of the request list's object steps on an `Object`, a node
  the outpost holds or an object acquired at its event's ids, and
  `descend` walks down the tree from one start; `step` is its one
  navigation step. What the outpost holds it reaches through `Resolve`.
- `WinEventHook::install(target_pid, kinds, callback)` — out-of-context
  WinEvent hooks for a caller-chosen set of event kinds, scoped to one
  process id (or global for pid zero). A thread holds one set at a time,
  since its hooks share one callback; a second install on the same thread
  fails. Two constant sets name the two
  callers: `CONSOLE_SUBSCRIPTIONS`, what the console host's outpost
  installs scoped to its process, the console's update and caret events,
  and `LISTENER_SUBSCRIPTIONS`, what the focus listener installs
  globally: every kind it tells Core of (ADR 0003), each event mapped to
  the kind its tell names by `tell_kind`. Each kind is one raw event,
  but for the console's updates, so a menu's end
  (`WinEventKind::MenuEnd`) and a popup menu's
  (`WinEventKind::MenuPopupEnd`), and a selection added, removed or
  within (`WinEventKind::SelectionAdd`, `SelectionRemove` and
  `SelectionWithin`, which NVDA handles as changes of state), are kinds
  of their own, beside a plain selection (`WinEventKind::Selection`). The
  system caret's events are `WinEventKind::Caret`
  (`EVENT_OBJECT_LOCATIONCHANGE` or `EVENT_OBJECT_SHOW` on `OBJID_CARET`,
  which NVDA handles as a caret event on its focus; every other object's
  location change, which windows raise constantly, is dropped at the hook,
  and a caret's show is never `WinEventKind::Show`). Its console events
  are hooked from the install until the listener's start, and afterwards
  only while a console window in front has not named its thread, and are
  told as nothing, since the console host's outpost keeps them; a text
  selection change is not hooked. Program Manager's and the taskbar's
  foreground events come to it alone, as `WinEventKind::ShellForeground`,
  never a foreground change, and so do the shows of NVDA's five show
  classes other than the tooltip's (`WinEventKind::OtherShow`,
  `OTHER_SHOW_CLASSES`). It holds focus (`EVENT_OBJECT_FOCUS`), foreground
  (`EVENT_SYSTEM_FOREGROUND`), the changes of name, value, state,
  description and selection, menu mode's start and menu-popup opens
  (`WinEventKind::MenuStart`, `EVENT_SYSTEM_MENUSTART`, which only the
  listener hooks, and `EVENT_SYSTEM_MENUPOPUPSTART`), an object hidden
  (`WinEventKind::Hide`, `EVENT_OBJECT_HIDE`, the listener's alone, which
  Core's acceptance refuses as NVDA's does), the end of a menu or of the Alt+Tab
  switcher (`WinEventKind::MenuEnd` and `WinEventKind::SwitchEnd`),
  destruction, alerts, the system caret's events, and a tooltip window
  shown (`WinEventKind::Show`:
  `EVENT_OBJECT_SHOW` from a `tooltips_class32` window on its client or a
  custom object), the input desktop's switch
  (`WinEventKind::DesktopSwitch`, `EVENT_SYSTEM_DESKTOPSWITCH`), and the
  desktop window's name change (`WinEventKind::DesktopName`,
  `EVENT_OBJECT_NAMECHANGE` on `GetDesktopWindow()`), which the set
  hooks scoped to the desktop window's own process, csrss's, apart from
  its other raw events, and tells apart by its hook, every other window's
  name change it delivers dropped. Callbacks are delivered on the installing thread's
  message loop and must never make blocking calls into the target. The
  callback receives a `WinEvent`: the event's kind, its MSAA address, the
  thread that raised it, which is a console window's real thread in the
  console host where `GetWindowThreadProcessId` names the client's, the
  time Windows raised it (`dwmsEventTime`), and how long ago that was;
  and beside it the class of its window when the hook's filter read it
  for the event, so the handling reads it no more.
  Every hook that receives one event is given the same kind, address,
  thread, and time. `event_clock()` reads the clock that time
  is on (`GetTickCount`). `WinEventKind` names the event; drop unhooks.
  NVDA's early filters run at the hook, some asking the window's class
  (Excel's `EXCEL7` child objects, a foreground change of the shell, a
  tooltip's show, a menu event), reading it with `GetClassNameW` at most
  once for each event and only when a filter asks it. None of those filters applies to the console host's events
  (`ConsoleUpdate` and `ConsoleCaret`), whose object and child ids carry
  data, such as a scroll's horizontal distance, which is at or below
  `OBJID_ALERT` for ten or more columns to the left; NVDA hooks them
  apart (`winConsoleHandler.py`), where its filters do not run.
- `WinEventHook::install_with_sync_marker(target_pid, kinds, callback,
  on_marker)`, only with the `sync-marker` feature, which mockapp's tests
  turn on through `verbatim-outpost`'s feature of the same name and no
  product build has — installs the same hooks and, as one more hook of the same
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
  the `WinEventHook` removes whichever are hooked then.
- `dialog` — `DialogObject`, the objects a dialog's own text is gathered
  from (`docs/nvda/object-model.md`, "A dialog's own text"; the gathering
  rule is `verbatim_reader_rules::dialog`'s, whose `DialogParent` and
  `DialogObject` traits it implements). `DialogObject::of_node(node,
  registry)` is the kept object behind a dialog this outpost reported;
  `children()` reads its children in order (`accChildCount` and
  `AccessibleChildren`), standing in a window's client area for each child
  that is the window object of another window, as NVDA does, acquiring it
  the first time it is read, and knowing a hidden or disabled window from
  `IsWindowVisible` and `IsWindowEnabled`, so it is passed over without
  being acquired unless its role or name is asked for as the next sibling
  of a label; `role`, `states` (mapped as every MSAA read maps them, by
  the one function that also gives an edit control's client area
  multi-line from its window style), `invisible` (the invisible state,
  which a hidden window passed over carries too), `name`, `value`, and
  `description` each read their property the first time they are asked,
  and the first three keep it, since the gathering looks at a child's
  neighbors too. Its text keeps any bidirectional marks, which NVDA strips
  only from the clock, File Explorer and list views. The name, value and
  description rules of an object read alone, as an event's object or an
  equality check reads it, are those of a full read: one copy of each.
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
  window no longer exists, `Abandoned` when the thread's call gate refused the message
  (`verbatim_win32::calls`), never a fact about the control, and `Failed`
  otherwise. Offsets are the
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
  `ReadProcessMemory`), with the text range's pointer field sized for that
  process: four bytes for a 32-bit (WOW64) process, by `IsWow64Process2`.
  Each such message has a buffer of its own (`RemoteBuffer`), one
  allocation with the structure at its start and the text 16-byte aligned
  after it, freed whole by the thread that sent the message once the send
  has returned, never before, so the application never writes into memory
  freed or reused while the message waited. A free that fails because the
  process has exited is no error, logged at debug: the exit took the
  memory. The process is opened once per edit control from the window's
  process, and its id kept (`GetProcessId` on the open handle, which keeps
  the id from being reused). A rich edit window that is not Unicode may still
  answer in UTF-16, so its whole text buffer is read back and decoded as
  NVDA decodes it (`decode_text_range`): UTF-16 when more than one
  character was copied and a byte past the copied count is not zero, and
  otherwise ANSI text converted from the system code page. Every message
  goes through the window messages' one send, below, and counts as one
  window message; the memory calls are the kernel's and are not counted. A
  password field (`ES_PASSWORD`) reads as stars, as NVDA reads it.
- `uniscribe` — `character(text, at)` and `word(text, at)`: the span of the
  character or word containing a UTF-16 offset of a line, as Uniscribe
  breaks it, ported from NVDA's `_calculateUniscribeOffsets`
  (`nvdaHelper/local/textUtils.cpp`), which NVDA's `EditTextInfo` uses for
  an edit control's characters and a plain edit's words. The line is
  itemized (`ScriptItemize`) and each run broken (`ScriptBreak`) from its
  start to the end of the line, last run first, so the first run's
  analysis is left for the whole line, as NVDA does to segment as Notepad
  does. A character runs from the last `fCharStop` at or before the offset
  to the next; a word from the last `fWordStop` to the next, then
  corrected as NVDA's #1656 corrections are: white space at and just
  before the offset belongs to the word before, and a word ends after the
  first run of white space from the offset, white space being what the C
  runtime's `iswspace` says. At or past the line's end the span is the one
  code unit there, as NVDA's helper answers; the caller decides what the
  end means. Its tests compare every offset of each line with what the
  pinned NVDA build's own helper gives.
- `acquire` — the query-pool side: `snapshot_from_event` (from
  `AccessibleObjectFromEvent` through name, role, value, state,
  description, keyboard-shortcut, and location reads to a `NodeSnapshot` —
  the `NodeDetails` half plain MSAA can express, with `top_level_client`
  set on a client object, child id 0, of a window with the `WS_SYSMENU`
  style, from one local style read, as NVDA's
  `isPresentableFocusAncestor` singles it out, only for an object read at
  its own address, since NVDA knows a client's object id only from its
  event or its identity string, so a Windows Forms form's client reached
  through `accParent` is not marked as reached, but read at its address
  instead when reached from a child window's window object, below; `window` set on a window or client
  object, child id 0, read at its own address, and on the object a walk
  reaches from a child window's window object in its parent window,
  which oleacc's window object answers with that window's client
  whether or not it has an identity string, as a Windows Forms panel's
  has none; never on any other object a walk keys at an address it made
  up, which may be any object inside the window; `STATE_SYSTEM_INVISIBLE`
  maps to the invisible state; position-in-set is
  counted for list-view and tree-view items, as described below, and is
  otherwise `None` until IA2's `groupPosition` (roadmap M7)),
  `tree_view_child_count` (a `SysTreeView32` item's number of children,
  its first child and each next sibling through `TVM_GETNEXTITEM`, as
  NVDA counts them; read by the outpost only with a state change that
  leaves the item expanded),
  `read_node` (a node as it is now, for a review command's live read: an
  object that no longer answers reads with an unknown role and no name, as
  NVDA's does),
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
  `focused_object` also uses, so the child-id and child-object forms are
  handled once), `menu_start_candidate` (a menu start's object at its
  event's address with no redirect, as NVDA builds it),
  `focus_candidate` (the same acquisition and redirect,
  reading no more than the redirect needs, so the outpost can make NVDA's
  checks before the read: `key()` is the address after the redirect, for
  NVDA's duplicate check and the ids a redirected focus is read by;
  `property(id, registry)` one value read alone by the functions the
  full read uses (states, role, a `SysListView32` item's position and set
  size, name, value), for a batch's `Test` reading in its matcher's
  order, which is how Core's identity check reads it, and
  `object()` the object as an `EventObject`, whose `property(id, role,
  registry)` reads a name, value, description, role, states or place
  alone and whose `is_equal_to` and `which_of` answer a batch's
  `IsOneOf`;
  `role()` the object's role, read once and kept, which a
  menu start's facts carry; `has_focused_state(max_hops, plain,
  parent_window)` is NVDA's `shouldAllowIAccessibleFocusEvent`, the
  object's state and then each ancestor's read live through `accParent`,
  the window it is in followed by each object's role, a window's own
  window object being the one step that leaves a window; past an
  `accParent` that fails or names nothing it goes on from the parent
  window by that window's own object, as `parent_window` says the caller
  chooses it (`ParentWindow`: its MSAA client, read and walked up from;
  its UIA element's focused state, read by the caller; a plain window,
  never focused, gone past to its parent window; or above the process
  boundary, where it ends), as NVDA's `Window._get_parent` does (its
  UIA, plain-window and above-the-boundary cases, `ParentWindow::Uia`,
  `Plain` and `AboveBoundary`, have no test, a known gap); it never
  steps into a parent window `plain` names (another process's, a ghost's
  or a hung one), going on from it by the window hierarchy, the parent
  window checked again after the step for a window reparented meanwhile; `read(registry)` reads it
  without reading again the role and state words the checks read;
  `snapshot_from_focus_event` is a candidate read at once),
  `event_object` (an event's object acquired and nothing read: its
  `role`, `states`, `value` and `location` read one property each, as a
  full read gives it;
  `which_of(nodes, registry)` tells whether it is one of the given nodes by
  the address the node was issued for or by the same COM object and child
  id, reading no property; `is_equal_to(node, registry)` tells whether it
  is a kept node by NVDA's equality (`IAccessible._isEqual`) against the
  event's own address, reading the object's role and name and the kept
  object's location only when the node's event parameters, window and
  child id match the event's; `role()` reads its role alone; `name`,
  `description`, `states`, and `value` read that one property alone, as
  a full read of an object of the given role would give it, for a change
  of the focus or an ancestor whose role is known; `read(registry,
  purpose)` reads it in full),
  `Purpose` (`Announce` or `Context`, passed to
  `snapshot_from_event` and `EventObject::read`: an object read for
  context, such as an ancestor or the object of a change, has no list view
  or tree view position counted, as NVDA counts it only when it speaks the
  object, and no location read, since a read of many objects, an ancestor
  chain, never reads one (`books/dev/src/explanation/design/uia-shapes.md`, decision 9) and a
  change reports none; ancestor walks read for context too),
  `focused_object(hwnd, serves)` ("what is focused right now"
  in `hwnd`, the window with the keyboard focus, for a focus-now query:
  from its client, `accFocus` followed until it names nothing or the
  object it was asked of, a child id turned into the child's own object by
  `accChild` where it has one, as NVDA finds its focus object, stopping
  after 64 steps where NVDA has no limit; the window of each object of its
  own a step reaches is checked with `serves` before anything more is asked
  of it, and an object of a window the caller's application does not serve
  answers `None`),
  acquired and not yet read (`FocusedObject`), so a fake focus can make
  NVDA's focused-state test on it first (`has_focused_state`, the walk a
  focus event's candidate gets) before it is read (`read`);
  `read_at_event(hwnd, id_object, id_child, set)` (the listener's read of
  an event address's name, role, value, states and place, each property of
  the set one call: the virtual desktop's name, and a classic progress
  bar's read at its value change); and the
  node-relative operations, which take a `NodeId` and
  read through the object the registry kept for it (a node issued from
  local window data, which has no object, is acquired at its address).
  `navigate`, `activate`, and `ancestor_chain_until` answer `AcquireError::Gone`
  when the node is no longer kept, its window no longer exists, or its
  object has disconnected:
  `ancestor_chain_until` with `AncestorLimits`
  (stopping at a known ancestor, `Walked::MetKnown`; at `max_hops`, `Walked::Cut`
  with the outermost ancestor's window, which the outpost completes by
  plain windows; and `Walked::Onward` with the window the chain goes on
  from by its own object, for a parent in a different window that
  `read_by_other_api` says is read through UIA or `plain` says is read as a
  plain window, the walk not stepping from a window's own window object
  into a parent window `plain` names, and for an `accParent` that fails or
  names nothing below the desktop, the parent window of the outermost
  ancestor's window, as NVDA's `Window._get_parent` falls back to; a parent
  that names no window is taken to be in the walk's window, or above a
  window's own window object in its parent window; a control's window object met on the way has as its
  next ancestor the group box enclosing it, a `Button` window with
  `BS_GROUPBOX` before it in z-order, as NVDA's `findGroupboxObject`
  finds it, and the walk goes on from the group box; the client of a
top-level window reached from a child window's window object is acquired
at its own address and read there instead of the object `accParent` gave,
as NVDA reads its foreground object, so it is the node any other path
gives that client, marked `top_level_client` by the rule above and said
by Core on entering the window: one acquisition) (per-hop `accParent` walks, outermost first, with the simple-child
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
  `children` (an object's children as NVDA's MSAA object lists them,
  through `AccessibleChildren` for its own `accChildCount`, those outside
  its window or a window inside it left out, a gone child kept in its
  place as a disconnected object reads so that the children after it are
  still listed; `Ok(None)` for a window's own object, a simple child and
  a tree view's objects, whose children are reached by `navigate`),
  and `activate` (`accDoDefaultAction`, MSAA's only
  activation primitive, answering the `accDefaultAction` name read first). Every snapshot is minted through `node_for`,
  which matches a new sighting against the kept nodes in NVDA's
  comparison order (`docs/parity/focus-events.md`, "Held objects"): the same COM
  object (by its canonical `IUnknown`, confirmed against the kept object)
  with the same child id in the same window and the same role, else, for
  an object acquired at its address, a kept node acquired at the same
  address with the same role and, when both objects offer one, the same
  `IAccIdentity` string; else, for a focus walk, the first of its
  `equality::Candidates` (the kept nodes of Core's focus line and of the
  newest unjudged focus, each line innermost first) that NVDA's
  `IAccessible._isEqual` finds equal, in NVDA's order with its role and
  location steps swapped, since each step only rejects and the role needs
  no read: the same child id, event parameters where both are known,
  window, identity string where both asked, raw role, location (the
  sighting's as its read made it, else read once; the kept object's read
  live once per walk; a kept object gone is never equal) and name (the
  kept object's read live when the kept one differs). A reissue gives the
  sighting the candidate's id and nothing else, the node keeping its
  object, address and event parameters and taking the sighting's name;
  the lines are then scanned only outward of the candidate, so it gives
  its id at most once per walk. Anything else
  is a new node that keeps the object just read, with the facts the
  equality compares (`registry::Facts`). Each snapshot is read with its provenance: objects
  acquired at an event or window address, and children by id on them,
  are at their address, and so is a window's own standard object reached
  through `accParent`, at the address its identity string names
  (`accessible::decompose`, `IAccPropServices::DecomposeHwndIdentityString`),
  so a window keeps one node even when its title changes; other objects
  reached through `accParent`, through `accNavigate`, or as child objects
  are not, and never claim the address made up for them.

  Identity. A COM identity is never shared by two sightings of one object
  in another process: oleacc delivers every object wrapped in a new
  wrapper of its own, the Dynamic Annotation wrapper. Against mockapp,
  whose provider hands out one cached object
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
  matched to it, as NVDA never matches it (`docs/parity/focus-announcement.md`, "Focus announcement content and property order").
  A top-level window's client area an ancestor walk reaches with no
  address, from a child window's window object or from a windowless
  object inside the window (an object of the client role in the window
  the walk is in), is acquired again at the window's client address and
  read there, as NVDA reads its foreground object, so it carries its
  window and is marked a top-level window's client. Two seams
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
  rather than colliding; a client object of role window is keyed there
  too, while it keeps the event parameters of the address it was acquired
  at, so a change event there is matched to it by equality) — navigates
  the Win32 window hierarchy rather
  than `accNavigate`/`accParent`: parent is `GA_PARENT`, siblings are the
  next and previous visible top-level child windows
  (`GW_HWNDNEXT`/`GW_HWNDPREV`), and the first child is the first visible
  child window or, for a leaf control, its own client object. This is
  NVDA's `Window`/`WindowRoot` navigation, and it is what makes
  parent-then-sibling-then-child navigation between the controls of a
  dialog work: MSAA's own answers there are the control's scroll-bar and
  client pieces, not the sibling controls.
- `class` — `normalized_class_of(hwnd)`, a window's class name as NVDA
  normalizes it before matching any class-based rule
  (`verbatim_model::class::normalize`,
  [verbatim-model](verbatim-model.md)): a Windows Forms
  name (`WindowsForms10.SysTreeView32.app.0.…`) cut down to the control
  class it wraps, an `ATL:` prefix dropped, and the result looked up in
  NVDA's class map. Every class-based rule in this crate compares the
  normalized name, so a Windows Forms tree view gets the `SysTreeView32`
  handling below and a Windows Forms list view the `SysListView32`
  position. The outpost's backend arbitration uses the same function.
- `event_thread` — the thread that installs the `WinEvent` hooks and
  pumps the messages their out-of-context callbacks are delivered
  through, which an outpost and the focus listener each run, differing
  only in their pid and kinds. `EventThread::spawn(kinds, make_handlers)`
  starts it with no hook; `hook(pid)`, on the thread or on any copy of its
  `Nudger` (`Nudger::hook`), installs its one set when asked, behind every
  nudge sent before and returning once the hooks are in (an outpost asks
  when `Follow` first names a console's terminal). `EventHandlers` are made
  on the thread: the `WinEvent` callback, an optional nudge handler and
  an optional timer handler, which the thread calls for each `WM_TIMER`
  of its one thread timer (`ThreadTimer`, `SetTimer` with no window, set
  and stopped only on the thread, a stale `WM_TIMER` left by `KillTimer`
  included), as the focus listener ends its foreground hold.
  A window object's show of the window in front is kept as a show,
  whatever its class, for a set that hears shows (`shown_in_front`, one
  `GetForegroundWindow`), and delivered on its client object, as every
  window object's event but a destruction is (`object_of`). A
  `Nudger` hands the thread two words between events (`nudge`), which is
  how the listener unhooks some kinds and hooks them again, and, with the
  `sync-marker` feature, waits for it to catch up (`sync`): every nudge
  sent and, once the hooks are in, every `WinEvent` raised before it,
  through `SyncMarker`.
- The window functions the client calls by handle (`window_exists`,
  `window_class_name`, `parent`, `style` and the rest) are
  `verbatim_win32::window`'s
  ([verbatim-win32](verbatim-win32.md)), which the private `window` module
  re-exports beside the list view and tree view messages it sends.
- `map` — `role_from_msaa` and `states_from_msaa`, the tables from
  MSAA constants to the normalized vocabulary, following NVDA's MSAA
  role and state tables (`role_from_msaa` takes the role as `accRole`
  answered it, a role not read being unknown; MSAA's caret and cursor
  are `Role::Caret` and `Role::Cursor`, as NVDA maps them, Dickson,
  2026-11-21; `STATE_SYSTEM_DEFAULT` is dropped and
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
  fields sized for it, one buffer per message as for `edit`).
  `list_view::column_name(hwnd, child_id, description)` is public, for the
  mockapp tests of its messages. Every name, value, and description read loses its
  bidirectional formatting characters (`verbatim_text::strip_bidi_controls`),
  as NVDA strips the direction marks from list view items and the clock.
- Window messages (`window::send_message`): every message this crate sends
  to an application, the list view's, tree view's and edit control's,
  whatever the window's state: Core judges a hang. A message carrying the
  address of a buffer in the target's memory is sent with no check that
  the window is still in the buffer's process: a window handle carries a
  16-bit reuse counter, so a handle's value recurs only after about
  65,536 reuses of its slot, no practical risk (Dickson, 2026-11-07). The
  message is then sent with `SMTO_BLOCK | SMTO_ERRORONEXIT` and
  no time limit of its own, the largest that `SendMessageTimeoutW` waits,
  `i32::MAX` milliseconds (it takes a larger one as passed at once),
  without `SMTO_ABORTIFHUNG`: only a send with no time limit tells when its
  message was answered, and Core's due time for its request is its bound.
  A
  failed send is gone when the window no longer exists, as it does once
  its thread ends, which `SMTO_ERRORONEXIT` fails the send for. Under the
  `faults` feature each send reaches the observation point
  `WindowMessageSent` with its window, message, time limit and buffer
  ([verbatim-faults](verbatim-faults.md)).
- `calls` — what `acquire` and the rest count as a cross-process call,
  in the one per-thread count `verbatim_win32::calls` keeps for both
  backends: `verbatim_win32::calls::count(kind)`, which fails with
  `CallError::Abandoned`, making and counting no call, once the thread's
  call gate's worker is abandoned.
  `count`, and the making of an object's agile reference, call the
  debug-build lock check (`verbatim_win32::held_locks::check`), which
  fails when the thread holds one of the outpost's locks.
  Every `IAccessible` method, `IAccIdentity`'s identity string, the
  acquisitions (`AccessibleObjectFromEvent`, `AccessibleObjectFromWindow`,
  `AccessibleChildren`, `WindowFromAccessibleObject`), and a
  `QueryInterface` for any interface but `IUnknown` on an object from the
  application count as MSAA calls, one per API call however many round
  trips it makes inside; the list view and tree view messages count as
  window messages. A snapshot read is seven calls (six properties and the
  location), plus the role and identity reads that match a sighting to a
  kept node. Local window functions on the application's handles are not
  counted (`books/dev/src/reference/performance.md`, "What counts as a call").
- `accessible` and `window` (private) — the safe wrappers `acquire` is
  written against, so that `acquire` holds no `unsafe` code. `unsafe`
  lives only in these two modules, in `hook`, the WinEvent hook's install,
  removal, and callback, and in the modules that wrap their own platform
  calls, `event_thread`, `edit` and `uniscribe`; each block carries a
  `SAFETY` comment. `Accessible` is an
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
    `navigate` (`accNavigate`), `focus` (`accFocus`), `child_object`
    (`accChild`, then a counted `QueryInterface`), `window`
    (`WindowFromAccessibleObject`), and
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
- `Interning::details` — the node details the walk's request names
  (a `verbatim_model::property::NodeDetailSet`): a snapshot skips
  `accDescription`, `accKeyboardShortcut`, the list and tree position
  reads, and a tree item's level when the request does not name them.
  The outpost's store answers its request's set; the registry itself,
  untagged, a tool's or a test's, answers every detail.
- `NodeIdRegistry` — the nodes the outpost has issued, each with its
  address, an `MsaaKey` (window handle, object id, and child id, the
  child id `CHILDID_SELF`, zero, for the object itself; `id_for(key)`
  mints or finds a node, `key_of(node)` gives its address back, and a
  `FocusCandidate`'s `key()` is the address its focus is at), and what
  it was issued from: local window data, with no object, role or facts,
  or an accessible object read, with the role read then, what NVDA's
  equality compares of it, and the object, kept as an agile reference;
  a node of local window data is never compared. It shares the outpost-wide counter with the UIA registry. It
  only stores and looks up, and never makes a COM call under its lock;
  `acquire` makes the comparisons. `retain` releases the nodes the outpost
  no longer needs, returning their objects as `Released` (dropped after the lock is released, since
  releasing one can call into its process), and `forget_window` drops a
  destroyed window's nodes so a reused handle never inherits them,
  returning their ids and `Released` likewise. The walks in `acquire`,
  `equality` and `dialog` take an `Interning` (`&dyn Interning`) rather
  than the registry: the registry's own operations (`id_for`, `find`,
  `insert`, `reissue`, `facts_of`, `key_of`, `locate`, `details`, and
  `touch` for a found node the caller confirmed), which the outpost's
  object store implements to tag each node issued or looked up with the
  request that named it, and the registry implements untagged, its
  `touch` doing nothing. `Facts`, `Found`, `Held`, `Kept` and `KeptObject`
  are public for it, their fields the crate's own.

## The MSAA executor

`batch` is the one implementation of the request list's object
operations on MSAA subjects, as `verbatim-uia-rops`' executor is UI
Automation's (the one-path rule, the verbatim-outpost crate guide). Each
step runs alone, by calls:

- `Read` of a node reads its snapshot (`acquire::read_node`) only when
  the set names a property a snapshot holds, a tree view item's children
  by the control's messages (`tree_view_child_count`), and the rest the
  outpost holds (`Resolve::held`: its MSAA ids, whether its window is
  shown, its window's class, its text's summary); suggestions, the
  `Value` pattern and a combo box's list, UI Automation's alone, answer
  none. An acquired object's properties are each read alone
  (`FocusCandidate::property`), its focused state over it and every
  ancestor up to the desktop (`FocusCandidate::has_focused_state`, the
  window facts through the resolver).
- `Test` evaluates its matcher lazily, a node read at most once, at the
  first property the outpost does not hold.
- `Related` steps by `acquire::navigate`, the object reached read
  through its window's backend (`Resolve::corrected`); a last child is
  the last of the children; an MSAA object controls nothing and has no
  selected item of its own.
- `Ancestors` walks up by `accParent` (`ancestor_chain_until`), to the
  top for a batch's own step; the outpost's chaining of a focus's
  ancestors calls the same arm (`batch::ancestors`) with its stops.
- `Descendants` walks depth first in tree order, judging each node by
  the walk's matchers on its snapshot's values: an object's children as
  it lists them (`acquire::children`), each through its window's backend,
  and a node MSAA does not list the children of (a window's own object, a
  tree view's item, a node read through UI Automation) through the
  resolver, which steps on that node's backend.
- `IsOneOf` compares a node by its id, and an acquired object by NVDA's
  equality for the equal node and by address for the rest, keeping the
  role the equality read.
- `Activate` performs exactly the node's default action
  (`acquire::activate`); the walk up the parents is the dispatcher's.
- `FocusRead` (`batch::focus_read`) reads an acquired candidate, or the
  focus of the window with the keyboard focus found by `accFocus`, its
  focused state over it and its ancestors first when the subject asks
  (`FocusSubject::Focus { if_focused }`, answering `FocusFound::Unfocused`
  without one); then its ancestors chained across backends and cut at
  the first Core knows, and the text of the MSAA dialogs it enters,
  gathered by `verbatim_reader_rules::dialog`'s rule, through the
  outpost's `FocusResolve` for the chaining, a UI Automation dialog's
  text, the foreground object and the answer. A candidate's dialogs are
  read before the foreground object, the application's focus's after it.
  `test_focus` is a `Test` of the application's focus: that read with its
  focused state first, the matcher evaluated lazily over it, and what it
  found answered for a later `FocusRead` to take.
