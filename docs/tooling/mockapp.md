# Running mockapp by hand

How to run `mockapp` on its own against a fixture, and which tests use the interactive desktop.

`mockapp` is a real, separate-process Win32 application that answers
`WM_GETOBJECT` as a genuine out-of-process UIA or MSAA provider over a
JSON-scripted tree — see [the mockapp crate guide](../crates/mockapp.md) for the
full internal design. This section is about running it standalone, outside
its own integration tests, to poke at it directly.

Build it and run it against a fixture:

```
cargo build -p mockapp
target\debug\mockapp.exe --fixture crates\mockapp\tests\fixtures\tree.json --backend uia
```

`--backend` is `uia` or `msaa`; `--title` overrides the window title
(default `mockapp`) when you need more than one instance running at once,
for example one of each backend side by side. The process creates one real
top-level window, prints `ready` once the window exists and the provider is
answering, then reads commands from stdin until `quit`.

Fixture format: a JSON file holding one object per node. Each node has an
`id` (a unique string), a `role` (a `verbatim_model::Role` name in snake
case, such as `check_box` or `list_item`), optional `name` and `value`
strings, an optional `states` array (`State` names in snake case, such as
`read_only` or `mixed`), and an optional `children` array of nested node
objects. The root object corresponds to the window itself.
`crates/mockapp/tests/fixtures/tree.json` is a full worked example — a
window containing a button group, a list, a combo box, an editable text
field, a slider, a spin button, a tab control, a link, a toolbar, a status
bar, and a menu — and doubles as the reference for every role and state
name the fixture format accepts.

Stdin commands, one per line, once `ready` has printed. Each is
acknowledged on stdout once it has taken effect, `applied` or `rejected:
<reason>`; `stall` prints its own two lines instead, and `quit` prints
nothing. `crates/mockapp/src/stdin.rs` documents each command in full.

- `focus <id>` raises the backend's real focus-changed notification for
  that node (`UiaRaiseAutomationEvent` under UIA,
  `NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` under MSAA).
- `set-focus <id>` moves the focused state as `focus` does but raises no
  event, for tests that count an operation's provider calls exactly.
- `set-name <id> <text>` and `set-value <id> <text>` update the scripted
  tree in place and raise the matching name-changed, value-changed, or
  property-changed notification; with no text they clear it.
- `set-description <id> <text>` sets the node's description, served as
  both its full description and its help text, and raises
  `EVENT_OBJECT_DESCRIPTIONCHANGE`, or under UIA a help text change.
- `set-controller-for <id> [<controlled>]` (UIA only) makes the node's
  `ControllerFor` relation name node `controlled`, or nothing when it is
  left out, and raises its property change.
- `set-states <id> <state>...` (MSAA only) replaces the node's states with
  those named, in the fixture's snake case, and raises
  `EVENT_OBJECT_STATECHANGE`.
- `select <id>` marks the node selected, moving the state off any other,
  and raises `SelectionItem_ElementSelected` under UIA or
  `EVENT_OBJECT_SELECTION` under MSAA.
- `caret <id> <start> [<end> [end]]` selects a text node's text between
  two UTF-16 offsets, or puts the caret alone at `start`, raising no
  event; with `end`, `TextPattern2`'s caret is the selection's end, as
  Windows 11 Notepad's is after Shift with Right Arrow (UIA only).
- `caret-event <id>` raises the backend's caret event for a text node:
  UIA's text selection changed event, or under MSAA
  `EVENT_OBJECT_TEXTSELECTIONCHANGED` from the edit control's client
  object.
- `console-caret <column> <row>` raises `EVENT_CONSOLE_CARET` from the
  window, its cursor visible at that cell, as the console host raises it
  for its console window.
- `system-caret` and `system-caret show` raise
  `EVENT_OBJECT_LOCATIONCHANGE`, or `EVENT_OBJECT_SHOW`, on the system
  caret from the window that owns it, as Windows does for an application
  whose text raises no caret event of its own.
- `set-text <id> <text>` (UIA only) replaces a text node's text, raising
  no event; `\n` in the text is a line feed and `\\` a backslash.
- `write-on-read <id> <text>` writes the text at the caret, and moves the
  caret past it, the next time any range of a UIA node's text is read, as
  a terminal writes a typed character in the middle of a client's read; in
  the MSAA backend, it types the text over the edit control's selection
  and raises the control's value change the next time another thread
  reads its text or selection.
- `text-changed <id>` (UIA only) raises UIA's text changed event from a
  text node.
- `screen <id> <rows>` (UIA only) puts only the last `rows` lines of a
  text node's text on screen, its visible range, as a terminal shows the
  end of its buffer; 0 shows the whole text again. It raises no event.
- `active-text-position <id> <start> <end>` (UIA only) raises UIA's
  active text position changed event from a text node, with the range
  between those UTF-16 offsets.
- `notify <text>` (UIA only) raises an `AutomationNotification` carrying
  the text from the root provider; the MSAA backend reports it as
  unsupported.
- `stall <ms> [<at> <command>]` blocks the window thread for that many
  milliseconds, so every cross-process call into the window waits,
  printing `stall started` as it begins and `stall ended <us>` as it ends.
  With `<at> <command>`, the command is applied `at` milliseconds into the
  stall and acknowledged between the two lines.
- `on-activate <command>` applies the command once the window is next
  activated, after the activation's window messages; it is acknowledged
  when armed and again when the command runs.
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
- `answer-parent <id> <fail|none|usual> [<after>]` (UIA only) makes that
  node's provider answer `Navigate(Parent)` failing with `E_FAIL`, with no
  parent, or as usual again, after first answering `after` such calls as
  usual; UIA hands a client no parent either way.
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
- `focus-child <container> <child>` (MSAA only) addresses the container's
  children as numbered simple children, moves the focused state to
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
- `hold` makes the next provider call, whichever client makes it, print
  `held <method>` as it begins and wait until `release`.
- `release` lets the call `hold` held go on; it is applied and
  acknowledged by the stdin thread, since the window thread is the one
  held.
- `quit` exits the process.

A typical by-hand session: start `mockapp` with a fixture, point a real
accessibility client (`verbatim-inspect dump-tree` against a Verbatim whose
outpost is targeting the mockapp window, or any other UIA/MSAA inspection
tool) at the window, then type `focus chk_enable` or `set-value slider1 75`
at the `mockapp` process's own stdin and watch the notification arrive on
the client side. This is exactly what `crates/mockapp/tests/events.rs`
automates, minus the automation.

## Which tests use the desktop

Every `mockapp` test runs on a desktop no other test is using
(`tests/common/harness.rs`, `run_isolated`): each of the runner's worker
threads makes one desktop and runs its tests there one after another,
each starting once every process the one before it started has ended, so
`cargo test -p mockapp` and `cargo xtask ci` put no window on the
interactive desktop and can run while an end-to-end run has it. A test is
the only client on its desktop while it runs, as it was when each test
had a desktop of its own; what differs is that the desktop has been used
before, by tests whose processes, and so their windows, hooks and
providers, have all ended, and no test reads anything the desktop keeps
beyond its processes, such as its foreground window. One desktop per
thread rather than per test spares the Desktop Window Manager about a
tenth of a second of work for each desktop not made (169 desktops with
12 worker threads in a full `cargo test -p mockapp`, not 219), whose
backlog delays title-bar reads (`troubleshooting.md`). A window
and its events are seen only from its own desktop, so nothing an
end-to-end run's Verbatim, NVDA, or another test does reaches mockapp
either. None of these tests needs the foreground or the keyboard focus,
which no window on such a desktop can have; a test that needs a window to
be the foreground or an element to be focused tells the outpost under
test which (`tests/common/outpost.rs`). Checked on 2026-10-08 with an
out-of-context `WinEvent` hook on the desktop the run started from,
logging every top-level window created or shown by a process whose
executable is under `target` or is `mockapp.exe`: `cargo test -p mockapp`
and `cargo xtask ci` logged none. The end-to-end suite (`#[ignore]`d,
run only with `--ignored`) is the one set of tests that takes the
interactive desktop, by design.
