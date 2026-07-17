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

## Elevated applications on the user desktop

Distinct from secure screens: an administrator command prompt, an
elevated installer, or Task Manager runs elevated *on the normal
desktop*, and it is the ordinary user-session NVDA that must read
it — across the integrity-level wall
([Processes and security](../explainers/processes-and-security.md)).
Everything hinges on the uiAccess flag, which only an *installed*
NVDA has (signed binaries in Program Files):

- **With uiAccess** (installed copies): winevent hooks see elevated
  processes, messages and input may be sent to them, UIA serves
  their content, and the in-context hooks reach them too, so the
  injected features (typed-character echo, IME reporting;
  [Process injection](process-injection.md)) keep working in
  elevated apps. An elevated console reads like any other console —
  the UIA console path needs no injection at all
  ([Editable text and terminals](editable-text-and-terminals.md)).
- **Without uiAccess** (portable copies, source runs): UIPI blocks
  the upward path; elevated windows are effectively unreadable and
  cannot be interacted with. This is the documented limitation of
  portable NVDA, the first diagnosis for "NVDA goes quiet in the
  admin prompt," and why NVDA warns when run portable
  ([Installation, portable copies, updates, and COM fixes](installation-and-updates.md)).

So the full privilege map has three tiers: normal apps (everything
works), elevated apps on the user desktop (everything works *if*
uiAccess, else nothing), and secure screens (a separate SYSTEM
instance in secure mode, previous sections). UAC prompts are the
third tier, not the second — the consent dialog lives on the secure
desktop, which is why reading it requires the ease-of-access
integration rather than merely uiAccess.

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
