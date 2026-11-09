# Rules

A rule is "match this, do that": a [matcher](matchers.md) and an action, which Verbatim applies itself, without running your code. A module made only of rules costs nothing per event beyond a table lookup, so prefer a rule wherever one can express your fix.

This page describes rules as designed for milestone M5.

Every rule has:

- an `id`, unique within its module; the focus report and the log name a rule as `<module>/<rule id>` wherever it took effect;
- a `subject`, `window`, `event` or `node`, which fixes the properties it may match and the actions it may take;
- a `match`, a matcher.

A rule applies only to the applications its module claims, so global modules have no rules.

## Window rules

Decide how a window is read, before anything in it is.

| Action | Effect |
|---|---|
| `backend = "uia"` or `"msaa"` | Read this window through UI Automation, or through MSAA. |
| `silent = ["focus", "foreground"]` | A focus or foreground change to this window cuts speech and says nothing, for windows that only stage another. |
| `normalize-class = "Name"` | Treat the window's class as another, so everything after sees the normalized class. |

```toml
[[rules]]
id = "shell-roots-are-uia"
subject = "window"
match = { root-class = ["Shell_TrayWnd", "XamlExplorerHostIslandWindow"],
          not = { class = "Start" } }
action = { backend = "uia" }
```

## Event rules

Decide whether an event is handled at all.

| Action | Effect |
|---|---|
| `action = "accept"` | Handle this event even when its application does not have the focus, which Verbatim otherwise filters out; Verbatim starts reading the application if it was not. |
| `action = "drop"` | Ignore the event. |

```toml
[[rules]]
id = "snap-results-are-spoken"
subject = "event"
match = { kind = "notification", activity-id = "Windows.Shell.SnapComponent.SnapHotKeyResults" }
action = "accept"
```

## Node rules

Fix what a control says, on every control Verbatim reads in your applications. A node rule names its `backend`.

| Action | Effect |
|---|---|
| `set = { role = "edit" }` | Set the role, name, value or description to a value. |
| `set = { name = { from = "description" } }` | Set a property from another of the control's properties. |
| `clear = ["description"]` | Clear the name, value or description. |
| `transform = { name = "strip-ampersand" }` | Apply a named text transform: `strip-ampersand`, `strip-bidi-marks` or `trim`. |
| `add-states = [ … ]`, `remove-states = [ … ]` | Add or remove states. |
| `check-state = "checked"` | Set the check state: `unchecked`, `checked` or `mixed`. |
| `expansion = "collapsed"` | Set the expansion state: `expanded` or `collapsed`. |
| `presentation = "layout"` | Hide the control from navigation and announcement, or `"content"` to show it. |
| `position = "suppress"` | Do not report its position, such as "3 of 7". |
| `drop = "event"` or `"notifications"` | Ignore this event, or every notification from this control. |
| `veto-focus = true` | Ignore a focus event on this control. |
| `redirect-focus = "parent"` | Announce another control as the focus: `"parent"`, `"first-child"`, or `{ automation-id = "…" }` for a descendant. |

```toml
[[rules]]
id = "goto-line-is-edit"
subject = "node"
backend = "uia"
match = { automation-id = "LineNumberBox" }
action = { set = { role = "edit" } }
```

## Where rules run and what they cost

| Rule | Runs | Cost |
|---|---|---|
| Window `backend` | where Verbatim first reads the window | a table lookup |
| Window `normalize-class` | everywhere Verbatim judges a window's class, so every rule matching `window-class` sees the normalized class | a table lookup |
| Window `silent` | where Verbatim decides what to say | a table lookup |
| Event `drop` | as early as its properties allow: before Verbatim reads the control when the matcher names only the event's own properties | a table lookup |
| Event `accept` | where Verbatim decides what to say | a table lookup, and starting to read the application if needed |
| Focus veto | before Verbatim reads the focus, when the matcher names only class, automation id, control type, framework, name, runtime id or window, and also on every focus Verbatim reads by itself, such as after a menu closes; otherwise on the read control | none extra, or the read Verbatim makes anyway |
| Other node rules, redirects | on the read control | a check of properties already read; a redirect adds the read of its target |

Verbatim chooses where each rule runs from the properties it names. Every property works in every node rule; only the cost differs.

## Order and conflicts

- Within one module, rules apply in the order written, and a later rule wins a property an earlier one set.
- Overlapping rules are never refused or warned of: where two rules of one module set the same property, the later one wins, and the focus report names both.
- Across layers, the developer folder or an installed extension applies after the built-in, so its rule wins a property both set.
- A drop from any layer stands.

The focus report shows each property with where it came from, for example "role: edit, set by notepad/goto-line-is-edit, was dialog from UIA".
