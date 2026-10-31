# Security model

Proposed on 2026-10-31, pending Dickson's decisions in section 1. Decided by Dickson on 2026-10-31: crash dumps keep their text, and developer mode, which gates the developer surfaces below, comes in an M6.x milestone after M6.

This document is Verbatim's threat model, followed by the architectural mitigations it calls for. Sections 3 to 6 are the threat model: what is protected, where untrusted data and control enter, who the adversaries are and which of them cross a real boundary, and the classes of threat. Section 7 gives the mitigations for each class, each with its status in today's code. Sections 8 to 10 judge the architecture, rank the findings and place the work in the roadmap, and section 11 compares Verbatim with NVDA and other screen readers. It is a model, not a build plan: each build that takes up a mitigation gets the usual design and adversarial review.

## 1. Decisions required

1. **Control plane.** Split it into a narrow user endpoint that is always on (the extension install request and status) and a developer endpoint (key injection, the speech stream, tree and recorder dumps) that exists only in developer mode (M6.x). Should signed builds with UIAccess carry the developer endpoint at all, given that its key injection would reach elevated windows? Recommended: split, and keep the developer endpoint in signed builds behind developer mode, refusing `SendKeys` while a window of higher integrity than the client is in front, so end-to-end tests can still run against a release build.
2. **Injected gestures.** Ignore Verbatim gestures in keys injected by a lower-integrity process, and keep accepting keys injected at the same level, as NVDA does and users' scripts rely on? Recommended: yes.
3. **Passwords in dumps.** Dumps keep their text (decided). Record characters typed into a protected field as stars, which the reducer speaks anyway, so replay loses nothing? Recommended: yes.
4. **Installed state.** Settings, logs and dumps under the user's profile for installed copies, and beside the executable for portable copies, as NVDA does? Recommended: yes.
5. **Synthesizer fallback.** After three crashes within a minute, switch to eSpeak NG for the session and say so? Recommended: yes.
6. **Developer folder.** Watch the extension developer folder only in developer mode? Recommended: yes.
7. **Behaviour users hear.** Two mitigations change what is heard: speaking text-direction override characters (S2), and playing through another audio device while the default refuses Verbatim (R5). Adopt them? Recommended: yes.

## 2. Method

The threat model follows the usual shape: model the system and its trust boundaries, list the entry points where data or control crosses them, name the adversaries, enumerate threats by class, then choose mitigations and state what risk remains. The classes in section 6 are adapted from STRIDE (spoofing, tampering, repudiation, information disclosure, denial of service and elevation of privilege), regrouped around how a screen reader is attacked: most of its input is other programs' content, its output is the user's only view of the screen, and it holds the privileges of an assistive technology. Each class names the STRIDE categories it covers.

The working principle for mitigations is to remove attack surface by architecture or by product choice first, and to add checks only where no structural answer exists. A check guards one path; a surface that does not exist cannot be attacked by any path.

## 3. Assets

In priority order:

1. **Availability.** A silent or hung screen reader leaves a blind user without a computer, so denial of service is a security failure here, not a nuisance.
2. **Privilege boundaries.** Verbatim must never let a less trusted process reach something it could not reach on its own. With UIAccess (M8.1) Verbatim can read elevated windows and send them input, and its secure-desktop instance runs as SYSTEM.
3. **The user's content.** Verbatim sees everything on screen, every keystroke and the clipboard. What it keeps, where, and who can read it is the privacy question raised by Windows Recall.
4. **What the user hears.** For a blind user, the speech is the interface. Whoever controls what Verbatim says can deceive the user in ways a sighted user would notice at once.
5. **The user's machine.** Verbatim runs with the user's rights, and later with UIAccess and at sign-in, so code execution inside Verbatim is code execution on the machine.

## 4. System, trust boundaries and entry points

Verbatim's processes are Core (`verbatim.exe`), the focus listener, one outpost per application, and one synthesizer host per engine (architecture section 1). Core talks to its children over unnamed pipes passed by handle. The trust boundaries are:

- between each application and the process that reads it (the listener or its outpost);
- between an outpost and Core, since an outpost is only as trustworthy as the application it reads, and in M7 it loads IA2 proxy code and talks to an injected helper;
- between a synthesizer engine and Core;
- between Verbatim and other processes of the same user, which section 5 shows is not a real boundary until UIAccess;
- between Verbatim and lower-integrity processes, which is real;
- between Verbatim and elevated windows and secure screens, which Verbatim is meant to cross on the user's behalf only;
- between the machine and the network.

The entry points, where data or control from outside Verbatim enters it:

- **Accessibility APIs.** UIA and MSAA events and every property, text and tree read, in the listener and the outposts; later IA2, Java Access Bridge and the injection helper.
- **Text that applications show.** Web pages, documents, chat, file names and terminal output, including output from remote programs over SSH. This is how network content reaches Verbatim.
- **The keyboard hook.** Every key on the desktop, physical or injected.
- **Window messages** to Verbatim's windows.
- **The control pipe** and, during development, the test agent's TCP port.
- **Files Verbatim reads.** Settings, profiles, themes, sounds, locale files, eSpeak NG's data, extension packages and module folders.
- **Synthesizer engines,** their output and their failures.
- **The audio device,** which another program can take away.
- **The clipboard,** read and written.
- **Named objects:** the startup mutex, the replacement event and the control pipe's name.
- **Planned:** the updater and installer (M8), the secure-desktop launch (M8.1), Remote support (M12), and image description or OCR services.

## 5. Adversaries

Each adversary is listed with whether a real boundary separates it from Verbatim, because effort spent defending a non-boundary is wasted.

- **The application being read, and whoever controls its content.** Any application, web page or remote program whose output the user reads. Most often not malicious but broken: cyclic parents, endless sibling chains, huge child counts, multi-megabyte names, event floods and hung providers. A real boundary, and the most important one.
- **A process of the same user at the same integrity level.** Windows offers no boundary here. Such a process can open Verbatim's process with full access, write its memory, load a DLL into it, inject input and read every window through UIA itself. Defending Verbatim's settings, logs or pipe against it gains nothing, since it can do directly whatever it would do through Verbatim.
- **A lower-trust process.** Low integrity, AppContainers such as browser renderers and Store apps, and sandboxed code. A real boundary below Verbatim, which User Interface Privilege Isolation (UIPI, the Windows rule blocking window messages and injected input from a lower integrity level to a higher one) mostly enforces already.
- **A higher-privilege context, reached through Verbatim.** Elevated applications and secure screens. Here the adversary is any of the above using Verbatim as its deputy.
- **An extension's author.** Malicious, compromised or, most often, buggy. Section 9 of [extensions](extensions.md) holds this threat model in detail.
- **A synthesizer engine.** Closed native code that crashes or hangs on particular text, such as Eloquence's known crash strings.
- **A network attacker.** Today only the test agent listens. Later, the update channel and Remote support's relay and peers.
- **A remote supporter,** in M12: someone the user lets in, who may be a scammer.
- **Whoever builds or ships Verbatim's code:** dependencies, the build machines and the release key.

### 5.1 Can Verbatim defend its configuration?

This is the question of a program editing Verbatim's configuration, and it follows from the adversaries above:

- **Files under Program Files are protected already.** Only an administrator can write there. The defence is free as long as Verbatim never makes any folder of its installation writable by the user. Today Verbatim writes `settings.toml`, `logs` and `dumps` beside its executable, so an installed copy must move them to the user's profile. Making the installation folder writable instead would let any program plant a DLL or replace a binary that Windows starts with UIAccess, or as SYSTEM on the sign-in screen.
- **Settings in the user's profile cannot be protected** from that user's programs, and need not be. They must be data and never code (mitigation X3), so editing them can change how Verbatim behaves but cannot run code or cross a boundary.
- **UIAccess changes the picture, but not completely.** Windows starts a UIAccess process at a raised integrity level: the caller's level plus 16 for a standard user, and High for an administrator's split token, so ordinary programs cannot write its memory. Then everything that steers Verbatim (its pipe, injected keys, window messages, configuration and extensions) becomes a way for them to borrow its reach into elevated windows. But James Forshaw showed in 2019 that an ordinary process can duplicate the token of any running UIAccess process, such as the on-screen keyboard, and start its own, and called UIAccess "not a security boundary". So the goal is that Verbatim adds no easier path than Windows already has, and M8.1 starts by measuring what an ordinary process can still do to a UIAccess Verbatim (mitigation B1).

## 6. Threat classes

Each class states the threat, its entry points, examples (from Verbatim's code, from other screen readers' history in section 11, or from reasoning), the assets at risk, and where Verbatim is exposed today. Mitigations are in section 7 under the same letter.

### 6.1 Class U: hostile structure and volume in accessibility data

STRIDE: denial of service, and tampering with what is read.

An application's accessibility tree and events are input of arbitrary shape and size. The threat is that their shape or volume makes Verbatim hang, exhaust memory or repeat itself, either in the process reading that application or, worse, in Core, which every application shares.

- Entry points: accessibility APIs; text applications show.
- Examples: a parent that returns itself, cycles of parents, next and previous siblings that do not agree, a list of a million children, a name of 30 MB, a 100,000-character line, thousands of combining marks forming one grapheme, a flood of notifications, a provider that never answers, a program that starts many short-lived processes each raising events, a wide terminal row of distinct characters printed by a remote program.
- Assets: availability first, then what the user hears.
- Exposure today: walks are capped by hop count and time but detect no repeat, so a self-parent is reported as up to 64 identical ancestors. The classic UIA child listing fetches all children before capping. Names, values and descriptions are uncapped up to the 32 MiB frame, and the 64 KiB cut of a text read is silent. Terminal alignment in Core is quadratic in row width and allocates memory in proportion to distinct characters times width. The background flood cap is not built, Core's completion port has no count bound, nothing bounds the number of outposts, and Core has no memory cap.

### 6.2 Class J: content interpreted as instructions

STRIDE: tampering, elevation of privilege, denial of service.

Text from applications passes through components that interpret some text as commands: synthesizers, pattern matchers, markup renderers and, later, language models. The threat is that crafted text is executed rather than read.

- Entry points: text applications show, flowing to the speech pipeline, dictionaries, dialogs and future recognition services.
- Examples: SSML or an engine's command sequences in a web page changing the voice, rate or volume or reaching an engine's crash paths; a regular expression that backtracks catastrophically on a long line; NVDA's HTML message dialog turning web text into code run on two key presses (GHSA-xg6w-23rw-39r8); on-screen text instructing a model that describes the screen.
- Assets: what the user hears, availability, the machine.
- Exposure today: both built-in drivers escape XML, but differently: OneCore's escape replaces control characters, and eSpeak NG's does not. No dictionaries, rendered markup or models exist yet. The roadmap's M8 dictionary work mentions rules written with lookarounds, which the `regex` crate, linear-time because it never backtracks, does not support.

### 6.3 Class S: spoofed or altered speech

STRIDE: spoofing.

The threat is that an extension or crafted text changes what Verbatim says, so that the user hears something other than what the application presents. An application lying about its own controls is out of scope (section 7.3).

- Entry points: extensions' rules and hooks, Unicode in names and text.
- Examples: an extension that makes a Delete button speak as OK; a file name whose direction override makes an executable read as a PDF; zero-width characters hiding words from speech that the screen shows.
- Assets: what the user hears, and through it every other asset.
- Exposure today: Unicode format and direction characters are spoken as the synthesizer handles them. Extensions do not exist yet; their design already refuses claims on Verbatim's own executables.

### 6.4 Class C: steering Verbatim through its control channels

STRIDE: spoofing, tampering, denial of service, elevation of privilege.

Verbatim accepts control from keys, its pipe, window messages, files and, later, remote peers. The threat is that a process that should not control Verbatim does, either to make it act (inject keys, install an extension) or to stop it.

- Entry points: the keyboard hook, the control pipe, the test agent, window messages, named objects, files Verbatim watches, Remote support.
- Examples: a Low-integrity process ending Verbatim with `WM_QUIT`; any program triggering Verbatim gestures with `SendInput`; a program dropping a module into the extension developer folder, which loads without a prompt; anyone on the network reaching the test agent; a second signed-in user's Verbatim unable to create its control pipe because the name is taken; a remote "supporter" who is a scammer.
- Assets: availability, the machine, privilege boundaries once UIAccess exists.
- Exposure today: `verbatim-agent` binds every interface with no authentication and can launch any process and read, write or delete any file, and the VM provisioning opens a firewall port for it. The control pipe is always on, offers `SendKeys`, the speech stream, dumps and `Quit` to any process of the user, reads lines of any length, and has a fixed name global across sessions. Its security descriptor admits the owning user only, it refuses remote clients, and its first instance claims the name. Gestures accept injected keys from any integrity level. `ChangeWindowMessageFilter(WM_QUIT, MSGFLT_ADD)` admits `WM_QUIT` from any level to every window of the process. Verbatim's own injected keys are marked by fixed constants any program can copy.

### 6.5 Class B: crossing privilege boundaries through Verbatim

STRIDE: elevation of privilege, information disclosure.

Verbatim reads and drives contexts more privileged than the programs around it: elevated windows (with UIAccess), the lock screen in the user's session, and the sign-in and UAC screens, where Windows runs it as SYSTEM. The threat is that a user or program uses Verbatim to reach those contexts, or to carry information across them.

- Entry points: everything in class C, plus the GUI and commands available on secure screens and the lock screen.
- Examples, mostly from NVDA's record: a dialog on a secure screen (the log viewer, context help, a clipboard error, a formatting dialog) leading to a file dialog, a console or a SYSTEM shell; object navigation walking out of the lock screen; the lock state judged from the foreground window, which another window can take over; a restart on the lock screen assuming it is unlocked; the speech viewer's or braille display's contents carried onto a secure screen; starting elevated after an elevated install; replacing or redirecting the binary Windows starts at sign-in (MITRE ATT&CK T1546.008).
- Assets: privilege boundaries, the user's content.
- Exposure today: none of these contexts is reached yet: there is no UIAccess, secure-desktop instance or `--secure`. Started from an elevated prompt, Verbatim runs elevated, since its manifest asks to run as the invoker.

### 6.6 Class X: running code that is not Verbatim's

STRIDE: tampering, elevation of privilege.

The threat is code running inside Verbatim's processes, or with their rights, that Verbatim did not intend: by memory corruption, a planted file, a configuration that names code, a malicious extension or engine, a tampered update, or a compromised dependency or build.

- Entry points: files Verbatim reads and loads, extension packages, synthesizer engines, the updater, the build, and every parser of untrusted data.
- Examples: a DLL planted beside a portable copy in a Downloads folder (NVDA's installer had this, GHSA-qf5h-qw92-rx2f); a writable installation folder; an add-on patching the screen reader's elevation code (NVDA's GHSA-727q-h8j2-6p45); an update checked only by a hash from the same server; a memory-safety bug in a parser of application data.
- Assets: the machine, and every other asset with it.
- Exposure today: no executable restricts its DLL search or sets process mitigations. Configuration names no code. Synthesizers are compiled in and run in a host process without a sandbox yet. Extensions do not run yet. Unsafe code is confined to the Windows crates; eSpeak NG's C runs in the synthesizer host. There is no fuzzing and no dependency audit. The wxWidgets download is pinned by SHA-256 and submodules by commit.

### 6.7 Class L: leaking what the user reads and types

STRIDE: information disclosure, and repudiation in the sense of records the user cannot see or control.

A screen reader's diagnostics are a record of the screen and keyboard. The objection to Recall was a continuous, searchable record of the screen kept on disk by default; a flight recorder is a smaller version of the same thing. The threat is that what Verbatim stores or sends reaches someone the user did not intend.

- Entry points (as exits): logs, crash dumps, the control pipe's speech stream and dumps, the clipboard, the network, the audio itself.
- Examples: a dump holding a password typed into a password field, attached to a public bug report; debug logging enabled on a secure screen (NVDA's GHSA-354r-wr4v-cx28); NVDA's IO log recording every utterance and key; copied speech kept in Windows' clipboard history and synced between devices; a cloud recognition service receiving the screen; VoiceOver reading saved passwords aloud.
- Assets: the user's content.
- Exposure today: logs carry no content at their default level. The flight recorder holds 1,024 inputs or 8 MiB and is written to `dumps` beside the executable on every panic, kept forever, with typed characters unmasked, including those in protected fields. Child logs keep 10 launches with no size cap. Verbatim makes no network connection outside the test agent.

### 6.8 Class R: losing speech through Verbatim's own components

STRIDE: denial of service.

Class U covers applications making Verbatim fail. This class covers Verbatim's own parts and environment failing, or being made to fail, with the same result: silence.

- Entry points: synthesizer engines, the audio device, the keyboard hook's registration, Core itself.
- Examples: an engine crashing on a string the user keeps arrowing onto, or at every start; another program holding the default audio device in exclusive mode or muting Verbatim's session; Windows silently removing the keyboard hook after `LowLevelHooksTimeout`, a per-user registry value any program of the user can lower; another program's hook, installed later, swallowing Verbatim's modifier; a panic in Core.
- Assets: availability.
- Exposure today: a synthesizer host that crashes or stays silent for 10 seconds fails one utterance and restarts on the next request, with no cap on restarts and no fallback to another synthesizer after startup. The WASAPI device retries the same default device while it is held. Verbatim prevents its own hook timeout but does not detect losing the hook. A panic writes a dump and Verbatim is gone.

## 7. Architectural mitigations

Each mitigation states what holds, why, and its status today. Mitigations that are structural, a property of the architecture rather than a check, are marked so, because they are the ones that hold against attacks nobody has thought of.

### 7.1 Against class U

- **U1. Per-application isolation (structural, holds).** One outpost per application (D9), a listener whose callbacks make no cross-process call (D13), a deadline on every call, abandoned workers replaced, and failing outposts killed and restarted with backoff. A hostile application costs reading that application, not the screen reader. NVDA, whose one main thread runs everything, has no equivalent.
- **U2. Walks have a hop cap and a time budget, and stop at an element already visited.** Status: caps and budgets hold (64 hops in the outpost and the listener, `ENRICHMENT_BUDGET` and the deadlines); repeat detection is missing. The remote operation's search for a window above the walk (`crates/verbatim-uia-rops/src/focus.rs`, the second loop of the focus program) has no cap of its own and stops only at UIA's instruction limit, after which the read falls back to the classic path; give it one. Detect repeats by runtime id for UIA and by window handle for MSAA, because an MSAA object's COM identity does not match across reads (`docs/crates/verbatim-ia2.md`, "Identity").
- **U3. Lists are capped before they are fetched.** Status: the MSAA child list (10,000) and dumps (4,096 nodes) hold; the classic UIA child listing (`children_from` in `crates/verbatim-uia/src/client.rs`) caps after fetching.
- **U4. Strings are capped where they are read, and a cut is known.** Each property is capped at the outpost, for example at the text chunk's 64 KiB, grapheme clusters are capped in length during segmentation, and Core speaks a cut as NVDA does once that is checked in the pinned source. Status: text reads hold their cap and set `truncated`, which Core does not use for speech; properties are uncapped.
- **U5. Core's work grows at most as n log n in any one application input, or the input is capped first.** Core is shared by every application, so one costly input silences everything. Status: terminal alignment in `crates/verbatim-text/src/terminal_rows/align.rs` breaks this. Cap the row width compared exactly, and above it compare rows by hash.
- **U6. Core verifies every outpost claim it can check (structural, mostly holds).** Node ids are re-stamped with the outpost the pipe belongs to, frames are capped at 32 MiB before allocation, the raw section's ranges are checked, and a malformed frame retires the outpost without a panic in Core. A spare's self-reported process id is forwarded unverified; use the one the supervisor holds from the spawn.
- **U7. Every queue an application can fill has a stated bound and overload policy.** Status: the outpost's intake applies NVDA's limiter and terminal output waiting in Core is bounded at 10 MB; the background flood cap (architecture section 2) and a count bound on the outposts' outbound queues and Core's completion port are missing.
- **U8. Outposts are bounded in number and spawn rate.** A global cap and a rate, past which events wait for an existing outpost to retire. Status: missing; two spares wait and one application's failures back off, but nothing bounds outposts across applications.

### 7.2 Against class J

- **J1. Each synthesizer adapter neutralises its engine's whole in-band syntax in one tested function.** That covers SSML for OneCore and eSpeak NG, and command sequences for engines that read them from text, such as Eloquence's annotations. Status: both built-in drivers escape XML; eSpeak NG's escape passes control characters through, unlike OneCore's.
- **J2. Pattern matching runs in linear time.** Only the `regex` crate, maintained by the Rust project though not part of the standard library, which never backtracks and so runs in time linear in the text; rules that need lookaround are rewritten as two passes; patterns from users and extensions are compiled with a size limit. Status: no dictionaries yet (M8).
- **J3. No text from an application is ever rendered as markup or code.** Dialogs and viewers show application text as plain text. Status: holds; keep it as a review rule.
- **J4. A language model's output is speech and synthetic nodes only.** Never commands, gestures or settings, because text on screen can instruct a model (prompt injection). Status: no models yet; a standing review rule.

### 7.3 Against class S

- **S1. Extensions cannot change speech in Verbatim's own windows, elevated windows or secure screens.** Status: the extension design refuses claims on Verbatim's own executables and limits secure screens to an administrator-made copy; add elevated applications, unless the user allows a module there by name. Every overridden property names the layer and rule that set it (extensions design, section 7), so an alteration can be found.
- **S2. Unicode cannot hide what text says.** Direction override and isolate characters in names, file names and link text are spoken (NVDA's handling is to be checked in the pinned source). Status: missing.
- **Accepted risk.** An application can lie inside its own window, labelling a Delete button "OK", exactly as it can to a sighted user. Verbatim does not try to detect it.

### 7.4 Against class C

- **C1. No control surface is on by default unless a user feature needs it, and each exposes only what that feature needs.** Two endpoints: a user endpoint, always on, offering the `.vext` install request from the M5 helper (which opens Verbatim's own confirmation dialog and installs nothing by itself) and status; and a developer endpoint, with everything else, existing only while developer mode is on. Neither exists in the secure-desktop instance. Question 1. Status: one endpoint, always on.
- **C2. The control pipe is named per session.** Include the session id, as the startup mutex already lives in the session's `Local` namespace; NVDA names its endpoint per desktop. Status: a fixed global name.
- **C3. Every parser of a file or message has a size limit and fails safe.** On a malformed settings file, run on compiled-in defaults without reading another folder, and say so; refuse to start if the executable's folder is unknown. Status: settings, profiles, themes and the pipe's lines have no size limit; a malformed `settings.toml` makes Verbatim load from `%TEMP%\verbatim-defaults`, a folder any process can write to; `exe_dir` falls back to the current directory.
- **C4. Development tools are never reachable from the network unauthenticated, and never ship.** The agent binds to loopback by default, the VM harness passes the guest's address explicitly, a random token made at deploy time and passed by `cargo xtask` is required, and the VM's firewall rule is scoped to the host's virtual switch. Status: missing (finding F1); the agent is in no installer.
- **C5. A gesture injected by a lower-integrity process is ignored; one injected at the same level is accepted.** The hook reads `LLKHF_LOWER_IL_INJECTED`, which costs nothing on its path. Same-level injection stays, because it matches NVDA, users' AutoHotkey scripts rely on it, and such a process has equal power anyway. Once Verbatim runs with UIAccess above the user's ordinary level, ordinary processes' keys should carry the flag too; B1 verifies that. Question 2. Status: every injected key drives gestures.
- **C6. Verbatim's own injected keys are recognised by more than a constant, and Verbatim injects the minimum.** It never types text from one application into another, so it cannot carry content between them. Status: fixed `dwExtraInfo` constants.
- **C7. No window message from a lower integrity level reaches Verbatim.** Remove `allow_quit_across_integrity_levels` in `crates/verbatim-app/src/single_instance.rs`, since a replacing instance runs at the same or a higher level, or admit the message on the one hidden window with `ChangeWindowMessageFilterEx`. Status: missing.
- **C8. Nothing installs or loads an extension without the user's confirmation in Verbatim's own dialog.** The developer folder is watched only in developer mode. Question 6. Status: the design watches it always.
- **C9. Remote support is built for a world of support scams.** An active session is announced when it starts and at intervals; one local gesture ends it at once and cannot be remapped away; unattended access is off by default and approved each time it starts; secure screens need local consent (B9); and a certificate trusted once is checked again on every connection (NVDA's GHSA-m268-mc77-j2cr). Status: M12.

### 7.5 Against class B

- **B1. Measure the UIAccess boundary first.** On the target Windows build, record what an ordinary process can do to a UIAccess Verbatim: open its process, duplicate its token, send it window messages, and whether its injected keys reach the hook with `LLKHF_LOWER_IL_INJECTED` set. The record goes in `experiments/`, and the other mitigations are adjusted to it. Status: M8.1's first step.
- **B2. Verbatim never runs elevated on the user's desktop.** Started from an elevated prompt, it restarts with the user's ordinary token or refuses to start. Status: missing.
- **B3. Whether Windows is locked is asked of Windows when it is decided, and an unknown answer counts as locked.** From the session notifications, queried at startup too. Status: not built.
- **B4. On the lock screen, only an allowlist of commands runs, and navigation stays inside the lock screen.** The reducer is the one place every command passes, so the allowlist lives there. Status: not built.
- **B5. The secure-desktop instance reads nothing a user can write.** It reads a configuration copy and the chosen extensions that an elevated step put in an administrator-owned folder ([extensions](extensions.md), section 2.4), runs no control plane, developer folder, console or install path, and writes nothing. Status: M8.1.
- **B6. The secure instance's GUI and commands are an allowlist (structural).** Opening a window or running a command needs a capability the secure instance does not grant, so no browser, file or folder dialog, help file, URL, log viewer, error dialog or window that can start a process can open there. NVDA's denylist, checks scattered through each feature, is what each of its secure-screen advisories slipped past. Status: M8.1.
- **B7. Nothing of the user's session follows them onto a secure screen.** The speech history, the last braille text and any viewer clear on the switch and at exit. Status: M8.1.
- **B8. Every executable Windows starts for Verbatim on the secure desktop is administrator-owned and signed,** and the registration names an absolute path inside the installation. Status: M8.1.
- **B9. Remote input reaches a secure screen only with the local user's consent, and no local program can stop a permitted session reaching it.** Status: M12.

### 7.6 Against class X

- **X1. The installation is administrator-owned, signed and contains no writable folder.** Per-user state lives in the profile (question 4). Status: there is no installer; state is beside the executable today.
- **X2. DLL loading is pinned.** Each executable calls `SetDefaultDllDirectories` with the system folder and the application folder only, before anything else loads, and links rarely used system DLLs for delay loading; the installer does the same. Status: missing.
- **X3. Configuration is data and never code (structural, holds).** The synthesizer is chosen from a fixed set, theme and sound names are plain names, and no setting names a DLL, an executable or a path. A native adapter is found by name among adapters in an administrator-owned folder.
- **X4. Extensions are Wasm with capabilities derived from imports (structural, designed).** No WASI is offered, epoch deadlines bound compute, store limits bound memory, and the outpost's job caps the rest; NVDA's add-ons are unrestricted Python in its process. A runtime escape is contained by the outpost process, which cannot be an AppContainer, since Windows denies one UIA and MSAA client access to other processes; outposts are to get restricted tokens and process mitigations, measured for what UIA still allows. Wasmtime's compiler needs dynamic code, so the mitigation forbidding it cannot apply to an outpost hosting modules unless precompiled artifacts load as images.
- **X5. Native code runs only in a contained synthesizer host (structural, partly built).** One process per engine (D18), with the AppContainer in its less privileged form planned for M6; until then the host has the user's full token. Native adapters load only from an administrator-owned folder in installed copies.
- **X6. The injection helper (M7) is minimal, in Rust, loaded only from the installation, and treats the browser's side of its channel as hostile.** It runs inside browsers, so its defects are the user's browser's.
- **X7. Updates are verified by Authenticode signature against Verbatim's own certificate before they run.** Status: no updater yet (M8).
- **X8. The build and release are protected.** Dependencies audited with `cargo deny` in `cargo xtask ci`, which also serves the platform-neutral tier's licence rule; the release signing key in hardware or a cloud key service, used only by a protected release job; workflows that run code from pull requests get no secrets. Status: downloads and submodules are pinned; the rest is missing.
- **X9. Memory safety (structural, holds).** Unsafe code stays in the Windows crates behind small safe wrappers: the platform-neutral crates forbid it, and it is concentrated in `verbatim-uia` (about 195 uses), `verbatim-outpost` (123), `verbatim-process` (96), `verbatim-ia2` (74) and `verbatim-app` (70). C and C++ that parse untrusted input run outside Core: eSpeak NG in the synthesizer host. Core does not panic on application data: `verbatim-core` and `verbatim-text` have no `unwrap` or `expect` outside tests.
- **X10. Every parser and every algorithm that takes untrusted data is fuzzed.** Targets, all platform-neutral and so able to run on the Linux job: the frame and raw-section decoders, the control-plane reader, the configuration, theme and package readers, the synthesizer escapes, terminal alignment and segmentation, and the reducer fed generated inputs. A `mockapp` mode serves hostile trees (self-parents, cycles, a million children, multi-megabyte names, hanging providers) as end-to-end checks of U2 to U7. Status: missing; proptest covers only terminals.

### 7.7 Against class L

- **L1. Crash dumps keep their text, and their life is controlled instead.** A dump without the text is useless for replay (Dickson, 2026-10-31). A dump is handled as the user's private data: written only where L4 says, never sent anywhere by Verbatim (L6), never written by the secure-desktop instance, and the user is told what a dump holds before attaching it to a bug report.
- **L2. Input from a protected field is masked when it is recorded.** The reducer echoes a star for a protected field, so recording a star loses nothing for replay, while a recorded password would travel in every dump the user shares. Question 3. Status: unmasked.
- **L3. Logs carry no content at their default level.** Content at a debug level only behind developer mode, and never on secure screens. Status: holds; keep it a review rule as M9 adds file logging.
- **L4. Stored diagnostics have a home, an owner and an end.** Logs and dumps under the user's local application data, readable by that user only, capped in count and age. Status: beside the executable; dumps kept forever.
- **L5. The speech stream and tree dumps are content,** exposed only through the developer endpoint (C1).
- **L6. Verbatim makes no network connection except the updater and Remote (structural, holds).** No telemetry, no crash upload, no online lookup; a dump leaves the machine only by the user's hand. Cloud recognition, if ever offered, is opt-in per use.
- **L7. What Verbatim puts on the clipboard from a protected field stays out of clipboard history.** Copying the last spoken text (M8) marks such a copy with the clipboard formats Windows defines for excluding it from history and cloud sync.

### 7.8 Against class R

- **R1. Core restarts itself after a crash, with a breaker.** Through Windows' restart manager (`RegisterApplicationRestart`) or a small guardian process, announced on restart, and stopped after a few crashes in a short window, as NVDA's crash handler does. Status: missing.
- **R2. Core has a memory cap.** A job object of its own, so unforeseen growth ends in a restart rather than paging the machine to a halt. Status: missing.
- **R3. A synthesizer failure costs one utterance, and repeated failures cost the synthesizer, never speech.** After three crashes within a minute, switch to eSpeak NG for the session and say why in its voice, and to OneCore if eSpeak NG fails; adapters filter their engines' known crash strings. Measure whether a hang time shorter than 10 seconds, counted from the first audio owed, holds without false alarms. Question 5. Status: one utterance fails and the host restarts; no fallback after startup and no restart cap.
- **R4. The keyboard hook's health is checked.** A hook that stops receiving keys while `GetLastInputInfo` shows input is reinstalled, and Verbatim says so. Status: missing.
- **R5. Speech survives losing the audio device.** While the default device refuses a shared stream, play through another active device if there is one, and report the time lost on recovery. Status: retries the same device.

## 8. How well the architecture serves the goal

The architecture is strongest exactly where screen readers have historically been weakest, and weakest in product surfaces that have not been designed yet.

Strengths, all structural:

- **Class U:** per-application isolation (U1) and an outpost boundary Core checks (U6). A hostile or broken application costs reading that application only.
- **Classes U and J:** a pure reducer with no COM, which makes Core's logic fuzzable on Linux.
- **Class C:** private channels to children, unnamed pipes passed by handle, so there is nothing to discover or to protect with an access list.
- **Class X:** extensions as Wasm with derived capabilities (D6), native engines out of process (D18), no injection until M7 and then through one helper (D2), unsafe code confined to the Windows crates, and configuration that cannot name code.
- **Class L:** no network code and no content in default logs.

Weaknesses, all fixable without changing the architecture:

- Class C: control surfaces on by default and broad, and a test agent open to the network.
- Classes X and L: a file layout that assumes a writable executable folder, which conflicts with Program Files and UIAccess, and diagnostics with no owner or end that record passwords typed into protected fields.
- Class U: no repeat detection in walks, no cap on property strings, a quadratic step in Core's terminal code, and no bound on outposts.
- Classes S and B: nothing designed yet, which is the chance to design them from allowlists rather than repair them advisory by advisory as NVDA did.
- Class R: no restart for Core and no fallback when a synthesizer, the audio device or the hook is lost.
- Class X: no fuzzing and no dependency audit.

## 9. Findings in today's code

Ranked by severity, judged for today's development use; those marked as rising with UIAccess block M8.1.

- **F1, critical on any networked development machine or VM.** `verbatim-agent` listens on every interface with no authentication and can launch processes and write files. Mitigation C4.
- **F2, high, rising with UIAccess.** The control plane is always on and offers key injection, the speech stream and dumps. Mitigation C1.
- **F3, high for privacy.** Crash dumps hold characters typed into password fields, beside the executable, kept forever. Mitigations L2 and L4.
- **F4, medium.** Settings, logs and dumps live beside the executable, with fallbacks to `%TEMP%\verbatim-defaults` and to the current directory. Mitigations C3 and X1.
- **F5, medium.** Any Low-integrity process can end Verbatim with `WM_QUIT`. Mitigation C7.
- **F6, medium.** Gestures accept injected keys from any integrity level. Mitigation C5.
- **F7, medium.** Terminal alignment in Core is quadratic in row width, Core has no memory cap, and the input reaches it from remote programs' output. Mitigations U5 and R2.
- **F8, medium.** No DLL search restriction or process mitigations in any executable. Mitigation X2.
- **F9, medium.** No Core restart after a crash. Mitigation R1.
- **F10, medium.** No cap on synthesizer restarts and no fallback after startup. Mitigation R3.
- **F11, medium, rising with UIAccess.** Started from an elevated prompt, Verbatim runs elevated. Mitigation B2.
- **F12, medium.** The control pipe's name is global, so a second signed-in user's Verbatim cannot create its pipe. Mitigation C2.
- **F13, medium.** No bound on outposts, no check that the keyboard hook is still installed, and no fallback to another audio device. Mitigations U8, R4 and R5.
- **F14, low.** No repeat detection in walks, so a self-parent is reported up to 64 times; the classic UIA child listing is capped only after fetching. Mitigations U2 and U3.
- **F15, low.** Name, value and description strings are uncapped up to 32 MiB, and the 64 KiB text cut is silent. Mitigation U4.
- **F16, low.** The control pipe's and configuration's readers have no size limit. Mitigation C3.
- **F17, low.** eSpeak NG's text escape passes control characters through, unlike OneCore's. Mitigation J1.
- **F18, process.** No fuzzing, no `cargo deny`. Mitigations X8 and X10.

## 10. Where the work goes

- Now, as a small build of its own: F1 (the agent, C4), F5 (the message filter, C7), and the configuration fallbacks of C3.
- M5: L1 and L2, since M5 brings the log ring forward; C1's user endpoint, since the install request arrives with M5; S1 for extensions.
- M6: R3, X5 and J1, and the AppContainer as designed.
- The M6.x developer-mode milestone (Dickson, 2026-10-31): C1's developer endpoint, C8's developer folder, L3's developer setting, and C2's per-session pipe name.
- M6.3, the speech and audio pipeline: U4's cut reaching speech, and R5.
- M6.4, terminal residuals: U5.
- M7: X6 and J3, and U2 and U3 before the IA2 tree walks multiply.
- M8, with the installer skeleton: X1, X2, X7, B2, L4, R1 and R2, the per-user state location, X8's signing key, and J2, S2 and L7 with dictionaries and the clipboard command.
- M8.1: B1 first, then B3 to B8 and C5 and C6.
- M9: L3 as file logging arrives.
- M12: C9 and B9.
- Ongoing, from now: X8's audit and X10's fuzzing in `cargo xtask ci`; U7's background flood cap, U8 and R4 with the next focus or input build; J3, J4 and L6 as standing review rules.

## 11. Comparison with NVDA and other screen readers

NVDA's mechanisms below were read from its source at the pinned commit. Its advisories come from its GitHub advisory list, which held 24 entries on 2026-10-31. The other readers' history comes from NVD, vendors' documentation and published research. The detailed survey with its sources is in [the security comparison](../../experiments/security-model-survey/readme.md).

### 11.1 NVDA's record

About half of NVDA's advisories are escapes from secure screens or the lock screen, several rated critical: the Python console, the log viewer, context help, a clipboard error dialog and a formatting dialog each led to code running as SYSTEM, and object navigation walked out of the lock screen. The rest are information left behind across that boundary (debug logging, the speech viewer, braille contents), code execution (DLL planting in the installer, web text run through an HTML dialog, an add-on able to patch NVDA's elevation code, which NV Access treats as a consequence of add-ons being unrestricted), and Remote Access flaws, the worst a missed certificate check on reconnecting.

The lesson Verbatim takes is that the secure-desktop instance, not the everyday one, is where a screen reader is attacked, and that a denylist of features there fails one feature at a time.

### 11.2 Mechanism by mechanism

- **Isolation from hostile applications.** NVDA runs its logic on one main thread, and its watchdog cancels COM calls and `SendMessage` after a stall. Verbatim isolates each application in its own process (D9). Verbatim is ahead. Both cap ancestor walks (NVDA at 100 hops, then it jumps to the desktop; Verbatim at 64), and neither detects a repeat (mitigation U2).
- **Control surface.** NVDA's controller is a local RPC endpoint whose access list grants full access to Everyone and to every AppContainer, with no authentication, and carries both the public speech functions and internal ones, including an add-on install request and a Remote Access link handler. Its injected helper reports through the same endpoint. Verbatim's pipe admits only the owning user and refuses remote clients, so it is ahead on reach, but it offers key injection, which NVDA's controller does not (mitigation C1).
- **Logs and dumps.** NVDA's log at its IO level records every utterance and every key, including keys typed in password fields. It lives in the temporary folder, and only its startup configuration dump is redacted. In secure mode its logging is off. Verbatim logs no content today, which is ahead, but its crash dumps keep typed characters and the clipboard in readable form (mitigations L1 and L2). NVDA's crash handler writes a minidump of its memory, which holds whatever it was reading, so neither keeps content out of crash data.
- **Protected fields.** Both echo a star for each typed character and suppress typed-word echo.
- **Injected input.** Both treat injected keys as gestures. NVDA lets the user turn that off, and neither looks at the injector's integrity level (mitigation C5).
- **Extensions.** NVDA's add-ons are unrestricted Python in its process. Its store pins each release by SHA-256 and scans it, but nobody reviews the code and nothing is signed. Verbatim's Wasm modules with derived capabilities are ahead by design (mitigation X4). NVDA's copy of add-ons to the secure screens, chosen by the user and made by an elevated step, is the model Verbatim's design already follows.
- **Configuration.** NVDA keeps it in the user's profile for installed copies, beside the executable for portable ones, and in an administrator-owned copy for secure screens; it validates the file against a specification and resets a corrupt one to defaults. Verbatim matches this only once mitigations C3 and X1 are done.
- **Recovery from its own crash.** NVDA's crash handler restarts NVDA, with a breaker that stops after too many crashes in a short window. Verbatim does not restart (mitigation R1), and should copy the breaker.
- **Synthesizers.** NVDA runs most synthesizers in its own process. A failure to load falls back from OneCore to eSpeak NG to silence, but a crash at runtime takes NVDA with it, and only 32-bit SAPI voices run in a separate host. Verbatim's host per synthesizer (D18) is ahead. Neither falls back after a runtime crash (mitigation R3).
- **Secure mode.** NVDA's is mature, built from twenty advisories. Verbatim has none yet. It has the chance to build one from an allowlist from the start (mitigations B3 to B8).
- **Updates.** NVDA checks a SHA-1 hash from its own update server and runs the launcher; no signature check was found. Verbatim has no updater yet (mitigation X7).
- **Injection.** NVDA injects its helper DLL into every process that raises a focus or foreground event, through global in-context hooks. Verbatim injects nothing until M7, and then only through one helper (D2).

### 11.3 Other screen readers

- **JAWS.** Its scripts are compiled code loaded per application. Its one public vulnerability found is a service, in versions 13 and earlier, that any user could reconfigure to run code as SYSTEM. No other advisory or CVE was found, which may reflect disclosure practice rather than an absence of flaws.
- **Narrator.** It was the route of a BitLocker lock-screen bypass (CVE-2020-1398), and NVD lists several privilege-escalation entries for Narrator's braille support and for the accessibility broker, `ATBroker.exe`, in 2026, chiefly untrusted search paths. Even Windows' own screen reader is attacked at the sign-in screen and through DLL search order.
- **Orca and AT-SPI.** The Linux accessibility bus has no access control, and any client can see every other's events. GNOME's recent work moves keyboard capture into the compositor and sends keys only to an allowlisted screen reader, so that no process can act as a keylogger. The lesson for Verbatim is that a screen reader's input path is a keylogger by design, and must not be readable by others (mitigations L5 and L1).
- **VoiceOver.** Apple's fixes include VoiceOver reading secure input fields, saved passwords and the passcode aloud, and a login window bypass. macOS asks the user to grant accessibility access per application. The lesson is that protected text must never be spoken by any path, including paths added later (mitigation L2).
- **Android accessibility services.** Banking malware uses accessibility services to read the screen and inject input, which is why Android 13 restricts them for sideloaded apps, a restriction droppers have bypassed. Any API that grants a screen reader's reach is a malware target: Verbatim's extension imports must not hand that reach to an extension the user did not knowingly install (mitigation C8).
- **Research.** Jang and others (CCS 2014, "A11y Attacks") bypassed UAC and the sandboxes of four systems through accessibility. On Windows, speech synthesized by a process drove Speech Recognition, which runs at High integrity, and UI Automation clicked a password field's reveal button. Their recommendation, that an assistive technology check that its input comes from the user, is mitigation C5.

### 11.4 Where Verbatim can do better

- Build the secure instance from an allowlist before it first runs, rather than after advisories.
- Decide lock state from Windows, failing closed.
- Give the control surface the least reach, and none on secure screens.
- Keep passwords out of every file, including crash dumps, without losing the text dumps need.
- Refuse lower-integrity injected input, which no surveyed screen reader does.
- Fall back to another synthesizer after runtime crashes, which neither NVDA nor Verbatim does today.
- Verify updates by signature.
