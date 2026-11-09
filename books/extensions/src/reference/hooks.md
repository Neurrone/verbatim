# Hooks

A hook is a function your component exports and Verbatim calls. Rules always run first, and a hook sees the control as the rules left it. Every hook has a deadline ([Reliability](../explanation/reliability.md)).

This page describes hooks as designed for milestone M5.

## The hooks

| Hook | Called | Returns | Kinds |
|---|---|---|---|
| `on-focus(node)` | when the focus moves to a control matching its registration | `pass`, `drop`, `replace(node)` or `rewrite(changes)` | app |
| `on-event(node, event)` | for each event of the kinds and matcher it registered ([Events](events.md)) | `pass`, `drop` or `rewrite(changes)` | app |
| `on-node(nodes)` | once for each batch of controls Verbatim reads in your applications, such as a new focus and its ancestors, for those matching its registration | the changes for each control | app |
| `status-bar(focus)` | when the user asks for the status bar | a control, or none | app |
| `on-command(command, trigger)` | when one of the module's gestures is pressed ([Commands and gestures](commands.md)) | nothing | app, global |
| `on-list-dialog(dialog, answer)` | when the user answers a list dialog the module showed | nothing | app, global |

## Verdicts

- `pass`: carry on to the layer below, then Verbatim's own behaviour.
- `drop`: say nothing for this event, and stop.
- `replace(node)`: announce another control as the focus instead, and stop; it must be one your module read. This is a focus redirect in code.
- `rewrite(changes)`: change properties of the control for this announcement only, and carry on to the layer below, which sees your changes.

`on-node`'s changes apply wherever the control goes, as a node rule's do; `rewrite` applies to the one announcement.

## Registering

A hook is called only for what its registration in `module.toml` names, so Verbatim calls nothing for events you do not handle.

```toml
[[hooks]]
hook = "on-event"
kinds = ["notification"]
backend = "uia"
match = { automation-id = "CalculatorResults" }
```

`on-command` and `on-list-dialog` need no registration: they are called for the module's own commands and dialogs.

## Order

Across layers, hooks run from the highest layer down: a module in the developer folder or an installed extension, then the built-in. `pass` and `rewrite` hand the event to the next, which sees the control as the layers above left it; `drop` and `replace` end the chain, so a lower layer never undoes them. `on-node` runs the same way, each layer's changes added to those above, and on a focus it runs before `on-focus`, which sees its changes. For `status-bar`, the highest layer that returns a control wins.

A module's hooks never run at the same time as each other for one application, so your instance needs no locking.

## Where hooks run

Your code runs in your extension's own process. Verbatim calls a hook from the process reading the application, which waits for your answer, and your reads go back to it. Each hook costs a round trip between processes on the event's path, and each read another, which is why a rule is cheaper wherever one is enough.
