# verbatim-gui

The wxDragon GUI (decision D4, architecture section 11): the hidden main
frame, the tray icon and Verbatim menu, and the NVDA-style settings dialog.

Public API:

- `GuiCommand` (`ShowMenu`, `OpenSettings`,
  `OpenShellItemList(ShellItemKind)`, `Shutdown`) and `GuiEvent`
  (`QuitRequested`) — the two seams to the application. The GUI never exits
  the process; Exit raises `QuitRequested` and the app decides.
- `GuiHandle::send(command)` — cloneable, callable from any thread;
  internally enqueues through wxDragon's call-after queue and wakes the
  idle loop.
- `run_gui(settings_host, events, on_ready)` — runs the event loop on the
  calling thread (the app calls it from the process main thread); once the
  frame and tray exist, `on_ready` hands out the `GuiHandle`.
- `list_dialog` — the reusable list dialog component (M3): a title, a
  static label above a single-selection list box around 550 by 250, and a
  configurable row of buttons plus an automatic Cancel. Callers describe
  the dialog as data: `ListDialogSpec` carries items as display strings
  whose index is the opaque payload key, and each `ListDialogButton` is a
  Fluent label message id plus a callback returning a `ButtonVerdict`
  (close or keep open). Escape and window close dismiss; Enter or a double
  click on a list item triggers the default button; activation only ever
  runs against a selected item. The systrayList replica presents through
  it and M6's elements list will present through the same one. GUI thread
  only, like all widget code.
- `shell_items` — system tray and taskbar item enumeration for the
  systrayList replica: `request_shell_items(kind, present)` enumerates on
  a short-lived worker thread and hands `present` the outcome on the GUI
  thread through the call-after queue. `ShellItemKind` picks the surface
  (the notification area plus the overflow flyout window while visible,
  or the taskbar with the notification area's subtree excluded);
  `ShellItem` is a name plus screen rectangle, and `center_of` is the
  click target. Enumeration goes through the existing `verbatim-uia`
  client — `element_from_handle` on shell windows resolved by class, a
  control-view walk with the base cache request extended by the bounding
  rectangle — and collects named, on-screen buttons. A hung shell cannot
  hang Verbatim: a guard thread abandons the worker after a three second
  deadline (the outpost query pool's discipline, kept local and simple
  for a one-shot query) and the request just logs and presents nothing.
- `plan` — the pure, unit-tested layer: `plan_for(descriptor, value)` maps
  a `SettingDescriptor` to a `ControlPlan` (slider, choice, or check box
  with clamped initial value), `accessible_name` strips ampersand
  mnemonics for accessible names (a check box otherwise announces as a
  bare "check"), `cycle_index` implements category wraparound,
  `initial_list_selection` picks the list dialog's starting selection, and
  `DialogGuard` is the settings-dialog singleton state machine.

Implementation notes: the hidden one-by-one frame titled "Verbatim" is the
single-instance rendezvous and dialog parent. It is stamped, right after
creation, with the `verbatim_model::HIDDEN_FRAME_WINDOW_PROP` window
property (`hidden_frame::mark`, `SetPropW`) and unstamped at shutdown
(`hidden_frame::unmark`, `RemovePropW`) — the marker every outpost checks
before announcing a `FocusChanged` (decision D9; see `verbatim-outpost`'s
section), so the frame's transit through real focus during the popup dance
below is never spoken as a nameless "Verbatim" window with role unknown.
One localized menu object serves both the tray icon and the Verbatim+V
popup. Showing the menu or the dialog performs NVDA's prePopup dance — show
the frame, raise it, and force it foreground through the native window
handle — because a popup from a hidden background window never receives
foreground or keyboard focus, and no outpost would ever be watching it (D9:
outposts are per-application, spawned or re-announced by Core's foreground
trigger). `force_foreground` tries `SetForegroundWindow` directly first,
but a gesture that arrived via the control plane (no physical input, as
every E2E test and any future remote session sends) fails Windows'
foreground-lock heuristic outright; a bare `VK_CONTROL` tap injected first
satisfies the heuristic directly (confirmed live: roughly 150 to 450 ms,
versus roughly two seconds for the `AttachThreadInput` fallback the code
still keeps for the case even the tap does not help). `VK_MENU` was tried
and rejected as the nudge — a lone Alt press activates menu bars and
bounces foreground straight back. The frame hides again after the menu
closes or the dialog is dismissed. The settings dialog mirrors NVDA's
shape: a labeled single-column report list of categories on the left, a
lazily built panel on the right, OK, Cancel, and Apply buttons, hand-rolled
Enter, Ctrl+S, and Ctrl+Tab handling (wxDragon binds no accelerator
tables), and a title that tracks the active category. The Speech panel is
generated from the settings host's descriptors; every control change
applies live, OK and Apply persist, Cancel reverts. Escape and window close
follow the dialog's escape id.

The systrayList replica (M3) composes the two new modules:
`OpenShellItemList` focuses the existing dialog when one is open (the
settings singleton discipline), drops the request when an enumeration is
already in flight, and otherwise starts one; when the results arrive on
the GUI thread, presentation performs the same prePopup dance as settings
and builds the dialog through `list_dialog` — a label over the item names
with Left Click, Left Double Click, Right Click, and Cancel, where each
click action moves the pointer to the center of the selected item's
rectangle (`SetCursorPos`), injects the matching mouse events
(`SendInput`; left down and up, twice for the double click, right down
and up), and then closes the dialog. Dismissal routes through one close
path per dialog, and the deferred postPopup frame hide only happens once
neither the settings dialog nor the shell list still needs the frame as
its visible owner.
