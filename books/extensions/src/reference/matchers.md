# Matchers

A matcher says which windows, events or controls something applies to. One grammar serves every place a matcher appears: rules, hook registrations, and the `find-descendant` and `find-ancestor` reads. Which properties and forms are allowed depends on where the matcher is used, and is checked when the extension is loaded.

This page describes matchers as designed for milestone M5.

## Forms

| Form | Matches when | Example |
|---|---|---|
| `property = "text"` | the property equals the text | `automation-id = "LineNumberBox"` |
| `property = ["a", "b"]` | the property equals any of them | `class = ["ForegroundStaging", "LauncherTipWnd"]` |
| several keys in one table | every one of them matches | `{ role = "button", name = "Volume" }` |
| `all = [ … ]` | every matcher in the list matches, for two conditions on one property | `all = [{ states = "selected" }, { states = "focused" }]` |
| `any = [ … ]` | at least one matcher in the list matches | `any = [{ automation-id = "A" }, { automation-id = "B" }]` |
| `not = { … }` | the matcher does not match | `not = { class = "Start" }` |
| `property = { prefix = "text" }` | the property starts with the text | `name = { prefix = "Search" }` |
| `property = { regex = "…" }` | the property matches a regular expression | `name = { regex = "^\\d+ results$" }` |
| `property = { at-least = n }` | a number is at least `n` | `windows-build = { at-least = 26200 }` |
| `property = { below = n }` | a number is below `n` | `windows-build = { below = 26200 }` |

For `states`, which is a set, a value matches when the control has that state; a list matches when it has any of them.

Regular expressions run in time proportional to the text, with a limit on their size, so no expression can stall Verbatim.

## Properties

| Property | Of | Values |
|---|---|---|
| `role` | a control | a role name, such as `"button"` or `"edit"` |
| `states` | a control | state names, such as `"selected"`, `"focused"` or `"unavailable"` |
| `name`, `value`, `description` | a control | text |
| `window-class` | a control | its window's class, after any `normalize-class` rule |
| `automation-id`, `uia-class-name`, `framework` | a UI Automation control | text |
| `control-type` | a UI Automation control | a control type name, such as `"Button"` |
| `runtime-id` | a UI Automation control | a list of numbers; `prefix` takes a list, such as `{ prefix = [14] }` |
| `control-id` | an MSAA control | a number |
| `msaa-role` | an MSAA control | a number |
| `class`, `root-class` | a window, in window rules | text: the window's class, and its top-level window's class |
| `kind`, `activity-id` | an event, in event rules | an event kind ([Events](events.md)), and a notification's activity id |
| `windows-build`, `windows-revision` | the computer | numbers |

## Where each form may be used

| Used in | Properties | Forms |
|---|---|---|
| Window rules | `class`, `root-class`, the Windows version | exact text and lists, `all`, `any`, `not`, `at-least`, `below`; at least one property named directly at the top level, not only inside `not` |
| Event rules | `kind`, `class`, `activity-id`, the Windows version | the same as window rules |
| Node rules, hook registrations, the find reads | every control property of the backend the rule or hook names, the Windows version | every form |

Window and event rules are looked up in a table on every event, which is why they take exact text and need one property named directly: Verbatim finds the candidates by that property, then checks the rest.

A node rule or a hook names its backend, `uia` or `msaa`, and a property of the other backend is refused, so it is always clear which backend a rule is about. A fix needed under both backends is two rules.
