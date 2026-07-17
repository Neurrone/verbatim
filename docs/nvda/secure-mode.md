# Secure screens, secure mode, and the slave process

How NVDA speaks on the login screen and UAC prompts, and how it
locks itself down there. Background on the desktop/session security
model: [Processes and security](../explainers/processes-and-security.md).

## Getting onto secure screens

A normal process cannot reach the secure desktop; the sanctioned
path is Windows' *Ease of Access* framework: NVDA registers itself
as an assistive technology (`source/easeOfAccess.py`, the
`ATs`/configuration registry keys under
`Software\Microsoft\Windows NT\CurrentVersion\Accessibility`), so
the OS itself launches an NVDA instance on the sign-in screen and
secure desktop when the user has enabled it ("Use NVDA during
sign-in", requiring admin rights to set — `setAutoStart` writes
HKLM). The instance on the secure desktop is a *separate NVDA
process* running as SYSTEM in session context, started by Windows,
always in secure mode with a copy of the user's settings that an
admin explicitly propagated ("Use currently saved settings during
sign-in and on secure screens" copies the user config to the system
profile).

Transitions are tracked: `source/winAPI/secureDesktop.py` publishes
`post_secureDesktopStateChange`; the user-session NVDA mutes/hands
off (braille displays are single-open devices —
`braille.handleSecureDesktop` releases the display so the secure
instance can grab it), and resumes when the secure desktop goes
away.

## Secure mode

Secure mode (`globalVars.appArgs.secure`) is a hardening flag, on
whenever NVDA runs on a secure screen, forceable via the `--secure`
CLI switch or the `forceSecureMode` system-wide registry parameter
(`source/NVDAState.py`; `serviceDebug` conversely disables secure
mode on secure screens for debugging — documented in the user guide
system-wide parameters section). What it disables
(`source/utils/security.py`, `source/gui/blockAction.py` and checks
sprinkled at each feature): the Python console, add-on
installation and the add-on store, profile/config *saving*, update
checking and browsing to URLs, the COM registration fixing tool,
speech/braille viewers writing files — in general, anything that
executes arbitrary code, writes to disk, or opens escape hatches
from the secure context. The crash-recovery exception filter also
stays off in secure mode (`watchdog.initialize`;
[Main loop and watchdog](main-loop-and-watchdog.md)).

Additionally, while the *lock screen* is active in the user session
(a related but distinct state — `winAPI/sessionTracking.py`), NVDA
suppresses interaction with content below the lock: objects are
wrapped with `LockScreenObject` ([Object model](object-model.md)), navigation
refuses to leave lock-screen windows, and only "safe scripts" run
([Keyboard input](input.md)).

## The slave process

`nvda_slave.pyw` (`source/nvda_slave.pyw`) is NVDA's helper
executable for actions needing a *different* process context:
launched elevated for admin-only operations (installing NVDA,
setting the ease-of-access registration, config-in-system-profile
copying), and used to relaunch NVDA across UAC boundaries. It
parses a small verb list and calls into the same codebase; the
pattern exists because NVDA itself runs unelevated (uiAccess but
Medium IL) and must delegate anything requiring administrator
rights.
