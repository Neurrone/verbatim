# Imports

The functions your component can call, grouped into the interfaces of the `verbatim:ext` WIT package. Which interfaces your component imports decides what the install dialog shows ([Capabilities and the install dialog](capabilities.md)). Imports can only be called during a hook, since that is the only time your code runs.

This page describes the imports as designed for milestone M5. Once the WIT files exist, this page is generated from them, and the interface and type names here may change to match.

## Reading controls

A control is a `node`, a handle owned by your instance: you cannot make one up, and `replace`, `speak-node` and `click` accept only nodes you read.

| Function | Returns | Notes |
|---|---|---|
| `node.properties()` | role, name, value, description, states, check state, expansion state, window class, and the backend's own: automation id, UIA class name, control type, framework and runtime id for UI Automation; control id and MSAA role for MSAA | Read from what Verbatim already holds, with no call into the application. |
| `node.parent()`, `node.first-child()`, `node.last-child()`, `node.next-sibling()`, `node.previous-sibling()` | a node, or none | |
| `node.children()` | a list of nodes | |
| `node.find-descendant(matcher, depth-limit)`, `node.find-ancestor(matcher)` | a node, or none | The [matcher](matchers.md) grammar; one call into the application where the backend allows. |
| `node.window()` | the control's top-level window | |
| `node.text()` | a text control's whole value | |
| `node.process()` | the control's process: executable, path, product version, bitness and console client | |
| `focus()`, `foreground()` | the current focus or foreground | none when it is in an application you cannot read |
| `find-window(application, class-path, backend)` | a node, or none | The first class names a top-level window of the application, each later one a window inside it at any depth. |

Every read stops at your applications: the ones an app module claims, or a global module's `reads`. Anything beyond comes back as none. A read fails with:

- `gone`: the control no longer exists;
- `deadline`: the event's time is spent;
- `outside`: the control is beyond your applications;
- `wrong-backend`: a matcher named a property of the other backend.

## Speech and sound

| Function | Notes |
|---|---|
| `speak(message)` | A message is text, such as a value read from the application, or a message id from your strings with its arguments. It is queued behind current speech. Speech for an application that is not in front is always queued. Speech from `on-command` is dropped once another key is pressed. |
| `speak-node(node)` | Announces a control exactly as Verbatim would announce it as the focus, with its role, name, states and position, styled by the user's theme. |
| `cancel-speech()` | Stops current speech; it has an effect only while your application has the user's attention, or, during `on-command`, until another key is pressed, in a global module too. |
| `tone(hertz, milliseconds)` | Plays a tone. |
| `browseable-message(title, text)` | Shows plain text in a window the user can read and close. |

## Commands

| Function | Notes |
|---|---|
| `pass-through()` | Sends the gesture that ran your command on to the application. Only during `on-command`. |

## List dialogs

| Function | Notes |
|---|---|
| `show-list-dialog(spec)` | Shows a list dialog and returns its id. The spec has a title, a label for the list, the items, and buttons, each with its label and access key, one of them the default. Verbatim adds Cancel. The answer comes to `on-list-dialog`: the button pressed with the selected item, or cancelled. |

A module has at most one list dialog. Calling `show-list-dialog` while yours is open replaces its title, label, items and buttons, selects the first item and brings it to the front. Different modules' dialogs can be open together, and each appears in Alt+Tab.

## The mouse

| Function | Notes |
|---|---|
| `click(node, button)` | Clicks the centre of the control where it is now: `left`, `left-double` or `right`. Fails with `gone` or `unclickable` when it has gone or has no place on the screen. |

## The system

| Function | Returns |
|---|---|
| `windows-version()` | major, minor, build and revision |
| `clock()` | the local time and the long date, formatted by the user's regional settings |

## Log

| Function | Notes |
|---|---|
| `log(level, message)` | Writes to Verbatim's log under your module's id. Never write the user's content to the log. |
