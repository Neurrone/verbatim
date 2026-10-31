# Security model survey

The evidence behind section 8 of [the security model](../../docs/design/security-model.md), gathered on 2026-10-31. NVDA's mechanisms were read from its source at the pinned commit `732d1ce2f0f051da3ef1e6a6247c64cf04596337`, with paths relative to that tree. Everything else comes from the public sources named beside each item. Entries marked unconfirmed could not be checked from a primary source.

## NVDA, from its source

### Secure mode and the lock screen

- Secure mode is set by `--secure`, by the `forceSecureMode` machine-wide registry value, or by running on the secure desktop unless `serviceDebug` is set (`source/nvda.pyw` lines 260 to 269, `source/NVDAState.py` lines 370 to 388). Running on the secure desktop is detected by the desktop's name being "Winlogon" (`source/utils/security.py` lines 421 to 434).
- What secure mode blocks is a denylist spread across the code: saving configuration, the settings and dictionary dialogs, the log viewer, the speech and braille viewers, the Python console, the add-on store, help documents and the website, the licence (whose links open a browser), wx inspection, copying the log, opening the configuration folder, and starting Remote Access (`source/gui/__init__.py` lines 237 to 863, `source/globalCommands.py`, `source/documentationUtils.py` lines 89 to 94). wx's logging is turned off because its error dialog could open a file dialog (`source/core.py` lines 638 to 640). The clipboard is disabled by hooking `OpenClipboard` (`nvdaHelper/local/nvdaHelperLocal.cpp` lines 137 to 160). Nothing is written to disk and the crash handler is off.
- On the lock screen, only an allowlist of safe scripts runs (`source/utils/security.py` lines 71 to 152, enforced in `source/scriptHandler.py` lines 101 to 108). Objects are checked for being below the lock screen by walking the window z-order for the lock screen's window classes; the source calls this "a hack". The check treats an object as safe when it raises an error (`source/utils/security.py` lines 155 to 283). `LockScreenObject` makes neighbours outside the lock screen read as absent (`source/NVDAObjects/lockscreen.py`).

### Input

- The keyboard hook passes `LLKHF_INJECTED` to its handlers; `LLKHF_LOWER_IL_INJECTED` is not used (`source/winInputHook.py`). Injected keys are gestures by default, under the `keyboard.handleInjectedKeys` setting (`source/config/configSpec.py` line 213, `source/keyboardHandler.py`).

### Logging

- The IO level logs every gesture and every utterance, and the typed-word echo, built from stars in protected fields (`source/inputCore.py` lines 617 to 619, `source/speech/speech.py` lines 1211 and 1466). Keys typed into password fields are logged as keys.
- Redaction applies only to calls passing `redactSecrets`, which are the configuration dumps at startup (`source/logHandler.py` lines 294 to 310).
- In secure mode logging is off, with the comment that logging on secure screens "could allow keylogging of passwords" (`source/logHandler.py` lines 589 to 638).
- The log is `%TEMP%\nvda.log` for installed and portable copies, with the previous one kept as `nvda-old.log`.

### The controller

- The controller is a local RPC endpoint, `NvdaCtlr.<desktop>`, with the security descriptor `D:(A;;GA;;;wd)(A;;GA;;;AC)`, full access for Everyone and every AppContainer, registered to accept calls with no authentication (`nvdaHelper/local/rpcSrv.cpp` lines 49 to 70).
- The public interface speaks text and SSML, cancels speech, shows braille and reports state. The internal interface on the same endpoint takes typed-character and display-model notifications from the injected helper, and also offers an add-on install request and opening the configuration folder (both refused in secure mode or while locked), and a Remote Access link handler with no secure-mode check (`source/NVDAHelper/__init__.py` lines 688 to 742).

### Remote Access

- TLS 1.2 with system certificates. A certificate that fails verification is accepted if its SHA-256 fingerprint matches one the user trusted for that host and port (`source/_remoteClient/transport.py` lines 338 to 466). Clients join a channel by its key. No end-to-end encryption beyond TLS to the relay was found.

### Add-ons, configuration, updates

- Add-ons live in the user's configuration folder and are unrestricted Python. The store downloads over HTTPS and checks a SHA-256 from its metadata (`source/addonStore/network.py` lines 370 to 395). No signature check was found.
- Configuration is in `%APPDATA%\nvda` for installed copies and beside the executable for portable ones; secure screens use an administrator-owned copy made by `nvda_slave.exe`, which leaves out the scratchpad code folders and executables and copies only the add-ons the user chose (`source/config/__init__.py` lines 176 to 386). It is validated against a specification, and a corrupt file is set aside and replaced by defaults.
- Updates are checked against a SHA-1 hash supplied by the update server, when one is supplied; no Authenticode check was found (`source/updateCheck.py`).

### Robustness and recovery

- The watchdog treats the main thread as frozen after half a second, then cancels COM calls and `SendMessage` every 50 ms; NVDA's helper forces `SMTO_ABORTIFHUNG` and caps `SendMessageTimeout` at 10 seconds (`source/watchdog.py`, `nvdaHelper/local/nvdaHelperLocal.cpp` lines 60 to 155).
- The focus ancestry walk stops at 100 hops, logs "Never ending focus ancestry" and jumps to the desktop (`source/api.py` lines 110 to 124). WinEvents are limited to 10 per thread (`source/IAccessibleHandler/orderedWinEventLimiter.py`). No general cap on spoken text length was found.
- A synthesizer that fails to load falls back through OneCore, eSpeak NG and silence (`source/synthDriverHandler.py` lines 501 to 579). Synthesizers run in NVDA's process except 32-bit SAPI voices.
- The crash handler writes a minidump and restarts NVDA, with a breaker for crash loops.

### Injection

- `nvdaHelperRemoteLoader.exe` runs with NVDA's token and installs global in-context WinEvent hooks, which load `nvdaHelperRemote.dll` into every process that raises focus or foreground events (`nvdaHelper/remote/injection.cpp` lines 322 to 333).

## NVDA's advisories

From https://github.com/nvaccess/nvda/security/advisories, 24 advisories, none with a CVE number.

Escapes from secure screens or the lock screen:

- GHSA-x2c3-c6j2-87w4 (2022): object navigation out of the lock screen.
- GHSA-mvc8-5rv9-w3hx, GHSA-vw48-6q24-rppq, GHSA-wg65-7r23-h6p9, GHSA-xc5m-v23f-pgr7 (2022): an elevated Python console through wx inspection; escalation through context help and the add-ons manager; denial of service through two dialogs; symbol settings changed in secure mode.
- GHSA-rmq3-vvhq-gp32 (2022, critical): the console and object navigation from the lock screen, because the lock was judged from the foreground process.
- GHSA-585m-rpvv-93qg (2022, critical): the log viewer from the lock screen, leading to the console.
- GHSA-72mj-mqhj-qh4w (2023, high): an NVDA restarted on the lock screen assumed it was unlocked.
- GHSA-3jj9-295f-h69w (2023, critical): the document formatting dialog opened a SYSTEM shell.
- GHSA-fpwc-2gxx-j9v7 (2023): the console from the "forgot my PIN" screen.
- GHSA-h7pp-6jqw-g3pj (2024, high): a clipboard error dialog led to a file dialog and to running programs.
- GHSA-669f-7gpr-5vqm (2026, low): the selection of add-ons for secure screens bypassed by a folder name.

Information across the boundary:

- GHSA-354r-wr4v-cx28 (2022): debug logging could be enabled on secure screens.
- GHSA-grvr-j2h8-3qm4 (2023): the speech viewer's text kept on the lock screen.
- GHSA-8f8q-2jc3-6rf4 (2025): the last braille text of the user's session shown on the secure screen.
- GHSA-qhjv-3xf4-9c66 (2026): the braille display not cleared at shutdown.

Code execution and elevation:

- GHSA-q7c2-pgqm-vvw5 (2023, high): self-compiled DLLs used for escalation; a malicious add-on on the secure screen ran as SYSTEM.
- GHSA-xg6w-23rw-39r8 (2024, high): web text run as code through the HTML message dialog.
- GHSA-qf5h-qw92-rx2f (2025, high): the installer loaded DLLs planted beside it.
- GHSA-cq78-fjp7-3rp5 (2026, moderate): NVDA started elevated after an install from an elevated prompt.
- GHSA-727q-h8j2-6p45 (2024, moderate, unpatched by design): an add-on can patch NVDA's elevation code.

Remote Access:

- GHSA-vr27-g5ph-xvq2 (2025, low): a planted file blocked Remote Access on secure screens.
- GHSA-42v6-wjv6-h3jj (2025, low): invalid certificates blocked another user's access.
- GHSA-m268-mc77-j2cr (2026, critical): a manually trusted certificate was not checked again on reconnecting.

## Other screen readers and research

- JAWS: in versions 13 and earlier, a service any user could reconfigure ran code as SYSTEM (https://seclists.org/bugtraq/2016/Apr/53). No other public advisory or CVE was found. Scripts are compiled code loaded per application (https://support.freedomscientific.com/Content/Documents/Other/ScriptManual/03-1_JAWSScriptsAndScriptFiles.htm).
- Narrator: CVE-2020-1398, a BitLocker lock-screen bypass that used Narrator at sign-in (https://secret.club/2021/01/15/bitlocker-bypass.html). NVD lists several 2026 privilege-escalation entries for Narrator's braille support and for `ATBroker.exe`, chiefly untrusted search paths (identifiers as reported by the survey, not checked one by one).
- UIAccess: Microsoft's requirements are a trusted signature and an administrator-only location (https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-only-elevate-uiaccess-applications-that-are-installed-in-secure-locations). James Forshaw showed that a UIAccess token can be duplicated from a running UIAccess process to start new ones, that the integrity level is High for an administrator's split token and the caller's plus 16 otherwise, and called UIAccess "not a security boundary" (https://www.tiraniddo.dev/2019/02/accessing-access-tokens-for-uiaccess.html). Microsoft's own position on UIAccess specifically is unconfirmed.
- Orca and AT-SPI: the bus has no access control (https://wiki.gnome.org/Projects(2f)GTK(2f)Accessibility.html). Keyboard capture moved into the compositor, sent only to an allowlisted screen reader so no process can act as a keylogger (https://blogs.gnome.org/a11y/2024/06/18/update-on-newton-the-wayland-native-accessibility-project/).
- VoiceOver: CVE-2005-2513 (secure input fields read), CVE-2013-0969 (login window bypass), CVE-2023-32359, CVE-2024-44204 and CVE-2025-31229 (passwords and the passcode read aloud). macOS grants accessibility access per application (https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac).
- Android: banking malware abuses accessibility services; Android 13 restricts them for sideloaded apps, and droppers bypass the restriction (https://thehackernews.com/2024/02/anatsa-android-trojan-bypasses-google.html).
- Accessibility tools at sign-in replaced or redirected to get a SYSTEM shell: MITRE ATT&CK T1546.008 (https://attack.mitre.org/techniques/T1546/008/).
- Jang and others, "A11y Attacks: Exploiting Accessibility in Operating Systems", CCS 2014 (https://www.cs.ucr.edu/~csong/ccs14-a11y.pdf): twelve attacks on four systems, among them a UAC bypass by synthesized speech driving Speech Recognition and passwords revealed through UI Automation; they recommend that an assistive technology check its input comes from the user.
