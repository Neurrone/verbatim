# System integration

Starting and stopping, the settings dialog, other Windows integration, and features not built yet.

### Starting

NVDA plays its start sound and speaks no start message: "NVDA
started" goes to braille only, and "Loading NVDA. Please wait..." is
spoken only when start-up has taken more than five seconds; then it
queues the initial focus (`core.py`). Verbatim: **matched since
2026-10-08**: the start sound, then the foreground's announcement, its
first speech. It used to speak "Verbatim is starting.", which the
foreground's announcement cut off or not depending on how soon the
foreground's outpost answered, a race. The slow-start message is **not
yet**.

### Windows' screen reader flag

Windows' screen reader flag (`SPI_SETSCREENREADER`). NVDA sets it as it
starts and clears it as it exits, saving it in the user's profile and
telling every window (`nvda.pyw` lines 285 and 286, and 308 and 309;
`winUser.setSystemScreenReaderFlag`, `winUser.py` lines 460 and 461),
unless started with `--no-sr-flag` (`argsParsing.py` lines 179 to 185)
or on a secure desktop (`nvda.pyw` line 267). Verbatim: **matched since
2026-10-09**; it has no such option yet, nor a secure-desktop instance
(M8): set as it starts, after
replacing a running instance, and cleared as it exits, before the
startup mutex is released, on a normal exit, a failed start, a panic
on the main thread, the console's Control+C, Control+Break or close,
and the session ending (`verbatim-app`'s `screen_reader_flag`); a crash
or a process ended from outside leaves it set, as it leaves NVDA's.
Programs change their behaviour by it: Windows PowerShell, started
while either runs, leaves `PSReadLine` out and prints "PowerShell
detected that you might be using a screen reader and has disabled
PSReadLine for compatibility purposes", which both then speak as output
(captured live with NVDA, and tried with Verbatim, on 2026-10-09 in
both terminals); `PSReadLine`'s keys, MenuComplete (Control+Space)
among them, then need `Import-Module PSReadLine`, as the notice says,
and behave as before once it is imported.

### Replacing a running instance

NVDA finds the running NVDA's window,
posts it `WM_QUIT`, waits four seconds for its process to exit, and
ends it with `TerminateProcess` if it has not (`nvda.pyw` lines 110 to
137), waiting two seconds more for it to exit. Verbatim: **matched
since 2026-10-10**, but for the first wait, five seconds where NVDA's
is four (Dickson, 2026-10-10). An ended Verbatim's outposts, focus
listener and synthesizer hosts end with it, and the new one sets the
screen reader flag again, as NVDA does. When ending the old process
fails, both show a message box and do not start (Verbatim also when it
does not exit once ended): NVDA's says "Couldn't terminate existing NVDA process, abandoning
start" followed by the exception (`nvda.pyw` lines 157 to 166), and
Verbatim's says "Couldn't end the running Verbatim, abandoning start.",
without the exception, which is logged; neither shows it when the old
one was ended. A deliberate difference (Dickson, 2026-10-10): NVDA
being replaced plays its exit sound to the end, and Verbatim being
replaced plays none and stops at once, which saves about a second (the
exit sound took 1.08 of the 1.4 seconds a replacement took, measured on
2026-10-10). Every other way Verbatim quits still plays it.

### Vision framework (focus highlight, screen curtain, magnifier), OCR, secure screens, remote access

all **not yet** (M8 for OCR
and secure desktop, M12 remote); references [The vision framework](../nvda/vision.md),
[OCR and content recognition](../nvda/ocr-and-content-recognition.md),
[Secure mode](../nvda/secure-mode.md), [Remote access](../nvda/remote-access.md).

### Elevated applications on the user desktop

Elevated applications on the user desktop (admin consoles,
elevated installers): reading them requires uiAccess — signed
binaries in a trusted install location. NVDA:
[Secure mode](../nvda/secure-mode.md). Verbatim: **not yet** —
unsigned dev builds cannot reach elevated windows at all; the
signing/packaging work has no milestone, and M8's secure-desktop
work depends on it. Recognize the symptom now: "Verbatim goes
quiet in the admin prompt" is this, not a bug.

### Braille

**not yet (M15, D7)**; the inputs braille needs preserved
are listed at the end of [Braille](../nvda/braille.md).

### Configuration profiles and triggers

Configuration profiles and triggers (app, say-all, manual;
queue-synchronized application to speech). NVDA:
[Configuration and profiles](../nvda/config-and-profiles.md).
Verbatim: **not yet (M8)** — `verbatim-config` covers the base
store only; the layering and trigger semantics are undesigned.

### Settings GUI generated from driver descriptors

NVDA:
`AutoSettingsMixin` ([NVDA's GUI and the settings framework](../nvda/gui-and-settings.md)).
Verbatim: **matched (unverified)** in pattern — `SettingsHost`
feeds the GUI from `SettingDescriptor`s; NVDA's panel framework
and accessibility glue are the reference as the GUI grows.

### Enter in the settings dialog

NVDA: Enter anywhere activates OK,
even on Cancel or Apply, since the dialog's key hook sees it first
(`gui/settingsDialogs.py`, `_enterActivatesOk_ctrlSActivatesApply`);
Control+S applies. Verbatim: **different** by decision (Dickson,
2026-10-06): Enter on a focused button activates that button, so Enter
on Cancel cancels and Enter on Apply applies and keeps the dialog
open; Enter on the synthesizer's name opens Change; Enter elsewhere is
OK (`verbatim-gui`'s `keys` module). Control+S, Control+Tab, and
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
(`docs/design/phase6-design.md`, "M4: text, editing, and terminals", Questions): a
Terminal page in the settings dialog, after Theme, with "Report new
output", the flood policy's two limits ("Lines spoken in full" and
"Last lines to speak", 1 to 10,000, sliders, more than either
terminal's history holds, Core's 10 MB bound on waiting output
protecting memory, per Dickson's coherence decision 3 of 2026-10-09), which NVDA has no equivalent of, and "Speak passwords typed in terminals". Its changes wait for OK
or Apply, as NVDA's do, where the Speech and Theme pages apply live
(`terminal_settings_page` verifies that an applied change reaches
Core).

### The system tray and taskbar list

The system tray and taskbar list (Verbatim+F11, the systrayList
add-on's dialog). NVDA's add-on clicks the center of the rectangle it
recorded for the chosen icon when it made the list, so an icon that
moved since, as tray icons do when another is added or removed, leaves
a different icon, or none, under the click. Verbatim: **different**,
deliberately, since 2026-10-07: a click enumerates the tray or taskbar
again, finds the chosen item by its UIA runtime id, or else by a name
no other item has, and clicks it where it is now; an item that cannot
be found is not clicked, and Verbatim says "is no longer there" after
its name (`verbatim-gui`'s `tray_list` and `shell_items`).

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
Verbatim: **not yet** (no packaging milestone yet).
