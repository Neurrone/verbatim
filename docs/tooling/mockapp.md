# Running mockapp by hand

How to run `mockapp` on its own against a fixture, and the stdin commands that drive it.

`mockapp` is a real, separate-process Win32 application that answers
`WM_GETOBJECT` as a genuine out-of-process UIA or MSAA provider over a
JSON-scripted tree; [the mockapp crate guide](../crates/mockapp.md) has its
design, its full command line, its fixture format and its tests. This guide
is about running it standalone, outside its own integration tests, to poke
at it directly.

Build it and run it against a fixture:

```
cargo build -p mockapp
target\debug\mockapp.exe --fixture crates\mockapp\tests\fixtures\tree.json --backend uia
```

`--backend` is `uia`, `msaa` or `win32`; `--title` overrides the window title
(default `mockapp`) when you need more than one instance running at once,
for example one of each backend side by side. The process creates one real
top-level window, hidden unless `--show` is given, prints `ready` once
the window exists and the provider is
answering, then reads commands from stdin until `quit`.

Fixture format: a JSON file holding one object per node, each with an
`id`, a `role`, optional `name`, `value` and `states`, and nested
`children`; the root object is the window itself. The crate guide lists
every field. `crates/mockapp/tests/fixtures/tree.json` is a full worked
example: a
window containing a button group, a list, a combo box, an editable text
field, a slider, a spin button, a tab control, a link, a toolbar, a status
bar, and a menu, which doubles as the reference for every role and state
name the fixture format accepts.

Stdin commands, one per line, once `ready` has printed. The window thread
acknowledges each on stdout once it has taken effect, events included:
`applied`, or `rejected: <reason>` for a command naming a node the fixture
lacks, one the backend does not support, or a line that is no command;
`stall` prints its own two lines instead, and `quit` prints nothing; a call
`slow-parent`, `slow-acc-parent` or `fail-acc-parent` changes prints two
lines of its own as well, when it is made. A test
waits for that line before it goes on, so it never reads the state from
before its command, and a typo in a node id fails at once. On the MSAA
backend a text node's text is a real edit control's (see the crate guide),
so the text commands act on that control. `crates/mockapp/src/stdin.rs`
parses them.

- `focus <id>` raises the backend's real focus-changed notification for
  that node (`UiaRaiseAutomationEvent` under UIA,
  `NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` under MSAA).
- `set-focus <id>` moves the focused state as `focus` does but raises no
  event, so no client on the machine calls into mockapp in response, for
  tests that count an operation's provider calls exactly and hand the
  focus to the outpost themselves.
- `set-name <id> <text>` and `set-value <id> <text>` update the scripted
  tree in place and raise the matching name-changed, value-changed, or
  property-changed notification; with no text they clear it.
- `set-description <id> <text>` sets the node's description, served as
  both its full description and its help text, and raises
  `EVENT_OBJECT_DESCRIPTIONCHANGE`, or under UIA a help text change.
- `set-controller-for <id> [<controlled>]` (UIA only) makes the node's
  `ControllerFor` relation name node `controlled`, or nothing when it is
  left out, and raises its property change.
- `set-states <id> <state>...` replaces the node's states with those
  named, in the fixture's snake case, and raises
  `EVENT_OBJECT_STATECHANGE`, or under UIA the change of the
  `ExpandCollapseState` property, as a combo box opening or closing
  raises.
- `select <id>` marks the node selected, moving the state off any other,
  and raises `SelectionItem_ElementSelected` under UIA or
  `EVENT_OBJECT_SELECTION` under MSAA.
- `caret <id> <start> [<end> [end]]` selects a text node's text between
  two UTF-16 offsets, or puts the caret alone at `start`, raising no
  event, as an application's caret moves before a client asks where it
  is; with `end`, `TextPattern2`'s caret is the selection's end, as
  Windows 11 Notepad's is after Shift with Right Arrow (UIA only). On the
  MSAA backend the offsets are the edit control's, carriage returns
  included, sent as `EM_SETSEL`.
- `caret-event <id>` raises the backend's caret event for a text node:
  UIA's text selection changed event, or under MSAA
  `EVENT_OBJECT_TEXTSELECTIONCHANGED` from the edit control's client
  object, which a Common Controls version 6 edit control raises whenever
  its caret moves.
- `console-caret <column> <row>` raises `EVENT_CONSOLE_CARET` from the
  window, its cursor visible at that cell, as the console host raises it
  for its console window.
- `system-caret` and `system-caret show` raise
  `EVENT_OBJECT_LOCATIONCHANGE`, or `EVENT_OBJECT_SHOW`, on the system
  caret (`OBJID_CARET`) from the window that owns it, the window under UIA
  and the edit control under MSAA, as Windows does for an application
  whose text raises no caret event of its own.
- `set-text <id> <text>` replaces a text node's text, raising no event,
  as a terminal's buffer changes before a client reads it; `\n` in the
  text is a line feed, `\r` a carriage return, and `\\` a backslash. A
  UIA range keeps its offsets, so a range read after the text is replaced
  holds the new text at those offsets. The terminal tests write rows
  padded to the width, each ended by a carriage return and line feed, as
  the terminals give them. On the MSAA backend it sends `WM_SETTEXT`, on
  which the edit control raises its own value change.
- `write-on-read <id> <text>` writes the text at the caret, and moves the
  caret past it, the next time any range of a UIA node's text is read, as
  a terminal writes a typed character in the middle of a client's read; in
  the MSAA backend, it types the text over the edit control's selection
  and raises the control's value change the next time another thread
  reads its text or selection.
- `text-changed <id>` raises UIA's text changed event from a text node,
  or under MSAA `EVENT_OBJECT_VALUECHANGE` from the edit control's client
  object.
- `screen <id> <rows>` (UIA only) puts only the last `rows` lines of a
  text node's text on screen, its visible range, as a terminal shows the
  end of its buffer; 0 shows the whole text again. It raises no event.
- `active-text-position <id> <start> <end>` (UIA only) raises UIA's
  active text position changed event from a text node, with the range
  between those UTF-16 offsets.
- `notify <text>` (UIA only) raises an `AutomationNotification` from the
  root provider with the text as its display string, kind `Other`,
  processing `All`, and the activity id `mockapp-notify`.
- `stall <ms> [<at> <command>]` blocks the window thread for that many
  milliseconds, so every cross-process call into the window waits,
  printing `stall started` as it begins and `stall ended <us>` as it ends,
  the time in microseconds since the Unix epoch on the run's clock
  (`verbatim_process::clock`, whose offset the tests hand mockapp), so a
  test waits for the stall itself rather than a guessed time.
  With `<at> <command>`, the command is applied `at` milliseconds into the
  stall and acknowledged between the two lines.
- `on-activate <command>` applies the command once the window is next
  activated, after the activation's window messages; several run in the
  order given. It is acknowledged when armed and again when the command
  runs.
- `slow <ms>` answers every provider call from then on that many
  milliseconds late, as an application busy building a window does; 0
  answers at once again.
- `slow-patterns <ms>` answers every pattern request
  (`GetPatternProvider`) that many milliseconds late from then on, as
  `slow <ms>` does for every provider call; filling an element's cache
  makes such requests and a walk to its nearest window does not. 0
  answers them at once again.
- `refuse-text on` and `refuse-text off` (UIA only): while on, every
  request for a node's text pattern fails with `E_FAIL`, as a provider not
  yet ready does.
- `answer-property <property id> <fail|empty|usual>` (UIA only) makes
  every provider's `GetPropertyValue` for that numeric property id fail,
  answer `VT_EMPTY`, or answer as usual again; one property at a time.
  UIA fills an event's cache after the event's command is acknowledged,
  so a test keeps an answer set until the event has arrived.
- `answer-parent <id> <fail|none|usual> [<after>]` (UIA only) makes that
  node's provider answer `Navigate(Parent)` failing with `E_FAIL`, with no
  parent, or as usual again, after first answering `after` such calls as
  usual, so a client's walk to the nearest window, which steps to the
  parent once, can pass first; UIA hands a client no parent either way.
- `slow-parent <id> <ms> [<after>]` (UIA only) makes that node's provider
  answer `Navigate(Parent)` that many milliseconds late, after first
  answering `after` such calls at once, counted as `answer-parent` counts
  them; 0 answers at once again. The wait blocks the window thread, so
  every call into the window made meanwhile waits too, as with an
  application stuck in one call, and past five seconds Windows reports the
  window hung. Each late call prints `slow-parent started <us>` as its
  wait begins and `slow-parent ended <us>` as it ends, both on the run's
  clock, as `stall` does, so a test waits for the call itself.
- `slow-acc-parent <id> <ms>` (MSAA only) does the same for the node's
  `accParent`, from the next call on, printing `slow-acc-parent started
  <us>` and `slow-acc-parent ended <us>`; 0 answers at once again.
- `move <id> <left> <top> <width> <height>` (MSAA only) makes the node's
  `accLocation` answer that rectangle, raising no event; a fixture node's
  `location`, four integers, gives its first one (all 0 otherwise).
- `fail-acc-location <id>` (MSAA only) makes the node's `accLocation`
  fail with `E_FAIL` from then on.
- `recreate <id>` (MSAA only) makes the node's object again, as an
  application that destroys a container and makes another in its place:
  the object handed out before answers every call as one whose server has
  disconnected it.
- `fail-acc-parent <id>` (MSAA only) makes the node's `accParent` fail
  with `E_FAIL` from then on, every other call on it answering as usual,
  printing `fail-acc-parent started <us>` and `fail-acc-parent ended <us>`
  as each failing call begins and ends. A client's first call on a new
  MSAA object's proxy that is `accParent` reaches the provider twice, one
  call after the other, so it prints its lines twice and, under
  `slow-acc-parent`, waits twice; a first call of `get_accName` reaches it
  once, as does every `accParent` after it (observed on Windows 11 26200,
  2026-10-29).
- `report-window <id>` (UIA only) makes a child window of the host
  window and destroys it, and that node's provider answers
  `NativeWindowHandle` with its handle, a window it does not host that
  no longer exists, as a fragment element of some providers can. UIA
  then reads the node as that window's element, runtime id
  `[42, handle]`, with no parent. A live window's handle would not do:
  UIA navigates from such an element by its window, up to the top-level
  window.
- `take-runtime-id <id> <from>` (UIA only) removes node `from` from the
  tree, every call on its elements failing with
  `UIA_E_ELEMENTNOTAVAILABLE`, and gives node `id` its runtime id, as File
  Explorer gives a new item the runtime id of one it destroyed. It raises
  no event.
- `replace-items <id>` (UIA only) replaces every child of that node, with
  everything under it, by a copy with a runtime id of its own, under the
  same fixture id and with the same states, so the copy of the selected
  item is selected; the old elements are gone, every call on them failing
  with `UIA_E_ELEMENTNOTAVAILABLE`. It raises no event, as the Settings
  app replaces its search suggestions' items after raising a selection
  for one of the old ones.
- `disconnect <id>` kills that node where it stands: its parent still
  lists it, but every call on it fails as on an object that is gone,
  with `UIA_E_ELEMENTNOTAVAILABLE` on UIA and `RPC_E_DISCONNECTED` on
  MSAA, as an item of a menu that closed after its parent named it does.
  It raises no event.
- `disconnect-at-first-child <id>` (UIA only) arms that node to die where
  it stands the first time its provider is asked for its first child,
  that call failing with `UIA_E_ELEMENTNOTAVAILABLE`, as an item of a
  menu that closed while a reader went down into it does: a read of its
  parent's children made before then has it. It raises no event.
- `focus-child <container> <child>` (MSAA only) addresses the container's
  children as numbered simple children from then on, as a Win32 tree
  view's items are (its `accFocus` names the focused child by child id,
  and its `accChild` has no object for them), moves the focused state to
  `child`, and raises `EVENT_OBJECT_FOCUS` on the container and then on
  the child in one turn of the window thread, as a Win32 tree view does.
- `client-name <text>` (MSAA only) names the window's client area, the
  root node, leaving the window's text as it is, and raises
  `EVENT_OBJECT_NAMECHANGE` on it, as Windows 11 Notepad does when first
  activated.
- `client-identity` (MSAA only) makes the client area answer as Windows'
  own client object does: `IAccIdentity` with the window's `OBJID_CLIENT`
  address, and `accParent` with the window object.
- `disable-client` (MSAA only) disables the window, as a modal dialog's
  owner is: the root node gains the unavailable state and
  `EVENT_OBJECT_STATECHANGE` is raised on the client area.
- `acc-focus-window <hwnd>` (MSAA only) makes every node's `accFocus`
  answer the client object of window `hwnd`, a decimal handle, from then
  on, acquired with `AccessibleObjectFromWindow` as it is asked, as an
  application whose own `accFocus` names an object another process serves
  would.
- `child-window` makes a hidden child window of the host window, of the
  host's class, which answers `WM_GETOBJECT` as the host does, so an event
  can name a node at its object id in a window of its own inside the
  host's, as a control's window is inside a dialog's; a test finds it with
  `FindWindowExW` under the host.
- `child-client-identity` (MSAA only) makes the client area of the child
  window `child-window` made answer as `client-identity` makes the host's,
  so its parent is its own window object, whose parent Windows answers
  with the host's client area, as a control's window is inside a Windows
  Forms panel whose client area offers no identity string.
- `form-client` (MSAA only) makes the host's client area answer as a
  Windows Forms form's: no identity string, its own window as its
  `IOleWindow`, and as its parent its window object, which offers no
  identity string either.
- `window-focus` gives the host window the keyboard focus within its
  thread (`SetFocus`), so it is its thread's focus window, and is
  rejected if the window did not take it.
- `hold` makes the next provider call, whichever client makes it, print
  `held <method>` as it begins and wait until `release`, so a test knows a
  client's call is in progress and decides when it ends.
- `release` lets the call `hold` held go on; it is applied and
  acknowledged by the stdin thread, since the window thread is the one
  held.
- `quit` exits the process.

A typical by-hand session: start `mockapp` with a fixture, point a real
accessibility client (`verbatim-inspect dump-tree` against a Verbatim whose
outpost is targeting the mockapp window, or any other UIA/MSAA inspection
tool) at the window, started with `--show`, then type `focus chk_enable` or `set-value slider1 75`
at the `mockapp` process's own stdin and watch the notification arrive on
the client side. This is exactly what `crates/mockapp/tests/events.rs`
automates, minus the automation.

## Which tests use the desktop

No `mockapp` test uses the interactive desktop; the
[crate guide](../crates/mockapp.md), "Which tests use the desktop", says
how they are kept off it.
