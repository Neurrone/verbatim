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

Stdin commands, one per line, once `ready` has printed:

- `focus <id>` raises the backend's real focus-changed notification for
  that node (`UiaRaiseAutomationEvent` under UIA,
  `NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` under MSAA).
- `set-name <id> <text>` and `set-value <id> <text>` update the scripted
  tree in place and raise the matching name-changed, value-changed, or
  property-changed notification.
- `replace-items <id>` (UIA only) replaces every child of that node, with
  everything under it, by a copy with a runtime id of its own, under the
  same fixture id and with the same states, so the copy of the selected
  item is selected; the old elements are gone, every call on them failing
  with `UIA_E_ELEMENTNOTAVAILABLE`. It raises no event, as the Settings
  app replaces its search suggestions' items after raising a selection
  for one of the old ones.
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
interactive desktop and can run while an end-to-end run has it. A window
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
