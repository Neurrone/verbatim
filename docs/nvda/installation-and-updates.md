# Installation, portable copies, updates, and COM fixes

The deployment machinery: how NVDA installs, runs portably, updates
itself, and repairs the system's accessibility plumbing. Reference
for Verbatim's eventual packaging work (no milestone yet).

## One binary, three modes

The same distribution runs as: the **launcher** (the downloaded exe —
a temporary self-extracting copy that runs NVDA immediately, from
which the user installs or creates a portable copy), an **installed
copy** (Program Files, uninstaller registered, ease-of-access
integration, uiAccess-signed binaries — the only mode with full
secure-screen support; [Secure mode](secure-mode.md)), or a
**portable copy** (any directory; config defaults to a `userConfig`
subdirectory there; no uiAccess, so elevated apps and secure screens
are out of reach — the degraded mode users are warned about).

`source/installer.py` implements install as *copy plus registration*:
`copyProgramFiles` with in-use files renamed and scheduled for
delete-on-reboot (`removeOldLibFiles` — the injected DLLs may be
loaded in other processes at upgrade time; [Process injection](process-injection.md)),
shortcut and uninstall-registry creation, previous-version comparison
(`_comparePreviousInstall`) to decide upgrade behavior, and config
migration. Elevation for all of it goes through the slave process
([Secure mode](secure-mode.md)). Install-time options: copy portable
config in, start at logon.

## Startup and shutdown

`nvda/projectDocs/design/startupShutdown.md` is the authoritative
outline; the short version: NVDA can start from the launcher, the
logon screen, ease-of-access, or a user shortcut, with a mutex-based
single-instance rule where a new start replaces the running instance;
exit paths (menu, gesture, `WM_QUIT`, session end) converge on
`triggerNVDAExit` which serializes save-config and teardown hooks so
double-exits and exit-during-startup are safe. Restarts (after
install, language change, or crash recovery;
[Main loop and watchdog](main-loop-and-watchdog.md)) relaunch with
the same arguments minus the crash-relevant ones.

## Updates

`source/updateCheck.py`: a periodic HTTPS check against NV Access's
endpoint (`_getCheckURL`, with a mirror override), sending version
and anonymized capability stats (which synth/braille drivers are in
use — `getQualifiedDriverClassNameForStats`); persistent state
(`state`) tracks the pending download so an update survives restarts.
Downloads are verified by certificate pinning against known
failure modes (`CERTIFICATE_VERIFY_FAILED` handling) and the
downloaded launcher's signature; installation is the launcher run
elevated. Update checking is disabled in secure mode and can be
disabled by policy.

## COM registration fixes

`source/COMRegistrationFixes/` is the curiosity with a lesson: a
tool (Tools menu, elevated) that re-registers system OLE/COM
components — `oleacc.dll` and friends, via bundled `.reg` fragments
like `oleaccProxy.reg` — because real systems turn up with broken
MSAA/IAccessible proxy registrations (typically after aggressive
"cleaner" utilities), and the symptom is "screen reader reads
nothing in some apps." Any MSAA-consuming product eventually meets
these systems; NVDA's answer was to ship the repair.
