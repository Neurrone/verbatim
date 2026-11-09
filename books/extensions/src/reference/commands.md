# Commands and gestures

A command is something the user does with a gesture, such as a key press. Your module declares its commands and their default gestures in `module.toml`, and Verbatim calls `on-command` when one is pressed.

This page describes commands as designed for milestone M5.

## Declaring a command

| Key | Required | What it is |
|---|---|---|
| `id` | yes | The command's id, unique in the module. |
| `gestures` | yes | Its default gestures (below). |
| `description` | yes | A message id for what the command does, shown in input help and the install dialog. |
| `category` | no | One of Verbatim's command categories, for grouping in input help. Left out, the command is grouped under your extension's name. |
| `pass-through` | no | `true` to send the gesture on to the application as well, always. |

An app module's commands work only while one of its applications is in front. A global module's commands work everywhere. Where a global module and the focused application's app module bind the same gesture, the global module's command runs, as in NVDA, which checks global plugins before the application's module.

## Gestures

A gesture is written as text for a single press, or as a table with a number of presses:

```toml
[[commands]]
id = "tray-list"
gestures = ["verbatim+f11"]
description = "tray-list-description"

[[commands]]
id = "taskbar-list"
gestures = [{ keys = "verbatim+f11", presses = 2 }]
description = "taskbar-list-description"
```

`keys` uses Verbatim's gesture names; `presses` defaults to 1.

Every press runs a command at once, never waiting to see if another follows: a single press runs the command bound to one press, and a second press within the multi-press interval runs the one bound to two. A press with no binding for its count runs the one with the highest count below it, so a third press of Verbatim+F11 above runs `taskbar-list` again.

## What `on-command` receives

`on-command(command, trigger)` receives the command's id and a `trigger` with two gestures, each a set of modifiers and a main key:

- `declared`: the gesture in your manifest that this press resolved to. If the user changed that gesture to another key, `declared` is still yours, so a command bound to several keys can tell them apart: a command bound to `alt+1` to `alt+9` reads the number from `declared`.
- `pressed`: the keys actually pressed.

A gesture the user added to your command, rather than changed, has no `declared` gesture; treat that as your command's default case.

## Dialogs opened by a command

If one press of a gesture opens a dialog and a second press of the same gesture runs another command, Verbatim holds the first dialog until the multi-press interval after the press has passed and every later press of the gesture within it has run its command, however long the first command took. If the second press comes, the second command's dialog replaces it, so the user never sees the first. Commands that only speak are never held: a second press interrupts the first one's speech, as with the time and date.

## Conflicts

A gesture your command shares with one already bound, by Verbatim, a built-in or another extension, is listed in the install dialog's commands, with what it replaces. Where your command applies, it takes the gesture.
