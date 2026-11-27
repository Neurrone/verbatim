# System integration

Starting and stopping, the settings dialog, other Windows integration, and features not built yet.

### Starting

NVDA plays its start sound and speaks no start message; "Loading NVDA.
Please wait..." is spoken only when start-up has taken more than five
seconds ([Installation, portable copies, updates, and COM
fixes](../nvda/installation-and-updates.md), "Startup and shutdown").
Verbatim: **partial**. The start sound, then the foreground's
announcement, its first speech, as NVDA. The slow-start message is not
yet built.

### Windows' screen reader flag

Windows' screen reader flag (`SPI_SETSCREENREADER`). NVDA sets it as it
starts and clears it as it exits, unless started with `--no-sr-flag` or
on a secure desktop ([Installation, portable copies, updates, and COM
fixes](../nvda/installation-and-updates.md), "Startup and shutdown").
Verbatim: **partial**. Set as it starts, after replacing a running
instance, and cleared as it exits, before the startup mutex is
released, on a normal exit, a failed start, a panic on the main thread,
the console's Control+C, Control+Break or close, and the session ending
(`verbatim-app`'s `screen_reader_flag`); a crash or a process ended from
outside leaves it set, as it leaves NVDA's. Not yet built: an option to
leave the flag alone, and a secure-desktop instance (M8.7). Programs
change their behaviour by it: Windows PowerShell, started while either
screen reader runs, leaves `PSReadLine` out and prints "PowerShell
detected that you might be using a screen reader and has disabled
PSReadLine for compatibility purposes", which both then speak as output
in both terminals; `PSReadLine`'s keys, MenuComplete (Control+Space)
among them, then need `Import-Module PSReadLine`, as the notice says,
and behave as before once it is imported.

### Replacing a running instance

NVDA posts the running NVDA `WM_QUIT`, waits four seconds for it to
exit, ends it with `TerminateProcess` if it has not, and waits two
seconds more; when ending it fails, it shows a message box and does not
start ([Installation, portable copies, updates, and COM
fixes](../nvda/installation-and-updates.md), "Startup and shutdown").
Verbatim: **matched (unverified)**, but for the first wait, five seconds
where NVDA's is four (Dickson, 2026-10-10). An ended Verbatim's
outposts, focus listener and synthesizer hosts end with it, and the new
one sets the screen reader flag again, as NVDA does. When ending the old
process fails, or it does not exit once ended, Verbatim shows a message
box, "Couldn't end the running Verbatim, abandoning start.", without
the exception, which is logged, and does not start; neither shows it
when the old one was ended. Different (Dickson, 2026-10-10): NVDA being
replaced plays its exit sound to the end, and Verbatim being replaced
plays none and stops at once, which saves about a second (the exit
sound took 1.08 of the 1.4 seconds a replacement took). Every other way
Verbatim quits still plays it.

### Exiting

NVDA, once its main loop ends, handles the events already queued, then
cancels speech, stops its handlers, and plays its exit sound to the end,
saying nothing after it (`nvda/source/core.py`, the end of `main`).
Verbatim: **matched (unverified)** (Dickson, 2026-11-03). Once its GUI
loop ends, the keyboard hook is removed and Core is told it is exiting
(`Input::Exiting`) behind whatever was already queued: a focus read
before then may be spoken, and is cut off, since Core then cancels speech
and plays the exit sound, and reduces no later input, so nothing is said
after the sound. `verbatim_lifecycle` exits through the menu and asserts
that the exit sound is the last thing played, not whether the desktop is
spoken before it.

### Vision framework (focus highlight, screen curtain, magnifier), OCR, secure screens, remote access

NVDA: [The vision framework](../nvda/vision.md),
[OCR and content recognition](../nvda/ocr-and-content-recognition.md),
[Secure mode](../nvda/secure-mode.md), [Remote access](../nvda/remote-access.md).
Verbatim: **not yet (M8.4 and M8.5)** for the focus highlight, screen curtain and
OCR, and not yet for the secure desktop (M8.7); remote access is
M12's.

### Desktop switches

NVDA takes `EVENT_SYSTEM_DESKTOPSWITCH` through its MSAA hook, accepts it
from any window (`docs/parity/focus-events.md`, "Event acceptance", rule
7) and handles it in the MSAA pump, in the flush's order, after the run
of focus events before it has been processed
(`IAccessibleHandler/__init__.py` 1069 to 1077); the MSAA pump runs
before the event queue in each core cycle (`core.py` 1026 to 1027), so
the switch is handled ahead of any event still queued.
`processDesktopSwitchWinEvent` (796 to 806) calls `OpenInputDesktop`:

- When it opens, the input desktop is the user's again: NVDA closes the
  handle and 200 ms later (`core.callLater(200, _handleUserDesktop)`)
  queues a focus for the desktop object's object with the focus
  (`objectWithFocus`), straight onto the event queue with no duplicate
  or focused-state test (809 to 813), and notifies that the secure
  desktop has ended.
- When it does not open, the input desktop is another, usually the
  secure desktop (`_handleSecureDesktopChange`,
  `winAPI/secureDesktop.py` 35 to 70): NVDA clears its record of the
  modifiers held, since no key release will come for them (45); cancels
  speech (50); speaks "Secure Desktop" at the highest priority (`NOW`,
  53); and commits, at once and without queueing, a focus on an object
  of its own tied to no window, whose sleep mode is full (55 to 70), so
  every key passes to Windows and no event is spoken. The next focus
  NVDA handles, normally the one queued 200 ms after the return,
  replaces it (`doPreGainFocus`, `eventHandler.py` 378 to 387).

Verbatim: **partial** (Dickson, 2026-11-12; [ADR 0003: Readers tell,
Core asks](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
"The secure desktop and the lock state"). The focus listener tells each
desktop switch with whether the input desktop opens, tested as NVDA tests
it, with `OpenInputDesktop`, and Core handles the switch at once in its
inbox, ahead of the event queue (`crates/verbatim-core/src/input_desktop.rs`):

- A switch to a desktop that does not open cuts speech and commits the
  sleeping focus, a focus Core commits itself with no reader and no
  ancestors, which follows nothing; the keyboard hook clears its record
  of the modifiers held, the held keys Core has not passed on are
  dropped, and the old application is asked nothing. Entries already
  queued run as NVDA's do, and a focus among them replaces the sleeping
  focus. "Secure Desktop" is not yet spoken (M8.7), so a switch away says
  nothing beyond the cut.
- A switch to a desktop that opens arms a 200 ms timer; when it comes
  due, an entry joins the back of the event queue and asks the front
  application for its focus, with no duplicate test and none of the
  ancestors Core already knows, so the window's title and the focus are
  said, as NVDA says them ("Window announcement on switching
  applications" in [Focus context](focus-context.md)). A further switch
  before then re-arms or cancels the timer.

Core's `input_desktop_tests.rs`; nothing on the secure desktop can be
tested end to end until M8.7.

### The lock screen

While Windows is locked, the user's desktop still runs beneath the lock
screen, and NVDA keeps what is below it from being read. Lock-screen
mode (`isLockScreenModeActive`, `winAPI/sessionTracking.py` 151 to 164)
is on when NVDA is not running on the secure desktop and its session
tracker says the session is locked; before NVDA's initialization has
finished it counts as unlocked (136 to 148), and the session query it
uses reads a failure or an unknown state as unlocked (167 to 182).

An object is below the lock screen (`_isObjectBelowLockScreen`,
`utils/security.py` 211 to 254) by these rules, in order: never when
NVDA has no UIAccess, as it cannot read below the lock screen then
(227 to 230); never when its process is the foreground window's process
(232 to 235); never when it is an Alt+Tab switcher item (`TaskListIcon`,
237 to 243), since the switcher never becomes the foreground process
on the lock screen; never when it is not a window object, which has no
window to test (247 to 252); otherwise by z-order, from its window's
root (`GA_ROOT`, 254). The z-order test (258 to 289, with
`_isWindowBelowWindowMatchesCond` 299 onward) walks the top-level
windows from the bottom up to the first lock-screen window, one whose
class is `LockScreenInputOcclusionFrame`,
`LockScreenControllerProxyWindow` or `LockScreenBackstopFrame`
(274 to 280): the object is below the lock screen when its root was
passed on the way, once. When no lock-screen window is found, or the
root is not found exactly once below it, the test fails open and the
object counts as not below (282 to 289). Any error in the whole check
also counts as not below (`objectBelowLockScreenAndWindowsIsLocked`,
155 to 199).

While lock-screen mode is on, an object below the lock screen is never
recorded as the last focus queued (`eventHandler.py` 179 to 183), its
events are not run (322 to 326), and it is never committed as the focus
(379 to 383; `api.setFocusObject`, `api.py` 91); nor is it made the
foreground object (71), the mouse object (216), the review position's
object (271), or the navigator object (311, 330). Speech for it is empty
(`speech/speech.py` 702 and 870), and say-all stops when it reaches one
(`speech/sayAll.py` 307 to 313). Only the safe scripts run while
lock-screen mode is on (`scriptHandler.findScript`, `scriptHandler.py`
101 to 108; the list in `getSafeScripts`, `utils/security.py` 71 to 152).

Verbatim: **not yet (M8.7)**. Core holds the session's lock state, from
Windows' session notifications and a query at startup
(`crates/verbatim-app/src/session_lock.rs`), and counts an unknown state
as locked, where NVDA counts it unlocked (Verbatim-only: security
mitigation B3). No object is judged below the lock screen while
Verbatim lacks UIAccess, as NVDA judges none without it
(`utils/security.py` 227 to 230);
the test and the rules that drop objects below the lock screen come with
UIAccess in M8.7 (Dickson, 2026-11-15; [ADR 0003: Readers tell, Core
asks](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
"The secure desktop and the lock state"). Nor are commands limited to a
safe list while the session is locked: every command runs, until M8.7.

### Elevated applications on the user desktop

Elevated applications on the user desktop (admin consoles,
elevated installers): reading them requires uiAccess — signed
binaries in a trusted install location. NVDA:
[Secure mode](../nvda/secure-mode.md). Verbatim: **not yet
(M8.7)**: unsigned dev builds cannot reach elevated windows at all;
M8.7 brings UIAccess with test-signing in the VM, and production
signing and packaging have no milestone. Recognize the symptom now: "Verbatim
goes quiet in the admin prompt" is this, not a bug.

### Braille

NVDA: [Braille](../nvda/braille.md). Verbatim: **not yet (M18)**,
designed for under D7; the inputs braille needs preserved are listed at
the end of [Braille](../nvda/braille.md).

### Configuration profiles and triggers

Configuration profiles and triggers (app, say-all, manual;
queue-synchronized application to speech). NVDA:
[Configuration and profiles](../nvda/config-and-profiles.md).
Verbatim: **not yet (M8.3)** — `verbatim-config` holds `settings.toml`
alone; the layering and trigger semantics are undesigned.

### A settings file that fails to load

NVDA: a base configuration file that fails to load is renamed with
`.corrupted.bak` after its name, replacing an older backup, the error is
logged, the configuration is reset to factory defaults, and once the GUI
is up a message box titled "Configuration File Error" says "Your
configuration file contains errors. Your configuration has been reset
to factory defaults. More details about the errors can be found in the
log file." (`nvda/source/config/__init__.py` 536-562, `nvda/source/core.py`
156-169). NVDA shows its start-up box before it takes any input: two
captures on 2026-11-20 recorded no response to Insert+T, Insert+N,
Insert+Control+G or Insert+Q while it was open.

Verbatim: **matched**, with the differences below. `settings.toml` is
kept as `settings.toml.corrupted.bak` and the defaults are used, the
error is logged as a warning, and the GUI shows the same box in NVDA's
words, spoken "Configuration File Error dialog", the text, then "OK
button", as NVDA speaks it
(`crates/verbatim-i18n/tests/captures/configuration-file-error.txt`, a
capture at NVDA alpha-58022). A rename that fails leaves the file, which
the first saved change overwrites, as NVDA's does.

Differences:

- Verbatim takes input while the box is open, so it follows the rule
  NVDA's source gives any modal message box, decided from that source
  because NVDA's own start-up box cannot be captured taking input
  (Dickson, 2026-11-20). While the box is up, the menu's Settings and
  Exit are refused: every box up is raised and the newest focused, then
  Verbatim says "Action unavailable while a dialog requires a response",
  as NVDA's refusal raises its blocking boxes, focuses the last shown and
  then speaks (`nvda/source/gui/blockAction.py` 25-31 and 100-104;
  `nvda/source/gui/message.py` 771 and 819-827); the menu itself opens, as NVDA's `showGui` is not
  blocked (`nvda/source/gui/blockAction.py` 55-59;
  `nvda/source/gui/__init__.py` 202, and 214, 223, 264 and 330, the
  resets to defaults, the settings dialogs and Exit, each blocked with
  `Context.MODAL_DIALOG_OPEN`). Verbatim offers no reset to defaults and
  no Verbatim+Q, so there is nothing more to block. NVDA speaks the
  message at its priority NOW, which interrupts and resumes; Verbatim
  speaks it at Next, after the current utterance, until M6.3 brings that
  priority. The block is pinned by the GUI lifecycle's tests
  (`crates/verbatim-gui/src/lifecycle.rs`), and the focus before the
  message by the GUI's `exit_refused_under_a_message_box_focuses_the_box_before_saying_why`
  (`crates/verbatim-gui/src/lib.rs`).
- One bad value fails Verbatim's whole file, where NVDA checks a value
  when it is read; keeping the other values is M8.3's.

### Settings GUI generated from driver descriptors

NVDA:
`AutoSettingsMixin` ([NVDA's GUI and the settings framework](../nvda/gui-and-settings.md)).
Verbatim: **matched (unverified)** in pattern — `SettingsHost`
feeds the GUI from `SettingDescriptor`s; NVDA's panel framework
and accessibility glue are the reference as the GUI grows.

### Enter in the settings dialog

NVDA: Enter anywhere activates OK, even on Cancel or Apply; Control+S
applies ([NVDA's GUI and the settings
framework](../nvda/gui-and-settings.md), "The settings dialog
framework"). Verbatim: **different** by decision (Dickson, 2026-10-06):
Enter on a focused button activates that button, so Enter on Cancel
cancels and Enter on Apply applies and keeps the dialog open; Enter on
the synthesizer's name opens Change; Enter elsewhere is OK
(`verbatim-gui`'s `keys` module). Control+S, Control+Tab, and
Control+Shift+Tab work from every control, the Speech page's included,
through one key hook on the dialog, as NVDA's do
(`settings_dialog_keys` verifies Cancel, Apply, Control+S, and
Control+Tab).

### The terminal settings

NVDA: no page of their own; "Speak passwords in
all enhanced terminals" and the choice between diffing and Windows
Terminal's notifications are in the Advanced panel, behind its warning,
and NVDA's "Report dynamic content changes", which Verbatim+5 matches,
is in Object Presentation. Verbatim: **different** by decision
(Dickson, 2026-10-06): a Terminal page in the settings dialog, after Theme, with
"Report new output", the flood policy's two limits ("Lines spoken in
full" and "Last lines to speak", 1 to 10,000, sliders, more than either
terminal's history holds, Core's 10 MB bound on waiting output
protecting memory (Dickson, 2026-10-09)), which NVDA has no equivalent
of, and "Speak passwords typed in terminals". Its changes wait for OK or
Apply, which write all four values, as NVDA's panels write every
control on save, where the Speech and Theme pages apply live
(`terminal_settings_page` verifies that an applied change reaches
Core).

### The system tray and taskbar list

The system tray and taskbar list (Verbatim+F11, the systrayList
add-on's dialog). NVDA's add-on clicks the center of the rectangle it
recorded for the chosen icon when it made the list, so an icon that
moved since, as tray icons do when another is added or removed, leaves
a different icon, or none, under the click. Verbatim: **different**
(`books/dev/src/reference/crates/verbatim-gui.md`, `refind`; the module documentation of
`crates/verbatim-gui/src/tray_list.rs`): the list records no
rectangles; a click has the outpost walk the tray or taskbar again and
read the rectangle of each button with the chosen item's UIA runtime id or
name, and Core takes the one with its runtime id, or else the only one
with its name when no other item of the list had that name and the walk
read every button, and clicks its center there, as the add-on clicks the
center. An item that cannot be found this way, or is found off the
screen since the list was made, is not clicked, and Verbatim says "is no
longer there" after its name (Dickson, 2026-10-26). When the request for
where it is fails (the outpost timed out, ended or dropped it), nothing is
clicked and Verbatim says "Could not click Clock, try again", with the
item's name (Dickson, 2026-11-28); the add-on, which asks nothing at the
click, has no such case. The list leaves out a
button off the screen, as the add-on does; one with no rectangle or an
empty one, which the add-on also leaves out, is listed, since the list
reads no rectangles, and is not clicked, and Verbatim says "cannot be
clicked" after its name (Dickson, 2026-10-25) (`verbatim-gui`'s
`tray_list` and `shell_items`). The items are read by the outpost of File Explorer's
taskbar process within its own application, where the add-on reads them
from NVDA's process (`verbatim-outpost`'s `shell_items`). The dialog
waits up to 11.5 seconds for a slow taskbar and shows its whole list once
it answers; a taskbar that does not finish in time gives no list
(Dickson, 2026-11-06).

### Logging and the log viewer

NVDA: [Logging](../nvda/logging.md).
Verbatim: **not yet (M9)** — tracing exists (flight recorder,
latency ledger); user-facing logging is unbuilt.

### Developer console

NVDA: [The Python console](../nvda/python-console.md).
Verbatim: **not yet (M10)** — `verbatim-inspect` covers part of
the inspection role today.

### Installation, portable copies, self-update, COM registration repair

NVDA: [Installation, portable copies, updates, and COM fixes](../nvda/installation-and-updates.md).
Verbatim: **not yet (M8.6)** for an installer and updater skeleton;
portable copies and COM registration repair have no milestone yet.
