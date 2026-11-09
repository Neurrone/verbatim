# Events

The events a module can match in event rules and receive in hooks. Each kind carries the control it is about and the data listed. Verbatim's own events carry more than this, such as the focus's ancestors; a module reads what it needs instead.

This page describes events as designed for milestone M5. Kinds are added as modules need them.

| Kind | What happened | Delivered to | Data besides the control |
|---|---|---|---|
| `focus` | Keyboard focus moved to the control. | `on-focus` | none; read ancestors with `parent()` |
| `selection` | The control, an item, was selected in its container, such as a tab. | `on-event` | none |
| `name-change` | The control's name changed. | `on-event` | the new name, or none |
| `value-change` | The control's value changed, such as a slider or a toggle. | `on-event` | the new value, or none |
| `notification` | The application raised a UI Automation notification. | `on-event` | the kind (`item-added`, `item-removed`, `action-completed`, `action-aborted` or `other`), the processing hint, the text or none, and the activity id or none |

NVDA has more event kinds, such as a window opening or a control being shown. Verbatim adds each when a module needs it and Verbatim's readers produce it.
