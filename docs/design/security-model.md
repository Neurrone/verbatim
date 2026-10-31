# Security model

Verbatim's threat model and the architectural mitigations it calls for, decided with Dickson on 2026-10-31. Sections 1 to 5 are the threat model: method, assets, the system's boundaries and entry points, adversaries, and the classes of threat. Section 6 gives the mitigations for each class, section 7 the decisions and accepted risks, and section 8 how well the architecture serves the goal. The work is scheduled in [the roadmap](../roadmap.md), "M6.6 — Security fixes", which also lists what other milestones carry; the evidence and the comparison with NVDA and other screen readers are in [the survey](../../experiments/security-model-survey/readme.md). Rules that belong to the extension surface are in [extensions](extensions.md), section 4.6.

## 1. Method

The model follows the usual shape: model the system and its trust boundaries, list the entry points where data or control crosses them, name the adversaries, enumerate threats by class, then choose mitigations and state what risk remains. The classes are adapted from STRIDE (spoofing, tampering, repudiation, information disclosure, denial of service, elevation of privilege), regrouped around how a screen reader is attacked: most of its input is other programs' content, its output is the user's only view of the screen, and it holds an assistive technology's privileges.

Mitigations remove attack surface by architecture or by product choice first, and add checks only where no structural answer exists, because a check guards one path and a surface that does not exist cannot be attacked by any path. Mitigations marked structural are properties of the architecture; they are the ones that hold against attacks nobody has thought of.

## 2. Assets

In priority order:

1. **Availability.** A silent or hung screen reader leaves a blind user without a computer, so denial of service is a security failure, not a nuisance.
2. **Privilege boundaries.** Verbatim must never let a less trusted process reach something it could not reach on its own. With UIAccess (M8.1) Verbatim reads elevated windows and sends them input, and its secure-desktop instance runs as SYSTEM.
3. **The user's content.** Verbatim sees everything on screen, every keystroke and the clipboard; what it keeps, where, and who can read it is the question Windows Recall raised.
4. **What the user hears.** For a blind user the speech is the interface.
5. **The user's machine.** Code running inside Verbatim runs with the user's rights, and later with UIAccess and at sign-in.

## 3. System, boundaries and entry points

Verbatim's processes are Core (`verbatim.exe`), the focus listener, one outpost per application, one synthesizer host per engine, and, from M5, one host per installed extension (architecture section 1; section 6.6 below). Core talks to its children over unnamed pipes passed by handle.

The trust boundaries:

- between each application and the process that reads it, the listener or its outpost;
- between an outpost and Core, since an outpost is only as trustworthy as the application it reads, and from M7 it loads IA2 proxy code and talks to a helper inside the browser;
- between a synthesizer engine, an installed extension or a braille driver and Core;
- between Verbatim and other processes of the same user, which is not a real boundary until UIAccess (section 4);
- between Verbatim and lower-integrity processes, which is real;
- between Verbatim and elevated windows and secure screens, which Verbatim crosses on the user's behalf only;
- between the machine and the network.

The entry points:

- **Accessibility APIs:** UIA and MSAA events and reads, in the listener and the outposts, including the COM objects applications marshal to Verbatim; later IA2, Java Access Bridge and the M7 helper.
- **Text applications show:** web pages, documents, chat, file names and terminal output, including remote programs' output over SSH. This is how network content reaches Verbatim.
- **The keyboard hook:** every key on the desktop, physical or injected.
- **Verbatim's own GUI:** its menu and dialogs, the input that reaches them, and the code Windows loads into the process that shows them, such as the system file dialog's shell extensions and the user's input methods.
- **Window messages** to Verbatim's windows.
- **The control pipe**, and during development the test agent's TCP port.
- **Named objects:** the startup mutex, the replacement event and the control pipe's name.
- **Files Verbatim reads or maps:** settings, profiles, themes, sounds, locale files, eSpeak NG's data, extension packages, module folders and compiled extension code.
- **Synthesizer engines and braille devices,** their data and their failures.
- **The audio device** and **the clipboard.**
- **Planned:** the installer and updater (M8), the secure-desktop launch (M8.1), Remote support (M12), image description or OCR services.

## 4. Adversaries

Each with whether a real boundary separates it from Verbatim, because effort spent defending a non-boundary is wasted.

- **The application being read, and whoever controls its content.** Most often broken rather than malicious: cyclic parents, endless sibling chains, huge child counts, multi-megabyte names, event floods, hung providers. A real boundary, and the most important one.
- **A process of the same user at the same integrity level.** Windows offers no boundary here: such a process can open Verbatim's process, write its memory, load a DLL into it, inject input and read every window through UIA itself. Defending Verbatim against it gains nothing until UIAccess raises Verbatim above it.
- **Another signed-in user, or a lower-trust process** (Low integrity, AppContainers such as browser renderers). Real boundaries. User Interface Privilege Isolation (UIPI), the Windows rule blocking window messages and injected input from a lower integrity level to a higher one, enforces most of the second already.
- **Any of these using Verbatim as a deputy** into elevated windows or secure screens.
- **An extension's author:** malicious, compromised or, most often, buggy, including the author of a closed engine binary a user downloads.
- **A synthesizer engine:** closed native code that crashes or hangs on particular text.
- **A network attacker:** today only the test agent listens; later the update channel and Remote's relay and peers.
- **A remote supporter** (M12), who may be a scammer.
- **Someone who can see the screen** without the user knowing, which a blind user cannot detect.
- **Whoever builds or ships Verbatim's code:** dependencies, build machines and the release key.

### 4.1 Can Verbatim defend its configuration?

- **Files under Program Files are protected already**, since only an administrator can write there, as long as Verbatim never makes a folder of its installation writable by the user. So an installed copy keeps its settings, logs and dumps in the user's profile, never beside its executable; a writable installation would let any program plant a DLL into a binary Windows starts with UIAccess, or as SYSTEM at sign-in.
- **Settings in the user's profile cannot be protected** from that user's programs, and need not be: they are data and never code (X3), so editing them changes behaviour but runs nothing and crosses no boundary.
- **UIAccess changes this only partly.** Windows starts a UIAccess process above the caller's integrity level (16 above it for a standard user, High for an administrator's split token), so ordinary programs cannot write its memory, and everything that steers it becomes a way to borrow its reach into elevated windows. But James Forshaw showed in 2019 that an ordinary process can duplicate the token of any running UIAccess process and start its own, and called UIAccess "not a security boundary". The goal is therefore that Verbatim adds no easier path than Windows already has, measured first (B1).

## 5. Threat classes

Each class names the threat, its entry points and examples; its mitigations are in section 6 under the same letter.

### 5.1 Class U: hostile structure and volume in accessibility data

STRIDE: denial of service, tampering with what is read. An application's tree and events are input of arbitrary shape and size, and the threat is that they make Verbatim hang, exhaust memory or repeat itself, in the reading process or, worse, in Core, which every application shares.

- Entry points: accessibility APIs, text applications show, braille devices.
- Examples: a parent that returns itself or cycles of parents; siblings that disagree; a million children; a 30 MB name; a 100,000-character line; thousands of combining marks forming one grapheme; a flood of notifications; a provider that never answers; a program starting many short-lived processes that raise events; a wide terminal printed by a remote program; a process id or window handle reused between the event that named it and the read.

### 5.2 Class J: content interpreted as instructions

STRIDE: tampering, elevation of privilege, denial of service. Text from applications passes through components that treat some text as commands, and the threat is that crafted text is executed rather than read.

- Entry points: text applications show, reaching synthesizers, pattern matchers, dialogs and, later, language models.
- Examples: SSML or an engine's command sequences in a web page changing the voice or reaching an engine's crash paths; a NUL character failing a whole utterance; a regular expression that backtracks catastrophically on a long line; NVDA's HTML message dialog running web text as code (GHSA-xg6w-23rw-39r8); on-screen text instructing a model that describes the screen.

### 5.3 Class S: altered speech

STRIDE: spoofing. The threat is that an extension or crafted text changes what Verbatim says, so the user hears something other than what the application presents.

- Entry points: extensions' rules and hooks, Unicode in names and text.
- Examples: an extension that makes a Delete button speak as OK; a direction override that makes an executable's name read as a PDF; zero-width characters hiding words from speech that the screen shows.

### 5.4 Class C: steering Verbatim through its control channels

STRIDE: spoofing, tampering, denial of service, elevation of privilege. The threat is that a process that should not control Verbatim makes it act, or stops it.

- Entry points: the keyboard hook, the control pipe, the test agent, window messages, named objects, files Verbatim watches, Remote.
- Examples: a program asking Verbatim to install a package of its choosing, or asking a hundred times to bury the user in dialogs; a Low-integrity process ending Verbatim with `WM_QUIT`; any program triggering gestures with `SendInput`; a key a lower-integrity program injected, held and re-sent by Verbatim into an elevated window; a process holding Verbatim's startup mutex or pipe name so that Verbatim cannot start; a module dropped into the extension developer folder; anyone on the network reaching the test agent; a remote "supporter" who is a scammer.

### 5.5 Class B: crossing privilege boundaries through Verbatim

STRIDE: elevation of privilege, information disclosure. Verbatim reads and drives contexts more privileged than the programs around it, and the threat is that a user or program uses it to reach them, or to carry information across.

- Entry points: everything in class C, and the GUI and commands available on secure screens and the lock screen.
- Examples, mostly from NVDA's record: a dialog on a secure screen (the log viewer, context help, a clipboard error, a formatting dialog) leading to a SYSTEM shell; object navigation walking out of the lock screen; the lock state judged from a window another program can take over; the speech viewer's or braille display's contents carried onto a secure screen; starting elevated after an elevated install; replacing or redirecting the binary Windows starts at sign-in (MITRE ATT&CK T1546.008); an application's custom COM unmarshaler loaded into a UIAccess outpost.

### 5.6 Class X: running code that is not Verbatim's

STRIDE: tampering, elevation of privilege. The threat is code running in Verbatim's processes, or with their rights, that Verbatim did not intend.

- Entry points: files Verbatim loads or maps, extension packages, engines, the M7 helper's channel, the updater, the build, COM unmarshalling, and every parser of untrusted data.
- Examples: a DLL planted beside a portable copy (NVDA's installer, GHSA-qf5h-qw92-rx2f); a writable installation; a compiled extension cache planted in a user-writable folder and mapped into an outpost; an add-on patching the elevation code (NVDA, GHSA-727q-h8j2-6p45); an update checked only by a hash from the same server; a memory-safety bug in a parser.

### 5.7 Class L: leaking what the user reads and types

STRIDE: information disclosure. A screen reader's diagnostics are a record of the screen and keyboard; a flight recorder is a small version of the record Recall was criticised for. The threat is that what Verbatim stores, sends or shows reaches someone the user did not intend.

- Exits: logs, crash dumps (Verbatim's own and Windows Error Reporting's), the control pipe's speech stream and dumps, the clipboard, the network, the audio, and the physical display.
- Examples: a dump holding a password, attached to a public bug report; debug logging enabled on a secure screen (NVDA, GHSA-354r-wr4v-cx28); copied speech kept in Windows' clipboard history and synced; a person reading the screen over the user's shoulder; a screen curtain that silently stops covering the display.

### 5.8 Class A: losing speech through Verbatim's own parts

STRIDE: denial of service. Class U covers applications making Verbatim fail; this class covers Verbatim's own parts and environment failing, or being made to fail.

- Entry points: synthesizer engines, the audio device, the keyboard hook's registration, extensions, Core itself.
- Examples: an engine crashing on a string the user keeps arrowing onto, or at every start; another program holding the audio device in exclusive mode; Windows removing the keyboard hook after `LowLevelHooksTimeout`, a per-user value any program can lower; a module that saturates its outpost; Core crashing repeatedly on a hostile application until a restart breaker gives up.

## 6. Mitigations

### 6.1 Against class U

- **U1. Per-application isolation (structural).** One outpost per application (D9), a listener whose callbacks make no cross-process call (D13), a deadline on every call, abandoned workers replaced, failing outposts killed and restarted with backoff. A hostile application costs reading that application, not the screen reader.
- **U2. Walks have a hop cap, a time budget and, where identity exists, repeat detection.** A walk stops at an element it has already visited, keyed by runtime id for UIA and, for an MSAA object with an address of its own, by window handle, object id and child id, since a window object and its client share one handle. An MSAA object reached through `accParent` or `accNavigate` has no address and no usable COM identity (`docs/crates/verbatim-ia2.md`, "Identity"), so a walk through such objects is bounded by its hop cap and budget alone. The remote operation's search for a window above the walk (`crates/verbatim-uia-rops/src/focus.rs`) gets a cap of its own rather than relying on UIA's instruction limit.
- **U3. Lists are capped before they are fetched,** since the provider decides how much it returns and the cost is paid on fetching.
- **U4. Strings are capped where they are read, and a cut is known.** Each property is capped at the outpost, grapheme clusters are capped in length during segmentation, and a cut is spoken as NVDA treats one, once checked in the pinned source.
- **U5. Core's work grows at most as n log n in any one application input, or the input is capped first,** because Core is shared by every application. Terminal alignment (`crates/verbatim-text/src/terminal_rows/align.rs`) is quadratic in both rows and row width, and its character slots saturate past 65,535 distinct characters; cap the row count and width compared exactly, and compare beyond them by hash.
- **U6. Core verifies every claim an outpost makes that it can check (structural).** Node ids are stamped with the outpost of the pipe they arrive on, frames are capped before allocation, the raw section's ranges are checked, and a malformed frame retires the outpost without a panic in Core. A spare's self-reported process id is replaced by the one the supervisor holds from the spawn.
- **U7. Every queue an application can fill has a stated bound and overload policy,** including the background flood cap (architecture section 2), the outposts' outbound queues and Core's completion port.
- **U8. Outposts are bounded in number and spawn rate,** past which events wait for an outpost to retire.
- **U9. An application is named by a handle, not a number.** An outpost binds to the process the event came from through a handle opened when the event is observed, or a process id paired with its creation time, never a bare id read later, which can name a new process.

### 6.2 Against class J

- **J1. Each synthesizer adapter neutralises its engine's whole in-band syntax in one tested, fuzzed function:** SSML for OneCore and eSpeak NG, command sequences for engines that read them from text, and NUL and other control characters for every engine, so a NUL never fails an utterance.
- **J2. Pattern matching runs in linear time.** Only the `regex` crate, which never backtracks; rules needing lookaround are rewritten as two passes; patterns from users and extensions are compiled with a size limit.
- **J3. No text from an application is rendered as markup or code;** HTML, if ever shown, passes an allowlist sanitizer first. A standing review rule.
- **J4. A language model's output is speech and synthetic nodes only,** never commands, gestures or settings. A standing review rule.

### 6.3 Against class S

- **S1. Extensions cannot change speech in Verbatim's own windows, elevated windows or secure screens,** unless the user allows a module there by name or, on secure screens, allowlists it ([extensions](extensions.md), sections 2.4 and 4.6). Every overridden property names the layer and rule that set it, so an alteration can be found.
- **S2. Direction override and isolate characters** in names, file names and link text are spoken (NVDA's handling to be checked in the pinned source).
- **S3. A link's destination can be heard** through a command that leaves the clipboard alone, as NVDA added in 2023.

### 6.4 Against class C

- **C1. One control protocol on two endpoints.** A user endpoint, always on, offers the `.vext` install request (which opens Verbatim's own confirmation dialog and installs nothing by itself) and status. A developer endpoint, with key injection, the speech stream and the dumps, exists only while developer mode is on. Neither exists in the secure-desktop instance. In a build with UIAccess, developer mode is turned on in Verbatim's own settings window for the session only and never read from a file, so only someone at the keyboard can turn it on, since UIPI keeps lower-integrity input out of Verbatim's windows; there the endpoint judges its client's integrity by impersonating it, not from its process id, and refuses key injection while a window of higher integrity than the client is in front.
- **C1a. Developer mode is said at startup and never reaches secure screens.**
- **C2. Squatting a name never stops Verbatim, and a client never talks to an impostor.** The startup mutex and the replacement event live in a private namespace whose boundary names the user and the integrity level, which no other user or level can enter. The control pipe's name includes the session and user, which keeps other sessions and users off it; a lower-integrity process of the same user may still be able to create a pipe of that name, so a failure to create the pipe never stops Verbatim from starting (it runs without the control plane and says so), and a client checks the server it reached, by the server's process (`GetNamedPipeServerProcessId`): the same user, Verbatim's executable from the installation or the client's own copy, and an integrity level no lower than the client's. Clients already refuse a server in another session.
- **C3. Every parser of a file or message has a size limit and fails safe.** A malformed settings file means compiled-in defaults, said aloud, and no other folder read; Verbatim refuses to start if its own folder is unknown rather than using the current directory.
- **C4. Development tools are never reachable from the network unauthenticated, and never ship.** The test agent binds to loopback by default, the VM harness passes the guest's address and a random token made at deploy time, and the VM's firewall rule is scoped to the host's virtual switch.
- **C5. Keys injected from a lower integrity level never trigger a gesture and are never held or re-sent;** they pass down the hook chain untouched, so they are outside the rule that keeps keys in order behind a held key ([holding caret keys](key-holding.md)) and may overtake one, which costs only the injecting program's own ordering. The hook reads `LLKHF_LOWER_IL_INJECTED`, which costs nothing on its path. Keys injected at the same level still trigger gestures, as in NVDA. Verbatim injects only the keys it holds or passes on, and never types text from one application into another.
- **C6. No window message from a lower integrity level reaches Verbatim, and a lower copy never replaces a higher one.** The process-wide `ChangeWindowMessageFilter(WM_QUIT, MSGFLT_ADD)` in `crates/verbatim-app/src/single_instance.rs` goes. It exists so that a copy at a lower level, such as a portable or development copy started while an installed copy with UIAccess runs, can make it quit; instead such a copy finds the running one by its window, which every level can see, and refuses to start with a message that Verbatim is already running, so ending a UIAccess Verbatim takes its own menu or a copy at its level.
- **C9. The user endpoint takes requests, never commands.** Any program of the user can connect to it, as it can to any of the user's programs, so what limits such a program is what the endpoint offers: status, and an install request that carries only a path, which Verbatim opens and validates itself with the package reader's limits and shows in its own confirmation dialog. One install dialog is open at a time, and every request that arrives while it is open is refused. No method changes a setting or installs anything without the user's confirmation. Every message is parsed as untrusted (C3). The pipe carries an explicit mandatory label at the user's ordinary integrity level and no AppContainer access entry, so the install helper Explorer starts can reach it while Low-integrity and AppContainer processes cannot. Core's channels to its children are unnamed and inherited, so no other process can open them, and what arrives on them is treated as hostile (U6, X6).
- **C7. Nothing installs or loads an extension without the user's confirmation in Verbatim's own dialog,** and the developer folder is watched only in developer mode.
- **C8. Remote support is built for a world of support scams.** Traffic is encrypted end to end, not only to the relay; an active session is announced when it starts and at intervals; one local gesture ends it and cannot be remapped away; unattended access is off by default and approved each time; a certificate trusted once is checked on every connection; and when the session locks, the keyboard returns to the local machine.

### 6.5 Against class B

- **B1. Measure the UIAccess boundary first.** On the target Windows build, record what an ordinary process can do to a UIAccess Verbatim: open its process, duplicate its token, send its windows messages, register per-user COM classes (`HKCU\Software\Classes`) that its processes would activate, and whether its injected keys reach the hook marked `LLKHF_LOWER_IL_INJECTED`. The record goes in `experiments/`, and the other mitigations are adjusted to it.
- **B2. Verbatim never runs elevated on the user's desktop.** Started from an elevated prompt, it restarts with the user's ordinary token or refuses to start.
- **B3. Whether Windows is locked is asked of Windows when it is decided, and an unknown answer counts as locked,** from the session notifications and at startup. Every long-running channel reacts to locking: an updater installs nothing until someone signs in, and Remote returns the keyboard (C8).
- **B4. On the lock screen, only an allowlist of commands runs, and navigation stays inside it,** enforced in the reducer, which every command passes.
- **B5. The secure-desktop instance reads nothing a user can write.** Its configuration and the extensions the user allowlisted for secure screens are a copy an elevated step made in an administrator-owned folder, and the copy is an allowlist of plain data, never whole folders. It runs no control plane, developer folder, console or install path, and writes nothing.
- **B6. The secure instance's GUI and commands are an allowlist (structural).** A window or command needs a capability the secure instance does not grant, so no browser, file dialog, help file, URL, log viewer, error dialog or window that can start a process opens there. A denylist, checks scattered through each feature, is what each of NVDA's secure-screen advisories slipped past.
- **B7. Nothing of the user's session follows them onto a secure screen:** speech history, the last braille text and viewers clear on the switch and at exit.
- **B8. Every executable Windows starts for Verbatim on the secure desktop is administrator-owned and signed,** and the registration names an absolute path in the installation.
- **B9. Remote reaches a secure screen only with the local user's consent,** in both directions: no remote input there, and no speech from there mirrored to the peer, unless allowed; and no local program can stop a permitted session reaching it.

### 6.6 Against class X

- **X1. The installation is machine-wide, administrator-owned, signed, and contains no writable folder.** UIAccess requires a secure location, so there is no per-user install. Uninstalling removes the sign-in registration before the files. The secure-screen copy of settings and extensions is machine-wide, so the administrator who last made it chooses what every user's sign-in and UAC screens run, which the copying step says.
- **X2. DLL loading is pinned.** Each executable calls `SetDefaultDllDirectories` with the system and application folders only before anything else loads, and links rarely used system DLLs for delay loading; so does the installer.
- **X3. Configuration is data and never code (structural).** No setting names a DLL, an executable or a path. Every state file is parsed into one concrete type, never by a format that can name the type to build.
- **X4. Extensions are Wasm with capabilities derived from imports (structural).** No WASI, an epoch deadline per hook, a store limit per instance. Built-ins run in outposts; each installed extension runs in a host process of its own, started while one of its applications has an outpost and ended when none does, with Chromium's renderer profile (section 6.9), and the outpost enforces the extension's boundary and capabilities by the pipe a request arrives on, never by a module id the host claims. A hook costs a round trip and two wake-ups on the event path, and each read it makes two more; while the hook runs, the outpost's worker answers the host's reads, a waiting state the host's design counts with its other lifecycle states; M5 measures both.
- **X5. Compiled code is mapped only from where ordinary processes cannot write.** Built-ins arrive compiled in the administrator-owned installation; an installed extension's compiled code is loaded only by that extension's own host, so a planted file reaches nothing beyond that sandbox. Wasmtime trusts the code it maps, so this is the boundary. The exception is developer mode, whose developer-folder modules compile at load and run in outposts (section 7).
- **X6. Native code runs only in a contained synthesizer host (structural)**, and is a property of one kind: only native synthesizer modules (and braille drivers, section 6.11) carry a DLL. Their adapters load from the extension's own folder, which the user can write, so the container is the defence; the secure-screen copy is administrator-owned. The host checks its own token at start and Core checks the child's, so a failed sandbox fails closed. The container reads only what the engine needs and writes only a scratch folder of its own. Limits are set at creation, before engine code runs: no child processes, the job's user-interface limits, no extension points, no images from remote shares or written at Low integrity, and the window-manager system calls removed where the engine allows. Known engine releases are pinned by SHA-256 and an unknown one is announced. Core treats everything from the host as hostile: PCM lengths and formats, mark ids and the order of endings are checked, and a host that breaks them is ended.
- **X7. COM unmarshalling is hardened.** Core, the listener and the outposts call `CoInitializeSecurity` with `EOAC_NO_CUSTOM_MARSHAL`, so an application cannot have Windows load a custom unmarshaler into Verbatim through an MSAA object it hands over; tested with UIA, MSAA and IA2 together with the write-restricted token of section 6.9.
- **X8. The M7 helper is narrow at both ends.** It is loaded only into the processes that need it, never by a global hook, from the installation only; the outpost hands it its channel as a handle, with no named rendezvous; the outpost treats everything it sends as class U input, since the browser may be compromised; and a helper whose version does not match the installation is refused.
- **X9. Updates are verified by Authenticode signature against Verbatim's own certificate before they run.**
- **X10. The build and release are protected.** Dependencies audited with `cargo deny` in `cargo xtask ci`; the release signing key in hardware or a cloud key service, used only by a protected release job; workflows running code from pull requests get no secrets.
- **X11. Memory safety (structural).** Unsafe code stays in the Windows crates behind small safe wrappers; C and C++ that parse untrusted input run outside Core; Core does not panic on application data.
- **X12. Parsers and costly algorithms are fuzzed, highest exposure first:** the frame and raw-section decoders, terminal alignment and the control-plane reader, then the configuration and package readers, the synthesizer escapes and the reducer fed generated inputs; a `mockapp` mode serves hostile trees for U2 to U7.

What a malicious extension can still do inside these limits: a Wasm module reads the controls and text of the applications it claims and hears the keys it binds there, changes speech there, and degrades them; it has no network, files, clipboard or storage, so it has nowhere to send what it learns except speech, a message window and Verbatim's log, which are capped, and a future import that is an exit is shown together with what the module reads. A native synthesizer hears everything Verbatim speaks, which is its job, and can call any Windows function, but every DLL in the host shares the host's token, so what the container and limits allow is the whole of what it can do; its exits are the audio it returns, the pipe to Core, the kernel and the few services that accept such callers, the residue Chromium's renderers rely on. Its install warning says it receives all the text Verbatim speaks.

### 6.7 Against class L

- **L1. Dumps keep their text, and their life is controlled.** A dump is the user's private data: written only where L4 says, never sent anywhere by Verbatim, never written by the secure-desktop instance, and the user is told what it holds before attaching it to a report.
- **L2. Input from a protected field is recorded as stars,** which the reducer speaks anyway, so replay loses nothing.
- **L3. Logs carry no content at their default level;** content at a debug level only in developer mode, never on secure screens.
- **L4. Diagnostics have a home, an owner and an end:** the user's local application data for installed copies, beside the copy for portable ones, readable by the user only, capped in count and age.
- **L5. Native crashes are dumped by Verbatim, never by Windows Error Reporting.** Core, the listener, the outposts and the synthesizer host each install a handler for native exceptions that writes a minidump into Verbatim's dumps folder, the synthesizer host into a file handle Core passes it at launch, and then reports the exception handled, so Windows' reporting never runs and nothing is uploaded, as NVDA's crash handler does. The secure instance writes no dump but still reports the exception handled.
- **L6. Verbatim makes no network connection except the updater and Remote (structural).** No telemetry or crash upload; the update check identifies no one and shows what it sends; cloud recognition, if ever offered, is opt-in per use.
- **L7. Speech copied to the clipboard from a protected field** is marked to stay out of clipboard history and cloud sync.
- **L8. The screen curtain is trustworthy to someone who cannot see it.** Built as NVDA builds it, with the Magnification API's full-screen colour effect. It is a global setting no profile changes; nothing turns it off but the user's command; a command reports its state without toggling it; its state is said when toggled and at every start; it is re-applied and confirmed after a display change, unlocking and a Remote Desktop reconnection, and first thing on starting after a crash; a failure to apply it is spoken. Whether the effect also darkens screen sharing, Remote Desktop and screenshots is measured and stated; the secure desktop needs its own curtain.
- **L9. Echo of typing fails closed.** A character is judged against the focus that held it when the key was pressed, ordered by the key number, and masked while a focus change is in flight; the protected state comes from the focus Core holds, never from a query on the key's path; a word buffer is cleared on every focus change and kept only in text inputs; an application whose password fields are not marked protected gets a rule in its built-in module. Terminals already echo only what the terminal shows (`docs/design/terminal-model.md`, rule 20).

### 6.8 Against class A

- **A0. Core does not crash.** Prevention comes first: no panic on application data (X11), fuzzing (X12), bounded input (class U), and a panic treated as a defect to fix, not a path to recover through.
- **A1. Core restarts after a crash, with a breaker that degrades rather than stops.** Verbatim's own crash handling owns the restart: the native exception handler of L5, after writing its dump, and the panic hook start a new Verbatim, as NVDA's crash handler does, announced on restart. Windows' restart manager is not used, since it runs through Windows Error Reporting, which L5 keeps from running. After a few crashes in a short window, Verbatim restarts once more with extensions off and without the outpost of the application last in front, and only then stops.
- **A2. Core has a memory cap,** a job of its own, so unforeseen growth ends in a restart.
- **A3. A synthesizer failure costs one utterance, and repeated failures cost the synthesizer, never speech.** After three crashes within a minute, eSpeak NG speaks for the session and says why, OneCore if eSpeak NG fails; failures J1 prevents are not counted. A host that keeps failing is given up for the session.
- **A4. The keyboard hook's loss is detected without a timer,** from evidence Verbatim already receives, such as focus or caret changes the listener reports for keyboard input while the hook has seen no key; the signal is chosen in the build, and the check is deferred if none proves reliable.
- **A5. Speech moves to another audio device** while the default refuses a shared stream.
- **A6. Every extension has a memory and processor budget,** enforced by its store and, for an installed extension, by its host's job, with a memory cap and a hard processor rate cap, since nothing in it is real-time. A share of the outpost worker's time per module over a window is deferred until a module needs it.
- **A7. A synthesizer host that uses processor time with nothing to say is ended,** judged from its job's processor time read when an utterance ends and at the next request, never by a timer; a hard cap would make real-time audio stutter. A host spinning while idle therefore runs on until the next speech, which is accepted. The host never runs above normal priority.
- **A8. Verbatim can start with every extension off,** by a key held at startup or a command-line switch, saying so, and the Extensions dialog disables one extension at a time.

### 6.9 Sandboxing each process

From how Chromium, Firefox, the Codex CLI and Windows' app isolation sandbox their processes (the survey, "Sandboxing"):

- **No process that reads applications can be an AppContainer or run below the user's ordinary integrity level.** UIA and MSAA begin every connection with a `WM_GETOBJECT` message, which UIPI drops from a lower level, and AppContainers cannot have UIAccess. So the listener and outposts run at the user's level, raised by UIAccess from M8.1, and are hardened by mitigations, not by token level.
- **Outposts and the listener.** Control-flow guard, strict handle checks, the image load policy, no child processes, and the job's limits on the clipboard, atoms, system settings and desktop switching; never the job's limit on other processes' window handles, the removal of window-manager calls or the Microsoft-only image policy, which UIA, MSAA and IA2's proxy DLLs need. A write-restricted token, as the Codex CLI uses, is adopted if an experiment shows UIA events and reads, MSAA, a remote operation and later IA2 all work under it, against `mockapp` and Notepad. Outposts need no network, so the installer adds a filtering rule blocking it for `verbatim-outpost.exe`.
- **Installed extensions' hosts.** Chromium's renderer profile: the strictest restricted token, untrusted integrity, no window-manager calls, one process in the job, no network.
- **Synthesizer hosts.** The less privileged AppContainer with X6's limits; the no-generated-code mitigation per engine; never the Microsoft-only image policy, as Chromium exempts its speech process. Where an engine fails in the container, Chromium's GPU profile (a limited token at low integrity) is the fall-back to measure beside the ordinary AppContainer; a 32-bit engine under x86 emulation in the container is M6's first measurement.
- **Core.** It holds the keyboard hook and, from M8.1, UIAccess, so it keeps its integrity level; it drops every privilege it does not use, enables strict handle checks and control-flow guard, and passes children only the handles in their attribute lists. The updater runs in a short-lived process of its own, keeping Core off the network until Remote.
- **Per architecture.** Hardware shadow stacks exist only on x64, so ARM64 and the x86 host under emulation each get their own measured list.
- **Signing and App Control.** Every binary is signed; an optional App Control supplemental policy lets an administrator choose which engine DLLs load.
- **Not adopted:** an interception broker, which Chromium says gives compatibility, not security; a private desktop, since Verbatim must see the user's; Win32 app isolation, which has no UIA.

### 6.10 Portable copies

A portable copy runs from any folder with its settings, logs and dumps beside it. It is a less valuable target by construction: Windows grants UIAccess only to signed programs in a secure location, and only the installer registers for sign-in, so a portable copy holds the user's rights and nothing more. X2 keeps a DLL planted beside it from loading; its configuration names no code; it loads no extension it did not install through its own confirmation; it leaves nothing of the user's in a borrowed machine's profile, the compiled-code cache included; it never runs elevated; and its updates are verified by signature.

### 6.11 Braille display drivers (M15)

A sandbox cannot open devices by itself, but a handle opened outside keeps working inside, and Chromium never hands a sandboxed process a device handle (the survey, "Braille drivers"). Each driver uses one of two models, chosen per driver:

- **A. Core opens the device and hands the handle to a contained driver,** after a test confirms handle passing; the driver can send the device any control code the handle allows.
- **B. Core does all device I/O, and the driver is a protocol parser fed bytes,** a Wasm component holding no handle, with Core refusing reports and control lines the driver has no use for.

A vendor SDK that needs its own kernel driver or service gets a weaker container, built only when someone first needs one, opt-in and labelled as trusted native code. In every model the driver treats the device's bytes as hostile, and a display's keys produce only the gestures the driver interface defines.

### 6.12 Verbatim's own GUI

The GUI runs on a thread in Core (D4), which holds the keyboard hook and, from M8.1, UIAccess, so whatever loads code into a dialog loads it into Core, and whatever drives a dialog drives Verbatim.

- **G1. No file choice on secure screens.** The system's file dialog loads Explorer's shell extensions, which are DLLs the user or an administrator installed, into the process that shows it, and can browse to and start programs, which is how several of NVDA's secure-screen escapes reached a SYSTEM shell. In the user's session Verbatim keeps the standard dialog in Core, where its shell extensions are the user's own code, as input methods are (G5), and B1 records whether a UIAccess Core gives them more than Windows already gives such code. The secure-desktop instance offers no file choice at all.
- **G2. Rich content in dialogs is a feature, and what applications supply is sanitized.** In the user's session, dialogs keep HTML views, links and help buttons. Text that comes from an application or an extension is never shown as active content: HTML from either passes an allowlist sanitizer first, and links open only web addresses in the default browser (J3). On secure screens none of these exist (G4), and wxWidgets' own log and assertion dialogs are routed to the log there, since NVDA turned off its toolkit's error dialogs after one led to a file dialog on a secure screen.
- **G3. Only someone at the keyboard drives a confirmation that matters.** Before UIAccess a program of the same user can press buttons in Verbatim's dialogs, as it can in any of the user's programs (section 4). From M8.1, UIPI keeps lower-integrity input and messages out of Verbatim's windows, so the extension install confirmation and turning on developer mode (C1) can only be done by the user.
- **G4. The secure instance's menu and dialogs are an allowlist** (B6). Reading settings the user needs at a sign-in or UAC prompt, such as rate, volume and voice, can be changed for the session in memory and are never saved; no settings page that touches files, profiles, extensions, logs, updates, developer mode or the installer exists there; there is no Extensions dialog, no help, no links and no clipboard.
- **G5. Input methods are the user's own code.** Text fields in Verbatim's dialogs load the user's chosen input methods into Core, as every program's do; in the user's session that is code the user installed, and for the secure desktop B1 records which input methods Windows loads there.

The GUI stays a thread in Core for now (Dickson, 2026-10-31); moving it to a process of its own, keeping wxWidgets, input methods and shell extensions out of the process that holds the hook and UIAccess, is reconsidered if B1 shows that code loaded into Core gains more than Windows already gives it.

## 7. Decisions and accepted risks

Decided by Dickson on 2026-10-31:

- The control plane's two endpoints (C1), lower-integrity injected keys ignored (C5), passwords in dumps recorded as stars (L2), per-user state in the profile for installed copies (4.1), synthesizer fallback (A3), the developer folder only in developer mode (C7), direction overrides spoken (S2), speech moved to another device (A5), engine binaries pinned (X6), installed extensions in hosts of their own (X4), the write-restricted token experiment (6.9), braille drivers in model A or B (6.11), the extension surface's rules ([extensions](extensions.md), section 4.6), and developer mode with the fixes that need no milestone of their own in the M6.6 security-fixes milestone.
- Not adopted: a guardian process holding the screen curtain across a crash, since not crashing is the answer; eSpeak NG compiled to Wasm inside its container, which the container makes unnecessary; a check of who can write a portable copy's folder; a per-module share of worker time until a module needs it.

Accepted risks:

- An application can lie inside its own window, labelling a Delete button "OK", as it can to a sighted user.
- An extension can remove the protected state where an application sets it wrongly, and so from a real password field; the provenance names the module that did it.
- Applications can tell that a screen reader is running, which is intended.
- With developer mode on in a UIAccess build, the developer endpoint's speech stream and dumps carry elevated windows' content to the developer's tools; its check for a higher-integrity window in front is made when a request arrives, so the foreground can change before the key is injected; and developer-folder modules compiled at load run in outposts (X5).
- A same-user process can do to Verbatim whatever it can do to any program of the user, until UIAccess, and after it whatever Windows' own UIAccess token allows (4.1).

## 8. How well the architecture serves the goal

The architecture is strongest where screen readers have historically been weakest, and its gaps are in product surfaces not yet built.

- **Strong, by structure:** per-application isolation and an outpost boundary Core checks (U1, U6), so a hostile application costs reading that application only, which NVDA's single main thread cannot offer; a pure reducer with no COM, fuzzable on Linux; private channels to children; Wasm extensions with derived capabilities and installed extensions in their own sandboxes; native engines contained out of process; no injection until M7; unsafe code confined to the Windows crates; configuration that names no code; and no network code.
- **Gaps, all fixable without changing the architecture:** control surfaces on by default and broad, named objects that can be squatted, and a test agent open to the network; a file layout that assumes a writable executable folder; diagnostics with no owner or end; unbounded strings, no repeat detection and a quadratic step in Core's terminal code; no restart or fallback when Core, a synthesizer, the audio device or the hook is lost; COM unmarshalling unhardened; no fuzzing or dependency audit. The secure-screen and lock-screen surfaces are not built at all, which is the chance to build them from allowlists rather than repair them advisory by advisory.
