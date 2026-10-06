# verbatim-gui

The GUI (decision D4, architecture section 11): the hidden main frame, the
tray icon and Verbatim menu, the NVDA-style settings dialog, and the
systrayList replica. The widgets are wxWidgets, built by a small C++ layer
(`cpp/gui.cpp`) that the crate's build script compiles against static
wxWidgets; Rust keeps `main`, the event loop's lifetime, and every
decision.

Public API:

- `GuiCommand` (`ShowMenu`, `OpenSettings`,
  `OpenShellItemList(ShellItemKind)`, `Shutdown`) and `GuiEvent`
  (`QuitRequested`) — the two seams to the application. The GUI never exits
  the process; Exit raises `QuitRequested` and the app decides.
- `GuiHandle::send(command)` — cloneable, callable from any thread;
  sends the command down the GUI's own channel and wakes the event loop,
  which drains the channel on the GUI thread.
- `run_gui(settings_host, events, on_ready)` — runs the event loop on the
  calling thread (the app calls it from the process main thread); once the
  frame and tray exist, `on_ready` hands out the `GuiHandle`. It returns
  when the loop ends: after `Shutdown`, or when a replacing instance posts
  `WM_QUIT` to the hidden frame's thread. It runs once per process; a
  second call fails, since wxWidgets cannot start again after it ends.
- `shell_items` — system tray and taskbar item enumeration for the
  systrayList replica: `request_shell_items(kind, deliver)` enumerates on
  a short-lived worker thread and hands `deliver` the outcome on its guard
  thread; the GUI sends it down its channel to the GUI thread.
  `ShellItemKind` picks the surface (the notification area plus the
  overflow flyout window while visible, or the taskbar with the
  notification area's subtree excluded); `ShellItem` is a name plus screen
  rectangle, and `center_of` is the click target. Enumeration goes through
  the existing `verbatim-uia` client — `element_from_handle` on shell
  windows resolved by class, a control-view walk with the base cache
  request extended by the bounding rectangle — and collects named,
  on-screen buttons. A hung shell cannot hang Verbatim: a guard thread
  abandons the worker after a three second deadline (the outpost
  watchdog's discipline, kept local and simple for a one-shot query) and
  the request just logs and presents nothing.
- `ControlPlan` — re-exported from the pure `plan` module described below.

The private modules:

- `bridge` — the cxx bridge. Rust calls C++ to run the event loop
  (`run_event_loop`), wake it from any thread (`wake_event_loop`, a
  `CallAfter` guarded by a mutex and an application pointer that is
  cleared when the loop ends), centre, show, and hide the frame, pop the
  menu, open, raise, and focus a dialog, read a window's native handle,
  and shut down. C++ calls methods of the opaque `GuiCore` when the user
  acts: a menu choice, a tray click, a setting change, commit, revert, the
  Select Synthesizer choice, a list dialog button, a dialog closing; and
  two pure functions, the key router and category cycling. Pages cross as
  shared structs Rust fills and C++ only reads: `SettingsDialog` and its
  `Category` list, `SpeechPage` and its `SettingControl`s,
  `SynthesizerPicker`, and `ListDialog`. Every string crosses resolved, so
  C++ never sees a Fluent message id; text is UTF-8 and C++ converts it
  explicitly, keeping its own narrow literals ASCII.
- `GuiCore` (in `lib.rs`) — the GUI's Rust half. It lives on `run_gui`'s
  stack for the whole loop and is used only on the GUI thread. Menus and
  modal dialogs run nested event loops that call back into it, so every
  method takes `&self`, its state sits in `RefCell`s, and no borrow is
  ever held across a call into C++.
- `lifecycle` — the menu's and dialogs' lifecycle as one tested state
  machine: the menu is popped once at a time; the settings dialog and the
  shell item list are singletons, a second request focusing the open one;
  a list request during an enumeration is dropped; requests and results
  after shutdown are ignored; and the hidden frame is hidden after a popup
  only once neither the menu nor an open dialog needs it as its visible
  owner.
- `settings` — the settings dialog's model, pure apart from reading the
  host: the categories (Speech only, so far), the Speech page generated
  from the host's descriptors through `plan`, `SpeechControls`, which turns
  a change reported by control index into a setting value (changes from a
  replaced set of controls, identified by a generation number, are
  ignored), and the Select Synthesizer choice, which switches only to a
  different synthesizer and only then asks for the controls to be
  rebuilt.
- `keys` — what a key does in the settings dialog (`route_key`, from the
  key and the focused control, with tests).
- `list_dialog` — the reusable list dialog component (M3): a title, a
  static label above a single-selection list box around 550 by 250, and a
  configurable row of buttons plus an automatic Cancel. Callers describe
  the dialog as data: `ListDialogSpec` carries items as display strings
  whose index is the opaque payload key, and each `ListDialogButton` is a
  resolved label plus a callback returning a `ButtonVerdict` (close or
  keep open). `split` turns the spec into the model C++ builds and the
  callbacks Rust keeps. Escape and window close dismiss; Enter or a double
  click on a list item triggers the default button; activation only ever
  runs against a selected item. The systrayList replica presents through
  it and M6's elements list will present through the same one.
- `plan` — `plan_for(descriptor, value)` maps a `SettingDescriptor` to a
  `ControlPlan` (slider, choice, or check box with clamped initial value),
  `accessible_name` strips ampersand mnemonics for accessible names (a
  check box otherwise announces as a bare "check"), `cycle_index`
  implements category wraparound, and `initial_list_selection` picks the
  list dialog's starting selection.

The build: `build/wx.rs` builds wxWidgets 3.3.3's base and core libraries
statically, adapted from wxdragon-sys's recipe: the pinned archive is
downloaded with the `curl.exe` that ships with Windows and its SHA-256
checked, then CMake builds the `wxcore` target against the release C
runtime in `RelWithDebInfo`, with debug information embedded in the
libraries; x64 uses Ninja, ARM64 the Visual Studio generator (with the
ARM64-hosted toolset on an ARM64 machine). The result goes to a fixed
directory, `<root>/3.3.3-<architecture>`, where `<root>` is
`VERBATIM_WX_DIR` or else `wxwidgets` in the cargo target directory; only
the headers, the libraries, and a stamp naming the recipe are kept, and a
lock file serializes concurrent builds. CI caches the directory, keyed on
`build/wx.rs`. `build.rs` then compiles the bridge and `cpp/gui.cpp` with
`cxx_build` and links the libraries.

Implementation notes: the hidden one-by-one frame titled "Verbatim" is the
single-instance rendezvous and dialog parent. It is stamped, once the GUI
is up, with the `verbatim_model::HIDDEN_FRAME_WINDOW_PROP` window property
(`hidden_frame::mark`, `SetPropW`) and unstamped at shutdown
(`hidden_frame::unmark`, `RemovePropW`) — the marker every outpost checks
before announcing a `FocusChanged` (decision D9; see `verbatim-outpost`'s
section), so the frame's transit through real focus during the popup dance
below is never spoken as a nameless "Verbatim" window with role unknown.
Closing the frame hides it; only shutdown destroys it. The Verbatim+V
popup and the tray icon's right-click menu have the same two items.
Showing the menu or a dialog performs NVDA's prePopup dance — show the
frame, raise it, and force it foreground through the native window handle
— because a popup from a hidden background window never receives
foreground or keyboard focus, and no outpost would ever be watching it
(D9: outposts are per-application, spawned or re-announced by Core's
foreground trigger). `force_foreground` tries `SetForegroundWindow`
directly first, but a gesture that arrived via the control plane (no
physical input, as every E2E test and any future remote session sends)
fails Windows' foreground-lock heuristic outright; a bare `VK_CONTROL` tap
injected first satisfies the heuristic directly (confirmed live: roughly
150 to 450 ms, versus roughly two seconds for the `AttachThreadInput`
fallback the code still keeps for the case even the tap does not help).
`VK_MENU` was tried and rejected as the nudge — a lone Alt press activates
menu bars and bounces foreground straight back. The frame hides again
after the menu closes or the last dialog is dismissed.

The settings dialog mirrors NVDA's shape: a labeled single-column report
list of categories on the left, a lazily built page on the right, OK,
Cancel, and Apply buttons, and a title that tracks the active category.
The Speech page is generated from the settings host's descriptors; every
control change applies live, OK and Apply persist, and Cancel reverts.
Escape and the close box arrive as a click on Cancel. One `wxEVT_CHAR_HOOK`
on the dialog sees every key from every child, the Speech page's controls
included, and asks `keys` what to do: Enter on a button activates that
button, so Enter on Cancel cancels and Enter on Apply applies; Enter on
the synthesizer's name opens Change; Enter elsewhere is OK; Control+Tab and
Control+Shift+Tab change category and Control+S applies, from any control.
Other keys go on to the focused control. The Change button runs the modal
Select Synthesizer dialog; when the active synthesizer changed, the
generated controls are destroyed and rebuilt from a fresh page. The
dialog has no check list box; one added later needs its own accessible,
and must not notify on toggle itself, since wxWidgets 3.3.2 and later
already do.

The systrayList replica (M3): `OpenShellItemList` focuses the existing
dialog when one is open, drops the request when an enumeration is already
in flight, and otherwise starts one; when the results arrive on the GUI
thread, presentation performs the same prePopup dance as settings and
builds the dialog through `list_dialog` — a label over the item names with
Left Click, Left Double Click, Right Click, and Cancel, where each click
action moves the pointer to the center of the selected item's rectangle
(`SetCursorPos`), injects the matching mouse events (`SendInput`; left
down and up, twice for the double click, right down and up), and then
closes the dialog. Each dialog closes through one path in C++, which
destroys it and tells `GuiCore`, and the deferred postPopup frame hide
only happens once nothing still needs the frame as its visible owner.
