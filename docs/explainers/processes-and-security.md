# Processes, integrity, and the security walls a screen reader crosses

A screen reader is architecturally a cross-process surveillance tool with
input injection — exactly what Windows' desktop security model exists to
restrict. This file explains the walls and the sanctioned doors through
them.

## Integrity levels and UIPI

Since Vista, every process carries an *integrity level* (IL): Low
(sandboxed browser/renderer processes), Medium (normal user apps), High
(elevated/admin), System. On top of the user-rights model, *UIPI* (User
Interface Privilege Isolation) blocks lower-IL processes from interfering
with higher-IL UI: they cannot send most window messages upward (bare
`SendMessage` fails with access denied), cannot install hooks into
higher-IL processes, and cannot inject input at them.

The practical bite: a Medium-IL screen reader confronted with an elevated
installer window could neither read it (its queries and hooks are blocked
upward) nor click it. The sanctioned door is the **uiAccess** flag: an
executable whose manifest declares `uiAccess="true"`, is *signed*, and is
launched from a trusted path (Program Files / System32) runs at Medium IL
but is exempted from UIPI's upward restrictions — it may observe and
message higher-IL UI (short of System-owned secure UI) and set winevent
hooks that see elevated processes. Assistive technologies are the intended
audience for uiAccess. Accessibility APIs also have their own carve-outs:
UIA can serve content from elevated apps to a uiAccess client, and winevent
data flows down where raw messages would be blocked. During development,
unsigned builds run without uiAccess and simply cannot reach elevated
windows — a class of bug reports to recognize instantly.

## The secure desktop

Windows keeps multiple *desktops* per session; the interesting one is the
Winlogon secure desktop, where UAC consent prompts, Ctrl+Alt+Del, and the
login screen live. Processes on the default desktop cannot see or touch it.
A screen reader that should speak UAC prompts must arrange for an instance
*on* that desktop — which only a SYSTEM-level service can launch there.
This is why NVDA (and eventually Verbatim, roadmap M8) runs a copy of
itself on the secure desktop with a reduced, no-user-config profile, and
why "why is the UAC prompt silent" is a deployment/signing/service
question, not an API question.

## AppContainer and sandboxes

UWP apps and hardened renderers run in *AppContainers*: capability-scoped
sandboxes whose objects live in per-package namespaces. Two directions
matter:

- Reading sandboxed apps: the accessibility APIs are the sanctioned path
  and generally work (UIA brokers across the boundary). Code injection
  into an AppContainer does not, which is one reason injection-era
  techniques stop at modern app boundaries.
- Sandboxing your own risky components: a process you spawn can be *placed
  in* an AppContainer or a restricted-token job so that a compromise of a
  parser/synth stays contained. Named kernel objects a sandboxed child
  must reach need ACLs granting the container SID. (This is Verbatim's
  plan for third-party synth hosts; the primitive is standard Windows.)

## Sessions and window stations, briefly

Services run in session 0 with no interactive desktop; interactive logins
get numbered sessions. Nothing UI — hooks, winevents, input, window
enumeration — crosses a session boundary. Everything a screen reader does
is per-session; the multi-session consequence is simply that each logged-in
session (including RDP) needs its own instance, and a service-based
launcher must use `CreateProcessAsUser` machinery to start one in the right
session.

## Input injection

`SendInput` synthesizes keyboard/mouse input at the *system* level,
subject to UIPI (no injecting at higher IL without uiAccess). It places
events on the same path as hardware, so the screen reader's own low-level
hooks will see its injected input too — clients tag injected events (the
`dwExtraInfo` field) to recognize and skip their own echoes. This machinery
is how a test harness drives a screen reader with "real" keystrokes, and
its interaction with hooks is why input tests need a live, unlocked
interactive desktop (a locked desktop has no input focus to deliver to —
the trap documented in [Tooling](../tooling.md)).

## References

- [Security Considerations for Assistive Technologies (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)
- [Windows Integrity Mechanism Design (Microsoft Learn, archived)](https://learn.microsoft.com/en-us/previous-versions/dotnet/articles/bb625963(v=msdn.10))
- [Desktops (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winstation/desktops)
- [AppContainer Isolation (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation)
- [SendInput (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
