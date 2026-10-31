# Security model survey

The evidence behind [the security model](../../docs/design/security-model.md), and its comparison with NVDA and other screen readers, gathered on 2026-10-31. NVDA's mechanisms were read from its source at the pinned commit `732d1ce2f0f051da3ef1e6a6247c64cf04596337`, with paths relative to that tree. Everything else comes from the public sources named beside each item. Entries marked unconfirmed could not be checked from a primary source.

## Comparison with NVDA and other screen readers

NVDA's mechanisms below were read from its source at the pinned commit. Its advisories come from its GitHub advisory list, which held 24 entries on 2026-10-31. The other readers' history comes from NVD, vendors' documentation and published research.

### NVDA's record

About half of NVDA's advisories are escapes from secure screens or the lock screen, several rated critical: the Python console, the log viewer, context help, a clipboard error dialog and a formatting dialog each led to code running as SYSTEM, and object navigation walked out of the lock screen. The rest are information left behind across that boundary (debug logging, the speech viewer, braille contents), code execution (DLL planting in the installer, web text run through an HTML dialog, an add-on able to patch NVDA's elevation code, which NV Access treats as a consequence of add-ons being unrestricted), and Remote Access flaws, the worst a missed certificate check on reconnecting.

The lesson Verbatim takes is that the secure-desktop instance, not the everyday one, is where a screen reader is attacked, and that a denylist of features there fails one feature at a time.

### Mechanism by mechanism

- **Isolation from hostile applications.** NVDA runs its logic on one main thread, and its watchdog cancels COM calls and `SendMessage` after a stall. Verbatim isolates each application in its own process (D9). Verbatim is ahead. Both cap ancestor walks (NVDA at 100 hops, then it jumps to the desktop; Verbatim at 64), and neither detects a repeat (mitigation U2).
- **Control surface.** NVDA's controller is a local RPC endpoint whose access list grants full access to Everyone and to every AppContainer, with no authentication, and carries both the public speech functions and internal ones, including an add-on install request and a Remote Access link handler. Its injected helper reports through the same endpoint. Verbatim's pipe admits only the owning user and refuses remote clients, so it is ahead on reach, but it offers key injection, which NVDA's controller does not (mitigation C1).
- **Logs and dumps.** NVDA's log at its IO level records every utterance and every key, including keys typed in password fields. It lives in the temporary folder, and only its startup configuration dump is redacted. In secure mode its logging is off. Verbatim logs no content today, which is ahead, but its crash dumps keep typed characters and the text Verbatim itself copied (mitigations L1 and L2). NVDA's crash handler writes a minidump of its memory, which holds whatever it was reading, so neither keeps content out of crash data.
- **Protected fields.** Both echo a star for each typed character and suppress typed-word echo.
- **Injected input.** Both treat injected keys as gestures. NVDA lets the user turn that off, and neither looks at the injector's integrity level (mitigation C5).
- **Extensions.** NVDA's add-ons are unrestricted Python in its process. Its store pins each release by SHA-256 and scans it, but nobody reviews the code and nothing is signed. Verbatim's Wasm modules with derived capabilities are ahead by design (mitigation X4). NVDA's copy of add-ons to the secure screens, chosen by the user and made by an elevated step, is the model Verbatim's design already follows.
- **Configuration.** NVDA keeps it in the user's profile for installed copies, beside the executable for portable ones, and in an administrator-owned copy for secure screens; it validates the file against a specification and resets a corrupt one to defaults. Verbatim matches this once its installed copies keep their state in the profile (section 4.1) and C3 is done.
- **Recovery from its own crash.** NVDA's crash handler restarts NVDA, with a breaker that stops after too many crashes in a short window. Verbatim does not restart (mitigation A1), and should copy the breaker.
- **Synthesizers.** NVDA runs most synthesizers in its own process. A failure to load falls back from OneCore to eSpeak NG to silence, but a crash at runtime takes NVDA with it, and only 32-bit SAPI voices run in a separate host. Verbatim's host per synthesizer (D18) is ahead. Neither falls back after a runtime crash (mitigation A3).
- **Secure mode.** NVDA's is mature, built from twenty advisories. Verbatim has none yet. It has the chance to build one from an allowlist from the start (mitigations B3 to B8).
- **Updates.** NVDA checks a SHA-1 hash from its own update server and runs the launcher; no signature check was found. Verbatim has no updater yet (mitigation X9).
- **Injection.** NVDA injects its helper DLL into every process that raises a focus or foreground event, through global in-context hooks. Verbatim injects nothing until M7, and then only through one helper (D2).

### Other screen readers

- **JAWS.** Its scripts are compiled code loaded per application. Its one public vulnerability found is a service, in versions 13 and earlier, that any user could reconfigure to run code as SYSTEM. No other advisory or CVE was found, which may reflect disclosure practice rather than an absence of flaws.
- **Narrator.** It was the route of a BitLocker lock-screen bypass (CVE-2020-1398), and NVD lists several privilege-escalation entries for Narrator's braille support and for the accessibility broker, `ATBroker.exe`, in 2026, chiefly untrusted search paths. Even Windows' own screen reader is attacked at the sign-in screen and through DLL search order.
- **Orca and AT-SPI.** The Linux accessibility bus has no access control, and any client can see every other's events. GNOME's recent work moves keyboard capture into the compositor and sends keys only to an allowlisted screen reader, so that no process can act as a keylogger. The lesson for Verbatim is that a screen reader's input path is a keylogger by design, and must not be readable by others (mitigations L5 and L1).
- **VoiceOver.** Apple's fixes include VoiceOver reading secure input fields, saved passwords and the passcode aloud, and a login window bypass. macOS asks the user to grant accessibility access per application. The lesson is that protected text must never be spoken by any path, including paths added later (mitigation L2).
- **Android accessibility services.** Banking malware uses accessibility services to read the screen and inject input, which is why Android 13 restricts them for sideloaded apps, a restriction droppers have bypassed. Any API that grants a screen reader's reach is a malware target: Verbatim's extension imports must not hand that reach to an extension the user did not knowingly install (mitigation C7).
- **Research.** Jang and others (CCS 2014, "A11y Attacks") bypassed UAC and the sandboxes of four systems through accessibility. On Windows, speech synthesized by a process drove Speech Recognition, which runs at High integrity, and UI Automation clicked a password field's reveal button. Their recommendation, that an assistive technology check that its input comes from the user, is mitigation C5.

### NVDA's issue tracker

NVDA's issues add risks its advisories do not show. The mitigations they led to are L9 (echo failing closed: unrecognised password fields, consoles, the sign-in PIN's last digit, typed words outside edit fields), B3 and C8 (remote control and updates on the lock screen, the sign-in setting rewritten), B5 (whole folders copied to the secure configuration), X3 (state read with Python's pickle), C1a (a debug switch left on), L8's additions (the curtain switched off by a profile, no way to query it), C8's end-to-end encryption (the relay reading Remote traffic), S3 (link destinations), X8's narrow injection, and L6's update check. The issues are listed below, under "NVDA's issue tracker".

### Where Verbatim can do better

- Build the secure instance from an allowlist before it first runs, rather than after advisories.
- Decide lock state from Windows, failing closed.
- Give the control surface the least reach, and none on secure screens.
- Keep passwords out of every file, including crash dumps, without losing the text dumps need.
- Refuse lower-integrity injected input, which no surveyed screen reader does.
- Fall back to another synthesizer after runtime crashes, which neither NVDA nor Verbatim does today.
- Verify updates by signature.

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

## NVDA's issue tracker

Searched through GitHub's issue search on 2026-10-31, not exhaustively (at most 100 results a query). Issue numbers are in `nvaccess/nvda`, each at https://github.com/nvaccess/nvda/issues/ followed by the number.

- Echo of secrets that fails open: 14529 (Excel password dialog not marked protected, open), 5221 (Excel sheet protection, closed as not planned), 11554 (a `sudo` password echoed in WSL, fixed in 2022.3 by UIA consoles), with 13781 and 14047 on detecting typing that never appears.
- Timing of the protected check: 7908 and 7792 (the sign-in PIN's last digit spoken, fixed by PR 7989), 20654 (a synchronous UIA query per typed character freezes NVDA on a slow provider, open), PR 20694 (deciding protection lazily, unmerged).
- Typed words echoed outside edit fields: 10331.
- Lock state and the lock screen: PR 20842 (an unreadable session state treated as unlocked, closed pending ADR 20843; that it is exploitable is the survey's inference), 18951 (keys stayed with a Remote peer after locking, fixed by PR 18973), 18357 (add-on updates installed from the lock screen, closed as not planned), 20399 (the "use during sign-in" setting rewritten at each launch, closed as intended).
- The secure-desktop configuration copy: 20266 (whole folders copied, open), 19446 (choosing add-ons, merged), 12879 (disabling add-ons on secure screens), 1426 (the original concern).
- Unsafe deserialisation: 19559 (add-on state read with pickle, moved to JSON by PR 19564 with a pickle fallback), 19560 (update state pickled, open), 19743 to 19745 (removing pickle, open).
- Debug switches: 19687 (warn when `serviceDebug` is on, open), 18086.
- Screen curtain: 10476 (turned off by a profile switch, fixed in 2026.1 by PR 19177), 21079, 20688 and 20108 (fails to enable in setup and after updates, open), 21084, 10265 and 10692 (no way to query its state without toggling).
- Remote Access: 17784 (the relay can read and change traffic; end-to-end encryption planned, with design record 19868 and PR 19857).
- Link destinations: 14535 (a command to report a link's address, added in 2023.1 by PR 14583).
- Add-on supply chain: 18974 (VirusTotal results in the store), 19984 and 19956 (malformed scan data crashed the store), 20386 (an out-of-process add-on runtime's transport), 20491 (updates offered from unselected channels), 18986 (add-ons shipping executables).
- Usage statistics: 16710 (an anonymous id with update checks), 18197, 8510 (show what is sent), 19296.
- Injection footprint: 20667 (in-context hooks rejected because they inject into every process), 3609.

## Other screen readers and research

- JAWS: in versions 13 and earlier, a service any user could reconfigure ran code as SYSTEM (https://seclists.org/bugtraq/2016/Apr/53). No other public advisory or CVE was found. Scripts are compiled code loaded per application (https://support.freedomscientific.com/Content/Documents/Other/ScriptManual/03-1_JAWSScriptsAndScriptFiles.htm).
- Narrator: CVE-2020-1398, a BitLocker lock-screen bypass that used Narrator at sign-in (https://secret.club/2021/01/15/bitlocker-bypass.html). NVD lists several 2026 privilege-escalation entries for Narrator's braille support and for `ATBroker.exe`, chiefly untrusted search paths (identifiers as reported by the survey, not checked one by one).
- UIAccess: Microsoft's requirements are a trusted signature and an administrator-only location (https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-only-elevate-uiaccess-applications-that-are-installed-in-secure-locations). James Forshaw showed that a UIAccess token can be duplicated from a running UIAccess process to start new ones, that the integrity level is High for an administrator's split token and the caller's plus 16 otherwise, and called UIAccess "not a security boundary" (https://www.tiraniddo.dev/2019/02/accessing-access-tokens-for-uiaccess.html). Microsoft's own position on UIAccess specifically is unconfirmed.
- Orca and AT-SPI: the bus has no access control (https://wiki.gnome.org/Projects(2f)GTK(2f)Accessibility.html). Keyboard capture moved into the compositor, sent only to an allowlisted screen reader so no process can act as a keylogger (https://blogs.gnome.org/a11y/2024/06/18/update-on-newton-the-wayland-native-accessibility-project/).
- VoiceOver: CVE-2005-2513 (secure input fields read), CVE-2013-0969 (login window bypass), CVE-2023-32359, CVE-2024-44204 and CVE-2025-31229 (passwords and the passcode read aloud). macOS grants accessibility access per application (https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac).
- Android: banking malware abuses accessibility services; Android 13 restricts them for sideloaded apps, and droppers bypass the restriction (https://thehackernews.com/2024/02/anatsa-android-trojan-bypasses-google.html).
- Accessibility tools at sign-in replaced or redirected to get a SYSTEM shell: MITRE ATT&CK T1546.008 (https://attack.mitre.org/techniques/T1546/008/).
- Jang and others, "A11y Attacks: Exploiting Accessibility in Operating Systems", CCS 2014 (https://www.cs.ucr.edu/~csong/ccs14-a11y.pdf): twelve attacks on four systems, among them a UAC bypass by synthesized speech driving Speech Recognition and passwords revealed through UI Automation; they recommend that an assistive technology check its input comes from the user.

## Sandboxing

### How others sandbox

- **Chromium** splits a broker, the browser process, from targets, each limited four ways: a restricted token, a job, an alternate desktop and a low or untrusted integrity level. Renderers get the strictest token (`USER_LOCKDOWN`), untrusted integrity and the window-manager system calls removed; the network service runs in a less privileged AppContainer; every process but those that load third-party code (speech recognition, screen AI, media decryption) may load only Microsoft-signed images. Its interception of system calls is for compatibility, and its own documentation says it provides no security.
- **Firefox** uses the same Chromium sandbox code at configurable levels, and compiles some third-party libraries (font shaping, spell checking, XML parsing) to Wasm and back to C with RLBox, so a memory-safety bug in them stays inside a Wasm boundary within the same process. Removing window-manager calls once broke its accessibility, because DLLs injected by other software made such calls while COM started.
- **Both browsers** moved accessibility out of their sandboxed processes: the browser process caches the whole tree and answers UIA and IA2 from it, because a sandboxed process cannot serve those interfaces and asking it for each call was too slow. No browser process is a UIA client.
- **The Codex CLI** on Windows uses no AppContainer: it runs commands under a write-restricted token, whose writes succeed only where an access entry names one of its restricting SIDs, adds those entries to the folders it allows, gives each command a job, a private desktop and a default access list that keeps launches apart, and, when installed by an administrator, blocks network access with Windows Filtering Platform rules scoped to a dedicated account.
- **Windows' Win32 app isolation** (preview) puts an ordinary program in an AppContainer with brokered file access. None of its capabilities covers UIA, observing input or crossing UIPI.
- Windows has no general system-call filter like Linux's seccomp; the closest are the policies that remove the window-manager or file-system-control calls. Its signed-image policy admits only Microsoft, Store or WHQL signatures, never a third party's own certificate; only App Control for Business (WDAC) can allow a chosen publisher.

### Sources

Gathered on 2026-10-31 for section 6.9 of the security model.

- Codex CLI on Windows: a restricted token made with `CreateRestrictedToken` (`DISABLE_MAX_PRIVILEGE`, `LUA_TOKEN`, `WRITE_RESTRICTED`), restricting SIDs of its own plus the user, logon and Everyone SIDs, write access granted by adding access entries to allowed folders, a default access list limited to the logon SID, a private desktop, a job per child; in its elevated mode, dedicated local accounts and Windows Filtering Platform rules scoped to them that block the network; no AppContainer in its classic backends (https://github.com/openai/codex/tree/main/codex-rs/windows-sandbox-rs). On macOS, a deny-by-default Seatbelt profile modelled on Chrome's; on Linux, bubblewrap with a read-only root, no new privileges and a seccomp filter on network calls.
- Chromium: the design and its four restrictions (https://chromium.googlesource.com/chromium/src/+/main/docs/design/sandbox.md), which says the interception and IPC "does not provide security"; token levels in `sandbox/win/src/security_level.h`; per-process policies in `sandbox/policy/win/sandbox_win.cc` (renderer lockdown with window-manager calls removed, network service and print compositor in a less privileged AppContainer, Microsoft-signed images forced except for speech recognition, screen AI, media decryption and the platform runtime). Accessibility is served by the browser process from a cached tree, since renderers "can't implement operating system APIs directly" (https://chromium.googlesource.com/chromium/src/+/main/docs/accessibility/overview.md).
- Firefox: content levels in `sandboxBroker.cpp` (https://raw.githubusercontent.com/mozilla-firefox/firefox/main/security/sandbox/win/src/sandboxbroker/sandboxBroker.cpp); removing window-manager calls broke COM and accessibility because injected DLLs made such calls (https://hacks.mozilla.org/?p=47829); Cache the World moved the accessibility tree to the parent process (https://blog.mozilla.org/en/uncategorized/firefox-accessibility-boost/); RLBox compiles third-party libraries to Wasm and back to C (https://hacks.mozilla.org/2021/12/webassembly-and-back-again-fine-grained-sandboxing-in-firefox-95/).
- Win32 app isolation: AppContainer-based, preview, capabilities with none for UIA or input (https://learn.microsoft.com/en-us/windows/win32/secauthz/app-isolation-overview, https://learn.microsoft.com/en-us/windows/win32/secauthz/app-isolation-supported-capabilities).
- UIA and integrity: UIA clients at medium integrity need UIAccess to reach higher levels, and UWP apps cannot have UIAccess (https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview); UIA and MSAA begin with `WM_GETOBJECT` (https://learn.microsoft.com/en-us/windows/win32/winauto/how-wm-getobject-works), which UIPI filters upward. Whether a medium-integrity write-restricted token keeps UIA working is not documented anywhere found.
- Wasmtime's Pulley interpreter avoids generating native code but "will never be as fast as native Cranelift backends" (https://docs.wasmtime.dev/examples-pulley.html).
- Windows system-call policy (https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_system_call_disable_policy) and signed-image policy, which admits only Microsoft, Store or WHQL signatures (https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-process_mitigation_binary_signature_policy).
- Others: Office Protected View moved from a low-integrity restricted token to an AppContainer; VS Code sandboxes only its renderer and keeps full Node in its extension host (https://code.visualstudio.com/blogs/2022/11/28/vscode-sandbox); Edge's enhanced security mode turns the JavaScript compiler off to allow shadow stacks and the no-generated-code mitigation.

## Extension resource limits

In short: Chrome stops an extension's service worker after 30 seconds idle or an event longer than 5 minutes and caps nothing else; Firefox stops content scripts after 5 seconds, warns about slow add-ons and gives up its extension process after 5 crashes in 30 seconds; VS Code profiles an unresponsive extension host after 3 seconds and gives up after 3 crashes in 5 minutes; Cloudflare Workers and Shopify Functions set hard limits per call; Figma and NVDA set none. Verbatim's design sits at the strict end, with a deadline per hook, a memory limit per instance and a job per process.

Gathered on 2026-10-31 for A6 to A8 of the security model.

- Chrome (Manifest V3): the service worker stops after 30 seconds idle, an event or API call longer than 5 minutes, or a fetch response slower than 30 seconds, with lifetime extensions in Chrome 110, 114, 116 and 118 (https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle). No per-extension memory cap; the renderer job's limit is 1 TB (`sandbox/policy/win/sandbox_win.cc`). The enterprise `ExtensionSettings` policy has no processor or memory field (https://support.google.com/chrome/a/answer/9867568). No current automatic kill or "slowing Chrome" warning was found.
- Firefox: `dom.max_ext_content_script_run_time` 5 seconds, `dom.max_script_run_time` 10 seconds (`modules/libpref/init/StaticPrefList.yaml`); an idle background page unloaded after 30 seconds (`ext-backgroundPage.js`); the extension process stops being respawned after 5 crashes in 30 seconds (`Extension.sys.mjs`); a bar saying an add-on "might be making Firefox run slowly", which can name the wrong add-on (https://bugzilla.mozilla.org/show_bug.cgi?id=2025327).
- Safari on iOS: developers report an extension memory limit of 6 MB, raised to 80 MB in iOS 15.1 (forum, unconfirmed by Apple's documentation).
- VS Code: the extension host is unresponsive after 3 seconds without a reply (`rpcProtocol.ts`), then profiled for up to 5 seconds to name the extension (`extensionsAutoProfiler.ts`); a crashed host restarts until 3 crashes in 5 minutes (`abstractExtensionService.ts`); no processor or memory cap; Extension Bisect finds a bad extension.
- Cloudflare Workers: processor time per request from 10 ms (free) to 5 minutes, 128 MB per isolate, a request over its limit failing with error 1102 (https://developers.cloudflare.com/workers/platform/limits/).
- Shopify Functions: 11 million Wasm instructions, counted deterministically, 10,000 kB linear memory and 512 kB stack (https://shopify.dev/docs/api/functions/latest).
- Envoy Wasm: a failure policy of failing closed, failing open or reloading the instance (https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/wasm/v3/wasm.proto).
- Figma: "Plugins can hang, and there is no way to interrupt a plugin" (https://www.figma.com/blog/how-we-built-the-figma-plugin-system/).
- Wasmtime: epoch interruption is cheaper than fuel but not deterministic; `StoreLimitsBuilder::memory_size` caps memory (https://docs.wasmtime.dev/api/wasmtime/struct.Config.html). Zed uses epoch interruption with a 100 ms tick and yields rather than trapping (`crates/extension_host/src/wasm_host.rs`).
- NVDA: add-ons run in NVDA's process with no limit; its watchdog cancels blocking cross-process calls but never stops add-on code; the remedy is restarting with add-ons disabled (`--disable-addons`).

## Braille drivers

Gathered on 2026-10-31 for section 6.11 of the security model, from NVDA's master branch (the pinned submodule was not checked out).

- NVDA's I/O layer (`source/hwIo`): `Hid` opens the HID path and reads its capabilities on that handle; `Bulk` opens two endpoint paths of a vendor kernel driver; `Serial` uses pyserial; Bluetooth LE uses bleak over WinRT. Drivers in `source/brailleDisplayDrivers` use HID and serial (alva, baum, brailliantB, handyTech, seikantk, eurobraille), a kernel bulk driver (freedomScientific; hims with a WinUSB fallback because its unsigned `himsusb.sys` cannot install on Windows 11), plain serial (hedo, seika, papenmeier_serial, ecoBraille, albatross), FTDI's D2XX DLL (papenmeier), `lilli.dll` (lilli), and BrlAPI (brltty). Detection in `source/bdDetect.py` matches USB vendor and product ids and Bluetooth names.
- BRLTTY: on Windows, LibUSB with WinUSB and an administrator-installed BrlAPI service (https://brltty.app/doc/Windows.html); on Linux, it drops from root to a dedicated user, keeps three capabilities, joins the input, dialout and audio groups, isolates namespaces and offers an optional system-call filter (https://brltty.app/doc/Linux.html).
- Device capabilities enable WinRT `Windows.Devices.*` APIs for packaged apps (https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations); Store apps reach devices through a device access broker (https://learn.microsoft.com/en-ie/windows/win32/deviceaccess/deviceaccess-glossary); whether a device object admits a container is per device and best tested directly (https://projectzero.google/2015/11/windows-sandbox-attack-surface-analysis.html). The WinRT HID API refuses keyboard, mouse and other input collections and devices with filter drivers (https://learn.microsoft.com/en-us/uwp/api/windows.devices.humaninterfacedevice). A custom driver's interface needs a Microsoft-approved custom capability (https://learn.microsoft.com/en-us/windows-hardware/drivers/devapps/hardware-support-app--hsa--steps-for-driver-developers).
- A duplicated handle refers to the same object with the access already granted (https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle); that it keeps working in a container child follows from access being checked at open, not found stated in one source.
- Chromium's device service runs in the browser process and gives renderers Mojo interfaces and data pipes, never handles (`services/device/public/mojom/serial.mojom`, `hid.mojom`).
- Device-side risks: BadUSB (https://opensource.srlabs.de/projects/badusb).

## Extension processes and crash reporting

Gathered on 2026-10-31 for X4 and L5 of the security model.

- Chromium: "extension documents and workers are semi-privileged and run in dedicated renderer processes", and "Chromium ensures that extensions do not share a process with each other or with web pages"; content scripts run in the page's renderer (https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md). One process per extension follows from that and process-per-site.
- Firefox: one extension process for all extensions (`dom.ipc.processCount.extension` 1 in `modules/libpref/init/all.js`; `extensions.webextensions.remote` true in `browser/app/profile/firefox.js`).
- Safari on macOS: eight extensions with background pages are eight processes (https://developer.apple.com/videos/play/wwdc2021/10027/).
- Windows Error Reporting: local dumps are configured independently of upload consent but only under `HKLM`, needing an administrator (https://learn.microsoft.com/en-us/windows/win32/wer/collecting-user-mode-dumps, https://learn.microsoft.com/en-us/windows/win32/wer/wer-settings); an unhandled exception filter that returns `EXCEPTION_EXECUTE_HANDLER` ends the process without the system's error handling (https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-setunhandledexceptionfilter), from which it is inferred, untested, that Windows' reporting then never runs.
- NVDA (master): `watchdog.py` installs `crashHandler` outside secure mode unless 3 crashes happened within 120 seconds; `utils/_crashHandler.py` writes `nvda_crash.dmp` beside the log in `%TEMP%`, logs every thread's stack, restarts NVDA and returns `EXCEPTION_EXECUTE_HANDLER`; `nvdaHelper/local/crashDump.cpp` calls `MiniDumpWriteDump` and overwrites the previous dump. No upload exists in the code.
