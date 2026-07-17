# NVDA's GUI and the settings framework

NVDA's own user interface (`source/gui/`) is wxPython — chosen, like
Verbatim's wx choice, because wx wraps native Win32 controls and is
therefore accessible through the same APIs NVDA reads. The
interesting machinery is the settings framework, the driver-setting
auto-generation, and the message facilities; the GUI runs on the main
thread alongside the core pump ([Main loop and watchdog](main-loop-and-watchdog.md)).

## The settings dialog framework

`source/gui/settingsDialogs.py`:

- `MultiCategorySettingsDialog` is the shell: a category list on the
  left, one `SettingsPanel` per category on the right, panels
  constructed lazily on first visit. Each panel implements
  `makeSettings` (build controls), `onSave` (write to `config.conf`),
  and optionally `onDiscard`/`postInit`; OK applies every *visited*
  panel's `onSave`, in order. Panels register into the category list
  declaratively, and add-ons can append their own panels — the same
  mechanism ships NVDA's own two dozen
  (`SpeechSettingsPanel` through `RemoteSettingsPanel`).
- Layout goes through `source/gui/guiHelper.py` (spacing and sizer
  conventions) and `source/gui/nvdaControls.py` (custom controls with
  fixed accessibility: checkable lists, feature-flag combo boxes, a
  `SettingsPanelAccessible` wx.Accessible subclass giving panels
  proper names/roles). Worth noting for any wx GUI: NVDA still needed
  hand-written accessibility glue for its own composite controls —
  wx alone was not sufficient.
- Settings apply live where possible (changers fire on control events
  — slider drags are audible immediately) with save/discard semantics
  on top; a profile-aware warning system tells the user when they are
  editing a profile rather than the base configuration
  ([Configuration and profiles](config-and-profiles.md)).

## Driver settings: GUI from descriptors

The pattern Verbatim's `SettingsHost` mirrors:
`source/autoSettingsUtils/` defines `AutoSettings` (a driver exposes
`supportedSettings`: typed `DriverSetting` descriptors — numeric with
range, boolean, string-choice) and `settingsDialogs.AutoSettingsMixin`
builds the panel *from the descriptors*: slider for numeric, combo
for choice, checkbox for boolean, each wired through a
`DriverSettingChanger` that writes the live driver immediately.
Synthesizers ([Synth drivers](synth-drivers.md)), braille displays,
and vision providers all get their settings GUI this way — a driver
author never writes wx code.

## Messages and browseable output

- `ui.message(text)` — the universal "speak and braille this"
  one-liner used everywhere; `ui.browseableMessage(text, title)` puts
  longer content (HTML or text) in a dialog the user can read with
  browse-mode-style navigation — the presentation surface for "report
  formatting", link destination reports, and similar.
- `gui/message.py` wraps modal dialogs: `messageBox` (a wx message
  box made screen-reader-safe — tracked so NVDA knows a modal is up,
  `isModalMessageBoxActive`) and the newer structured `MessageDialog`
  with typed buttons/return codes; `blockAction.py` decorates
  commands that must refuse to run in certain states (secure mode,
  modal active) with a consistent spoken refusal
  ([Secure mode](secure-mode.md)).
- The wx *app* itself is created early and owns the main loop; GUI
  work queued from other threads goes through `wx.CallAfter`, and
  long operations use `IndeterminateProgressDialog` with beeps for
  progress.

## Other GUI surfaces

The NVDA menu (systray), the elements-list and input-gestures
dialogs ([Browse mode](browse-mode.md), [Keyboard input](input.md)),
the log viewer ([Logging](logging.md)), the Python console
([The Python console](python-console.md)), the add-on store GUI, and
the profiles dialog — each a conventional wx dialog over the
corresponding subsystem, listed here mainly so a search for "where
is X's UI" starts in the right file: `gui/__init__.py` builds the
menu and owns dialog singletons.
