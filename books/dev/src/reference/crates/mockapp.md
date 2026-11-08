# mockapp

The scripted UIA and MSAA provider test host (`books/dev/src/explanation/architecture.md`
section 13, layer 2): a real, separate-process Win32 application that
answers `WM_GETOBJECT` as a genuine out-of-process accessibility provider
over a JSON-scripted tree, so `verbatim-uia` and `verbatim-ia2`'s real
client stacks, and the outpost's arbitration, are exercised cross-process
with no real applications, on plain CI Windows runners. `mockapp`'s own
binary depends only on `verbatim-model` (for `Role`, `State`, and
`StateSet`); the client crates it is built to be tested against
(`verbatim-uia`, `verbatim-ia2`, `verbatim-outpost`, `verbatim-listener`
and `verbatim-listener-logic`) are dev-dependencies,
used only by its own integration tests. Running it by hand, and its stdin
commands, are in [the mockapp tooling guide](../../how-to/mockapp.md).

## The command line

`mockapp --fixture <path.json> --backend <uia|msaa|win32> [--title <window
title>] [--show] [--hide-window-handle] [--count-registrations]
[--line-events <prefix>] [--owner-title <title>]` (default
title `mockapp`). It creates one real top-level Win32 window titled per
`--title`, hidden unless `--show` is given, prints `ready` (flushed) once
the window exists and the provider is answering, then processes stdin
commands until `quit`. The cross-process tests leave the window hidden;
the end-to-end suite's `spelling_errors` scenario shows it and drives it
with real keys ([verbatim-e2e](verbatim-e2e.md)). The focus starts on the
first node in the `focused` state, if the fixture has one, as an
application opens with its focus on a control.

- With `win32` the window answers nothing itself, as a plain Win32
  window: its client area is Windows' own standard object, named by the
  title, and only its real child controls answer for themselves; stdin
  commands that act on the scripted tree are rejected.
- With `--hide-window-handle`, the UIA root reports a window handle of 0,
  so a client holds the window's own element, runtime id `[42, window]`,
  without its handle.
- With `--line-events <prefix>`, as mockapp prints an acknowledgement or
  `held <method>`, it also sets the named event that the prefix and the
  line's first word name, such as `<prefix>applied` or `<prefix>held`,
  when one exists, and nothing when none does: the end-to-end suite reads
  no stdout, and waits on such an event it created instead
  (`return_from_a_held_application`).
- With `--owner-title <title>`, the window is an owned window of the
  top-level window of that exact title, found as mockapp starts, as a
  dialog another process opens is owned by the window it opens over: the
  owned-window fixture is `owner_window.json` and `owned_dialog.json` run
  in two processes ([the mockapp tooling guide](../../how-to/mockapp.md)).
- With `--count-registrations`, the UIA root answers through a provider
  that also implements `IRawElementProviderAdviseEvents` (see `hits`
  below). It is off otherwise, since implementing that interface changes
  the calls UIA makes as clients register, which the call-count tests pin.

The stdin commands are listed in the tooling guide; each is acknowledged
on stdout once it has taken effect, `applied` or `rejected: <reason>`, so
a test never reads the state from before its command.

## The fixture format

One JSON object per node; the root node is the window itself.
`fixture::role_from_fixture_str` and `state_from_fixture_str` hold the
complete name tables. The fields:

- `id` (a unique string), `role` (a `Role` name in snake case, such as
  `check_box`), optional `name` and `value` strings, `states` (an array of
  `State` names in snake case, such as `read_only`), and `children`.
- Detail properties: `description` and `keyboard_shortcut` strings, and
  one-based `position_in_set`, `set_size`, and `level` integers, the
  `NodeDetails` vocabulary of [verbatim-model](verbatim-model.md); each
  backend serves the subset its API can express.
- `default_action`: MSAA's `accDefaultAction`, which `accDoDefaultAction`
  then succeeds for; on the UIA backend the node serves `InvokePattern`.
- `controller_for`: the `id` of a node this one controls, served on the
  UIA backend as the `ControllerFor` relation, as a search box names its
  suggestion list.
- `cell`: `row` and `column`, zero-based, and `grid`, the `id` of the node
  that is its `ContainingGrid`, served on the UIA backend by the `GridItem`
  pattern.
- `automation_id`: the node's UIA automation id.
- `text`: the node's text, with bare line feeds. On the UIA backend the
  node serves the text pattern over it (`uia::text`, below). On the MSAA
  backend, which has no text interface, mockapp creates a real multi-line
  Win32 `EDIT` control holding the first such text, its line feeds made
  carriage return and line feed pairs, read through the edit control's
  own messages (`edit`, below). Optional text fields, each a list of
  UTF-16 stretches, shape what the text pattern answers: `spelling_errors`,
  `bold`, `italic`, `cultures` (each with a Windows locale id), and
  `styles` (below).
- `content`: true unless set false; on the UIA backend a node set false
  answers `IsContentElement` false, so a client that presents only control
  and content elements, as NVDA does, takes it for layout.
- `dialog_keys`, on the root: on the UIA backend the window takes a
  dialog's keys, Tab and Shift+Tab moving the focus among its buttons in
  tree order and around, each move raising UIA's focus event, and Space
  or Enter pressing the focused button, which closes the window
  (`owned_dialog.json`; `events.rs` sends the keys to the hidden window as
  its messages).
- `focus_on_activate`, on the root: on the UIA backend the window raises
  UIA's focus event for its focused node each time it is activated
  (`WM_ACTIVATE`, active or click-active), its first activation included,
  as a Win32 dialog's `DefDlgProc` restores its focus on every
  activation and an application restores its focus when its window is
  activated again (`owner_window.json`; `events.rs` sends the activation
  to the hidden window as its message). The event is raised from a
  message the window posts to itself, once the activation's messages are
  handled.
- `msaa_client`, on the root: on the UIA backend the window also answers
  MSAA's client object with the scripted root, as a window with a native
  UIA provider can serve MSAA too, so a Win32 control inside it has an
  MSAA parent with the root's states, the focused state included.
- `client_role`: on the MSAA backend the node answers its role as
  `ROLE_SYSTEM_CLIENT`, a window's client area's, as a Windows Forms
  form's does (`tests/fixtures/form.json`, with the `form-client` command,
  which gives the root and the window object above it no identity string).
- `nested`: how many groups of its own the node is placed under, between
  it and its parent, `<id>-nest-1` named "Nest 1", outermost, to
  `<id>-nest-<nested>` right above it, for a tree deeper than a fixture's
  JSON can nest (`tests/fixtures/nested.json`, a button under 66 groups).
- `window_class`, on the root: the host window's class. For example
  `tests/fixtures/msaa_class_window.json` is a window of a class on NVDA's
  MSAA class list, `MozillaWindowClass`, whose focus the listener skips by
  its class; and
  `tests/fixtures/console_window.json` is a console host's window,
  `ConsoleWindowClass`, whose text area reports the automation id "Text
  Area" as the console's does; `console_terminal.json` is the same window
  with a terminal's text; and `console_terminal_button.json` adds a native
  push button and serves the root's MSAA client, so a focus can move from
  the text area, read through UIA, to a Win32 control read through MSAA,
  as a Find dialog takes it. The tests start mockapp with no console, so
  no console host can be read for that window, and mockapp serves it.
- `frameless`, on the root: the host window is a pop-up window with no
  frame, as the taskbar's `Shell_TrayWnd` is, so UIA's proxy for a
  window's frame adds no title bar to its tree, whose buttons' reads wait
  seconds on `WM_GETTITLEBARINFOEX` under a parallel run
  (`tests/fixtures/taskbar.json`).
- `native`: the node is a real child window rather than a scripted node,
  on any backend: `tree_view`, `list_view`, `group_box`, `button`, or
  `uia_window` (each described under the modules below).
- `edit_version_6`, on the node with the text: the edit control is Common
  Controls version 6's, as a Windows Forms text box is
  (`tests/fixtures/text_version_6.json`), rather than the classic one.

## Modules

`mockapp` is a binary, not a library; its crate-internal modules are the
reviewable surface.

- `fixture`: JSON parsing and role and state name validation.
- `tree`: the owned, mutable scripted tree, a flat arena (`Tree`, shared
  as `SharedTree` behind `Arc<Mutex<_>>`) so provider COM objects and
  stdin command handling can both reach it without borrowing from the
  window's state; index 0 is always the root.
- `window`: Win32 class registration, window creation, and the message
  loop; dispatches `WM_GETOBJECT` to whichever backend is active and drains
  stdin commands posted from the reader thread.
- `stdin`: command parsing and the reader thread.
- `held_message`: `hold-message`, which puts a window procedure of its
  own in front of the native control's and holds the next message of the
  kind named inside it (`hits::held`), passing every message on.
- `uia`: the UIA provider. Every node gets its own COM object on demand:
  the root is a `RootProvider` (also the fragment root), every other node
  a `ChildProvider`; only the root implements
  `IRawElementProviderFragmentRoot`, so non-root elements never misreport
  themselves as fragment roots. Role and property mapping is the inverse
  of `verbatim_uia::map`.
- `uia::text`: the UIA text pattern (below).
- `msaa`: the MSAA provider. Every node is its own full `IAccessible`
  object (never a numbered simple child, unless `focus-child` makes it
  one), addressed two ways: the window's default client object
  (`OBJID_CLIENT`) is always the root, and every node also answers
  `WM_GETOBJECT` under a custom positive object id (`index + 1`) so
  `NotifyWinEvent` can address any node directly. It never answers the
  UIA root object id, so the arbitration probe finds nothing and the
  window arbitrates to MSAA. Role and state mapping is the inverse of
  `verbatim_ia2::map`. The list's `accSelection` returns the selected
  child as its own object, or an empty variant when none is selected.
- `common_controls`: Common Controls version 6 for mockapp's real
  controls. mockapp's manifest (`mockapp.exe.manifest`, embedded by
  `build.rs` as resource 2, not the process's own) declares version 6,
  whose tree view maps MSAA child ids to items; the tree view and the
  list view are made in an activation context built from it, so the
  process otherwise keeps the classic controls.
- `edit`: the MSAA backend's real edit control, created inside the host
  window, found by class, and selected with `EM_SETSEL` on the window
  thread. The classic control and version 6's answer `EM_LINEINDEX` for
  the line after the last differently: the classic control's -1 is
  sign-extended into the message's result, and version 6's is
  zero-extended, 4294967295, so a client must read the answer's low 32
  bits; `text.rs` pins both answers and reads both controls alike.
- `tree_view`: a `native` `tree_view` node is a comctl32 tree view, a
  child window of the host whose items are the node's `tree_item`
  children, nested as they are, read through comctl32's own MSAA
  implementation and its `TVM_*` messages, as Verbatim reads real
  applications' tree views (`tests/fixtures/tree_view.json`). Its own
  `window_class` registers it under another name first, a superclass whose
  `WM_GETOBJECT` answers the client object with comctl32's tree view proxy
  (`CreateStdAccessibleProxyW`), as Windows Forms names and wraps its tree
  view. With `state_images`, every item gets a state image, as a tree view
  that draws its own check boxes has: unchecked, or checked or partly
  checked by the item's `checked` or `mixed` state. (A tree view with
  comctl32's own check boxes, `TVS_CHECKBOXES`, reports its items to MSAA
  as check boxes instead.) An item's `expanded` state expands it, and
  `selected` selects it. A test reaches the items through the control's
  messages, which take plain integers (`tests/common/tree_view.rs`), and
  gives the scripted root the focused state, so a focus handed to an
  outpost passes NVDA's focused-state check without the control taking the
  keyboard focus.
- `list_view`: a `native` `list_view` node is a comctl32 list view in the
  report view, made in a version 6 activation context, with the node's
  `columns`, each a header and a width (0 hides it), and its `list_item`
  children as items, each named by its first column, its value split at
  `|` the texts of the others (`tests/fixtures/list_view.json`).
- `buttons`: a `native` `group_box` node is a standard group box (`Button`
  with `BS_GROUPBOX`) with its children as push buttons inside its
  rectangle, and a `native` `button` node a push button beside it; each
  window is placed below the last in z-order, as a dialog places its
  controls (`tests/fixtures/group_box.json`).
- `uia_window`: a `native` `uia_window` node becomes a child window of the
  host that answers UIA's root object id with the same scripted UIA
  provider as the host's, over a tree of its own: the node, as its root,
  and its children. Windows' own window objects answer MSAA for it. At
  most one per fixture. `island <command>` applies a command to its tree,
  with its window.
- `popup`: the owned popup the `popup <id>` command makes, a top-level
  window owned by the host, which hosts one node of the host's own tree:
  its `WM_GETOBJECT` answers that node's provider, and the node's
  provider names the popup as its host (`HostRawElementProvider`) and
  its `NativeWindowHandle`, so UIA moves the popup from the desktop's
  children to the node's place, its accessibility parent in its owner,
  as UIA's provider repositioning does for a combo box's drop-down
  (`tests/fixtures/popup.json`).
- `menu`: the Win32 popup menu the `popup-menu` command shows, the
  system's own `#32768` window, served by MSAA's menu proxy: two items,
  "First" and "Second", tracked by `TrackPopupMenuEx` on a thread of its
  own, owned by a hidden window there, so the host's thread goes on
  serving its provider and the commands. `menu-select <index>` selects an
  item by the menu's own `MN_SELECTITEM`, which raises its focus event,
  and `menu-close` ends the menu, answered once its loop has returned.
  Nothing takes the foreground or the input.
- `hits`: the provider-side hit counters (below).

### The UIA text pattern

`uia::text` serves `ITextProvider2` and `ITextRangeProvider`, both
answered for either pattern id, for a node with `text`. A range is two
UTF-16 offsets into the node's text, read afresh on every call, so the
`caret` command shows through at once. A range handed back by a client
(`CompareEndpoints`, `MoveEndpointByRange`) is one mockapp made, so its
offsets are read from its implementation.

- Units: a character is one code unit; a word is a run of letters and
  digits with the spaces after it, or one other character; a line ends
  after its line feed, and a text ending in one has an empty last line, as
  an editor shows; a paragraph is a line; a stretch of the format unit
  ends wherever a `spelling_errors`, `bold`, or `styles` stretch starts or
  ends (not an `italic` one, so a stretch holding italic and upright text
  reads as mixed, as in Windows Terminal); the page and the document are
  the whole text, unless the node's `page_lines` makes a page that many
  lines. Moving lands on a unit's start and never past the last
  unit; a move back from inside a unit to its start counts as one, as UIA
  specifies.
- The caret (`GetCaretRange`) is the selection's start, as the edit
  controls report it. On the UIA backend the keys pressed in the window
  move the focused node's caret by these units (`caret_key`): Right Arrow
  by a character, Control+Right Arrow to the next word's start, and Down
  and Up Arrow to the same column of the next or previous line, or its end
  when that line is shorter, raising UIA's text selection changed event
  when the caret moved, and nothing for a key that left it in place.
- Attributes: the language (`Culture`) is `en-US` outside the `cultures`
  stretches and theirs within one, mixed over a range holding more than
  one. The annotation types are the spelling error type (60001) within
  the spelling errors, mixed for a range holding both misspelled and
  other text, and unsupported for a range with no spelling error, as
  Windows 11 Notepad reports them. The font is Consolas, black outside the
  `styles.foregrounds` stretches (each with a `COLORREF`), 11 point
  outside the `styles.font_sizes` stretches, underlined only within the
  `styles.underlines` stretches (each with a UIA underline style), italic
  within the `italic` stretches, and weighing 700 within a bold stretch,
  400 outside, and mixed across both. Strikethrough, the background
  color, bullets, and links are supported only by a node whose `styles`
  lists their stretches (`strikethroughs`, `backgrounds`, `bullets`,
  `links`; `tests/fixtures/formatting.json`), with a default outside them
  (none, white, none, and no link), a link's value being its own range; a
  node without them answers "not supported" for each, as Windows Terminal
  answers the attributes it lacks (`text.json`). Every other attribute is
  unsupported.
- Terminal colours: with `styles.colors_skip_last`, both colors over a
  range of two or more characters come from all its characters but the
  last, as both terminals' providers do: the first character's when the
  rest but the last agree, mixed otherwise
  (`tests/fixtures/terminal-colours.json`, and
  `terminal-plain-colours.json` with no colored stretch). With
  `styles.distinct_colors`, every character's foreground is its offset
  and its background white less it, so every character's colors differ
  from its neighbors', a terminal colour read's worst case
  (`terminal-checkered.json`).
- Provider faults, each a fixture flag: `italic_fails` fails the
  `IsItalic` read with `E_FAIL`; `find_text_fails` fails `FindText` the
  same way, as Windows Terminal's has (`terminal_find_fails.json`);
  `find_backward_fails` fails only a backward `FindText`, so a failure
  that closes one direction of a search can be told from one that ends it
  (`terminal_find_backward_fails.json`); `lines_clamp` gives the last line
  with text for an expansion to any line below it, as the console host's
  provider does (`lines_clamp.json`); and `backward_moves_positive`
  answers a backward `Move` or `MoveEndpointByUnit` with a positive count,
  as some providers do (`backward_moves.json`).
- `FindText` matches the text within the range exactly (or ignoring ASCII
  case), the last match when searching backward, and returns no range
  when there is none; `FindAttribute` is not implemented.

### Hit counters

`hits` keeps one atomic counter per provider method: every
`IRawElementProviderSimple`, `IRawElementProviderFragment`,
`IRawElementProviderFragmentRoot`, and pattern-provider method, the text
pattern's and text range's methods a client reads with, and every
`IAccessible` and `IDispatch` method, plus one for `WM_GETOBJECT`. A test
reads them with synchronous window messages to the host window, answered
by the same thread that runs every provider call, so a read made after a
client's call returns counts every hit that call caused:

- `WM_APP + 2` returns the counter whose index in `Method::ALL` is
  `wParam`, and `WM_APP + 3` zeroes every counter.
- `WM_APP + 7` returns the count of `WM_GETOBJECT`s of the kind
  (`GetObjectKind`) whose index is `wParam` (`common::read_get_objects`):
  `UiaRootObjectId` with a nonzero `wParam`, the message a UIA connection
  begins with, `UiaRootObjectId` with a zero one, `OBJID_CLIENT`,
  `OBJID_WINDOW`, `OBJID_QUERYCLASSNAMEIDX`, and any other object id.
- The host window also counts each `WM_GETTITLEBARINFOEX` it takes, as the
  method `WM_GETTITLEBARINFOEX` (`hit_title_bar_info`), before its default
  procedure answers it: UIA's proxy for the window's frame sends one for
  each read of the title bar's location or off-screen state, and of its
  buttons', and Windows answers it for one window at a time across the
  desktop, so a test shows a read passes the title bar by. It is only
  counted, never delayed by `slow` or held by `hold`.
- With `--count-registrations`, the root provider is a
  `CountingRootProvider` (answering every other call through the ordinary
  root provider), which UIA calls as clients register and remove handlers
  for events in the fragment; `WM_APP + 4` returns the live registrations
  for the event whose id is `wParam` (`common::advised` in the tests), how
  `tests/shutdown.rs` sees that an outpost removed its handlers.

Every counted call is also where `slow` delays a call and `hold` holds
one. A parent step's own delay or failure, `slow-parent`,
`slow-acc-parent` and `fail-acc-parent`, is a per-node setting in the
tree instead (`Tree::parent_delay`), applied by the step after it is
counted and bracketed by `window::announce_call`'s two lines;
`hold-acc-parent` marks the node's next `accParent` to wait as `hold`'s
call does (`hits::held`). The tests compile `src/hits.rs` into their shared module, so the
method list and message numbers cannot drift apart.

## Implementation notes

- **Threading.** The window thread joins a single-threaded apartment
  (`COINIT_APARTMENTTHREADED`); every provider COM object is built with
  `Agile = false`, so cross-process calls into them are marshaled back onto
  this thread's message queue, which is why the message loop must keep
  pumping for the process's whole lifetime. Stdin is read on a separate
  thread (reading blocks, which the message loop cannot afford) and handed
  to the window thread over a channel, woken by a payload-free posted
  message rather than a pointer in `LPARAM`.
- **`idObject` recovery.** `WM_GETOBJECT`'s `lParam` carries `idObject`
  zero-extended into the full pointer width on some code paths (`oleacc`'s
  `AccessibleObjectFromWindow` probes arrive as a huge positive `lParam`),
  so recovering the standard negative object ids (`OBJID_CLIENT` and
  others) truncates back to 32 bits and reinterprets (`lparam.0 as i32`);
  `i32::try_from` would reject exactly the values this must recognize.
- **Null interface results.** UIA methods that can return "no such
  element" (`Navigate` at a tree boundary, `GetPatternProvider` for an
  unsupported pattern, `HostRawElementProvider` for a non-root node)
  cannot represent a null interface pointer in `windows-core`'s
  `NonNull`-backed interface types. `Err(windows_core::Error::empty())`
  is the documented escape: the generated COM glue turns it into `S_OK`
  with an untouched, null out-parameter, the UIA contract for "nothing
  here."
- **Real pattern objects.** `IUIAutomationCacheRequest`'s cache-building
  calls `GetPatternProvider` for `TogglePattern`, `ExpandCollapsePattern`,
  `ValuePattern`, and `SelectionItemPattern` before trusting a cached
  value, which is the path `verbatim-uia`'s shapes always use, so a
  `GetPropertyValue` override alone does not do. mockapp implements small
  `IToggleProvider`, `IExpandCollapseProvider`, `IValueProvider`, and
  `ISelectionItemProvider` objects, gated on role for toggle, on the
  fixture's `expanded` or `collapsed` states or a `value` for the middle
  two, and on its `selectable` or `selected` states for selection, and
  an `IInvokeProvider` for a node with a `default_action`, which only
  activation asks for. `Toggle` turns the node's checked state over, with
  no event, so a read after an activation finds the new state. A
  list, a tab control or a combo box also serves an `ISelectionProvider`
  whose selection is its children in the `selected` state, so a fixture's
  initial selection and the `select` command both show through the
  Selection pattern and its `Selection` property. A list's provider also
  serves `ISelectionProvider2` (`FirstSelectedItem`, `LastSelectedItem`,
  `CurrentSelectedItem`, the last selected, and `ItemCount`), and a tab
  control's does not, so the tests see a provider with
  `SelectionPattern2` and one without.
- **Raw-view host furniture.** A real window's UIA raw tree can include
  host-provided native elements, such as window-chrome furniture merged in
  through `HostRawElementProvider`, alongside the fixture's own children.
  The UIA tree-construction test tolerates unrecognized nodes only among
  the window's direct children for that reason, while still requiring
  every fixture node to be found.

## The test harness

The cross-process integration tests in `tests/` spawn the compiled binary
through `env!("CARGO_BIN_EXE_mockapp")`, using fixtures under
`tests/fixtures/` and a shared `tests/common/mod.rs` harness: `MockApp`,
killed on drop; `find_window` by exact, per-test-unique title; and
`named_element`, an element found by one search of the window's
descendants by name, which reads nothing of the window's title bar, where
a walk of the window's whole tree would wait on its
`WM_GETTITLEBARINFOEX`, seconds on a busy machine.

- Before it starts its first `mockapp`, a test process puts itself in a
  kill-on-close job of its own (`contain_children`), so every `mockapp` it
  starts ends with it however it ends: a panic in a callback the system
  calls aborts the process without running `MockApp`'s drop, and a
  `mockapp` left running would keep the standard error it inherited open,
  on which `cargo test` then waits forever. `child_cleanup.rs` pins this:
  it runs itself as a subprocess that starts a `mockapp` and panics where
  the panic cannot unwind, and asserts that the subprocess's standard
  error closes promptly.
- Every test file runs through `tests/common/harness.rs` instead of
  libtest (`harness = false`): it runs and reports the tests as libtest
  does, then ends the process without running DLL detach code, because
  `UIAutomationCore.dll`'s own detach code sometimes hangs or crashes in a
  process that has connected to providers (`books/dev/src/reference/uia-bugs.md`, "A process
  that used UIA can hang or crash as it exits").
- The outpost the tests run (`tests/common/outpost.rs`) reads the time
  from the test, which stands still unless the test moves it on
  (`pass_time`), so a window's verdict of no UIA provider runs
  out, its probe's `WM_GETOBJECT` is made, and a classic walk's time
  budget is spent only where the test says. The test hands the outpost
  focus facts as the listener would, and the outpost reads the focused
  element from the test rather than the system. The test moves the time
  on itself (`pass_time`). The outpost stamps its own events and times its verdicts' keeps by the
  test's observation clock (`observed_now`), with which the facts handed
  over are stamped. Each test chooses that clock once, as it makes the
  outpost, so none switches partway through (Fable, 2026-10-30):
  - `OutpostUnderTest::on_frozen_time` (or
    `on_frozen_time_with_options`) gives the frozen clock: the run's clock
    when the outpost was made, moved on only by the time passed and by a
    microsecond each read, so no real time is in it and no two reads are
    equal. Every test whose assertions depend on a verdict's 500 ms keep
    must use it, because on the real clock a slow runner's own steps run
    out a keep early: every `call_counts` test.
  - `OutpostUnderTest::new`, `with_options` and the rest give the real
    clock, the run's clock moved on by the time passed. Every other test
    uses it, because the outpost compares the stamps of its caret, text
    and terminal reads, which it reads from the run's clock itself, with
    events' observations, which the frozen clock would put in the past.

  An idle round (`idle_round`) reaches the outpost's idle answer,
  which waits while its record is open. `focus_moves_during_the_read`
  moves the focus window as the outpost reads the focused element, and
  `listener_forces_uia` has the listener's reader skip no focus by its
  window's class (`FocusReader::forces_uia`), so it reads one in a window
  on NVDA's MSAA class list. The listener's reader has no clock: it waits
  for every call. A test of the listener's watchdog can run the listener's
  reading threads in its process
  (`verbatim_listener::focus_read::ReadingThreads`).
- `tests/uia_bugs/` is one test binary (`main.rs` declares a module per
  file) holding the tests that reproduce a fault or limitation of UI
  Automation or show that Verbatim's workaround for one still holds, each
  file named after the entry's heading in `books/dev/src/reference/uia-bugs.md`, with
  `support.rs` holding the mechanics they share. A test that needs a
  second UIA client process starts the binary again with an environment
  variable that `main` recognizes: an element's owner, or a process racing
  UIA's first-time setup.

### Which tests use the desktop

None: every `mockapp` test runs on a desktop no other test is using
(`harness::run_isolated`), so `cargo test -p mockapp` and `cargo xtask ci`
put no window on the interactive desktop and can run while an end-to-end
run has it; the end-to-end suite (`#[ignore]`d, run only with
`--ignored`) is the one set of tests that takes the interactive desktop,
by design.

The runner starts the binary again for each test, with `--isolated-test`
and its name, and reports its result. Each of the runner's worker threads
makes one desktop and one job object, and runs its tests on that desktop
one after another, every process a test starts being in the job. Before
the next test starts there, the runner waits until the job's count of
running processes, which includes those of the job nested in it, is none,
so nothing of the earlier test is left on the desktop and the test is the
only client there while it runs. The job's completion port only wakes
that wait, because Windows does not guarantee to deliver its message that
the count reached none, and the port cannot tell that message from a
nested job's; if processes are still running a minute after the test
ended, the run fails naming the test. No test reads anything the desktop
keeps beyond its processes, such as its foreground window.

Desktops are made per thread, not per test, because making one costs the
Desktop Window Manager about a tenth of a second of work, done in turn for
the whole session, and while it is behind, anything that waits on it is
late, such as a window's title-bar information, which the MSAA proxy
reads (a full `cargo test -p mockapp` with 12 worker threads makes 169
desktops rather than 219; [troubleshooting](../../how-to/troubleshooting.md)).

A window and its events are seen only from its own desktop, and a process
started on one, mockapp included, starts its children there, so nothing
an end-to-end run's Verbatim, NVDA, or another test does reaches mockapp.
That matters most to the tests that pin the calls mockapp's providers
answer (`call_counts.rs`, `terminal.rs`) and to those that raise mockapp's
events and must hear no other client's: a provider cannot tell which
client called it, since UIA's calls reach it from UI Automation's own
threads in mockapp. No test needs the foreground or the keyboard focus,
which no window on such a desktop can have: a test that needs a window to
be the foreground, or an element to have the focus, tells the outpost
under test which (`tests/common/outpost.rs`).

## The tests

Each file's module comment says exactly what it asserts; in outline:

- `uia_tree.rs` and `msaa_tree.rs` walk a rich scripted tree through each
  real client stack and assert normalized roles, names, values, states,
  and detail properties match the fixture: every node's details must read
  back exactly what was scripted, and all-`None` for every node the
  fixture left plain (rectangles excluded: mockapp scripts no geometry,
  but UIA merges the real window's rectangle into the window-hosted root).
  `uia_tree.rs` also navigates remotely from a node kept from a text
  event, read as the `Kept` shape, and activates `activation.json`'s
  elements, each by the first of Invoke, Toggle and SelectionItem it has; `msaa_tree.rs` reads a list's selected
  child through `verbatim_ia2::acquire::selected_child` after a `select`,
  and that `slow-acc-parent` answers a node's `accParent` late, its wait
  bracketed by its two lines inside the client's call, and
  `fail-acc-parent` fails it with `E_FAIL` while the node's name still
  reads. A client's first call on a new MSAA object's proxy that is
  `accParent` reaches the provider twice, so the tests read the node's
  name first.
- `object_equality.rs` reports two MSAA focuses over
  `object_equality.json` through the outpost and reduces both in Core,
  asserting the containers entered with the second: NVDA's equality of
  MSAA objects found by the outpost for a container reached by another
  path, renamed, moved (`move`), or whose location fails
  (`fail-acc-location`), and not found for a second unnamed list
  elsewhere, a container made again (`recreate`), or one focused at an
  event child id.
- `arbitration.rs`: the raw probe (`verbatim_uia::raw_probe`) answers
  `0x10000` for a `uia`-backend window and the default window procedure's
  0 for an `msaa`-backend one, `verbatim_outpost::arbitration::Arbitrator`
  resolves each to the matching backend, and a window too busy to answer
  is asked once, with one window message and no connection, its "no"
  taken.
- `events.rs`: `set-name`, `set-value`, `select`, `notify`, and an active
  text position change are each observed by the matching
  `verbatim_uia::Registration` and `verbatim_ia2::WinEventHook`, one
  registration of all three hears each, and a registration on a focus
  hears its changes and neither its group's nor a sibling's. A `flood` of
  name changes raised back to back is heard whole, each event carrying
  the last name, since UIA fills an event's cache with calls into the
  window, which answers none until the flood ends. These use
  property, selection and notification changes rather than focus, so they
  never depend on real keyboard focus.
- `controller_for.rs`: `verbatim_uia::Uia::controlled_descendant` finds a
  selection inside the list a search box's `ControllerFor` names and
  nothing elsewhere, and a search box holding text has its focus come with
  the line at its caret ahead of a selection in its list.
- `combo_box.rs`: a UIA combo box without the `Value` pattern, and a
  looping selector, read through the focus listener's reader and a real
  outpost: each focus is reported with its first selected item's name as
  its value, and a later selection is a change of its value.
- `native_controls.rs`: real Win32 controls inside mockapp read through
  their own MSAA implementations by a real outpost, and the window
  boundary both ways. A `native` `button` in a host on the UIA backend is
  a Win32 control inside a window whose UIA provider is native
  (`boundary_uia_host.json`, and `boundary_uia_layout_host.json`, whose
  root is not content; both set `msaa_client`), and a `uia_window` in a
  host on the `win32` backend is a UIA window inside a plain Win32 window
  (`boundary_win32_host.json`). The test reports a focus in each and
  asserts its ancestors and what Core speaks: the UIA window's element
  before the button when it is a control and content, and not at all when
  it is not; and the Win32 window's client area, marked as a top-level
  window's, unspoken since the hidden window's client area is invisible,
  before the UIA window's root and its focused button. A UIA slider of
  class `SensitiveSlider` (`sensitive_slider.json`) above the focus
  carries its class from the listener's walk and the outpost's own, and
  Core speaks it with its value.
- `msaa_events.rs`: MSAA change events handled by a real outpost and
  spoken by the reducer: a description change on the focus is spoken, a
  change on any other object says nothing (`docs/nvda/events.md`, "The
  focus gate: what the default handlers actually speak"), and an MSAA window's foreground event handled before the
  window is in front is dropped.
- `window_names.rs`: a window renamed by its accessible name after it
  took the foreground, as Windows 11 Notepad renames its window, is
  announced by the name it had, and its rename is not spoken.
- `focus_reports.rs`, over `reuse.json` (two lists named alike, as a File
  Explorer folder and its subfolder): with remote operations and without,
  a focus on an item whose runtime id `take-runtime-id` gave it from a
  dead item is reported under that item's node with the evidence that
  the element it named is gone, a repeated focus keeps that node, and each costs exactly the calls pinned there; over
  `reuse_text.json` and `reuse_terminal.json`, a text field or terminal
  given the runtime id of another, gone (`take-runtime-id`) or alive
  without the focus (`share-runtime-id`), reads its own line or is read
  afresh as a terminal of its own, and its changes are followed; the
  listener's checks that fail let a focus through, as NVDA's COM error
  rule does: an earlier element's keyboard focus read that times out reads
  it gone, so the focus is no repeat, and the sender's that times out, or
  fails on an element disconnected (`disconnect`) after its nearest
  window's lookup, lets it through to its walk, whose failed read goes on
  from its window's parent; a windowless focus whose walk failed is found
  from the nearest window its read carries; a focus captured
  before its item was selected is reported with the item selected; a
  foreground event for a UIA-read window is dropped and the focus that
  follows reported alone; and over `described_group.json` an unnamed
  grouping with a description is reported as focus context without the
  description when the theme turns descriptions off.
- `slow_application.rs`: a real outpost waits for a slow application
  rather than cutting it off. It reports a focus on an `msaa`-backend
  mockapp whose window thread is stalled with `stall`, the read having
  started before the stall ended; and a navigation step, a walk down a
  focus's path, a UIA dialog's text (`dialog_class_window.json`, a
  top-level window of a dialog's class) and the taskbar's items
  (`taskbar.json`, a frameless window of `Shell_TrayWnd`'s class) each wait past the
  short limit they once had and answer in full. Each hold ends after
  evidence that the old limit has passed: the outpost's frozen clock moved
  past a limit that ran on it, or mockapp's window thread stalled for twice
  a limit that ran in real time, the stall's printed end the evidence. A
  query whose held call's window the outpost's reader then answers hung
  tells Core once that the window stopped responding, is not abandoned,
  the pong counting no worker parked, and answers its neighbor once
  mockapp answers; a listener read's marker waiting while mockapp's window
  thread stalls past Windows' five seconds tells Core once of the hang,
  found in front with no program in flight, and the focus is reported
  once its read is delivered after the stall; and a query on a windowless
  node of a hung window is refused with no call.
- `window_messages.rs` holds a message to a native control inside its
  window procedure (`hold-message`) and watches `verbatim-ia2`'s sends at
  its observation point `WindowMessageSent`, so the binary requires the
  `faults` feature: a text query whose `EM_GETSEL` waits behind a stall of twice the
  old half-second limit answers the caret and selection the focus read,
  its send made with no time limit; a list view column read's buffer is
  still allocated in mockapp while its `LVM_GETITEMTEXTW` is held and free
  once the read answers; a second read on another thread during the hold
  sends its own messages, each with a buffer of its own, every one free
  after both answer; a read whose message is held is gone once mockapp is
  ended; and an edit control whose buffers are in the test's own process,
  standing for a reused window handle, is gone with no message sent.
- `time_limits.rs` holds mockapp's next provider call (`hold`) and shows
  each of UIA's time limits ending the wait while the call is still held,
  each test in a process of its own since the timeouts belong to the
  process: a property read fails with `UIA_E_TIMEOUT` after one
  `UIA_TRANSACTION_TIMEOUT` (ten seconds); and a subscription
  moved onto a held window's subtree has its registration end at the
  two-second connection timeout, its next move made after it, exactly
  one registration call having taken past the timeout, as
  `verbatim-uia`'s observation point `RegistrationCallPassedTimeout` counts
  it (a wait into the held application by anything but a registration
  call is not counted); so the binary requires the `faults` feature and runs in `cargo
  xtask faults`.
- `shutdown.rs`: an outpost told to shut down during a read into mockapp,
  a remote operation in one test and classic reads in the other, lets the
  read finish and sends its answer, then removes every UIA event handler
  it registered. Each test waits for the shutdown to reach its wait for
  the calls in progress, the observation point `ShutdownWaitsForCalls`,
  before it checks that the held call keeps it there, so the binary
  requires the `faults` feature and runs in `cargo xtask faults`.
- `idle_barrier.rs`: the outpost's part of the harness's idle barrier
  (`docs/design/communication.md`, section 6.7): the answer to an idle
  query goes behind every message the outpost sent before it, and is held
  back while a caret key's watch is open.
- `caret_watch.rs`, over `caret_watch.json` (a UIA document and a
  button), through the outpost harness: a focus delivered while a caret
  key's watch is open is reported first and the watch ends, answered
  `WatchEnded`; a caret moved and reported after the watch opened answers
  it; a key that moves nothing says nothing, and a caret event that shows
  no change leaves its watch open; a focus-now query's document is the
  outpost's focus, so a caret event no key caused is reported; and
  Verbatim+Tab's query's (`FocusPurpose::Report`, through
  `OutpostUnderTest::focus_now_for`) is neither adopted nor followed, its
  text not read and such a caret event reported with nothing. mockapp
  acknowledges `caret-event` once the call that raised the event has
  returned, but the outpost hears it later, on UI Automation's threads,
  and again as the `WinEvent` UIA raises alongside it, in no fixed order;
  so before each step moves on, the test waits until the outpost has heard
  both (`OutpostUnderTest::heard`, from `Outpost::observe_heard`), or the
  first step's event could answer the second key's watch.
- `text.rs` drives `verbatim-outpost`'s text module over `text.json` on
  both stacks: lines, words, and characters; movement stopping at the
  empty last line; UIA's missing sentence unit and the edit control's
  paragraph in its place; a page unsupported; movement by page over
  `pages.json` and its refusal by the edit control; a point's location on
  both stacks; UIA's language; every
  attribute over `formatting.json` and the unsupported ones over
  `text.json`; backward moves over `backward_moves.json`; and a caret key
  answered with the word it reached and with a selection. It also runs
  Core's say-all over UIA text, which has no sentence unit
  (`uia_say_all_speaks_by_sentence_without_a_sentence_unit`).
- `terminal_snapshot.rs` reads `terminal.json`'s terminal with
  `verbatim-uia-rops`'s terminal read program remotely and classically,
  the two required to agree on every field, and pins the colours of
  `terminal-colours.json` as exact runs both ways.
- `terminal.rs` reads the terminal through the outpost's `terminal`
  module and gives each read to Core's reducer: new output is said line by
  line with one UIA call a read remotely, and classically, and Backspace
  says what it deleted with no caret event.
- `terminal_reading.rs` drives a real outpost's worker over the same
  terminal and asserts the reads and answers it sends Core: focus-now and
  foreground reports, each on-demand reading request and its answer, caret
  events with and without text changes, the console host's caret, the
  hooks' change events through MSAA, the whole document and history
  blocks, colours requests, and the read before a held key. A console
  host's caret watch ended by its poll is told from one ended at its own
  bound by the reason the outpost's observation point `CaretWatchEnded`
  gives, so the binary requires the `faults` feature and runs in `cargo
  xtask faults`.
- `remote_ops.rs` runs `verbatim-uia-rops`'s remote and classic focus
  ancestry over `ancestry.json` (a five-level chain, a list and a tab
  control with selected children) and asserts they return the same
  ancestors with the same cached properties, that an element that lost
  the focus returns early, and how both behave against a stalled mockapp
  and one that has exited; that a walk that hits UIA's transaction
  timeout is a failure both ways and never answered by the classic walk,
  and a real outpost then reports the focus below mockapp's window, a
  plain window named by its title;
  that a list whose selected item is gone is read both ways without it;
  that the listener drops a focus whose nearest window's lookup timed out
  for that reason, with no call after the lookup;
  and that `Uia::selected` fails with UIA's timeout when mockapp is slow
  or stalled, and as gone once it has exited, never answering that
  nothing is selected ([verbatim-uia-rops](verbatim-uia-rops.md)). It
  also shows that `slow-parent` lets the steps it counts pass at once and
  answers the next raw-view step to the parent late, its wait bracketed by
  its two lines inside that step's call.
- `call_counts.rs` is the operation ledger's ratchet
  (`books/dev/src/reference/performance.md`, "The ledger"): over `counts.json`, and
  `dialog.json` (a message box) for a dialog's own text, it measures each
  ledger operation on each backend and asserts exactly how many
  cross-process calls the client side made, by kind, and how many calls
  mockapp's providers answered, by method. The MSAA operations, and a
  focus on a text field with its caret, run through a real outpost; the
  UIA ones run on the test's own thread, making the same `verbatim-uia`
  calls in the same order as the outpost's worker once it has the element,
  since the worker finds a UIA focus by reading the system's keyboard
  focus, which a test must not take. It also pins event registrations by
  the provider calls they cost; that the focus listener's reader makes no
  connection to a window by its handle; that a focus chain stops below the
  process boundary and continues through MSAA above a parent with a UIA
  provider; a container's selected child; a caret move, a caret report and
  a caret key's watch on both stacks; say-all's batches over forty-five
  lines and over `languages.json`; and a line's mixed italic formatting
  over `italic.json`.
- `instruction_limit.rs` finds UIA's limit on the instructions one remote
  operation may execute and pins how many each of Verbatim's programs
  executes, counted with `verbatim_uia_rops::counting`, at its worst
  against mockapp and at a typical size: over `deep.json` (a list sixty
  groups deep) for the focus ancestry and navigation, `mixed.json` and
  `italic.json` for the caret read, `languages.json` and `terminal.json`
  for say-all and a terminal's tail, and `terminal-checkered.json` and
  `terminal-plain-colours.json` for a terminal's colours on a 120 by 30
  screen.

On a mismatch the counting tests print every measured count, so a
deliberate change updates all the numbers that moved in one pass.
